//! Helpers shared by RarX test suites (AGENT.md §22).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::path::{Path, PathBuf};

/// Root of the repository's test corpus (`corpus/`).
#[must_use]
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// Path of a file inside the test corpus.
#[must_use]
pub fn corpus_file(relative: impl AsRef<Path>) -> PathBuf {
    corpus_dir().join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_dir_exists() {
        assert!(corpus_dir().is_dir());
    }
}
