//! Checksums and hashes used by RarX archive formats (AGENT.md §9).
//!
//! Every algorithm implements [`Checksum`] so callers can stream data through it
//! without knowing which one an archive uses. Scalar table-driven implementations
//! live here; SIMD dispatch is added per algorithm behind the `simd` feature later.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod crc32;
mod crc64;

pub use crc32::Crc32;
pub use crc64::Crc64;

/// A streaming checksum or hash.
pub trait Checksum: Default {
    /// The finished digest type.
    type Output;

    /// Feeds `data` into the running checksum.
    fn update(&mut self, data: &[u8]);

    /// Returns the digest of everything fed so far without consuming the state.
    fn finalize(&self) -> Self::Output;

    /// Returns the state to its initial value.
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Computes the digest of `data` in one call.
    #[must_use]
    fn checksum(data: &[u8]) -> Self::Output {
        let mut state = Self::default();
        state.update(data);
        state.finalize()
    }
}
