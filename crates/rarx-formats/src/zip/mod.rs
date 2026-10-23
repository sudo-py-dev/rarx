//! ZIP / ZIP64 reader (AGENT.md §6.4).
//!
//! The central directory is authoritative for names, sizes and CRCs; local headers
//! are only used to find where each item's data starts. Data prepended to the
//! archive (SFX stubs) is detected by comparing where the central directory is with
//! where the end record says it should be.

mod cp437;

use std::io::{BufReader, Read, Seek, SeekFrom};

use flate2::bufread::DeflateDecoder;
use rarx_checksum::{Checksum, Crc32};
use rarx_io::Cursor;

use crate::verify::VerifyReader;
use crate::{ArchiveReader, Entry, EntryKind, FileTime, FormatError, FormatId, ReadSeek};

const LOCAL_SIG: u32 = 0x0403_4B50;
const CENTRAL_SIG: u32 = 0x0201_4B50;
const EOCD_SIG: u32 = 0x0605_4B50;
const ZIP64_EOCD_SIG: u32 = 0x0606_4B50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4B50;

const EOCD_LEN: u64 = 22;
const ZIP64_EOCD_LEN: u64 = 56;
const ZIP64_LOCATOR_LEN: u64 = 20;
const LOCAL_HEADER_LEN: u64 = 30;
const MAX_COMMENT_LEN: u64 = 0xFFFF;

const FLAG_ENCRYPTED: u16 = 1 << 0;
const FLAG_UTF8: u16 = 1 << 11;

const HOST_UNIX: u8 = 3;
const HOST_MACOS: u8 = 19;

const DOS_DIRECTORY: u32 = 0x10;
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

const METHOD_STORE: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// Where to find an item's data, beyond what [`Entry`] exposes.
#[derive(Clone, Copy, Debug)]
struct Location {
    local_header: u64,
    method: u16,
    flags: u16,
    crc32: u32,
}

/// Location of the central directory, taken from the (ZIP64) end records.
struct Directory {
    offset: u64,
    size: u64,
    /// Bytes of foreign data before the archive (SFX stub), added to every offset.
    shift: u64,
    comment: Option<String>,
}

/// An open ZIP archive.
pub(crate) struct ZipReader {
    src: Box<dyn ReadSeek>,
    entries: Vec<Entry>,
    locations: Vec<Location>,
    comment: Option<String>,
}

impl ZipReader {
    pub(crate) fn new(mut src: Box<dyn ReadSeek>) -> Result<Self, FormatError> {
        let dir = find_directory(&mut src)?;
        let start = dir
            .offset
            .checked_add(dir.shift)
            .ok_or_else(|| corrupt("central directory offset overflows"))?;
        let mut raw = Vec::new();
        src.seek(SeekFrom::Start(start))?;
        (&mut src).take(dir.size).read_to_end(&mut raw)?;
        if raw.len() as u64 != dir.size {
            return Err(corrupt("central directory is truncated"));
        }

        let mut entries = Vec::new();
        let mut locations = Vec::new();
        let mut cur = Cursor::new(&raw);
        while cur.remaining() >= 4 {
            let (entry, mut location) = parse_central_header(&mut cur)?;
            location.local_header = location
                .local_header
                .checked_add(dir.shift)
                .ok_or_else(|| corrupt("local header offset overflows"))?;
            entries.push(entry);
            locations.push(location);
        }
        Ok(Self {
            src,
            entries,
            locations,
            comment: dir.comment,
        })
    }
}

impl ArchiveReader for ZipReader {
    fn format(&self) -> FormatId {
        FormatId::Zip
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    fn open_entry(&mut self, index: usize) -> Result<Box<dyn Read + '_>, FormatError> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| corrupt("no such item"))?;
        let loc = self.locations[index];
        if loc.flags & FLAG_ENCRYPTED != 0 {
            return Err(FormatError::Encrypted);
        }
        let (size, packed) = (entry.size, entry.packed_size);

        self.src.seek(SeekFrom::Start(loc.local_header))?;
        let mut header = [0u8; 30];
        self.src.read_exact(&mut header)?;
        let mut cur = Cursor::new(&header);
        if cur.read_u32_le()? != LOCAL_SIG {
            return Err(corrupt("local header signature not found"));
        }
        cur.skip(22)?;
        let name_len = u64::from(cur.read_u16_le()?);
        let extra_len = u64::from(cur.read_u16_le()?);
        let data_start = loc
            .local_header
            .saturating_add(LOCAL_HEADER_LEN + name_len + extra_len);
        self.src.seek(SeekFrom::Start(data_start))?;

        let raw = (&mut self.src).take(packed);
        let decoded: Box<dyn Read + '_> = match loc.method {
            METHOD_STORE if packed != size => {
                return Err(corrupt(
                    "stored item has different packed and unpacked sizes",
                ));
            }
            METHOD_STORE => Box::new(raw),
            METHOD_DEFLATE => Box::new(DeflateDecoder::new(BufReader::new(raw))),
            other => {
                return Err(FormatError::Unsupported(format!(
                    "compression method {}",
                    method_name(other)
                )));
            }
        };
        Ok(Box::new(VerifyReader::new(decoded, size, loc.crc32)))
    }
}

fn corrupt(what: &str) -> FormatError {
    FormatError::Corrupt(what.to_owned())
}

/// Locates the end of central directory record (and its ZIP64 counterpart).
fn find_directory(src: &mut Box<dyn ReadSeek>) -> Result<Directory, FormatError> {
    let file_len = src.seek(SeekFrom::End(0))?;
    if file_len < EOCD_LEN {
        return Err(corrupt("file is too small to be a ZIP archive"));
    }
    let tail_len = file_len.min(EOCD_LEN + MAX_COMMENT_LEN);
    let tail_start = file_len - tail_len;
    let mut tail = Vec::new();
    src.seek(SeekFrom::Start(tail_start))?;
    (&mut *src).take(tail_len).read_to_end(&mut tail)?;

    let eocd_rel = (0..=tail.len() - 22)
        .rev()
        .find(|&i| tail[i..].starts_with(&EOCD_SIG.to_le_bytes()))
        .ok_or_else(|| corrupt("end of central directory not found"))?;
    let eocd_pos = tail_start + eocd_rel as u64;

    let mut cur = Cursor::new(&tail[eocd_rel..]);
    cur.skip(4)?;
    let disk = cur.read_u16_le()?;
    let dir_disk = cur.read_u16_le()?;
    cur.skip(4)?;
    let mut size = u64::from(cur.read_u32_le()?);
    let mut offset = u64::from(cur.read_u32_le()?);
    let comment_len = usize::from(cur.read_u16_le()?);
    let comment_len = comment_len.min(cur.remaining());
    let comment_bytes = cur.read_bytes(comment_len)?;
    let comment = (!comment_bytes.is_empty()).then(|| decode_name(comment_bytes, 0, 0));

    let mut dir_end = eocd_pos;
    if let Some((zip64_pos, zip64)) = read_zip64(src, eocd_pos)? {
        (size, offset) = (zip64.size, zip64.offset);
        if zip64.disk != 0 || zip64.dir_disk != 0 {
            return Err(FormatError::Unsupported(
                "multi-volume ZIP archives".to_owned(),
            ));
        }
        dir_end = zip64_pos;
    } else if disk != 0 || dir_disk != 0 {
        return Err(FormatError::Unsupported(
            "multi-volume ZIP archives".to_owned(),
        ));
    }

    let shift = dir_end
        .checked_sub(size)
        .and_then(|start| start.checked_sub(offset))
        .ok_or_else(|| corrupt("central directory lies outside the file"))?;
    Ok(Directory {
        offset,
        size,
        shift,
        comment,
    })
}

struct Zip64Directory {
    disk: u32,
    dir_disk: u32,
    size: u64,
    offset: u64,
}

/// Reads the ZIP64 end record if a locator precedes the classic end record.
fn read_zip64(
    src: &mut Box<dyn ReadSeek>,
    eocd_pos: u64,
) -> Result<Option<(u64, Zip64Directory)>, FormatError> {
    let Some(locator_pos) = eocd_pos.checked_sub(ZIP64_LOCATOR_LEN) else {
        return Ok(None);
    };
    let mut locator = [0u8; 20];
    src.seek(SeekFrom::Start(locator_pos))?;
    src.read_exact(&mut locator)?;
    let mut cur = Cursor::new(&locator);
    if cur.read_u32_le()? != ZIP64_LOCATOR_SIG {
        return Ok(None);
    }
    cur.skip(4)?;
    let stated = cur.read_u64_le()?;

    // The stated offset ignores any SFX prefix; the record normally sits right
    // before the locator, so fall back to that position.
    let candidates = [Some(stated), locator_pos.checked_sub(ZIP64_EOCD_LEN)];
    for pos in candidates.into_iter().flatten() {
        let mut record = [0u8; 56];
        src.seek(SeekFrom::Start(pos))?;
        if src.read_exact(&mut record).is_err() {
            continue;
        }
        let mut cur = Cursor::new(&record);
        if cur.read_u32_le()? != ZIP64_EOCD_SIG {
            continue;
        }
        cur.skip(12)?;
        let disk = cur.read_u32_le()?;
        let dir_disk = cur.read_u32_le()?;
        cur.skip(16)?;
        let size = cur.read_u64_le()?;
        let offset = cur.read_u64_le()?;
        return Ok(Some((
            pos,
            Zip64Directory {
                disk,
                dir_disk,
                size,
                offset,
            },
        )));
    }
    Err(corrupt("ZIP64 end of central directory not found"))
}

fn parse_central_header(cur: &mut Cursor<'_>) -> Result<(Entry, Location), FormatError> {
    if cur.read_u32_le()? != CENTRAL_SIG {
        return Err(corrupt("central directory header signature not found"));
    }
    let made_by = cur.read_u16_le()?;
    cur.skip(2)?;
    let flags = cur.read_u16_le()?;
    let method = cur.read_u16_le()?;
    let dos_time = cur.read_u16_le()?;
    let dos_date = cur.read_u16_le()?;
    let crc32 = cur.read_u32_le()?;
    let packed32 = cur.read_u32_le()?;
    let size32 = cur.read_u32_le()?;
    let name_len = usize::from(cur.read_u16_le()?);
    let extra_len = usize::from(cur.read_u16_le()?);
    let comment_len = usize::from(cur.read_u16_le()?);
    cur.skip(4)?;
    let external = cur.read_u32_le()?;
    let offset32 = cur.read_u32_le()?;
    let raw_name = cur.read_bytes(name_len)?;
    let extra = cur.read_bytes(extra_len)?;
    let raw_comment = cur.read_bytes(comment_len)?;

    let host = made_by.to_be_bytes()[0];
    let mut size = u64::from(size32);
    let mut packed = u64::from(packed32);
    let mut local_header = u64::from(offset32);
    let mut modified = FileTime::from_dos(dos_date, dos_time);
    let (mut created, mut accessed) = (None, None);
    let mut unicode_name = None;

    for (id, data) in extra_fields(extra) {
        match id {
            0x0001 => {
                let mut z = Cursor::new(data);
                for (field, value) in [
                    (size32, &mut size),
                    (packed32, &mut packed),
                    (offset32, &mut local_header),
                ] {
                    if field == u32::MAX {
                        *value = z.read_u64_le()?;
                    }
                }
            }
            0x000A => {
                if let Some(times) = ntfs_times(data) {
                    (modified, accessed, created) =
                        (Some(times[0]), Some(times[1]), Some(times[2]));
                }
            }
            0x5455 => {
                // Extended timestamp; in the central directory it carries only mtime.
                if let [flags, a, b, c, d, ..] = *data
                    && flags & 1 != 0
                {
                    let seconds = i64::from(i32::from_le_bytes([a, b, c, d]));
                    modified = Some(FileTime::Unix { seconds, nanos: 0 });
                }
            }
            0x7075 => unicode_name = unicode_path(data, raw_name),
            _ => {}
        }
    }

    let name = unicode_name.unwrap_or_else(|| decode_name(raw_name, flags, host));
    let unix_mode = matches!(host, HOST_UNIX | HOST_MACOS)
        .then_some(external >> 16)
        .filter(|&mode| mode != 0);
    let kind = match unix_mode.map(|m| m & S_IFMT) {
        Some(S_IFLNK) => EntryKind::Symlink,
        Some(S_IFDIR) => EntryKind::Directory,
        _ if name.ends_with('/') || external & DOS_DIRECTORY != 0 => EntryKind::Directory,
        _ => EntryKind::File,
    };
    let name = name.trim_end_matches('/').to_owned();

    let mut entry = Entry::new(name, kind);
    entry.size = size;
    entry.packed_size = packed;
    entry.modified = modified;
    entry.created = created;
    entry.accessed = accessed;
    entry.crc32 = Some(crc32);
    entry.method = method_name(method);
    entry.encrypted = flags & FLAG_ENCRYPTED != 0;
    entry.unix_mode = unix_mode;
    entry.dos_attributes = Some(external & 0xFFFF);
    entry.comment = (!raw_comment.is_empty()).then(|| decode_name(raw_comment, flags, host));
    Ok((
        entry,
        Location {
            local_header,
            method,
            flags,
            crc32,
        },
    ))
}

/// Splits an extra-field block into `(header id, data)` pairs, ignoring a
/// truncated trailing field.
fn extra_fields(mut extra: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    std::iter::from_fn(move || {
        let mut cur = Cursor::new(extra);
        let id = cur.read_u16_le().ok()?;
        let len = usize::from(cur.read_u16_le().ok()?);
        let data = cur.read_bytes(len).ok()?;
        extra = &extra[4 + len..];
        Some((id, data))
    })
}

/// Parses the NTFS extra field into `[modified, accessed, created]`.
fn ntfs_times(data: &[u8]) -> Option<[FileTime; 3]> {
    let mut cur = Cursor::new(data);
    cur.skip(4).ok()?;
    while cur.remaining() >= 4 {
        let tag = cur.read_u16_le().ok()?;
        let len = usize::from(cur.read_u16_le().ok()?);
        let body = cur.read_bytes(len).ok()?;
        if tag == 1 && len >= 24 {
            let mut t = Cursor::new(body);
            let mut next = || t.read_u64_le().ok().map(FileTime::from_windows_ticks);
            return Some([next()?, next()?, next()?]);
        }
    }
    None
}

/// Uses the Info-ZIP Unicode Path field if it was written for this exact name.
fn unicode_path(data: &[u8], raw_name: &[u8]) -> Option<String> {
    let mut cur = Cursor::new(data);
    if cur.read_u8().ok()? != 1 || cur.read_u32_le().ok()? != Crc32::checksum(raw_name) {
        return None;
    }
    let rest = cur.read_bytes(cur.remaining()).ok()?;
    std::str::from_utf8(rest).ok().map(str::to_owned)
}

/// Decodes a name or comment: UTF-8 when flagged (or when a Unix/macOS archiver
/// wrote valid UTF-8 without flagging it), otherwise CP437.
fn decode_name(raw: &[u8], flags: u16, host: u8) -> String {
    if flags & FLAG_UTF8 != 0 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    if raw.is_ascii() {
        return String::from_utf8_lossy(raw).into_owned();
    }
    if matches!(host, HOST_UNIX | HOST_MACOS)
        && let Ok(text) = std::str::from_utf8(raw)
    {
        return text.to_owned();
    }
    cp437::decode(raw)
}

fn method_name(method: u16) -> String {
    match method {
        0 => "Store",
        1 => "Shrink",
        2..=5 => "Reduce",
        6 => "Implode",
        8 => "Deflate",
        9 => "Deflate64",
        12 => "BZip2",
        14 => "LZMA",
        93 => "Zstd",
        95 => "XZ",
        98 => "PPMd",
        99 => "AES",
        other => return format!("#{other}"),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_fields_stop_at_truncation() {
        let extra = [
            0x01, 0x00, 0x02, 0x00, 0xAA, 0xBB, 0x55, 0x54, 0x09, 0x00, 0x01,
        ];
        let fields: Vec<_> = extra_fields(&extra).collect();
        assert_eq!(fields, vec![(0x0001, &[0xAA, 0xBB][..])]);
    }

    #[test]
    fn utf8_flag_and_heuristics() {
        let cafe = "café".as_bytes();
        assert_eq!(decode_name(cafe, FLAG_UTF8, 0), "café");
        assert_eq!(decode_name(cafe, 0, HOST_UNIX), "café");
        assert_eq!(decode_name(b"caf\x82", 0, 0), "café");
    }

    #[test]
    fn unicode_path_requires_matching_crc() {
        let raw = b"caf\x82";
        let mut data = vec![1];
        data.extend_from_slice(&Crc32::checksum(raw).to_le_bytes());
        data.extend_from_slice("café".as_bytes());
        assert_eq!(unicode_path(&data, raw).as_deref(), Some("café"));
        assert_eq!(unicode_path(&data, b"other"), None);
    }
}
