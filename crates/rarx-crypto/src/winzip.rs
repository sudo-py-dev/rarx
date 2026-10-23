//! WinZip AES (AE-1 / AE-2): PBKDF2-HMAC-SHA1 key derivation, AES in little-endian
//! counter mode, and a 10-byte HMAC-SHA1 authentication code over the ciphertext.

use std::io::{Read, Write};
use std::{fmt, io};

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use zeroize::Zeroizing;

use crate::block::{Aes, BLOCK};
use crate::{WrongPassword, random_bytes, read_full};

const ITERATIONS: u32 = 1000;
const VERIFIER_LEN: usize = 2;
/// Length of the authentication code after the data.
pub const AUTH_LEN: usize = 10;

/// AES key strength, as stored in the `0x9901` extra field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strength {
    /// AES-128.
    Aes128,
    /// AES-192.
    Aes192,
    /// AES-256.
    Aes256,
}

impl Strength {
    /// Parses the extra-field code (1, 2 or 3).
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Aes128),
            2 => Some(Self::Aes192),
            3 => Some(Self::Aes256),
            _ => None,
        }
    }

    /// The extra-field code.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Aes128 => 1,
            Self::Aes192 => 2,
            Self::Aes256 => 3,
        }
    }

    /// Key length in bits.
    #[must_use]
    pub fn bits(self) -> u16 {
        match self {
            Self::Aes128 => 128,
            Self::Aes192 => 192,
            Self::Aes256 => 256,
        }
    }

    fn key_len(self) -> usize {
        usize::from(self.bits() / 8)
    }

    /// Length of the salt that starts the item data.
    #[must_use]
    pub fn salt_len(self) -> usize {
        self.key_len() / 2
    }

    /// Bytes added to the data: salt, verifier and authentication code.
    #[must_use]
    pub fn overhead(self) -> u64 {
        (self.salt_len() + VERIFIER_LEN + AUTH_LEN) as u64
    }
}

/// The data does not match its authentication code (damaged or tampered with).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticationFailed;

impl AuthenticationFailed {
    /// Whether `err` carries this error.
    #[must_use]
    pub fn find(err: &io::Error) -> bool {
        err.get_ref()
            .is_some_and(|inner| inner.downcast_ref::<Self>().is_some())
    }
}

impl fmt::Display for AuthenticationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("authentication code mismatch")
    }
}

impl std::error::Error for AuthenticationFailed {}

struct Keys {
    aes: Aes,
    mac: Hmac<Sha1>,
    verifier: [u8; VERIFIER_LEN],
}

fn derive(password: &[u8], salt: &[u8], strength: Strength) -> io::Result<Keys> {
    let n = strength.key_len();
    let mut material = Zeroizing::new(vec![0u8; 2 * n + VERIFIER_LEN]);
    pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, ITERATIONS, &mut material);
    let (Some(aes), Ok(mac)) = (
        Aes::new(&material[..n]),
        Hmac::<Sha1>::new_from_slice(&material[n..2 * n]),
    ) else {
        return Err(io::Error::other("AES key setup failed"));
    };
    Ok(Keys {
        aes,
        mac,
        verifier: [material[2 * n], material[2 * n + 1]],
    })
}

/// Little-endian 128-bit counter mode starting at 1.
struct Ctr {
    counter: u128,
    stream: [u8; BLOCK],
    used: usize,
}

impl Ctr {
    fn new() -> Self {
        Self {
            counter: 0,
            stream: [0; BLOCK],
            used: BLOCK,
        }
    }

    fn apply(&mut self, aes: &Aes, data: &mut [u8]) {
        for b in data {
            if self.used == BLOCK {
                self.counter = self.counter.wrapping_add(1);
                self.stream = self.counter.to_le_bytes();
                aes.encrypt(&mut self.stream);
                self.used = 0;
            }
            *b ^= self.stream[self.used];
            self.used += 1;
        }
    }
}

/// Decrypts and authenticates one WinZip AES item.
pub struct DecryptReader<R> {
    inner: R,
    keys: Keys,
    ctr: Ctr,
    remaining: u64,
}

impl<R: Read> DecryptReader<R> {
    /// Reads the salt and password verifier. `packed` is the item's whole stored
    /// size, including salt, verifier and authentication code.
    ///
    /// # Errors
    /// [`WrongPassword`] (inside an [`io::Error`]) if the verifier differs, or a
    /// read error. One wrong password in 65 536 passes this check; the
    /// authentication code then fails at the end.
    pub fn new(mut inner: R, password: &[u8], strength: Strength, packed: u64) -> io::Result<Self> {
        let remaining = packed
            .checked_sub(strength.overhead())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "AES item is too short"))?;
        let mut head = [0u8; 16 + VERIFIER_LEN];
        let head = &mut head[..strength.salt_len() + VERIFIER_LEN];
        if read_full(&mut inner, head)? != head.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AES header is truncated",
            ));
        }
        let (salt, verifier) = head.split_at(strength.salt_len());
        let keys = derive(password, salt, strength)?;
        if keys.verifier != verifier {
            return Err(WrongPassword.into());
        }
        Ok(Self {
            inner,
            keys,
            ctr: Ctr::new(),
            remaining,
        })
    }

    fn check_auth(&mut self) -> io::Result<()> {
        let mut code = [0u8; AUTH_LEN];
        if read_full(&mut self.inner, &mut code)? != AUTH_LEN {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AES authentication code is missing",
            ));
        }
        self.keys
            .mac
            .clone()
            .verify_truncated_left(&code)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, AuthenticationFailed))
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            if !buf.is_empty() {
                self.check_auth()?;
                self.remaining = u64::MAX; // checked; report EOF from now on
            }
            return Ok(0);
        }
        if self.remaining == u64::MAX {
            return Ok(0);
        }
        let want = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AES data is truncated",
            ));
        }
        self.keys.mac.update(&buf[..n]);
        self.ctr.apply(&self.keys.aes, &mut buf[..n]);
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Encrypts one WinZip AES item: writes salt and verifier first, the
/// authentication code on [`EncryptWriter::finish`].
pub struct EncryptWriter<W: Write> {
    inner: W,
    keys: Keys,
    ctr: Ctr,
    buf: Vec<u8>,
}

impl<W: Write> EncryptWriter<W> {
    /// Starts an item with a random salt.
    ///
    /// # Errors
    /// Any I/O error, or failure of the OS random source.
    pub fn new(mut inner: W, password: &[u8], strength: Strength) -> io::Result<Self> {
        let mut salt = [0u8; 16];
        let salt = &mut salt[..strength.salt_len()];
        random_bytes(salt)?;
        let keys = derive(password, salt, strength)?;
        inner.write_all(salt)?;
        inner.write_all(&keys.verifier)?;
        Ok(Self {
            inner,
            keys,
            ctr: Ctr::new(),
            buf: Vec::new(),
        })
    }

    /// Writes the authentication code and returns the sink.
    ///
    /// # Errors
    /// Any I/O error from the sink.
    pub fn finish(mut self) -> io::Result<W> {
        let code = self.keys.mac.clone().finalize().into_bytes();
        self.inner.write_all(&code[..AUTH_LEN])?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.clear();
        self.buf.extend_from_slice(data);
        self.ctr.apply(&self.keys.aes, &mut self.buf);
        self.keys.mac.update(&self.buf);
        self.inner.write_all(&self.buf)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_every_strength() {
        let plain: Vec<u8> = (0..3000u32)
            .map(|i| u8::try_from(i * 7 % 256).unwrap())
            .collect();
        for strength in [Strength::Aes128, Strength::Aes192, Strength::Aes256] {
            let mut writer = EncryptWriter::new(Vec::new(), b"pass", strength).unwrap();
            for piece in plain.chunks(100) {
                writer.write_all(piece).unwrap();
            }
            let sealed = writer.finish().unwrap();
            assert_eq!(
                sealed.len() as u64,
                plain.len() as u64 + strength.overhead()
            );
            let packed = sealed.len() as u64;

            let mut out = Vec::new();
            DecryptReader::new(sealed.as_slice(), b"pass", strength, packed)
                .unwrap()
                .read_to_end(&mut out)
                .unwrap();
            assert_eq!(out, plain, "{strength:?}");

            let mut tampered = sealed.clone();
            tampered[strength.salt_len() + 5] ^= 1;
            let err = DecryptReader::new(tampered.as_slice(), b"pass", strength, packed)
                .unwrap()
                .read_to_end(&mut Vec::new())
                .unwrap_err();
            assert!(AuthenticationFailed::find(&err));
        }
    }

    #[test]
    fn pbkdf2_sha1_vector() {
        // RFC 6070, 2 iterations.
        let mut out = [0u8; 20];
        pbkdf2::pbkdf2_hmac::<Sha1>(b"password", b"salt", 2, &mut out);
        assert_eq!(
            out,
            [
                0xea, 0x6c, 0x01, 0x4d, 0xc7, 0x2d, 0x6f, 0x8c, 0xcd, 0x1e, 0xd9, 0x2a, 0xce, 0x1d,
                0x41, 0xf0, 0xd8, 0xde, 0x89, 0x57
            ]
        );
    }
}
