//! 7z reader (AGENT.md §6.1).
//!
//! The header database may itself be compressed (`kEncodedHeader`); it is decoded
//! into memory, then items are mapped onto folders. Items of a solid folder are
//! read through one decoder that is kept between calls, so extracting a solid
//! archive in order decodes it once.

mod coders;
mod header;

use std::io::{self, Read, SeekFrom};
use std::sync::{Arc, Mutex};

use rarx_checksum::{Checksum, Crc32};
use rarx_io::Cursor;

use self::coders::{AES, Shared, Stream, folder_stream, method_label, read_folder_to_vec};
use self::header::{Folder, Header, K_ENCODED_HEADER, K_HEADER, parse_header, parse_streams_info};
use crate::verify::VerifyReader;
use crate::{ArchiveReader, Entry, EntryKind, FormatError, FormatId, ReadSeek};

const SIGNATURE: &[u8; 6] = b"7z\xBC\xAF\x27\x1C";
const START_HEADER_LEN: u64 = 32;
/// Compressed headers larger than this are rejected.
const MAX_HEADER: u64 = 1 << 30;
/// Headers may be encoded repeatedly; real archives use one level.
const MAX_HEADER_LEVELS: usize = 4;

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_UNIX_EXTENSION: u32 = 0x8000;
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// Where an item's data lives: which folder, and its position among the folder's
/// file streams.
#[derive(Clone, Copy, Debug)]
struct StreamLocation {
    folder: usize,
    ordinal: usize,
    crc: Option<u32>,
}

/// A folder decoder positioned at the start of file stream `next`.
struct FolderCursor {
    folder: usize,
    next: usize,
    stream: Stream,
    valid: bool,
}

/// An open 7z archive.
pub struct SevenZReader {
    src: Shared,
    entries: Vec<Entry>,
    locations: Vec<Option<StreamLocation>>,
    folders: Vec<Folder>,
    /// Absolute `(offset, size)` of every pack stream.
    packs: Vec<(u64, u64)>,
    /// Size of each file stream, per folder.
    stream_sizes: Vec<Vec<u64>>,
    cursor: Option<FolderCursor>,
}

fn corrupt(what: &str) -> FormatError {
    FormatError::Corrupt(format!("7z: {what}"))
}

impl SevenZReader {
    /// Reads the header database of the 7z archive in `src`.
    ///
    /// # Errors
    /// [`FormatError`] if the signature, start header or header database is invalid.
    pub fn new(mut src: Box<dyn ReadSeek>) -> Result<Self, FormatError> {
        let file_len = src.seek(SeekFrom::End(0))?;
        src.seek(SeekFrom::Start(0))?;
        let mut start = [0u8; 32];
        src.read_exact(&mut start)
            .map_err(|_| corrupt("file is too small"))?;
        if &start[..6] != SIGNATURE {
            return Err(corrupt("signature not found"));
        }
        if start[6] != 0 {
            return Err(FormatError::Unsupported(format!(
                "7z format version {}.{}",
                start[6], start[7]
            )));
        }
        let mut cur = Cursor::new(&start[8..]);
        let start_crc = cur.read_u32_le()?;
        if Crc32::checksum(&start[12..32]) != start_crc {
            return Err(corrupt("start header CRC mismatch"));
        }
        let next_offset = cur.read_u64_le()?;
        let next_size = cur.read_u64_le()?;
        let next_crc = cur.read_u32_le()?;

        let src: Shared = Arc::new(Mutex::new(src));
        let mut reader = Self {
            src,
            entries: Vec::new(),
            locations: Vec::new(),
            folders: Vec::new(),
            packs: Vec::new(),
            stream_sizes: Vec::new(),
            cursor: None,
        };
        if next_size == 0 {
            return Ok(reader);
        }
        let header_pos = START_HEADER_LEN
            .checked_add(next_offset)
            .filter(|pos| {
                pos.checked_add(next_size)
                    .is_some_and(|end| end <= file_len)
            })
            .ok_or_else(|| corrupt("header lies outside the file (truncated archive?)"))?;
        let mut raw = Vec::new();
        {
            let mut file = reader.lock()?;
            file.seek(SeekFrom::Start(header_pos))?;
            (&mut *file).take(next_size).read_to_end(&mut raw)?;
        }
        if Crc32::checksum(&raw) != next_crc {
            return Err(corrupt("header CRC mismatch"));
        }
        let header = reader.decode_header(raw, file_len)?;
        reader.load(header, file_len)?;
        Ok(reader)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Box<dyn ReadSeek>>, FormatError> {
        self.src
            .lock()
            .map_err(|_| FormatError::Io(io::Error::other("archive reader poisoned")))
    }

    /// Unwraps `kEncodedHeader` layers until a plain `kHeader` remains.
    fn decode_header(&self, mut raw: Vec<u8>, file_len: u64) -> Result<Header, FormatError> {
        for _ in 0..MAX_HEADER_LEVELS {
            let mut cur = Cursor::new(&raw);
            match cur.read_u8()? {
                K_HEADER => return parse_header(&mut cur),
                K_ENCODED_HEADER => {
                    let info = parse_streams_info(&mut cur)?;
                    let packs = pack_ranges(info.pack_pos, &info.pack_sizes, file_len)?;
                    let folder = info
                        .folders
                        .first()
                        .ok_or_else(|| corrupt("encoded header has no folder"))?;
                    if folder.coders.iter().any(|c| c.id == AES) {
                        return Err(FormatError::Encrypted);
                    }
                    let mine = &packs[folder.first_pack_stream..][..folder.packed_streams.len()];
                    let decoded = read_folder_to_vec(&self.src, folder, mine, MAX_HEADER)?;
                    if folder
                        .crc
                        .is_some_and(|crc| crc != Crc32::checksum(&decoded))
                    {
                        return Err(corrupt("compressed header CRC mismatch"));
                    }
                    raw = decoded;
                }
                _ => return Err(corrupt("unknown header type")),
            }
        }
        Err(corrupt("too many nested encoded headers"))
    }

    /// Builds the entry list from the header database.
    fn load(&mut self, header: Header, file_len: u64) -> Result<(), FormatError> {
        let streams = header.streams;
        self.packs = pack_ranges(streams.pack_pos, &streams.pack_sizes, file_len)?;
        self.stream_sizes = Vec::with_capacity(streams.folders.len());
        // (folder, ordinal) of every file stream, in order.
        let mut slots = Vec::new();
        let mut sizes = streams.substreams.iter();
        for (index, folder) in streams.folders.iter().enumerate() {
            let mut folder_sizes = Vec::new();
            for ordinal in 0..usize::try_from(folder.num_streams).unwrap_or(0) {
                let &(size, crc) = sizes
                    .next()
                    .ok_or_else(|| corrupt("stream table is short"))?;
                folder_sizes.push(size);
                slots.push((index, ordinal, size, crc));
            }
            self.stream_sizes.push(folder_sizes);
        }

        let mut slots = slots.into_iter();
        for file in header.files.into_iter().filter(|f| !f.is_anti) {
            let mut entry = Entry::new(file.name.replace('\\', "/"), EntryKind::File);
            let attributes = file.attributes.unwrap_or(0);
            entry.unix_mode = (attributes & ATTR_UNIX_EXTENSION != 0).then_some(attributes >> 16);
            entry.dos_attributes = file.attributes.map(|a| a & 0x7FFF);
            entry.kind = match entry.unix_mode.map(|m| m & S_IFMT) {
                Some(S_IFLNK) => EntryKind::Symlink,
                Some(S_IFDIR) => EntryKind::Directory,
                _ if file.is_dir || attributes & ATTR_DIRECTORY != 0 => EntryKind::Directory,
                _ => EntryKind::File,
            };
            entry.modified = file.mtime;
            entry.created = file.ctime;
            entry.accessed = file.atime;
            let location = if file.has_stream {
                let (folder, ordinal, size, crc) = slots
                    .next()
                    .ok_or_else(|| corrupt("more files than streams"))?;
                let f = &streams.folders[folder];
                entry.size = size;
                entry.crc32 = crc;
                entry.method = f
                    .coders
                    .iter()
                    .rev()
                    .map(method_label)
                    .collect::<Vec<_>>()
                    .join(" ");
                entry.encrypted = f.coders.iter().any(|c| c.id == AES);
                if ordinal == 0 {
                    entry.packed_size = self.packs[f.first_pack_stream..][..f.packed_streams.len()]
                        .iter()
                        .map(|&(_, size)| size)
                        .sum();
                }
                Some(StreamLocation {
                    folder,
                    ordinal,
                    crc,
                })
            } else {
                None
            };
            self.entries.push(entry);
            self.locations.push(location);
        }
        self.folders = streams.folders;
        Ok(())
    }

    /// Returns a folder decoder positioned at file stream `ordinal`.
    fn seek_stream(
        &mut self,
        folder: usize,
        ordinal: usize,
    ) -> Result<&mut FolderCursor, FormatError> {
        let reusable = self
            .cursor
            .as_ref()
            .is_some_and(|c| c.valid && c.folder == folder && c.next <= ordinal);
        if !reusable {
            let f = &self.folders[folder];
            let packs = self
                .packs
                .get(f.first_pack_stream..f.first_pack_stream + f.packed_streams.len())
                .ok_or_else(|| corrupt("folder references missing pack streams"))?;
            let stream = folder_stream(&self.src, f, packs)?;
            self.cursor = Some(FolderCursor {
                folder,
                next: 0,
                stream,
                valid: true,
            });
        }
        let sizes = &self.stream_sizes[folder];
        let cursor = self.cursor.as_mut().ok_or_else(|| corrupt("no decoder"))?;
        while cursor.next < ordinal {
            let size = sizes[cursor.next];
            let skipped = io::copy(&mut (&mut cursor.stream).take(size), &mut io::sink());
            if skipped.ok() != Some(size) {
                cursor.valid = false;
                return Err(corrupt("data ends before the requested item"));
            }
            cursor.next += 1;
        }
        Ok(cursor)
    }
}

/// Converts pack sizes into absolute `(offset, size)` ranges inside the file.
fn pack_ranges(
    pack_pos: u64,
    sizes: &[u64],
    file_len: u64,
) -> Result<Vec<(u64, u64)>, FormatError> {
    let mut offset = START_HEADER_LEN
        .checked_add(pack_pos)
        .ok_or_else(|| corrupt("pack position overflows"))?;
    let mut ranges = Vec::with_capacity(sizes.len());
    for &size in sizes {
        let end = offset.checked_add(size).filter(|&end| end <= file_len);
        let end =
            end.ok_or_else(|| corrupt("packed data lies outside the file (truncated archive?)"))?;
        ranges.push((offset, size));
        offset = end;
    }
    Ok(ranges)
}

/// Reads exactly one file stream from a shared folder decoder.
struct ItemStream<'a> {
    cursor: &'a mut FolderCursor,
    remaining: u64,
}

impl Read for ItemStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let want = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        if want == 0 {
            return Ok(0);
        }
        let n = self.cursor.stream.read(&mut buf[..want])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

impl Drop for ItemStream<'_> {
    fn drop(&mut self) {
        // A partly read item leaves the decoder mid-stream; start over next time.
        if self.remaining > 0 {
            self.cursor.valid = false;
        }
    }
}

impl ArchiveReader for SevenZReader {
    fn format(&self) -> FormatId {
        FormatId::SevenZip
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn comment(&self) -> Option<&str> {
        None
    }

    fn open_entry(&mut self, index: usize) -> Result<Box<dyn Read + '_>, FormatError> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| corrupt("no such item"))?;
        let size = entry.size;
        let Some(location) = self.locations[index] else {
            return Ok(Box::new(io::empty()));
        };
        if entry.encrypted {
            return Err(FormatError::Encrypted);
        }
        let cursor = self.seek_stream(location.folder, location.ordinal)?;
        cursor.next = location.ordinal + 1;
        let stream = ItemStream {
            cursor,
            remaining: size,
        };
        Ok(Box::new(VerifyReader::new(stream, size, location.crc)))
    }
}
