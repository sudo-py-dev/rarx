use std::fmt;

/// Failure while decoding untrusted bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer bytes remained than the field needs.
    UnexpectedEof {
        /// Offset at which the read started.
        offset: usize,
        /// Bytes the read needed.
        needed: usize,
    },
    /// A variable-length integer does not fit in `u64`.
    IntegerOverflow {
        /// Offset at which the integer started.
        offset: usize,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { offset, needed } => {
                write!(
                    f,
                    "unexpected end of data at offset {offset} (needed {needed} bytes)"
                )
            }
            Self::IntegerOverflow { offset } => {
                write!(
                    f,
                    "variable-length integer at offset {offset} overflows 64 bits"
                )
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Bounds-checked little-endian reader over a byte slice.
#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Creates a cursor positioned at the start of `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Current offset from the start of the slice.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Number of unread bytes.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Returns the next `len` bytes and advances past them.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if fewer than `len` bytes remain.
    pub fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], ParseError> {
        let eof = ParseError::UnexpectedEof {
            offset: self.pos,
            needed: len,
        };
        let end = self.pos.checked_add(len).ok_or(eof)?;
        let bytes = self.data.get(self.pos..end).ok_or(eof)?;
        self.pos = end;
        Ok(bytes)
    }

    /// Advances past `len` bytes.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if fewer than `len` bytes remain.
    pub fn skip(&mut self, len: usize) -> Result<(), ParseError> {
        self.read_bytes(len).map(|_| ())
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.read_bytes(N)?);
        Ok(out)
    }

    /// Reads one byte.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] at end of data.
    pub fn read_u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.read_array::<1>()?[0])
    }

    /// Reads a little-endian `u16`.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if fewer than 2 bytes remain.
    pub fn read_u16_le(&mut self) -> Result<u16, ParseError> {
        self.read_array().map(u16::from_le_bytes)
    }

    /// Reads a little-endian `u32`.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if fewer than 4 bytes remain.
    pub fn read_u32_le(&mut self) -> Result<u32, ParseError> {
        self.read_array().map(u32::from_le_bytes)
    }

    /// Reads a little-endian `u64`.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if fewer than 8 bytes remain.
    pub fn read_u64_le(&mut self) -> Result<u64, ParseError> {
        self.read_array().map(u64::from_le_bytes)
    }

    /// Reads a RAR5 `vint`: 7 data bits per byte, least significant group first,
    /// high bit set on every byte except the last.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if the data ends mid-integer, or
    /// [`ParseError::IntegerOverflow`] if the value exceeds `u64`.
    pub fn read_rar_vint(&mut self) -> Result<u64, ParseError> {
        let start = self.pos;
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.read_u8()?;
            let bits = u64::from(byte & 0x7F);
            if shift == 63 && bits > 1 {
                return Err(ParseError::IntegerOverflow { offset: start });
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(ParseError::IntegerOverflow { offset: start })
    }

    /// Reads a 7z `NUMBER`: the count of leading one bits in the first byte gives the
    /// number of extra little-endian bytes; the remaining low bits of the first byte
    /// are the most significant part of the value.
    ///
    /// # Errors
    /// [`ParseError::UnexpectedEof`] if the data ends mid-integer.
    pub fn read_7z_number(&mut self) -> Result<u64, ParseError> {
        let first = self.read_u8()?;
        let extra = first.leading_ones();
        let mut value = 0u64;
        for i in 0..extra {
            value |= u64::from(self.read_u8()?) << (8 * i);
        }
        if extra < 8 {
            let high = u64::from(first & (0x7F >> extra));
            value |= high << (8 * extra);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_fields() {
        let mut c = Cursor::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
        assert_eq!(c.read_u8().unwrap(), 0x01);
        assert_eq!(c.read_u16_le().unwrap(), 0x0302);
        assert_eq!(c.read_u32_le().unwrap(), 0x0706_0504);
        assert_eq!(c.remaining(), 0);
    }

    #[test]
    fn eof_reports_offset_and_does_not_advance() {
        let mut c = Cursor::new(&[0xAA, 0xBB, 0xCC]);
        c.skip(1).unwrap();
        assert_eq!(
            c.read_u32_le(),
            Err(ParseError::UnexpectedEof {
                offset: 1,
                needed: 4
            })
        );
        assert_eq!(c.position(), 1);
    }

    #[test]
    fn huge_length_does_not_overflow() {
        let mut c = Cursor::new(&[0; 4]);
        c.skip(2).unwrap();
        assert!(c.read_bytes(usize::MAX).is_err());
    }

    #[test]
    fn rar_vint() {
        assert_eq!(Cursor::new(&[0x05]).read_rar_vint().unwrap(), 5);
        assert_eq!(Cursor::new(&[0x80, 0x01]).read_rar_vint().unwrap(), 128);
        let max = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
        assert_eq!(Cursor::new(&max).read_rar_vint().unwrap(), u64::MAX);
    }

    #[test]
    fn rar_vint_overflow() {
        let too_big = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x02];
        assert_eq!(
            Cursor::new(&too_big).read_rar_vint(),
            Err(ParseError::IntegerOverflow { offset: 0 })
        );
        assert!(Cursor::new(&[0x80; 11]).read_rar_vint().is_err());
    }

    #[test]
    fn sevenzip_number() {
        assert_eq!(Cursor::new(&[0x7F]).read_7z_number().unwrap(), 0x7F);
        assert_eq!(Cursor::new(&[0x80, 0xFF]).read_7z_number().unwrap(), 0xFF);
        assert_eq!(Cursor::new(&[0x81, 0x23]).read_7z_number().unwrap(), 0x0123);
        assert_eq!(
            Cursor::new(&[0xC1, 0x34, 0x12]).read_7z_number().unwrap(),
            0x01_1234
        );
        let mut full = vec![0xFF];
        full.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(Cursor::new(&full).read_7z_number().unwrap(), u64::MAX);
    }
}
