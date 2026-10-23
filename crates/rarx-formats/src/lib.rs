//! Archive container formats for RarX (AGENT.md §5–§6).
//!
//! Each format becomes its own module implementing the `Format` traits once the core
//! engine API lands (M1). For now this crate owns format identities and signature
//! detection, which every front-end needs first.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod signature;

pub use signature::{FormatId, Signature, detect};
