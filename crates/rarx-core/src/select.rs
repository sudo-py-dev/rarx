//! Choosing archive items by wildcard masks (AGENT.md §10.3).

/// Include/exclude masks applied to archive item names.
///
/// A mask without `/` is matched against the item's file name and against every
/// folder on its path, so `*.txt` finds text files at any depth and `docs` selects
/// the `docs` folder with its contents. A mask with `/` is matched against the full
/// path or any leading part of it.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    include: Vec<String>,
    exclude: Vec<String>,
    case_sensitive: bool,
}

impl Selection {
    /// A selection that matches every item.
    #[must_use]
    pub fn all() -> Self {
        Self {
            case_sensitive: !cfg!(windows),
            ..Self::default()
        }
    }

    /// Adds an include mask. With no include masks, every item is included.
    #[must_use]
    pub fn include(mut self, mask: &str) -> Self {
        self.include.push(normalize(mask));
        self
    }

    /// Adds an exclude mask, which wins over include masks.
    #[must_use]
    pub fn exclude(mut self, mask: &str) -> Self {
        self.exclude.push(normalize(mask));
        self
    }

    /// Whether any include masks were given.
    #[must_use]
    pub fn has_includes(&self) -> bool {
        !self.include.is_empty()
    }

    /// Whether `name` (a `/`-separated archive path) is selected.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        let hit = |mask: &String| mask_matches(mask, name, self.case_sensitive);
        (self.include.is_empty() || self.include.iter().any(hit)) && !self.exclude.iter().any(hit)
    }
}

fn normalize(mask: &str) -> String {
    mask.replace('\\', "/").trim_matches('/').to_owned()
}

fn mask_matches(mask: &str, name: &str, case_sensitive: bool) -> bool {
    let name = name.trim_matches('/');
    if mask.contains('/') {
        let mut prefixes = name.match_indices('/').map(|(i, _)| &name[..i]);
        wildcard(mask, name, case_sensitive) || prefixes.any(|p| wildcard(mask, p, case_sensitive))
    } else {
        name.split('/')
            .any(|part| wildcard(mask, part, case_sensitive))
    }
}

/// Matches `text` against a mask where `*` is any run of characters and `?` is any
/// single character. `*.*` also matches names without a dot, as in WinRAR and 7-Zip.
#[must_use]
pub fn wildcard(mask: &str, text: &str, case_sensitive: bool) -> bool {
    if mask == "*.*" {
        return true;
    }
    let eq = |a: char, b: char| {
        if case_sensitive {
            a == b
        } else {
            a.to_lowercase().eq(b.to_lowercase())
        }
    };
    let mask: Vec<char> = mask.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut m, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match mask.get(m) {
            Some('*') => {
                backtrack = Some((m, t));
                m += 1;
            }
            Some(&c) if c == '?' || eq(c, text[t]) => {
                m += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((star, start)) => {
                    m = star + 1;
                    t = start + 1;
                    backtrack = Some((star, start + 1));
                }
                None => return false,
            },
        }
    }
    mask[m..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_basics() {
        assert!(wildcard("*.txt", "a.txt", true));
        assert!(!wildcard("*.txt", "a.txt.bak", true));
        assert!(wildcard("a?c", "abc", true));
        assert!(wildcard("*", "", true));
        assert!(wildcard("*.*", "README", true));
        assert!(wildcard("*a*b*", "xxaxxbxx", true));
        assert!(!wildcard("ABC", "abc", true));
        assert!(wildcard("ABC", "abc", false));
    }

    #[test]
    fn selection_masks() {
        let sel = Selection::all().include("*.txt");
        assert!(sel.matches("a.txt"));
        assert!(sel.matches("deep/dir/b.txt"));
        assert!(!sel.matches("c.bin"));

        let folder = Selection::all().include("docs");
        assert!(folder.matches("docs"));
        assert!(folder.matches("docs/a/b.bin"));
        assert!(!folder.matches("src/a.rs"));

        let path = Selection::all().include("src/sub");
        assert!(path.matches("src/sub/x"));
        assert!(!path.matches("other/src/sub"));

        let excluded = Selection::all().exclude("*.log");
        assert!(excluded.matches("a.txt"));
        assert!(!excluded.matches("logs/app.log"));
    }
}
