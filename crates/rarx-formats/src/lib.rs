//! Archive container formats for RarX (AGENT.md §5–§6).
//!
//! Each format lives in its own module and is exposed through [`ArchiveReader`].
//! Callers normally go through [`detect`] and [`open`] rather than naming a format.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod entry;
mod error;
mod new_item;
mod reader;
pub mod sevenz;
mod shared;
mod signature;
pub mod stream;
pub mod tar;
mod verify;
pub mod zip;

pub use entry::{CivilTime, Entry, EntryKind, FileTime};
pub use error::{DataError, FormatError};
pub use new_item::NewItem;
pub use rarx_crypto::Password;
pub use reader::{ArchiveReader, ReadSeek, open};
pub use signature::{FormatId, Signature, detect};
