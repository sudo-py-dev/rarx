//! Shared plumbing for the `rarx` (RAR-style) and `rx7` (7-Zip-style) binaries
//! (AGENT.md §13). Each binary keeps its reference tool's command syntax and exit codes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod common;
pub mod exit;
pub mod rar;
pub mod sevenzip;
