//! Archive encryption for RarX (AGENT.md §8): AES-CBC/CTR, ZipCrypto, the per-format
//! key-derivation functions, and zeroizing password handling.
//!
//! Every cipher is exposed as a streaming [`std::io::Read`] (decrypt) or
//! [`std::io::Write`] (encrypt) adapter, so container code only wires streams.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod block;
mod password;
pub mod sevenz;
pub mod winzip;
pub mod zipcrypto;

use std::{fmt, io};

pub use password::Password;

/// Encryption schemes RarX reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Scheme {
    /// 7z: AES-256-CBC, iterated SHA-256 KDF.
    SevenZipAes256,
    /// RAR5: AES-256-CBC, PBKDF2-HMAC-SHA256.
    Rar5Aes256,
    /// RAR 3.x/4.x: AES-128-CBC, SHA-1 based KDF (read only).
    Rar3Aes128,
    /// RAR 2.0 stream cipher (read only).
    Rar20,
    /// RAR 1.5 stream cipher (read only).
    Rar15,
    /// PKWARE traditional encryption.
    ZipCrypto,
    /// WinZip AES (AE-1/AE-2) with a 128-, 192- or 256-bit key.
    WinZipAes {
        /// Key length in bits.
        key_bits: u16,
    },
}

impl Scheme {
    /// Whether RarX can create archives with this scheme.
    #[must_use]
    pub fn can_encrypt(self) -> bool {
        !matches!(self, Self::Rar3Aes128 | Self::Rar20 | Self::Rar15)
    }
}

/// The password does not match the archive's check value. Carried inside an
/// [`io::Error`] of kind [`io::ErrorKind::InvalidData`]; use [`WrongPassword::find`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrongPassword;

impl WrongPassword {
    /// Whether `err` (or the error it wraps) reports a wrong password.
    #[must_use]
    pub fn find(err: &io::Error) -> bool {
        err.get_ref()
            .is_some_and(|inner| inner.downcast_ref::<Self>().is_some())
    }
}

impl fmt::Display for WrongPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("wrong password")
    }
}

impl std::error::Error for WrongPassword {}

impl From<WrongPassword> for io::Error {
    fn from(err: WrongPassword) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, err)
    }
}

/// Fills `buf` with bytes from the operating system's secure random source.
///
/// # Errors
/// If the OS random source is unavailable.
pub fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    getrandom::fill(buf).map_err(|err| io::Error::other(format!("random source failed: {err}")))
}

/// Reads until `buf` is full or the source ends; returns the bytes read.
fn read_full(src: &mut impl io::Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match src.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}
