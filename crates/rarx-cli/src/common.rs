//! Pieces shared by both command-line dialects.

use std::io::{self, BufRead, Write};
use std::path::Path;

use rarx_core::time::format_local;
use rarx_core::{Entry, EntryKind, FileTime, Report};

/// Arguments split into command, switches and positional arguments.
#[derive(Debug, Default)]
pub struct Args {
    /// First argument.
    pub command: String,
    /// Arguments starting with `-` (without the dash), in order.
    pub switches: Vec<String>,
    /// Everything else: archive name, then masks or paths.
    pub positionals: Vec<String>,
}

impl Args {
    /// Splits `args`; switches may appear anywhere before a `--` argument.
    #[must_use]
    pub fn parse(args: &[String]) -> Self {
        let mut parsed = Self::default();
        let mut rest = args.iter();
        if let Some(command) = rest.next() {
            parsed.command = command.to_lowercase();
        }
        let mut switches_done = false;
        for arg in rest {
            if !switches_done && arg == "--" {
                switches_done = true;
            } else if !switches_done && arg.len() > 1 && arg.starts_with('-') {
                parsed.switches.push(arg[1..].to_owned());
            } else {
                parsed.positionals.push(arg.clone());
            }
        }
        parsed
    }
}

/// Formats a timestamp in local time, or blanks of the same width.
#[must_use]
pub fn time_or_blank(time: Option<FileTime>, pattern: &str, width: usize) -> String {
    time.and_then(|t| format_local(t, pattern))
        .unwrap_or_else(|| " ".repeat(width))
}

/// 7-Zip style attribute column, e.g. `D....` or `....A`.
#[must_use]
pub fn dos_attribute_string(entry: &Entry) -> String {
    let attrs = entry.dos_attributes.unwrap_or(0);
    let flag = |bit: u32, c: char| if attrs & bit != 0 { c } else { '.' };
    let dir = if entry.is_dir() || attrs & 0x10 != 0 {
        'D'
    } else {
        '.'
    };
    [
        dir,
        flag(0x01, 'R'),
        flag(0x02, 'H'),
        flag(0x04, 'S'),
        flag(0x20, 'A'),
    ]
    .iter()
    .collect()
}

/// `ls -l` style permission string, e.g. `-rw-r--r--`.
#[must_use]
pub fn unix_mode_string(entry: &Entry) -> Option<String> {
    let mode = entry.unix_mode?;
    let kind = match entry.kind {
        EntryKind::Directory => 'd',
        EntryKind::Symlink => 'l',
        EntryKind::File => '-',
    };
    let bits = ['r', 'w', 'x'];
    let perms = (0..9).map(|i| {
        if mode & (1 << (8 - i)) != 0 {
            bits[i % 3]
        } else {
            '-'
        }
    });
    Some(std::iter::once(kind).chain(perms).collect())
}

/// Displays an archive path with the platform's separator.
#[must_use]
pub fn display_name(name: &str) -> String {
    let escaped: String = name
        .chars()
        .map(|c| {
            if c.is_control() || is_bidi_control(c) {
                '?'
            } else {
                c
            }
        })
        .collect();
    if cfg!(windows) {
        escaped.replace('/', "\\")
    } else {
        escaped
    }
}

/// Bidirectional-override characters can make `exe.txt` display as `txt.exe`
/// (AGENT.md §18.13).
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

/// Size of the archive file in bytes, or 0 if it cannot be read.
#[must_use]
pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Prints `question` and reads one trimmed, lower-cased line from stdin.
/// Returns `None` at end of input.
#[must_use]
pub fn ask(question: &str) -> Option<String> {
    print!("{question}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_lowercase()),
    }
}

/// Whether a report contains a checksum failure.
#[must_use]
pub fn has_error(report: &Report, pred: impl Fn(&rarx_core::ItemError) -> bool) -> bool {
    report.errors.iter().any(|(_, err)| pred(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_switches_anywhere_until_double_dash() {
        let args: Vec<String> = ["X", "-y", "a.zip", "-oout", "--", "-literal"]
            .map(String::from)
            .to_vec();
        let parsed = Args::parse(&args);
        assert_eq!(parsed.command, "x");
        assert_eq!(parsed.switches, ["y", "oout"]);
        assert_eq!(parsed.positionals, ["a.zip", "-literal"]);
    }

    #[test]
    fn mode_and_attribute_strings() {
        let mut entry = Entry::new("f", EntryKind::File);
        entry.unix_mode = Some(0o100_754);
        entry.dos_attributes = Some(0x21);
        assert_eq!(unix_mode_string(&entry).as_deref(), Some("-rwxr-xr--"));
        assert_eq!(dos_attribute_string(&entry), ".R..A");
    }

    #[test]
    fn display_name_hides_spoofing_characters() {
        assert_eq!(display_name("evil\u{202E}txt.exe"), "evil?txt.exe");
        assert_eq!(display_name("a\nb"), "a?b");
    }
}
