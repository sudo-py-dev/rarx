//! 7z AES-256-CBC (`06F10701`): the coder properties, the iterated SHA-256 key
//! derivation, and CBC stream adapters without padding.

use std::collections::HashMap;
use std::io::{self, Read, Write};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::block::{Aes, BLOCK};
use crate::{Password, random_bytes, read_full};

/// Iteration exponent 7-Zip uses for new archives.
pub const DEFAULT_CYCLES_POWER: u8 = 19;
/// Exponent meaning "no hashing": the key is salt ‖ password, zero padded.
const RAW_KEY: u8 = 0x3F;
/// Largest exponent accepted when reading (as in 7-Zip); bounds the work a
/// hostile archive can demand.
const MAX_CYCLES_POWER: u8 = 24;
const CHUNK: usize = 64 * BLOCK;

/// A derived 256-bit key.
pub type Key = Zeroizing<[u8; 32]>;

/// Parameters stored in the 7zAES coder properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AesParams {
    /// Key derivation runs `2^cycles_power` SHA-256 rounds.
    pub cycles_power: u8,
    /// Salt mixed into the key derivation (0–16 bytes).
    pub salt: Vec<u8>,
    /// CBC initialisation vector, zero padded to 16 bytes.
    pub iv: [u8; BLOCK],
    iv_len: usize,
}

impl AesParams {
    /// Fresh parameters for encryption: a random 16-byte IV, no salt (like 7-Zip).
    ///
    /// # Errors
    /// If the OS random source fails.
    pub fn generate(cycles_power: u8) -> io::Result<Self> {
        let mut iv = [0u8; BLOCK];
        random_bytes(&mut iv)?;
        Ok(Self {
            cycles_power,
            salt: Vec::new(),
            iv,
            iv_len: BLOCK,
        })
    }

    /// Parses coder properties; `None` if they are malformed or demand more than
    /// `2^24` hashing rounds.
    #[must_use]
    pub fn parse(props: &[u8]) -> Option<Self> {
        let mut params = Self {
            cycles_power: 0,
            salt: Vec::new(),
            iv: [0; BLOCK],
            iv_len: 0,
        };
        let Some((&b0, rest)) = props.split_first() else {
            return Some(params);
        };
        params.cycles_power = b0 & 0x3F;
        if params.cycles_power > MAX_CYCLES_POWER && params.cycles_power != RAW_KEY {
            return None;
        }
        if b0 & 0xC0 == 0 {
            return rest.is_empty().then_some(params);
        }
        let (&b1, rest) = rest.split_first()?;
        let salt_len = usize::from((b0 >> 7) & 1) + usize::from(b1 >> 4);
        let iv_len = usize::from((b0 >> 6) & 1) + usize::from(b1 & 0x0F);
        if rest.len() != salt_len + iv_len {
            return None;
        }
        params.salt = rest[..salt_len].to_vec();
        params.iv[..iv_len].copy_from_slice(&rest[salt_len..]);
        params.iv_len = iv_len;
        Some(params)
    }

    /// Encodes the parameters as coder properties.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let salt_len = self.salt.len().min(16);
        let iv_len = self.iv_len.min(BLOCK);
        let mut b0 = self.cycles_power & 0x3F;
        let mut b1 = 0u8;
        if salt_len > 0 {
            b0 |= 0x80;
            b1 |= u8::try_from(salt_len - 1).unwrap_or(0) << 4;
        }
        if iv_len > 0 {
            b0 |= 0x40;
            b1 |= u8::try_from(iv_len - 1).unwrap_or(0);
        }
        let mut out = vec![b0];
        if salt_len + iv_len > 0 {
            out.push(b1);
            out.extend_from_slice(&self.salt[..salt_len]);
            out.extend_from_slice(&self.iv[..iv_len]);
        }
        out
    }
}

/// Derives the AES key from `password` per 7-Zip's `CalcKey`.
#[must_use]
pub fn derive_key(password: &Password, params: &AesParams) -> Key {
    let pw = password.utf16le();
    let mut key = Zeroizing::new([0u8; 32]);
    if params.cycles_power == RAW_KEY {
        let joined = Zeroizing::new([params.salt.as_slice(), pw.as_slice()].concat());
        let n = joined.len().min(32);
        key[..n].copy_from_slice(&joined[..n]);
        return key;
    }
    let mut sha = Sha256::new();
    for round in 0..1u64 << params.cycles_power {
        sha.update(&params.salt);
        sha.update(pw.as_slice());
        sha.update(round.to_le_bytes());
    }
    key.copy_from_slice(&sha.finalize());
    key
}

/// Remembers derived keys so folders sharing a salt and cost pay for one derivation.
#[derive(Default)]
pub struct KeyCache {
    keys: HashMap<(u8, Vec<u8>), Key>,
}

impl KeyCache {
    /// The key for `password` under `params`, derived on first use.
    pub fn key(&mut self, password: &Password, params: &AesParams) -> Key {
        self.keys
            .entry((params.cycles_power, params.salt.clone()))
            .or_insert_with(|| derive_key(password, params))
            .clone()
    }

    /// Forgets every key (call when the password changes).
    pub fn clear(&mut self) {
        self.keys.clear();
    }
}

/// Decrypts an AES-256-CBC stream (no padding is removed).
pub struct DecryptReader<R> {
    inner: R,
    aes: Aes,
    prev: [u8; BLOCK],
    buf: Box<[u8; CHUNK]>,
    pos: usize,
    len: usize,
}

impl<R: Read> DecryptReader<R> {
    /// Wraps `inner`, which yields ciphertext.
    pub fn new(inner: R, key: &Key, params: &AesParams) -> Self {
        Self {
            inner,
            aes: aes256(key),
            prev: params.iv,
            buf: Box::new([0; CHUNK]),
            pos: 0,
            len: 0,
        }
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.len {
            let n = read_full(&mut self.inner, &mut self.buf[..])?;
            if n % BLOCK != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "AES data is not a whole number of blocks",
                ));
            }
            for block in self.buf[..n].as_chunks_mut::<BLOCK>().0 {
                let cipher = *block;
                self.aes.decrypt(block);
                for (b, p) in block.iter_mut().zip(self.prev) {
                    *b ^= p;
                }
                self.prev = cipher;
            }
            (self.pos, self.len) = (0, n);
        }
        let n = out.len().min(self.len - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Encrypts with AES-256-CBC, zero-padding the last block on [`EncryptWriter::finish`].
pub struct EncryptWriter<W: Write> {
    inner: W,
    aes: Aes,
    prev: [u8; BLOCK],
    pending: Vec<u8>,
}

impl<W: Write> EncryptWriter<W> {
    /// Wraps `inner`, which receives ciphertext.
    pub fn new(inner: W, key: &Key, params: &AesParams) -> Self {
        Self {
            inner,
            aes: aes256(key),
            prev: params.iv,
            pending: Vec::with_capacity(CHUNK + BLOCK),
        }
    }

    fn seal_whole_blocks(&mut self) -> io::Result<()> {
        let whole = self.pending.len() / BLOCK * BLOCK;
        for block in self.pending[..whole].as_chunks_mut::<BLOCK>().0 {
            for (b, p) in block.iter_mut().zip(self.prev) {
                *b ^= p;
            }
            self.aes.encrypt(block);
            self.prev = *block;
        }
        self.inner.write_all(&self.pending[..whole])?;
        self.pending.drain(..whole);
        Ok(())
    }

    /// Pads and writes the final block and returns the sink.
    ///
    /// # Errors
    /// Any I/O error from the sink.
    pub fn finish(mut self) -> io::Result<W> {
        let rem = self.pending.len() % BLOCK;
        if rem != 0 {
            self.pending.resize(self.pending.len() + BLOCK - rem, 0);
        }
        self.seal_whole_blocks()?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(CHUNK);
        self.pending.extend_from_slice(&buf[..n]);
        if self.pending.len() >= CHUNK {
            self.seal_whole_blocks()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn aes256(key: &Key) -> Aes {
    Aes::with_key256(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_round_trip() {
        let params = AesParams::generate(19).unwrap();
        let encoded = params.encode();
        assert_eq!(encoded.len(), 2 + 16);
        assert_eq!(encoded[..2], [0x40 | 0x13, 0x0F]);
        assert_eq!(AesParams::parse(&encoded), Some(params));

        let salted = AesParams {
            cycles_power: 6,
            salt: vec![1, 2, 3],
            iv: [9; BLOCK],
            iv_len: 8,
        };
        let parsed = AesParams::parse(&salted.encode()).unwrap();
        assert_eq!(parsed.salt, [1, 2, 3]);
        assert_eq!(parsed.iv[..8], [9; 8]);
        assert_eq!(parsed.iv[8..], [0; 8]);

        assert!(AesParams::parse(&[30]).is_none(), "too many rounds");
        assert!(AesParams::parse(&[0xC0 | 0x13, 0x0F, 1]).is_none(), "short");
    }

    #[test]
    fn raw_key_mode_concatenates() {
        let params = AesParams {
            cycles_power: RAW_KEY,
            salt: vec![0xAA],
            iv: [0; BLOCK],
            iv_len: 0,
        };
        let key = derive_key(&Password::from("ab"), &params);
        assert_eq!(key[..5], [0xAA, b'a', 0, b'b', 0]);
        assert_eq!(key[5..], [0; 27]);
    }

    #[test]
    fn known_key_derivation() {
        // 2^0 rounds: SHA-256("p\0" ‖ 0u64).
        let params = AesParams::parse(&[0]).unwrap();
        let key = derive_key(&Password::from("p"), &params);
        let expected = Sha256::digest([b'p', 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(key.as_slice(), expected.as_slice());
    }

    #[test]
    fn cbc_round_trip_pads_with_zeros() {
        let params = AesParams::generate(0).unwrap();
        let key = derive_key(&Password::from("secret"), &params);
        let plain: Vec<u8> = (0..5000u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let mut writer = EncryptWriter::new(Vec::new(), &key, &params);
        for piece in plain.chunks(333) {
            writer.write_all(piece).unwrap();
        }
        let sealed = writer.finish().unwrap();
        assert_eq!(sealed.len(), 5008);
        let mut opened = Vec::new();
        DecryptReader::new(sealed.as_slice(), &key, &params)
            .read_to_end(&mut opened)
            .unwrap();
        assert_eq!(opened[..5000], plain[..]);
        assert_eq!(opened[5000..], [0; 8]);
        let mut cut = DecryptReader::new(&sealed[..100], &key, &params);
        assert!(cut.read_to_end(&mut Vec::new()).is_err());
    }
}
