use std::io::{self, Read};

use rarx_checksum::{Checksum, Crc32};

use crate::DataError;

/// Passes data through while checking it against the expected size and, when
/// known, CRC-32.
///
/// Fails as soon as more data than `expected_size` arrives, so a lying header
/// cannot turn into unbounded output.
pub(crate) struct VerifyReader<R> {
    inner: R,
    crc: Crc32,
    expected_crc: Option<u32>,
    expected_size: u64,
    seen: u64,
}

impl<R: Read> VerifyReader<R> {
    pub(crate) fn new(inner: R, expected_size: u64, expected_crc: Option<u32>) -> Self {
        Self {
            inner,
            crc: Crc32::default(),
            expected_crc,
            expected_size,
            seen: 0,
        }
    }
}

impl<R: Read> Read for VerifyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        if n == 0 {
            if self.seen != self.expected_size {
                return Err(DataError::SizeMismatch.into());
            }
            if self
                .expected_crc
                .is_some_and(|crc| crc != self.crc.finalize())
            {
                return Err(DataError::ChecksumMismatch.into());
            }
            return Ok(0);
        }
        self.seen += n as u64;
        if self.seen > self.expected_size {
            return Err(DataError::SizeMismatch.into());
        }
        self.crc.update(&buf[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(data: &[u8], size: u64, crc: u32) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        VerifyReader::new(data, size, Some(crc)).read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn accepts_matching_data() {
        let crc = Crc32::checksum(b"hello");
        assert_eq!(read_all(b"hello", 5, crc).unwrap(), b"hello");
    }

    #[test]
    fn rejects_bad_crc() {
        let err = read_all(b"hello", 5, 0).unwrap_err();
        assert_eq!(DataError::find(&err), Some(DataError::ChecksumMismatch));
    }

    #[test]
    fn rejects_short_and_long_data() {
        let crc = Crc32::checksum(b"hello");
        let short = read_all(b"hello", 6, crc).unwrap_err();
        assert_eq!(DataError::find(&short), Some(DataError::SizeMismatch));
        let long = read_all(b"hello", 4, crc).unwrap_err();
        assert_eq!(DataError::find(&long), Some(DataError::SizeMismatch));
    }
}
