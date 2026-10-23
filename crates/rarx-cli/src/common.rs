//! Pieces shared by both command-line dialects.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use rarx_core::time::format_local;
use rarx_core::{Archive, Entry, EntryKind, Error, FileTime, FormatError, Password, Report};

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

/// What a `-p` switch asked for.
#[derive(Clone, Debug, Default)]
pub enum PasswordArg {
    /// No switch: ask only when the archive turns out to need a password.
    #[default]
    Unset,
    /// `-p<password>`.
    Given(Password),
    /// `-p` alone: ask before starting.
    Ask,
    /// `-p-`: never ask.
    Never,
}

impl PasswordArg {
    /// Interprets the text after `-p` (case preserved).
    #[must_use]
    pub fn from_switch(value: &str) -> Self {
        match value {
            "" => Self::Ask,
            "-" => Self::Never,
            text => Self::Given(Password::from(text)),
        }
    }

    /// The password to start with, asking now for `-p` alone. With `confirm`
    /// (creating an archive) the password must be typed twice.
    ///
    /// # Errors
    /// A message when input ends or the two entries differ.
    pub fn initial(&self, confirm: bool) -> Result<Option<Password>, String> {
        match self {
            Self::Given(pw) => Ok(Some(pw.clone())),
            Self::Unset | Self::Never => Ok(None),
            Self::Ask => {
                let first = read_password("Enter password (will not be echoed): ")
                    .ok_or("No password was entered")?;
                if confirm && io::stdin().is_terminal() {
                    let again =
                        read_password("Reenter password: ").ok_or("No password was entered")?;
                    if again.utf8() != first.utf8() {
                        return Err("The passwords do not match".to_owned());
                    }
                }
                Ok(Some(first))
            }
        }
    }

    /// Whether RarX may ask for a password the archive turns out to need.
    #[must_use]
    pub fn may_ask(&self) -> bool {
        matches!(self, Self::Unset)
    }
}

/// Reads a password: without echo from a terminal, else one line of standard input.
/// Returns `None` at end of input.
#[must_use]
pub fn read_password(prompt: &str) -> Option<Password> {
    if io::stdin().is_terminal() {
        return rpassword::prompt_password(prompt).ok().map(Password::new);
    }
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            let end = line.trim_end_matches(['\r', '\n']).len();
            line.truncate(end);
            Some(Password::new(line))
        }
    }
}

/// Opens `path`; if its header is encrypted and no password is known yet, asks for
/// one (when `arg` allows) and tries again.
///
/// # Errors
/// Whatever [`Archive::open_with_password`] reports.
pub fn open_archive(
    path: &Path,
    arg: &PasswordArg,
    password: &mut Option<Password>,
) -> rarx_core::Result<Archive> {
    match Archive::open_with_password(path, password.clone()) {
        Err(Error::Format(FormatError::Encrypted)) if password.is_none() && arg.may_ask() => {
            *password = read_password(&format!(
                "Enter password (will not be echoed) for {}: ",
                path.display()
            ));
            if password.is_none() {
                return Err(Error::Format(FormatError::Encrypted));
            }
            Archive::open_with_password(path, password.clone())
        }
        other => other,
    }
}

/// Before extracting or testing: asks for a password if items are encrypted and
/// none is known yet.
pub fn ensure_item_password(
    archive: &mut Archive,
    arg: &PasswordArg,
    password: &mut Option<Password>,
) {
    if password.is_none() && arg.may_ask() && archive.has_encrypted_items() {
        *password = read_password(&format!(
            "Enter password (will not be echoed) for {}: ",
            archive.path().display()
        ));
        archive.set_password(password.clone());
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
