//! PKWARE traditional ZIP encryption ("ZipCrypto"). Weak: kept for compatibility.

use std::io::{self, Read, Write};

use zeroize::Zeroize;

use crate::{WrongPassword, random_bytes, read_full};

/// Length of the encryption header that precedes the data.
pub const HEADER_LEN: usize = 12;

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)] // i < 256
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc_step(crc: u32, byte: u8) -> u32 {
    CRC_TABLE[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8)
}

/// The three-word cipher state.
#[derive(Clone)]
struct Keys([u32; 3]);

impl Keys {
    fn new(password: &[u8]) -> Self {
        let mut keys = Self([0x1234_5678, 0x2345_6789, 0x3456_7890]);
        for &b in password {
            keys.update(b);
        }
        keys
    }

    fn update(&mut self, plain: u8) {
        let [k0, k1, k2] = &mut self.0;
        *k0 = crc_step(*k0, plain);
        *k1 = k1
            .wrapping_add(*k0 & 0xFF)
            .wrapping_mul(134_775_813)
            .wrapping_add(1);
        *k2 = crc_step(*k2, k1.to_le_bytes()[3]);
    }

    fn stream_byte(&self) -> u8 {
        let t = (self.0[2] | 2) & 0xFFFF;
        (t.wrapping_mul(t ^ 1) >> 8).to_le_bytes()[0]
    }

    fn decrypt(&mut self, byte: u8) -> u8 {
        let plain = byte ^ self.stream_byte();
        self.update(plain);
        plain
    }

    fn encrypt(&mut self, plain: u8) -> u8 {
        let byte = plain ^ self.stream_byte();
        self.update(plain);
        byte
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Decrypts ZipCrypto data, starting with its 12-byte header.
pub struct DecryptReader<R> {
    inner: R,
    keys: Keys,
}

impl<R: Read> DecryptReader<R> {
    /// Reads and checks the encryption header. `check` is the high byte of the CRC,
    /// or of the DOS time when the item uses a data descriptor.
    ///
    /// # Errors
    /// [`WrongPassword`] (inside an [`io::Error`]) if the check byte differs, or
    /// any read error. One wrong password in 256 passes this check.
    pub fn new(mut inner: R, password: &[u8], check: u8) -> io::Result<Self> {
        let mut keys = Keys::new(password);
        let mut header = [0u8; HEADER_LEN];
        if read_full(&mut inner, &mut header)? != HEADER_LEN {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "encryption header is truncated",
            ));
        }
        for b in &mut header {
            *b = keys.decrypt(*b);
        }
        if header[HEADER_LEN - 1] != check {
            return Err(WrongPassword.into());
        }
        Ok(Self { inner, keys })
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        for b in &mut buf[..n] {
            *b = self.keys.decrypt(*b);
        }
        Ok(n)
    }
}

/// Encrypts data with ZipCrypto, writing the header first.
pub struct EncryptWriter<W> {
    inner: W,
    keys: Keys,
    buf: Vec<u8>,
}

impl<W: Write> EncryptWriter<W> {
    /// Writes a random encryption header ending in `check` (see
    /// [`DecryptReader::new`]).
    ///
    /// # Errors
    /// Any I/O error, or failure of the OS random source.
    pub fn new(mut inner: W, password: &[u8], check: u8) -> io::Result<Self> {
        let mut keys = Keys::new(password);
        let mut header = [0u8; HEADER_LEN];
        random_bytes(&mut header[..HEADER_LEN - 1])?;
        header[HEADER_LEN - 1] = check;
        for b in &mut header {
            *b = keys.encrypt(*b);
        }
        inner.write_all(&header)?;
        Ok(Self {
            inner,
            keys,
            buf: Vec::new(),
        })
    }

    /// Returns the sink.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.clear();
        self.buf.extend(data.iter().map(|&b| self.keys.encrypt(b)));
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
    fn round_trip_and_check_byte() {
        let mut writer = EncryptWriter::new(Vec::new(), b"pw", 0xAB).unwrap();
        writer.write_all(b"hello zipcrypto").unwrap();
        let sealed = writer.into_inner();
        assert_eq!(sealed.len(), HEADER_LEN + 15);

        let mut out = String::new();
        DecryptReader::new(sealed.as_slice(), b"pw", 0xAB)
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, "hello zipcrypto");

        let wrong = DecryptReader::new(sealed.as_slice(), b"pw", 0xAC)
            .err()
            .unwrap();
        assert!(WrongPassword::find(&wrong));
    }

    #[test]
    fn crc_table_matches_crc32() {
        let crc = b"123456789".iter().fold(!0u32, |c, &b| crc_step(c, b));
        assert_eq!(!crc, 0xCBF4_3926);
    }
}
