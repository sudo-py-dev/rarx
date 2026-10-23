//! Turns untrusted archive item names into safe relative paths (AGENT.md §18.1).

use std::path::PathBuf;

/// Which file-system naming rules to enforce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameRules {
    /// POSIX: only `/` and NUL are special.
    Unix,
    /// Windows: reserved characters, device names and trailing dots/spaces.
    Windows,
}

impl NameRules {
    /// Rules for the platform RarX is running on.
    #[must_use]
    pub fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

const WINDOWS_DEVICES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Converts an archive item name into a path relative to the extraction folder.
///
/// Both `/` and `\` separate components. Empty, `.` and `..` components are dropped,
/// so the result can never climb out of the destination; a leading drive letter or
/// UNC prefix is removed. Returns `None` when nothing usable is left.
#[must_use]
pub fn sanitize_path(name: &str, rules: NameRules) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for (index, component) in name.split(['/', '\\']).enumerate() {
        let component = if index == 0 {
            strip_drive(component)
        } else {
            component
        };
        if matches!(component, "" | "." | "..") {
            continue;
        }
        if let Some(clean) = sanitize_component(component, rules) {
            path.push(clean);
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Removes a `C:` drive prefix from the first component.
fn strip_drive(component: &str) -> &str {
    match component.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => &component[2..],
        _ => component,
    }
}

fn sanitize_component(component: &str, rules: NameRules) -> Option<String> {
    let mut clean: String = component
        .chars()
        .map(|c| match (c, rules) {
            ('\0', _) => '_',
            (c, NameRules::Windows)
                if c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') =>
            {
                '_'
            }
            (c, _) => c,
        })
        .collect();
    if rules == NameRules::Windows {
        clean.truncate(clean.trim_end_matches(['.', ' ']).len());
        let stem = clean.split('.').next().unwrap_or_default().trim_end();
        if WINDOWS_DEVICES
            .iter()
            .any(|dev| dev.eq_ignore_ascii_case(stem))
        {
            clean.insert(0, '_');
        }
    }
    (!clean.is_empty() && clean != "." && clean != "..").then_some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(name: &str) -> Option<String> {
        sanitize_path(name, NameRules::Unix).map(|p| p.to_string_lossy().into_owned())
    }

    fn win(name: &str) -> Option<String> {
        sanitize_path(name, NameRules::Windows).map(|p| {
            p.iter()
                .map(|c| c.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
    }

    #[test]
    fn keeps_normal_paths() {
        assert_eq!(
            unix("dir/sub/file.txt").as_deref(),
            Some("dir/sub/file.txt")
        );
    }

    #[test]
    fn removes_traversal_and_absolute_prefixes() {
        assert_eq!(unix("../../etc/passwd").as_deref(), Some("etc/passwd"));
        assert_eq!(unix("/etc/passwd").as_deref(), Some("etc/passwd"));
        assert_eq!(unix("a/../../b").as_deref(), Some("a/b"));
        assert_eq!(unix("..\\..\\win.ini").as_deref(), Some("win.ini"));
        assert_eq!(unix("C:\\Windows\\x.dll").as_deref(), Some("Windows/x.dll"));
        assert_eq!(
            unix("\\\\server\\share\\f").as_deref(),
            Some("server/share/f")
        );
        assert_eq!(unix("./a/./b").as_deref(), Some("a/b"));
    }

    #[test]
    fn rejects_empty_results() {
        assert_eq!(unix(""), None);
        assert_eq!(unix("../.."), None);
        assert_eq!(unix("/"), None);
    }

    #[test]
    fn replaces_nul_everywhere() {
        assert_eq!(unix("a\0b").as_deref(), Some("a_b"));
    }

    #[test]
    fn windows_rules() {
        assert_eq!(win("ab:c?.txt").as_deref(), Some("ab_c_.txt"));
        assert_eq!(win("a:b.txt").as_deref(), Some("b.txt"));
        assert_eq!(win("dir/CON").as_deref(), Some("dir/_CON"));
        assert_eq!(win("nul.txt").as_deref(), Some("_nul.txt"));
        assert_eq!(win("name. . ").as_deref(), Some("name"));
        assert_eq!(win("x/...").as_deref(), Some("x"));
        assert_eq!(win("file:stream").as_deref(), Some("file_stream"));
    }
}
