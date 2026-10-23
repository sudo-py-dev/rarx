//! A file shared by several positioned readers (pack streams, TAR items).

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use crate::ReadSeek;

/// The archive file, shared by every reader positioned inside it.
pub(crate) type Shared = Arc<Mutex<Box<dyn ReadSeek>>>;

/// Reads one byte range of a shared file, seeking before each read.
pub(crate) struct Region {
    src: Shared,
    pos: u64,
    end: u64,
}

impl Region {
    pub(crate) fn new(src: &Shared, offset: u64, size: u64) -> Self {
        Self {
            src: Arc::clone(src),
            pos: offset,
            end: offset.saturating_add(size),
        }
    }
}

impl Read for Region {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_sub(self.pos);
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        if want == 0 {
            return Ok(0);
        }
        let mut src = self
            .src
            .lock()
            .map_err(|_| io::Error::other("archive reader poisoned"))?;
        src.seek(SeekFrom::Start(self.pos))?;
        let n = src.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Region {
    /// Moves within the region; positions are absolute file offsets.
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            SeekFrom::End(delta) => self.end.checked_add_signed(delta),
        };
        self.pos = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        Ok(self.pos)
    }
}
