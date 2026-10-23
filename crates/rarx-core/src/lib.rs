//! The RarX archive engine (AGENT.md §4).
//!
//! Front-ends (CLI, GUI, shell, FFI) talk only to this crate. It owns operations,
//! the update planner, job control and progress reporting; format and codec details
//! stay in the lower crates.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod job;

use std::fs::File;
use std::io::Read;
use std::path::Path;

pub use error::{Error, Result};
pub use job::JobControl;
pub use rarx_formats::FormatId;

/// Bytes read from the start of a file for signature detection.
const DETECT_PREFIX_LEN: u64 = 64;

/// Identifies the archive format of the file at `path` by its signature.
///
/// # Errors
/// Returns [`Error::Io`] if the file cannot be opened or read.
pub fn detect_file(path: &Path) -> Result<Option<FormatId>> {
    let mut prefix = Vec::new();
    File::open(path)?
        .take(DETECT_PREFIX_LEN)
        .read_to_end(&mut prefix)?;
    Ok(rarx_formats::detect(&prefix))
}
