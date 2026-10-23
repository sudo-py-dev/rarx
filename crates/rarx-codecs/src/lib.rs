//! Compression codecs for RarX (AGENT.md §7.1).
//!
//! Every codec is a streaming state machine driven through [`Codec::process`]: callers
//! hand it whatever input and output space they have, and it reports how much of each
//! it used. No codec buffers a whole file or allocates based on untrusted sizes
//! without checking a memory budget first.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod copy;

use core::fmt;

pub use copy::Copy;

/// How much input a [`Codec::process`] call consumed and how much output it wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Bytes taken from the input slice.
    pub consumed: usize,
    /// Bytes written to the output slice.
    pub produced: usize,
}

/// Failure inside a codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// The compressed data is invalid.
    Corrupt,
    /// The stream uses a feature this codec does not implement.
    Unsupported,
    /// Decoding would need more memory than the caller allows.
    MemoryLimit {
        /// Bytes the stream requires.
        needed: u64,
        /// Bytes the caller allowed.
        limit: u64,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt => f.write_str("compressed data is corrupt"),
            Self::Unsupported => f.write_str("unsupported codec feature"),
            Self::MemoryLimit { needed, limit } => {
                write!(f, "needs {needed} bytes of memory, limit is {limit}")
            }
        }
    }
}

impl core::error::Error for CodecError {}

/// A streaming encoder or decoder.
pub trait Codec {
    /// Transforms as much of `input` into `output` as possible.
    ///
    /// # Errors
    /// Returns a [`CodecError`] when the data is invalid or unsupported.
    fn process(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, CodecError>;
}
