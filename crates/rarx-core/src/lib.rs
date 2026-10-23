//! The RarX archive engine (AGENT.md §4).
//!
//! Front-ends (CLI, GUI, shell, FFI) talk only to this crate. It owns operations,
//! the update planner, job control and progress reporting; format and codec details
//! stay in the lower crates.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod archive;
mod error;
mod extract;
mod job;
pub mod sanitize;
pub mod select;
pub mod time;

pub use archive::{Archive, detect_file};
pub use error::{Error, Result};
pub use extract::{
    ExtractHandler, ExtractOptions, ItemError, Outcome, OverwriteChoice, OverwriteMode, PathMode,
    Report, SilentHandler, extract, test,
};
pub use job::JobControl;
pub use rarx_formats::{DataError, Entry, EntryKind, FileTime, FormatError, FormatId};
