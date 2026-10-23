//! Helpers shared by RarX test suites (AGENT.md §22).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// A uniquely named temporary folder, deleted with its contents on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a new empty folder under the system temp directory.
    ///
    /// # Panics
    /// If the folder cannot be created; this is test-only code.
    #[must_use]
    #[allow(clippy::expect_used)] // test helper: failing to create the folder must abort the test
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rarx-test-{}-{n}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// Path of the folder.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_dir_exists() {
        assert!(corpus_dir().is_dir());
    }

    #[test]
    fn temp_dir_is_removed_on_drop() {
        let dir = TempDir::new("drop");
        let path = dir.path().to_owned();
        std::fs::write(path.join("f"), b"x").unwrap();
        drop(dir);
        assert!(!path.exists());
    }
}
