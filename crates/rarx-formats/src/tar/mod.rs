//! TAR reader (AGENT.md §6.5): v7, ustar, GNU (`L`/`K` long names) and pax
//! (`x`/`g`), plain or inside a gzip/bzip2/xz/zstd stream.
//!
//! Plain archives are read in place. Compressed ones are decoded once to build the
//! listing; item data is then read through a decoder that is kept between calls
//! (like a 7z solid block), restarting only when an earlier item is requested.

mod header;
mod writer;

use std::collections::HashMap;
use std::io::{self, BufReader, Cursor as IoCursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use self::header::{BLOCK, Block, Header, parse, parse_pax, pax_time};
use crate::shared::{Region, Shared};
use crate::stream::StreamCodec;
use crate::verify::VerifyReader;
use crate::{ArchiveReader, Entry, EntryKind, FileTime, FormatError, FormatId, ReadSeek};

pub use writer::TarWriter;

/// Long names and pax headers larger than this are treated as corruption.
const MAX_META: u64 = 1 << 20;
const READ_BUFFER: usize = 64 * 1024;

/// Where an item's data comes from.
#[derive(Clone, Debug)]
enum Data {
    /// `size` bytes at decoded offset `offset`.
    Stream { offset: u64, size: u64 },
    /// Symbolic-link target stored in the header.
    Inline(Vec<u8>),
    /// Hard link: the data of an earlier item.
    LinkTo(usize),
    /// Item kinds RarX cannot extract (devices, sparse files, …).
    Unsupported(&'static str),
}

/// A decoder positioned at decoded offset `pos`.
struct DecodedCursor {
    stream: Box<dyn Read + Send>,
    pos: u64,
    valid: bool,
}

/// An open TAR archive.
pub struct TarReader {
    src: Shared,
    codec: Option<StreamCodec>,
    entries: Vec<Entry>,
    data: Vec<Data>,
    cursor: Option<DecodedCursor>,
}

fn corrupt(what: &str) -> FormatError {
    FormatError::Corrupt(format!("tar: {what}"))
}

/// Opens the whole decoded archive as one stream.
fn decoded_stream(src: &Shared, codec: Option<StreamCodec>) -> io::Result<Box<dyn Read + Send>> {
    let region = Region::new(src, 0, u64::MAX);
    let raw: Box<dyn Read + Send> = Box::new(BufReader::with_capacity(READ_BUFFER, region));
    match codec {
        Some(codec) => codec.decoder(raw),
        None => Ok(raw),
    }
}

/// Reads exactly one block, or `None` at a clean end of data.
fn read_block(stream: &mut dyn Read) -> Result<Option<[u8; BLOCK]>, FormatError> {
    let mut block = [0u8; BLOCK];
    let mut filled = 0;
    while filled < BLOCK {
        match stream.read(&mut block[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(corrupt("unexpected end of archive")),
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(FormatError::Io(err)),
        }
    }
    Ok(Some(block))
}

fn skip(stream: &mut dyn Read, count: u64) -> Result<(), FormatError> {
    let skipped = io::copy(&mut stream.take(count), &mut io::sink())?;
    if skipped == count {
        Ok(())
    } else {
        Err(corrupt("unexpected end of archive"))
    }
}

fn padded(size: u64) -> u64 {
    size.div_ceil(BLOCK as u64).saturating_mul(BLOCK as u64)
}

/// Header fields overridden by GNU long-name records and pax headers.
#[derive(Default)]
struct Pending {
    long_name: Option<Vec<u8>>,
    long_link: Option<Vec<u8>>,
    pax: HashMap<String, Vec<u8>>,
}

impl TarReader {
    /// Reads the listing of the TAR archive in `src`, decoding it with `codec` first
    /// when given.
    ///
    /// # Errors
    /// [`FormatError::Corrupt`] if the data is not TAR (the first header's checksum
    /// fails) or ends early, or any decoding error.
    pub fn new(src: Box<dyn ReadSeek>, codec: Option<StreamCodec>) -> Result<Self, FormatError> {
        let src: Shared = Arc::new(Mutex::new(src));
        let mut reader = Self {
            src,
            codec,
            entries: Vec::new(),
            data: Vec::new(),
            cursor: None,
        };
        reader.scan()?;
        Ok(reader)
    }

    fn scan(&mut self) -> Result<(), FormatError> {
        let mut stream = decoded_stream(&self.src, self.codec)?;
        let mut pos = 0u64;
        let mut global: HashMap<String, Vec<u8>> = HashMap::new();
        let mut pending = Pending::default();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        while let Some(block) = read_block(stream.as_mut())? {
            pos += BLOCK as u64;
            let header = match parse(&block) {
                Block::End => break,
                Block::Invalid if pos == BLOCK as u64 => return Err(corrupt("not a TAR archive")),
                Block::Invalid => return Err(corrupt("header checksum mismatch")),
                Block::Header(header) => header,
            };
            let size = match pending.pax.get("size").or_else(|| global.get("size")) {
                Some(value) => parse_decimal(value).ok_or_else(|| corrupt("bad pax size"))?,
                None => header.size,
            };
            match header.typeflag {
                b'L' | b'K' | b'x' | b'g' => {
                    if size > MAX_META {
                        return Err(corrupt("extended header is too large"));
                    }
                    let mut meta = Vec::new();
                    (&mut stream).take(size).read_to_end(&mut meta)?;
                    if meta.len() as u64 != size {
                        return Err(corrupt("unexpected end of archive"));
                    }
                    skip(stream.as_mut(), padded(size) - size)?;
                    pos += padded(size);
                    match header.typeflag {
                        b'L' => pending.long_name = Some(trim_nul(meta)),
                        b'K' => pending.long_link = Some(trim_nul(meta)),
                        b'x' => parse_pax(&meta, &mut pending.pax),
                        _ => parse_pax(&meta, &mut global),
                    }
                    continue;
                }
                b'V' => {
                    // Volume label: not an item.
                    skip(stream.as_mut(), padded(size))?;
                    pos += padded(size);
                    pending = Pending::default();
                    continue;
                }
                _ => {}
            }

            let (entry, data) = self.build(&header, size, pos, &pending, &global, &by_name);
            skip(stream.as_mut(), padded(size))?;
            pos += padded(size);
            by_name.insert(normalize(&entry.name), self.entries.len());
            self.entries.push(entry);
            self.data.push(data);
            pending = Pending::default();
        }
        Ok(())
    }

    /// Turns a header plus its overrides into an entry.
    fn build(
        &self,
        header: &Header,
        size: u64,
        offset: u64,
        pending: &Pending,
        global: &HashMap<String, Vec<u8>>,
        by_name: &HashMap<String, usize>,
    ) -> (Entry, Data) {
        let pax = |key: &str| pending.pax.get(key).or_else(|| global.get(key));
        let raw_name = pax("path")
            .cloned()
            .or_else(|| pending.long_name.clone())
            .unwrap_or_else(|| header.name.clone());
        let link = pax("linkpath")
            .cloned()
            .or_else(|| pending.long_link.clone())
            .unwrap_or_else(|| header.linkname.clone());
        let name = String::from_utf8_lossy(&raw_name).into_owned();
        let link = String::from_utf8_lossy(&link).into_owned();

        let sparse =
            header.typeflag == b'S' || pending.pax.keys().any(|k| k.starts_with("GNU.sparse."));
        let (kind, data) = match header.typeflag {
            _ if sparse => (EntryKind::File, Data::Unsupported("sparse file")),
            b'5' | b'0' | 0 | b'7' if header.typeflag == b'5' || name.ends_with('/') => {
                (EntryKind::Directory, Data::Inline(Vec::new()))
            }
            b'2' => (EntryKind::Symlink, Data::Inline(link.clone().into_bytes())),
            b'1' => match by_name.get(&normalize(&link)) {
                Some(&target) => (EntryKind::File, Data::LinkTo(target)),
                None => (
                    EntryKind::File,
                    Data::Unsupported("hard link to a file not in the archive"),
                ),
            },
            b'3' | b'4' | b'6' => (EntryKind::File, Data::Unsupported("device or FIFO")),
            b'M' => (
                EntryKind::File,
                Data::Unsupported("multi-volume continuation"),
            ),
            // Regular files ('0', NUL, '7'); POSIX treats unknown types the same way.
            _ => (EntryKind::File, Data::Stream { offset, size }),
        };

        let mut entry = Entry::new(name.trim_end_matches('/'), kind);
        entry.size = match &data {
            Data::Stream { size, .. } => *size,
            Data::Inline(bytes) => bytes.len() as u64,
            Data::LinkTo(target) => self.entries[*target].size,
            Data::Unsupported(_) => size,
        };
        entry.packed_size = match &data {
            Data::Stream { size, .. } => *size,
            _ => 0,
        };
        entry.unix_mode = Some(u32::try_from(header.mode & 0o7777).unwrap_or(0o644));
        entry.modified = Some(match pax("mtime").and_then(|v| pax_time(v)) {
            Some((seconds, nanos)) => FileTime::Unix { seconds, nanos },
            None => FileTime::Unix {
                seconds: header.mtime,
                nanos: 0,
            },
        });
        entry.accessed = pax("atime")
            .and_then(|v| pax_time(v))
            .map(|(seconds, nanos)| FileTime::Unix { seconds, nanos });
        entry.created = pax("ctime")
            .and_then(|v| pax_time(v))
            .map(|(seconds, nanos)| FileTime::Unix { seconds, nanos });
        if header.typeflag == b'1' {
            entry.comment = Some(format!("hard link to {link}"));
        }
        (entry, data)
    }

    /// Returns a decoded stream positioned at `offset`.
    fn stream_at(&mut self, offset: u64) -> Result<&mut DecodedCursor, FormatError> {
        if self.codec.is_none() {
            // Plain archives: start a fresh positioned reader every time.
            let mut region = Region::new(&self.src, 0, u64::MAX);
            region.seek(SeekFrom::Start(offset))?;
            let stream = Box::new(BufReader::with_capacity(READ_BUFFER, region));
            self.cursor = Some(DecodedCursor {
                stream,
                pos: offset,
                valid: true,
            });
        } else {
            let reusable = self
                .cursor
                .as_ref()
                .is_some_and(|c| c.valid && c.pos <= offset);
            if !reusable {
                let stream = decoded_stream(&self.src, self.codec)?;
                self.cursor = Some(DecodedCursor {
                    stream,
                    pos: 0,
                    valid: true,
                });
            }
        }
        let cursor = self.cursor.as_mut().ok_or_else(|| corrupt("no decoder"))?;
        if cursor.pos < offset {
            let gap = offset - cursor.pos;
            if skip(cursor.stream.as_mut(), gap).is_err() {
                cursor.valid = false;
                return Err(corrupt("data ends before the requested item"));
            }
            cursor.pos = offset;
        }
        Ok(cursor)
    }
}

fn trim_nul(mut bytes: Vec<u8>) -> Vec<u8> {
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}

fn parse_decimal(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}

/// Name used to match hard-link targets (`./a` and `a` are the same item).
fn normalize(name: &str) -> String {
    name.trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned()
}

/// Reads one item's bytes from a shared decoder and tracks its position.
struct ItemStream<'a> {
    cursor: &'a mut DecodedCursor,
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
        self.cursor.pos += n as u64;
        Ok(n)
    }
}

impl Drop for ItemStream<'_> {
    fn drop(&mut self) {
        if self.remaining > 0 {
            self.cursor.valid = false;
        }
    }
}

impl ArchiveReader for TarReader {
    fn format(&self) -> FormatId {
        FormatId::Tar
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn comment(&self) -> Option<&str> {
        None
    }

    fn open_entry(&mut self, index: usize) -> Result<Box<dyn Read + '_>, FormatError> {
        let mut index = index;
        // Hard links resolve to their target (targets always come earlier).
        let data = loop {
            match self
                .data
                .get(index)
                .ok_or_else(|| corrupt("no such item"))?
            {
                Data::LinkTo(target) if *target < index => index = *target,
                Data::LinkTo(_) => return Err(corrupt("hard link loop")),
                other => break other.clone(),
            }
        };
        match data {
            Data::Inline(bytes) => Ok(Box::new(IoCursor::new(bytes))),
            Data::Unsupported(what) => Err(FormatError::Unsupported(what.to_owned())),
            Data::LinkTo(_) => Err(corrupt("hard link loop")),
            Data::Stream { offset, size } => {
                let cursor = self.stream_at(offset)?;
                Ok(Box::new(VerifyReader::new(
                    ItemStream {
                        cursor,
                        remaining: size,
                    },
                    size,
                    None,
                )))
            }
        }
    }
}

/// Whether the decoded start of `src` is a valid TAR header. Leaves `src` at 0.
///
/// # Errors
/// Any I/O error from seeking.
pub fn starts_with_tar(
    src: &mut Box<dyn ReadSeek>,
    codec: StreamCodec,
) -> Result<bool, FormatError> {
    src.seek(SeekFrom::Start(0))?;
    let first = codec
        .decoder(Box::new(&mut *src))
        .ok()
        .and_then(|mut stream| read_block(stream.as_mut()).ok().flatten());
    src.seek(SeekFrom::Start(0))?;
    Ok(first.is_some_and(|block| matches!(parse(&block), Block::Header(_))))
}

/// A gzip/bzip2/xz/zstd file that does not contain a TAR archive: one item.
pub struct StreamReader {
    src: Shared,
    codec: StreamCodec,
    entries: Vec<Entry>,
}

impl StreamReader {
    /// Wraps a single compressed stream; `name` becomes the item name.
    #[must_use]
    pub fn new(src: Box<dyn ReadSeek>, codec: StreamCodec, name: &str) -> Self {
        let mut entry = Entry::new(name, EntryKind::File);
        entry.method = format!("{codec:?}");
        Self {
            src: Arc::new(Mutex::new(src)),
            codec,
            entries: vec![entry],
        }
    }
}

impl ArchiveReader for StreamReader {
    fn format(&self) -> FormatId {
        self.codec.format()
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn comment(&self) -> Option<&str> {
        None
    }

    fn open_entry(&mut self, index: usize) -> Result<Box<dyn Read + '_>, FormatError> {
        if index != 0 {
            return Err(corrupt("no such item"));
        }
        Ok(decoded_stream(&self.src, Some(self.codec))?)
    }
}
