//! AES with a key length chosen at run time.

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes::{Aes128, Aes192, Aes256};

pub(crate) const BLOCK: usize = 16;

pub(crate) enum Aes {
    A128(Box<Aes128>),
    A192(Box<Aes192>),
    A256(Box<Aes256>),
}

impl Aes {
    /// A cipher for a 16-, 24- or 32-byte key.
    pub(crate) fn new(key: &[u8]) -> Option<Self> {
        Some(match key.len() {
            16 => Self::A128(Box::new(Aes128::new_from_slice(key).ok()?)),
            24 => Self::A192(Box::new(Aes192::new_from_slice(key).ok()?)),
            32 => Self::A256(Box::new(Aes256::new_from_slice(key).ok()?)),
            _ => return None,
        })
    }

    pub(crate) fn with_key256(key: &[u8; 32]) -> Self {
        Self::A256(Box::new(Aes256::new(key.into())))
    }

    pub(crate) fn encrypt(&self, block: &mut [u8; BLOCK]) {
        match self {
            Self::A128(c) => c.encrypt_block(block.into()),
            Self::A192(c) => c.encrypt_block(block.into()),
            Self::A256(c) => c.encrypt_block(block.into()),
        }
    }

    pub(crate) fn decrypt(&self, block: &mut [u8; BLOCK]) {
        match self {
            Self::A128(c) => c.decrypt_block(block.into()),
            Self::A192(c) => c.decrypt_block(block.into()),
            Self::A256(c) => c.decrypt_block(block.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    #[test]
    fn fips197_vectors() {
        // FIPS-197 appendix C.1 and C.3.
        let plain: [u8; 16] = core::array::from_fn(|i| u8::try_from(i * 0x11).unwrap());
        let key128: Vec<u8> = (0..16).collect();
        let key256: Vec<u8> = (0..32).collect();
        for (key, expected) in [
            (key128, "69c4e0d86a7b0430d8cdb78070b4c55a"),
            (key256, "8ea2b7ca516745bfeafc49904b496089"),
        ] {
            let aes = Aes::new(&key).unwrap();
            let mut block = plain;
            aes.encrypt(&mut block);
            let hex = block.iter().fold(String::new(), |mut hex, b| {
                let _ = write!(hex, "{b:02x}");
                hex
            });
            assert_eq!(hex, expected);
            aes.decrypt(&mut block);
            assert_eq!(block, plain);
        }
        assert!(Aes::new(&[0; 15]).is_none());
    }
}
