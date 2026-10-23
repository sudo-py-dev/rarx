//! Reversible pre-processing filters for RarX (AGENT.md §7.2).
//!
//! Filters transform data in place so that the following codec compresses it better
//! (branch-address conversion for executables, delta coding for samples). Each filter
//! is a streaming state machine: buffers may be fed in arbitrary-sized pieces.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod delta;

pub use delta::Delta;

/// A reversible in-place byte transform.
pub trait Filter {
    /// Applies the forward (compression-side) transform to `buf`.
    fn encode(&mut self, buf: &mut [u8]);

    /// Applies the inverse (extraction-side) transform to `buf`.
    fn decode(&mut self, buf: &mut [u8]);
}
