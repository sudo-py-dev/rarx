//! ZIP / ZIP64 writer (AGENT.md §6.4).
//!
//! Each item is written as local header + data, after which the header is patched
//! with the real CRC and sizes, so no data descriptors are needed for new items.
//! ZIP64 records are used only where a value does not fit the classic fields.

use std::io::{self, Read, Seek, SeekFrom, Write};

use flate2::Compression;
use flate2::write::DeflateEncoder;
use rarx_checksum::{Checksum, Crc32};

use super::{
    CENTRAL_SIG, EOCD_SIG, FLAG_DATA_DESCRIPTOR, FLAG_UTF8, LOCAL_SIG, METHOD_DEFLATE,
    METHOD_STORE, RawHeader, ZIP64_EOCD_SIG, ZIP64_LOCATOR_SIG,
};
use crate::{CivilTime, EntryKind, FileTime};

const DATA_DESCRIPTOR_SIG: u32 = 0x0807_4B50;
const MADE_BY_VERSION: u16 = 63;
const NEEDS_DEFAULT: u16 = 20;
const NEEDS_ZIP64: u16 = 45;
const HOST_DOS: u16 = 0;
const HOST_UNIX: u16 = 3;
const DOS_DIRECTORY: u32 = 0x10;
const DOS_ARCHIVE: u32 = 0x20;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;
const S_IFREG: u32 = 0o100_000;

/// Files up to this size are compressed in memory first so they can fall back to
/// Store when Deflate would make them bigger.
const BUFFERED_LIMIT: u64 = 4 << 20;
/// Size hints at or above this get ZIP64 fields in their local header up front.
const ZIP64_LOCAL_THRESHOLD: u64 = 0xFFFF_0000;
const MAX_32: u64 = 0xFFFF_FFFF;

/// Compression method for new items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipMethod {
    /// No compression.
    Store,
    /// Deflate at level 1–9.
    Deflate {
        /// Compression level, 1 (fastest) to 9 (best).
        level: u32,
    },
}

/// Metadata for an item being added.
#[derive(Clone, Debug)]
pub struct NewItem {
    /// `/`-separated path inside the archive, without a trailing slash.
    pub name: String,
    /// File, directory or symbolic link.
    pub kind: EntryKind,
    /// Modification time as an absolute time (stored in the UT extra field).
    pub modified: Option<FileTime>,
    /// Modification time in local wall-clock time (stored in the DOS fields).
    pub modified_local: Option<CivilTime>,
    /// Unix permission bits; when set, the item is marked as made on Unix.
    pub unix_mode: Option<u32>,
    /// MS-DOS / Windows attribute bits.
    pub dos_attributes: Option<u32>,
    /// Expected uncompressed size, used to decide on ZIP64 fields up front.
    pub size_hint: u64,
}

impl NewItem {
    /// Creates item metadata with only a name and kind.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: EntryKind) -> Self {
        Self {
            name: name.into(),
            kind,
            modified: None,
            modified_local: None,
            unix_mode: None,
            dos_attributes: None,
            size_hint: 0,
        }
    }
}

struct Record {
    header: RawHeader,
    size: u64,
    packed: u64,
    local_offset: u64,
}

/// Writes a new ZIP archive to a seekable sink.
pub struct ZipWriter<W: Write + Seek> {
    out: W,
    records: Vec<Record>,
    force_zip64: bool,
}

impl<W: Write + Seek> ZipWriter<W> {
    /// Starts an archive at the current position of `out`.
    pub fn new(out: W) -> Self {
        Self {
            out,
            records: Vec::new(),
            force_zip64: false,
        }
    }

    /// Writes ZIP64 fields and end records even when not required (for testing
    /// readers against ZIP64 without multi-gigabyte files).
    pub fn set_force_zip64(&mut self, force: bool) {
        self.force_zip64 = force;
    }

    /// Number of items written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no items have been written yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Adds a regular file whose contents are read from `data`.
    ///
    /// # Errors
    /// Any I/O error from reading `data` or writing the archive.
    pub fn add_file(
        &mut self,
        item: &NewItem,
        method: ZipMethod,
        data: &mut dyn Read,
    ) -> io::Result<()> {
        let level = match method {
            ZipMethod::Deflate { level } if item.size_hint <= BUFFERED_LIMIT => {
                return self.add_buffered(item, level, data);
            }
            ZipMethod::Deflate { level } => Some(level),
            ZipMethod::Store => None,
        };
        let zip64_local = self.force_zip64 || item.size_hint >= ZIP64_LOCAL_THRESHOLD;
        let compression = level.map_or(METHOD_STORE, |_| METHOD_DEFLATE);
        let mut header = new_header(item, compression, level);
        let local_offset = self.out.stream_position()?;
        self.write_local(&header, 0, 0, zip64_local)?;
        let data_start = self.out.stream_position()?;

        let mut source = CrcReader::new(data);
        match level {
            Some(level) => {
                let mut encoder = DeflateEncoder::new(&mut self.out, Compression::new(level));
                io::copy(&mut source, &mut encoder)?;
                encoder.finish()?;
            }
            None => {
                io::copy(&mut source, &mut self.out)?;
            }
        }
        let data_end = self.out.stream_position()?;
        let (size, packed) = (source.count, data_end - data_start);
        header.crc32 = source.crc.finalize();
        if !zip64_local && (size >= MAX_32 || packed >= MAX_32) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} grew past 4 GiB while being archived", item.name),
            ));
        }
        self.out.seek(SeekFrom::Start(local_offset))?;
        self.write_local(&header, size, packed, zip64_local)?;
        self.out.seek(SeekFrom::Start(data_end))?;
        self.records.push(Record {
            header,
            size,
            packed,
            local_offset,
        });
        Ok(())
    }

    /// Compresses a small file in memory, storing it instead if Deflate does not help.
    fn add_buffered(&mut self, item: &NewItem, level: u32, data: &mut dyn Read) -> io::Result<()> {
        let mut raw = Vec::new();
        (&mut *data)
            .take(BUFFERED_LIMIT + 1)
            .read_to_end(&mut raw)?;
        if raw.len() as u64 > BUFFERED_LIMIT {
            // The file grew since its size was taken; stream the rest.
            let mut chained = raw.as_slice().chain(data);
            let streamed = NewItem {
                size_hint: BUFFERED_LIMIT + 1,
                ..item.clone()
            };
            return self.add_file(&streamed, ZipMethod::Deflate { level }, &mut chained);
        }
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(level));
        encoder.write_all(&raw)?;
        let deflated = encoder.finish()?;
        let (method, level, payload) = if deflated.len() < raw.len() {
            (METHOD_DEFLATE, Some(level), deflated.as_slice())
        } else {
            (METHOD_STORE, None, raw.as_slice())
        };
        let mut header = new_header(item, method, level);
        header.crc32 = Crc32::checksum(&raw);
        let (size, packed) = (raw.len() as u64, payload.len() as u64);
        self.write_item(header, size, packed, &mut &payload[..])
    }

    /// Adds a directory entry.
    ///
    /// # Errors
    /// Any I/O error from writing the archive.
    pub fn add_directory(&mut self, item: &NewItem) -> io::Result<()> {
        let mut header = new_header(item, METHOD_STORE, None);
        header.needed = NEEDS_DEFAULT;
        self.write_item(header, 0, 0, &mut io::empty())
    }

    /// Adds a symbolic link pointing to `target`.
    ///
    /// # Errors
    /// Any I/O error from writing the archive.
    pub fn add_symlink(&mut self, item: &NewItem, target: &str) -> io::Result<()> {
        let mut header = new_header(item, METHOD_STORE, None);
        header.crc32 = Crc32::checksum(target.as_bytes());
        let len = target.len() as u64;
        self.write_item(header, len, len, &mut target.as_bytes())
    }

    /// Copies an item whose compressed bytes and header come from another archive.
    pub(crate) fn add_raw(
        &mut self,
        header: &RawHeader,
        size: u64,
        packed: u64,
        data: &mut dyn Read,
    ) -> io::Result<()> {
        let mut header = header.clone();
        header.extra = without_zip64(&header.extra);
        let descriptor = header.flags & FLAG_DATA_DESCRIPTOR != 0;
        let local_offset = self.out.stream_position()?;
        let zip64 = self.force_zip64 || size >= MAX_32 || packed >= MAX_32;
        self.write_local(&header, size, packed, zip64)?;
        let copied = io::copy(&mut data.take(packed), &mut self.out)?;
        if copied != packed {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "item data is truncated",
            ));
        }
        if descriptor {
            let mut buf = Vec::with_capacity(24);
            put_u32(&mut buf, DATA_DESCRIPTOR_SIG);
            put_u32(&mut buf, header.crc32);
            if zip64 {
                put_u64(&mut buf, packed);
                put_u64(&mut buf, size);
            } else {
                put_u32(&mut buf, clamp32(packed));
                put_u32(&mut buf, clamp32(size));
            }
            self.out.write_all(&buf)?;
        }
        self.records.push(Record {
            header,
            size,
            packed,
            local_offset,
        });
        Ok(())
    }

    fn write_item(
        &mut self,
        header: RawHeader,
        size: u64,
        packed: u64,
        data: &mut dyn Read,
    ) -> io::Result<()> {
        let local_offset = self.out.stream_position()?;
        self.write_local(&header, size, packed, self.force_zip64)?;
        io::copy(data, &mut self.out)?;
        self.records.push(Record {
            header,
            size,
            packed,
            local_offset,
        });
        Ok(())
    }

    /// Writes a local header. With `zip64`, sizes go in a ZIP64 extra field.
    fn write_local(
        &mut self,
        h: &RawHeader,
        size: u64,
        packed: u64,
        zip64: bool,
    ) -> io::Result<()> {
        let mut extra = h.extra.clone();
        if zip64 {
            let mut z = Vec::with_capacity(20);
            put_u16(&mut z, 0x0001);
            put_u16(&mut z, 16);
            put_u64(&mut z, size);
            put_u64(&mut z, packed);
            extra.splice(0..0, z);
        }
        let mut buf = Vec::with_capacity(30 + h.name.len() + extra.len());
        put_u32(&mut buf, LOCAL_SIG);
        put_u16(
            &mut buf,
            if zip64 {
                h.needed.max(NEEDS_ZIP64)
            } else {
                h.needed
            },
        );
        put_u16(&mut buf, h.flags);
        put_u16(&mut buf, h.method);
        put_u16(&mut buf, h.dos_time);
        put_u16(&mut buf, h.dos_date);
        put_u32(&mut buf, h.crc32);
        put_u32(&mut buf, if zip64 { u32::MAX } else { clamp32(packed) });
        put_u32(&mut buf, if zip64 { u32::MAX } else { clamp32(size) });
        put_u16(&mut buf, len16(h.name.len())?);
        put_u16(&mut buf, len16(extra.len())?);
        buf.extend_from_slice(&h.name);
        buf.extend_from_slice(&extra);
        self.out.write_all(&buf)
    }

    /// Writes the central directory and end records and returns the sink.
    ///
    /// # Errors
    /// Any I/O error from writing, or a comment longer than 65 535 bytes.
    pub fn finish(mut self, comment: Option<&str>) -> io::Result<W> {
        let dir_start = self.out.stream_position()?;
        let mut dir = Vec::new();
        for r in &self.records {
            let need = |v: u64| self.force_zip64 || v >= MAX_32;
            let mut z = Vec::new();
            for (value, wanted) in [
                (r.size, need(r.size) || need(r.packed)),
                (r.packed, need(r.size) || need(r.packed)),
                (r.local_offset, need(r.local_offset)),
            ] {
                if wanted {
                    put_u64(&mut z, value);
                }
            }
            let mut extra = Vec::new();
            if !z.is_empty() {
                put_u16(&mut extra, 0x0001);
                put_u16(&mut extra, len16(z.len())?);
                extra.extend_from_slice(&z);
            }
            extra.extend_from_slice(&r.header.extra);
            let sizes64 = need(r.size) || need(r.packed);
            let h = &r.header;
            put_u32(&mut dir, CENTRAL_SIG);
            put_u16(&mut dir, h.made_by);
            put_u16(
                &mut dir,
                if z.is_empty() {
                    h.needed
                } else {
                    h.needed.max(NEEDS_ZIP64)
                },
            );
            put_u16(&mut dir, h.flags);
            put_u16(&mut dir, h.method);
            put_u16(&mut dir, h.dos_time);
            put_u16(&mut dir, h.dos_date);
            put_u32(&mut dir, h.crc32);
            put_u32(&mut dir, if sizes64 { u32::MAX } else { clamp32(r.packed) });
            put_u32(&mut dir, if sizes64 { u32::MAX } else { clamp32(r.size) });
            put_u16(&mut dir, len16(h.name.len())?);
            put_u16(&mut dir, len16(extra.len())?);
            put_u16(&mut dir, len16(h.comment.len())?);
            put_u16(&mut dir, 0);
            put_u16(&mut dir, 0);
            put_u32(&mut dir, h.external);
            put_u32(
                &mut dir,
                if need(r.local_offset) {
                    u32::MAX
                } else {
                    clamp32(r.local_offset)
                },
            );
            dir.extend_from_slice(&h.name);
            dir.extend_from_slice(&extra);
            dir.extend_from_slice(&h.comment);
        }
        self.out.write_all(&dir)?;
        let dir_size = dir.len() as u64;
        let count = self.records.len() as u64;
        let zip64 =
            self.force_zip64 || count >= 0xFFFF || dir_start >= MAX_32 || dir_size >= MAX_32;

        let mut end = Vec::new();
        if zip64 {
            let zip64_pos = dir_start + dir_size;
            put_u32(&mut end, ZIP64_EOCD_SIG);
            put_u64(&mut end, 44);
            put_u16(&mut end, (HOST_UNIX << 8) | MADE_BY_VERSION);
            put_u16(&mut end, NEEDS_ZIP64);
            put_u32(&mut end, 0);
            put_u32(&mut end, 0);
            put_u64(&mut end, count);
            put_u64(&mut end, count);
            put_u64(&mut end, dir_size);
            put_u64(&mut end, dir_start);
            put_u32(&mut end, ZIP64_LOCATOR_SIG);
            put_u32(&mut end, 0);
            put_u64(&mut end, zip64_pos);
            put_u32(&mut end, 1);
        }
        let comment = comment.unwrap_or_default().as_bytes();
        let count16 = if zip64 {
            0xFFFF
        } else {
            u16::try_from(count).unwrap_or(0xFFFF)
        };
        put_u32(&mut end, EOCD_SIG);
        put_u16(&mut end, 0);
        put_u16(&mut end, 0);
        put_u16(&mut end, count16);
        put_u16(&mut end, count16);
        put_u32(&mut end, if zip64 { u32::MAX } else { clamp32(dir_size) });
        put_u32(&mut end, if zip64 { u32::MAX } else { clamp32(dir_start) });
        put_u16(&mut end, len16(comment.len())?);
        end.extend_from_slice(comment);
        self.out.write_all(&end)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Builds the header fields for a new item (CRC is filled in later).
fn new_header(item: &NewItem, method: u16, level: Option<u32>) -> RawHeader {
    let mut name = item.name.trim_matches('/').to_owned();
    if item.kind == EntryKind::Directory {
        name.push('/');
    }
    let mut flags = if name.is_ascii() { 0 } else { FLAG_UTF8 };
    flags |= match level {
        Some(8..=9) => 1 << 1,
        Some(2) => 2 << 1,
        Some(1) => 3 << 1,
        _ => 0,
    };
    let (dos_date, dos_time) = dos_date_time(item.modified_local);

    let type_bits = match item.kind {
        EntryKind::File => S_IFREG,
        EntryKind::Directory => S_IFDIR,
        EntryKind::Symlink => S_IFLNK,
    };
    let mut external = item.dos_attributes.unwrap_or(match item.kind {
        EntryKind::Directory => DOS_DIRECTORY,
        _ => DOS_ARCHIVE,
    });
    if item.kind == EntryKind::Directory {
        external |= DOS_DIRECTORY;
    }
    let host = if let Some(mode) = item.unix_mode {
        external |= ((mode & 0o7777) | type_bits) << 16;
        HOST_UNIX
    } else if item.kind == EntryKind::Symlink {
        external |= (0o777 | S_IFLNK) << 16;
        HOST_UNIX
    } else {
        HOST_DOS
    };

    let mut extra = Vec::new();
    if let Some(FileTime::Unix { seconds, .. }) = item.modified
        && let Ok(seconds) = i32::try_from(seconds)
    {
        put_u16(&mut extra, 0x5455);
        put_u16(&mut extra, 5);
        extra.push(1);
        extra.extend_from_slice(&seconds.to_le_bytes());
    }

    RawHeader {
        made_by: (host << 8) | MADE_BY_VERSION,
        needed: if method == METHOD_STORE {
            10
        } else {
            NEEDS_DEFAULT
        },
        flags,
        method,
        dos_time,
        dos_date,
        crc32: 0,
        external,
        name: name.into_bytes(),
        extra,
        comment: Vec::new(),
    }
}

/// Encodes a local time as MS-DOS `(date, time)`, clamped to 1980–2107.
fn dos_date_time(time: Option<CivilTime>) -> (u16, u16) {
    let Some(t) = time else {
        return ((1 << 5) | 1, 0);
    };
    if t.year < 1980 {
        return ((1 << 5) | 1, 0);
    }
    let field = |v: i8| u16::try_from(v).unwrap_or(0);
    let year = u16::try_from(t.year - 1980).unwrap_or(0).min(127);
    let date = (year << 9) | (field(t.month) << 5) | field(t.day);
    let time = (field(t.hour) << 11) | (field(t.minute) << 5) | (field(t.second) / 2);
    (date, time)
}

/// Removes ZIP64 fields from an extra block; the writer adds fresh ones.
fn without_zip64(extra: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(extra.len());
    for (id, data) in super::extra_fields(extra) {
        if id != 0x0001 {
            put_u16(&mut out, id);
            put_u16(&mut out, u16::try_from(data.len()).unwrap_or(u16::MAX));
            out.extend_from_slice(data);
        }
    }
    out
}

/// Counts and checksums bytes as they are read.
struct CrcReader<R> {
    inner: R,
    crc: Crc32,
    count: u64,
}

impl<R: Read> CrcReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            crc: Crc32::default(),
            count: 0,
        }
    }
}

impl<R: Read> Read for CrcReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }
}

fn clamp32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

fn len16(len: usize) -> io::Result<u16> {
    u16::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "name, comment or extra field exceeds 65 535 bytes",
        )
    })
}

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_time_encoding() {
        let t = CivilTime {
            year: 2026,
            month: 10,
            day: 23,
            hour: 10,
            minute: 47,
            second: 43,
        };
        let (date, time) = dos_date_time(Some(t));
        assert_eq!(
            FileTime::from_dos(date, time),
            Some(FileTime::Local(CivilTime { second: 42, ..t }))
        );
        assert_eq!(dos_date_time(None), (0x21, 0));
    }

    #[test]
    fn zip64_fields_are_stripped() {
        let extra = [
            0x01, 0x00, 0x02, 0x00, 0xAA, 0xBB, 0x55, 0x54, 0x01, 0x00, 0x07,
        ];
        assert_eq!(without_zip64(&extra), [0x55, 0x54, 0x01, 0x00, 0x07]);
    }
}
