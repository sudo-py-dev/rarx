//! I/O primitives for RarX (AGENT.md §3).
//!
//! All archive parsing goes through [`Cursor`], which never reads out of bounds and
//! reports the offset of every failure. Volume spanning, buffered writers and mmap
//! support land here in later milestones.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod cursor;

pub use cursor::{Cursor, ParseError};
