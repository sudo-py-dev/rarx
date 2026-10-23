//! 7z writer (AGENT.md §6.1): LZMA2 or Copy folders, solid or one folder per file,
//! an LZMA-compressed header, and optional AES-256 encryption of data and header.

use std::io::{self, Read, Seek, SeekFrom, Write};

use lzma_rust2::{Lzma2Options, Lzma2Writer, LzmaOptions, LzmaWriter};
use rarx_checksum::{Checksum, Crc32};
use rarx_crypto::Password;
use rarx_crypto::sevenz::{AesParams, DEFAULT_CYCLES_POWER, EncryptWriter, Key, derive_key};

use super::coders::{AES, lzma2_dictionary};
use super::header::{K_ENCODED_HEADER, K_END, K_HEADER};
use super::{ATTR_DIRECTORY, ATTR_UNIX_EXTENSION, S_IFDIR, S_IFLNK, SIGNATURE};
use crate::{EntryKind, FileTime, NewItem};

const K_MAIN_STREAMS_INFO: u8 = 0x04;
const K_FILES_INFO: u8 = 0x05;
const K_PACK_INFO: u8 = 0x06;
const K_UNPACK_INFO: u8 = 0x07;
const K_SUBSTREAMS_INFO: u8 = 0x08;
const K_SIZE: u8 = 0x09;
const K_CRC: u8 = 0x0A;
const K_FOLDER: u8 = 0x0B;
const K_CODERS_UNPACK_SIZE: u8 = 0x0C;
const K_NUM_UNPACK_STREAM: u8 = 0x0D;
const K_EMPTY_STREAM: u8 = 0x0E;
const K_EMPTY_FILE: u8 = 0x0F;
const K_NAME: u8 = 0x11;
const K_MTIME: u8 = 0x14;
const K_WIN_ATTRIBUTES: u8 = 0x15;

const ATTR_ARCHIVE: u32 = 0x20;
const S_IFREG: u32 = 0o100_000;
const FILETIME_UNIX_DIFF: i64 = 11_644_473_600;
const HEADER_DICTIONARY: u32 = 1 << 20;
const COPY_BUFFER: usize = 256 * 1024;

/// Settings for new 7z archives.
#[derive(Clone, Debug)]
pub struct SevenZOptions {
    /// 0 stores (Copy); 1–9 select the LZMA2 preset.
    pub level: u32,
    /// Put all files in one LZMA2 block instead of one block per file.
    pub solid: bool,
    /// Compress the header database with LZMA.
    pub compress_header: bool,
    /// Encrypt file data with AES-256.
    pub password: Option<Password>,
    /// Also encrypt the header (file names); needs `password`, implies
    /// `compress_header`.
    pub encrypt_header: bool,
}

impl Default for SevenZOptions {
    fn default() -> Self {
        Self {
            level: 5,
            solid: true,
            compress_header: true,
            password: None,
            encrypt_header: false,
        }
    }
}

/// Counts bytes passed through to the archive.
struct Counting<W> {
    inner: W,
    count: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Where a coder's output goes: straight to the archive, or through AES first.
enum Sink<W: Write> {
    Plain(Counting<W>),
    /// Counts the compressed bytes (the AES coder's unpacked size) on the way in.
    Aes(Counting<EncryptWriter<Counting<W>>>),
}

impl<W: Write> Sink<W> {
    fn new(out: W, crypto: Option<&(Key, AesParams)>) -> Self {
        let counting = Counting {
            inner: out,
            count: 0,
        };
        match crypto {
            None => Self::Plain(counting),
            Some((key, params)) => Self::Aes(Counting {
                inner: EncryptWriter::new(counting, key, params),
                count: 0,
            }),
        }
    }

    /// Ends the stream; returns the sink, bytes written to it, and the size
    /// before encryption when encrypting.
    fn finish(self) -> io::Result<(W, u64, Option<u64>)> {
        match self {
            Self::Plain(w) => Ok((w.inner, w.count, None)),
            Self::Aes(w) => {
                let out = w.inner.finish()?;
                Ok((out.inner, out.count, Some(w.count)))
            }
        }
    }
}

impl<W: Write> Write for Sink<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Aes(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Aes(w) => w.flush(),
        }
    }
}

enum Encoder<W: Write> {
    Copy(Sink<W>),
    Lzma2(Box<Lzma2Writer<Sink<W>>>),
}

impl<W: Write> Encoder<W> {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Copy(w) => w.write_all(buf),
            Self::Lzma2(w) => w.write_all(buf),
        }
    }

    fn finish(self) -> io::Result<Sink<W>> {
        match self {
            Self::Copy(w) => Ok(w),
            Self::Lzma2(w) => w.finish(),
        }
    }
}

/// A folder being written.
struct OpenFolder<W: Write> {
    encoder: Encoder<W>,
    /// The compression coder record, or `None` for Copy under AES.
    coder: Option<Vec<u8>>,
    aes: Option<Vec<u8>>,
    streams: Vec<(u64, u32)>,
}

/// A finished folder: a chain of coders (each feeding the one before it), the
/// unpacked size of each coder's output, the packed size and the file streams.
struct ClosedFolder {
    coders: Vec<Vec<u8>>,
    unpack_sizes: Vec<u64>,
    packed: u64,
    streams: Vec<(u64, u32)>,
}

impl ClosedFolder {
    /// Assembles a folder from its compression coder (if any) and AES coder (if any).
    fn new(
        coder: Option<Vec<u8>>,
        aes: Option<Vec<u8>>,
        packed: u64,
        compressed: Option<u64>,
        streams: Vec<(u64, u32)>,
    ) -> Self {
        let total = streams.iter().map(|(size, _)| size).sum();
        let mut coders = Vec::new();
        let mut unpack_sizes = Vec::new();
        if let Some(coder) = coder {
            coders.push(coder);
            unpack_sizes.push(total);
        }
        if let Some(aes) = aes {
            coders.push(aes);
            unpack_sizes.push(if coders.len() == 1 {
                total
            } else {
                compressed.unwrap_or(packed)
            });
        }
        Self {
            coders,
            unpack_sizes,
            packed,
            streams,
        }
    }
}

/// The 7zAES coder record for `params`.
fn aes_coder(params: &AesParams) -> Vec<u8> {
    let props = params.encode();
    let mut coder = vec![0x24];
    coder.extend_from_slice(AES);
    coder.push(u8::try_from(props.len()).unwrap_or(0));
    coder.extend_from_slice(&props);
    coder
}

struct FileRecord {
    name: String,
    has_stream: bool,
    is_dir: bool,
    mtime: Option<u64>,
    attributes: u32,
}

/// Writes a new 7z archive to a seekable sink.
pub struct SevenZWriter<W: Write + Seek> {
    out: Option<W>,
    start: u64,
    open: Option<OpenFolder<W>>,
    folders: Vec<ClosedFolder>,
    files: Vec<FileRecord>,
    options: SevenZOptions,
    /// Key derived once per archive (no salt), so every folder shares it.
    key: Option<Key>,
}

fn missing_sink() -> io::Error {
    io::Error::other("7z writer lost its output")
}

impl<W: Write + Seek> SevenZWriter<W> {
    /// Starts an archive at the current position of `out`.
    ///
    /// # Errors
    /// Any I/O error from writing the placeholder start header.
    pub fn new(mut out: W, options: SevenZOptions) -> io::Result<Self> {
        let start = out.stream_position()?;
        out.write_all(&[0u8; 32])?;
        let key = match &options.password {
            Some(password) => {
                let params = AesParams::generate(DEFAULT_CYCLES_POWER)?;
                Some(derive_key(password, &params))
            }
            None => None,
        };
        Ok(Self {
            out: Some(out),
            start,
            open: None,
            folders: Vec::new(),
            files: Vec::new(),
            options,
            key,
        })
    }

    /// Fresh AES parameters (a new IV) and the archive key, when encrypting.
    fn crypto(&self) -> io::Result<Option<(Key, AesParams)>> {
        match &self.key {
            Some(key) => Ok(Some((
                key.clone(),
                AesParams::generate(DEFAULT_CYCLES_POWER)?,
            ))),
            None => Ok(None),
        }
    }

    /// Number of items written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether no items have been written yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Adds a regular file whose contents are read from `data`.
    ///
    /// # Errors
    /// Any I/O error from reading `data` or writing the archive.
    pub fn add_file(&mut self, item: &NewItem, data: &mut dyn Read) -> io::Result<()> {
        let mut buf = vec![0u8; COPY_BUFFER];
        let first = read_some(data, &mut buf)?;
        let has_stream = first > 0;
        if has_stream {
            if !self.options.solid {
                self.close_folder()?;
            }
            let folder = self.open_folder()?;
            let mut crc = Crc32::default();
            let mut size = 0u64;
            let mut n = first;
            while n > 0 {
                folder.encoder.write_all(&buf[..n])?;
                crc.update(&buf[..n]);
                size += n as u64;
                n = read_some(data, &mut buf)?;
            }
            folder.streams.push((size, crc.finalize()));
        }
        self.files.push(record(item, has_stream));
        Ok(())
    }

    /// Adds a directory entry.
    ///
    /// # Errors
    /// Never fails today; kept fallible for symmetry with the other `add_*` methods.
    pub fn add_directory(&mut self, item: &NewItem) -> io::Result<()> {
        self.files.push(record(item, false));
        Ok(())
    }

    /// Adds a symbolic link pointing to `target`.
    ///
    /// # Errors
    /// Any I/O error from writing the archive.
    pub fn add_symlink(&mut self, item: &NewItem, target: &str) -> io::Result<()> {
        self.add_file(item, &mut target.as_bytes())
    }

    fn open_folder(&mut self) -> io::Result<&mut OpenFolder<W>> {
        if self.open.is_none() {
            let crypto = self.crypto()?;
            let out = self.out.take().ok_or_else(missing_sink)?;
            let sink = Sink::new(out, crypto.as_ref());
            let aes = crypto.as_ref().map(|(_, params)| aes_coder(params));
            let (encoder, coder) = if self.options.level == 0 {
                let copy = aes.is_none().then(|| vec![0x01, 0x00]);
                (Encoder::Copy(sink), copy)
            } else {
                let options = Lzma2Options::with_preset(self.options.level.min(9));
                let code = lzma2_code(options.lzma_options.dict_size);
                (
                    Encoder::Lzma2(Box::new(Lzma2Writer::new(sink, options))),
                    Some(vec![0x21, 0x21, 0x01, code]),
                )
            };
            self.open = Some(OpenFolder {
                encoder,
                coder,
                aes,
                streams: Vec::new(),
            });
        }
        self.open.as_mut().ok_or_else(missing_sink)
    }

    fn close_folder(&mut self) -> io::Result<()> {
        if let Some(folder) = self.open.take() {
            let (out, packed, compressed) = folder.encoder.finish()?.finish()?;
            self.folders.push(ClosedFolder::new(
                folder.coder,
                folder.aes,
                packed,
                compressed,
                folder.streams,
            ));
            self.out = Some(out);
        }
        Ok(())
    }

    /// Writes the header database and start header and returns the sink.
    ///
    /// # Errors
    /// Any I/O error from compressing or writing.
    pub fn finish(mut self) -> io::Result<W> {
        self.close_folder()?;
        let mut out = self.out.take().ok_or_else(missing_sink)?;
        let packed_total: u64 = self.folders.iter().map(|f| f.packed).sum();
        let mut header = vec![K_HEADER];
        if !self.folders.is_empty() {
            header.push(K_MAIN_STREAMS_INFO);
            write_streams_info(&mut header, 0, &self.folders);
        }
        if !self.files.is_empty() {
            // Like 7-Zip: items with data first (in stream order), then empty files
            // and folders. Some readers mis-assign streams when they are interleaved.
            self.files.sort_by_key(|f| !f.has_stream);
            write_files_info(&mut header, &self.files);
        }
        header.push(K_END);

        let encrypt_header = self.options.encrypt_header && self.key.is_some();
        if self.options.compress_header || encrypt_header {
            let crypto = if encrypt_header { self.crypto()? } else { None };
            let mut options = LzmaOptions::with_preset(5);
            options.dict_size = HEADER_DICTIONARY;
            let mut encoder =
                LzmaWriter::new_no_header(Sink::new(&mut out, crypto.as_ref()), &options, false)?;
            let props = encoder.props();
            encoder.write_all(&header)?;
            let (_, packed, compressed) = encoder.finish()?.finish()?;
            let mut coder = vec![0x23, 0x03, 0x01, 0x01, 0x05, props];
            coder.extend_from_slice(&HEADER_DICTIONARY.to_le_bytes());
            let folder = ClosedFolder::new(
                Some(coder),
                crypto.as_ref().map(|(_, params)| aes_coder(params)),
                packed,
                compressed,
                vec![(header.len() as u64, Crc32::checksum(&header))],
            );
            let mut wrapper = vec![K_ENCODED_HEADER];
            write_streams_info(&mut wrapper, packed_total, std::slice::from_ref(&folder));
            header = wrapper;
        }

        let header_pos = out.stream_position()?;
        out.write_all(&header)?;
        let end = out.stream_position()?;
        let next_offset = header_pos - self.start - 32;
        let mut tail = Vec::with_capacity(20);
        tail.extend_from_slice(&next_offset.to_le_bytes());
        tail.extend_from_slice(&(header.len() as u64).to_le_bytes());
        tail.extend_from_slice(&Crc32::checksum(&header).to_le_bytes());
        let mut start = Vec::with_capacity(32);
        start.extend_from_slice(SIGNATURE);
        start.extend_from_slice(&[0, 4]);
        start.extend_from_slice(&Crc32::checksum(&tail).to_le_bytes());
        start.extend_from_slice(&tail);
        out.seek(SeekFrom::Start(self.start))?;
        out.write_all(&start)?;
        out.seek(SeekFrom::Start(end))?;
        out.flush()?;
        Ok(out)
    }
}

/// Reads until `buf` has data or the source is exhausted.
fn read_some(data: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match data.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

/// Smallest LZMA2 dictionary code whose size covers `dict`.
fn lzma2_code(dict: u32) -> u8 {
    (0u8..40)
        .find(|&c| lzma2_dictionary(c).is_some_and(|size| size >= u64::from(dict)))
        .unwrap_or(40)
}

fn record(item: &NewItem, has_stream: bool) -> FileRecord {
    let is_dir = item.kind == EntryKind::Directory;
    let mut attributes =
        item.dos_attributes
            .unwrap_or(if is_dir { ATTR_DIRECTORY } else { ATTR_ARCHIVE });
    if is_dir {
        attributes |= ATTR_DIRECTORY;
    }
    let type_bits = match item.kind {
        EntryKind::File => S_IFREG,
        EntryKind::Directory => S_IFDIR,
        EntryKind::Symlink => S_IFLNK,
    };
    let mode = item
        .unix_mode
        .or((item.kind == EntryKind::Symlink).then_some(0o777));
    if let Some(mode) = mode {
        attributes =
            (attributes & 0x7FFF) | ATTR_UNIX_EXTENSION | (((mode & 0o7777) | type_bits) << 16);
    }
    let mtime = match item.modified {
        Some(FileTime::Unix { seconds, nanos }) => seconds
            .checked_add(FILETIME_UNIX_DIFF)
            .and_then(|s| u64::try_from(s).ok())
            .and_then(|s| s.checked_mul(10_000_000))
            .and_then(|ticks| ticks.checked_add(u64::from(nanos / 100))),
        _ => None,
    };
    FileRecord {
        name: item.name.trim_matches('/').to_owned(),
        has_stream,
        is_dir,
        mtime,
        attributes,
    }
}

/// Appends a 7z `NUMBER`.
fn put_number(buf: &mut Vec<u8>, n: u64) {
    for extra in 0u32..8 {
        if n < 1u64 << (8 * extra + 7 - extra) {
            let high = u8::try_from(n >> (8 * extra)).unwrap_or(0);
            let marker = u8::try_from((0xFF00u16 >> extra) & 0xFF).unwrap_or(0);
            buf.push(marker | high);
            buf.extend_from_slice(&n.to_le_bytes()[..extra as usize]);
            return;
        }
    }
    buf.push(0xFF);
    buf.extend_from_slice(&n.to_le_bytes());
}

fn put_count(buf: &mut Vec<u8>, n: usize) {
    put_number(buf, n as u64);
}

fn put_bits(buf: &mut Vec<u8>, bits: &[bool]) {
    let mut bytes = vec![0u8; bits.len().div_ceil(8)];
    for (i, _) in bits.iter().enumerate().filter(|(_, b)| **b) {
        bytes[i / 8] |= 0x80 >> (i % 8);
    }
    buf.extend_from_slice(&bytes);
}

fn put_property(buf: &mut Vec<u8>, id: u8, data: &[u8]) {
    buf.push(id);
    put_count(buf, data.len());
    buf.extend_from_slice(data);
}

/// Appends an `AllAreDefined` byte (and bit vector when needed).
fn put_defined(buf: &mut Vec<u8>, defined: &[bool]) {
    if defined.iter().all(|d| *d) {
        buf.push(1);
    } else {
        buf.push(0);
        put_bits(buf, defined);
    }
}

fn write_streams_info(buf: &mut Vec<u8>, pack_pos: u64, folders: &[ClosedFolder]) {
    buf.push(K_PACK_INFO);
    put_number(buf, pack_pos);
    put_count(buf, folders.len());
    buf.push(K_SIZE);
    for folder in folders {
        put_number(buf, folder.packed);
    }
    buf.push(K_END);

    buf.push(K_UNPACK_INFO);
    buf.push(K_FOLDER);
    put_count(buf, folders.len());
    buf.push(0);
    for folder in folders {
        put_count(buf, folder.coders.len());
        for coder in &folder.coders {
            buf.extend_from_slice(coder);
        }
        // Coder i reads the output of coder i + 1; the last one reads the pack
        // stream, which is implied when there is only one.
        for i in 1..folder.coders.len() {
            put_count(buf, i - 1);
            put_count(buf, i);
        }
    }
    buf.push(K_CODERS_UNPACK_SIZE);
    for folder in folders {
        for &size in &folder.unpack_sizes {
            put_number(buf, size);
        }
    }
    buf.push(K_END);

    buf.push(K_SUBSTREAMS_INFO);
    buf.push(K_NUM_UNPACK_STREAM);
    for folder in folders {
        put_count(buf, folder.streams.len());
    }
    buf.push(K_SIZE);
    for folder in folders {
        for &(size, _) in &folder.streams[..folder.streams.len().saturating_sub(1)] {
            put_number(buf, size);
        }
    }
    buf.push(K_CRC);
    buf.push(1);
    for &(_, crc) in folders.iter().flat_map(|f| &f.streams) {
        buf.extend_from_slice(&crc.to_le_bytes());
    }
    buf.push(K_END);
    buf.push(K_END);
}

fn write_files_info(buf: &mut Vec<u8>, files: &[FileRecord]) {
    buf.push(K_FILES_INFO);
    put_count(buf, files.len());

    let empty: Vec<bool> = files.iter().map(|f| !f.has_stream).collect();
    if empty.iter().any(|e| *e) {
        let mut bits = Vec::new();
        put_bits(&mut bits, &empty);
        put_property(buf, K_EMPTY_STREAM, &bits);
        let empty_files: Vec<bool> = files
            .iter()
            .filter(|f| !f.has_stream)
            .map(|f| !f.is_dir)
            .collect();
        if empty_files.iter().any(|e| *e) {
            let mut bits = Vec::new();
            put_bits(&mut bits, &empty_files);
            put_property(buf, K_EMPTY_FILE, &bits);
        }
    }

    let mut names = vec![0u8];
    for file in files {
        for unit in file.name.encode_utf16().chain([0]) {
            names.extend_from_slice(&unit.to_le_bytes());
        }
    }
    put_property(buf, K_NAME, &names);

    let defined: Vec<bool> = files.iter().map(|f| f.mtime.is_some()).collect();
    if defined.iter().any(|d| *d) {
        let mut times = Vec::new();
        put_defined(&mut times, &defined);
        times.push(0);
        for ticks in files.iter().filter_map(|f| f.mtime) {
            times.extend_from_slice(&ticks.to_le_bytes());
        }
        put_property(buf, K_MTIME, &times);
    }

    let mut attributes = vec![1, 0];
    for file in files {
        attributes.extend_from_slice(&file.attributes.to_le_bytes());
    }
    put_property(buf, K_WIN_ATTRIBUTES, &attributes);
    buf.push(K_END);
}

#[cfg(test)]
mod tests {
    use rarx_io::Cursor;

    use super::*;

    #[test]
    fn numbers_round_trip() {
        for n in [0, 0x7F, 0x80, 0x3FFF, 0x4000, 0x1F_FFFF, 1 << 40, u64::MAX] {
            let mut buf = Vec::new();
            put_number(&mut buf, n);
            assert_eq!(Cursor::new(&buf).read_7z_number().unwrap(), n, "{n:#x}");
        }
    }

    #[test]
    fn dictionary_codes() {
        assert_eq!(lzma2_code(1 << 20), 16);
        assert_eq!(lzma2_code(3 << 20), 19);
        assert_eq!(lzma2_code(8 << 20), 22);
        assert_eq!(lzma2_code(4096), 0);
    }
}
