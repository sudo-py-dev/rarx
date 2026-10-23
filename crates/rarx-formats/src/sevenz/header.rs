//! Parsing of the 7z header database (`7zFormat.txt` from the LZMA SDK).

use rarx_io::Cursor;

use crate::{FileTime, FormatError};

pub(super) const K_END: u8 = 0x00;
pub(super) const K_HEADER: u8 = 0x01;
const K_ARCHIVE_PROPERTIES: u8 = 0x02;
const K_ADDITIONAL_STREAMS_INFO: u8 = 0x03;
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
const K_ANTI: u8 = 0x10;
const K_NAME: u8 = 0x11;
const K_CTIME: u8 = 0x12;
const K_ATIME: u8 = 0x13;
const K_MTIME: u8 = 0x14;
const K_WIN_ATTRIBUTES: u8 = 0x15;
pub(super) const K_ENCODED_HEADER: u8 = 0x17;

/// More coders than this in one folder is treated as corruption.
const MAX_CODERS: u64 = 64;

/// One coder (decompressor or filter) of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Coder {
    pub id: Vec<u8>,
    pub num_in: usize,
    pub num_out: usize,
    pub props: Vec<u8>,
}

/// A chain of coders whose output is one solid stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Folder {
    pub coders: Vec<Coder>,
    /// `(in_index, out_index)`: coder input `in_index` is fed by output `out_index`.
    pub bind_pairs: Vec<(usize, usize)>,
    /// Coder inputs fed directly from pack streams, in pack-stream order.
    pub packed_streams: Vec<usize>,
    /// Size of every coder output, indexed by global output number.
    pub unpack_sizes: Vec<u64>,
    pub crc: Option<u32>,
    /// Index of this folder's first pack stream.
    pub first_pack_stream: usize,
    /// Number of files stored in this folder.
    pub num_streams: u64,
}

impl Folder {
    pub fn total_in(&self) -> usize {
        self.coders.iter().map(|c| c.num_in).sum()
    }

    pub fn total_out(&self) -> usize {
        self.coders.iter().map(|c| c.num_out).sum()
    }

    /// The output not consumed by any bind pair: the folder's final data.
    pub fn main_output(&self) -> Option<usize> {
        (0..self.total_out()).find(|out| !self.bind_pairs.iter().any(|&(_, o)| o == *out))
    }

    pub fn unpack_size(&self) -> u64 {
        self.main_output()
            .and_then(|o| self.unpack_sizes.get(o).copied())
            .unwrap_or(0)
    }
}

/// Pack streams, folders and per-file sizes/CRCs.
#[derive(Clone, Debug, Default)]
pub(super) struct StreamsInfo {
    pub pack_pos: u64,
    pub pack_sizes: Vec<u64>,
    pub folders: Vec<Folder>,
    /// Size and CRC of every file stream, in folder order.
    pub substreams: Vec<(u64, Option<u32>)>,
}

/// One item from `kFilesInfo`.
#[derive(Clone, Debug, Default)]
pub(super) struct FileRecord {
    pub name: String,
    pub has_stream: bool,
    pub is_dir: bool,
    pub is_anti: bool,
    pub ctime: Option<FileTime>,
    pub atime: Option<FileTime>,
    pub mtime: Option<FileTime>,
    pub attributes: Option<u32>,
}

/// The decoded header database.
#[derive(Clone, Debug, Default)]
pub(super) struct Header {
    pub streams: StreamsInfo,
    pub files: Vec<FileRecord>,
}

fn corrupt(what: &str) -> FormatError {
    FormatError::Corrupt(format!("7z header: {what}"))
}

/// Reads a 7z `NUMBER` that must fit in memory as a count or index.
fn read_count(cur: &mut Cursor<'_>, limit: u64) -> Result<usize, FormatError> {
    let value = cur.read_7z_number()?;
    if value > limit {
        return Err(corrupt("count is out of range"));
    }
    usize::try_from(value).map_err(|_| corrupt("count is out of range"))
}

/// Reads a count bounded by the remaining header bytes (every counted item needs
/// at least one bit of header).
fn read_bounded_count(cur: &mut Cursor<'_>) -> Result<usize, FormatError> {
    let limit = (cur.remaining() as u64).saturating_mul(8).saturating_add(8);
    read_count(cur, limit)
}

fn read_bits(cur: &mut Cursor<'_>, count: usize) -> Result<Vec<bool>, FormatError> {
    let bytes = cur.read_bytes(count.div_ceil(8))?;
    Ok((0..count)
        .map(|i| bytes[i / 8] & (0x80 >> (i % 8)) != 0)
        .collect())
}

/// Reads an `AllAreDefined` byte followed, if zero, by a bit vector.
fn read_defined(cur: &mut Cursor<'_>, count: usize) -> Result<Vec<bool>, FormatError> {
    if cur.read_u8()? != 0 {
        Ok(vec![true; count])
    } else {
        read_bits(cur, count)
    }
}

fn read_digests(cur: &mut Cursor<'_>, count: usize) -> Result<Vec<Option<u32>>, FormatError> {
    let defined = read_defined(cur, count)?;
    defined
        .into_iter()
        .map(|d| {
            if d {
                cur.read_u32_le().map(Some).map_err(Into::into)
            } else {
                Ok(None)
            }
        })
        .collect()
}

fn expect(cur: &mut Cursor<'_>, id: u8) -> Result<(), FormatError> {
    if cur.read_u8()? == id {
        Ok(())
    } else {
        Err(corrupt("unexpected property id"))
    }
}

/// Parses a plain `kHeader` block (the leading id already consumed).
pub(super) fn parse_header(cur: &mut Cursor<'_>) -> Result<Header, FormatError> {
    let mut header = Header::default();
    let mut id = cur.read_u8()?;
    if id == K_ARCHIVE_PROPERTIES {
        loop {
            if cur.read_u8()? == K_END {
                break;
            }
            let size = read_count(cur, cur.remaining() as u64)?;
            cur.skip(size)?;
        }
        id = cur.read_u8()?;
    }
    if id == K_ADDITIONAL_STREAMS_INFO {
        // Only used by very old archivers; parse to skip it.
        parse_streams_info(cur)?;
        id = cur.read_u8()?;
    }
    if id == K_MAIN_STREAMS_INFO {
        header.streams = parse_streams_info(cur)?;
        id = cur.read_u8()?;
    }
    if id == K_FILES_INFO {
        header.files = parse_files_info(cur)?;
        id = cur.read_u8()?;
    }
    if id != K_END {
        return Err(corrupt("missing end of header"));
    }
    Ok(header)
}

/// Parses `StreamsInfo` up to and including its `kEnd`.
pub(super) fn parse_streams_info(cur: &mut Cursor<'_>) -> Result<StreamsInfo, FormatError> {
    let mut info = StreamsInfo::default();
    let mut id = cur.read_u8()?;
    if id == K_PACK_INFO {
        info.pack_pos = cur.read_7z_number()?;
        let count = read_bounded_count(cur)?;
        id = cur.read_u8()?;
        if id == K_SIZE {
            info.pack_sizes = (0..count)
                .map(|_| cur.read_7z_number())
                .collect::<Result<_, _>>()?;
            id = cur.read_u8()?;
        }
        if id == K_CRC {
            read_digests(cur, count)?;
            id = cur.read_u8()?;
        }
        if id != K_END {
            return Err(corrupt("bad pack info"));
        }
        if info.pack_sizes.len() != count {
            return Err(corrupt("pack sizes missing"));
        }
        id = cur.read_u8()?;
    }
    if id == K_UNPACK_INFO {
        info.folders = parse_unpack_info(cur)?;
        id = cur.read_u8()?;
    }
    for folder in &mut info.folders {
        folder.num_streams = 1;
    }
    let mut sizes: Option<Vec<u64>> = None;
    let mut digests: Option<Vec<Option<u32>>> = None;
    if id == K_SUBSTREAMS_INFO {
        (sizes, digests) = parse_substreams(cur, &mut info.folders)?;
        id = cur.read_u8()?;
    }
    if id != K_END {
        return Err(corrupt("bad streams info"));
    }

    let mut next_pack = 0usize;
    for folder in &mut info.folders {
        folder.first_pack_stream = next_pack;
        next_pack += folder.packed_streams.len();
    }
    if next_pack > info.pack_sizes.len() {
        return Err(corrupt("folders reference missing pack streams"));
    }
    info.substreams = substream_table(&info.folders, sizes, digests)?;
    Ok(info)
}

fn parse_unpack_info(cur: &mut Cursor<'_>) -> Result<Vec<Folder>, FormatError> {
    expect(cur, K_FOLDER)?;
    let count = read_bounded_count(cur)?;
    if cur.read_u8()? != 0 {
        return Err(FormatError::Unsupported(
            "7z external folder data".to_owned(),
        ));
    }
    let mut folders = Vec::with_capacity(count.min(cur.remaining()));
    for _ in 0..count {
        folders.push(parse_folder(cur)?);
    }
    expect(cur, K_CODERS_UNPACK_SIZE)?;
    for folder in &mut folders {
        folder.unpack_sizes = (0..folder.total_out())
            .map(|_| cur.read_7z_number())
            .collect::<Result<_, _>>()?;
    }
    let mut id = cur.read_u8()?;
    if id == K_CRC {
        for (folder, crc) in folders.iter_mut().zip(read_digests(cur, count)?) {
            folder.crc = crc;
        }
        id = cur.read_u8()?;
    }
    if id != K_END {
        return Err(corrupt("bad unpack info"));
    }
    Ok(folders)
}

fn parse_folder(cur: &mut Cursor<'_>) -> Result<Folder, FormatError> {
    let num_coders = read_count(cur, MAX_CODERS)?;
    if num_coders == 0 {
        return Err(corrupt("folder without coders"));
    }
    let mut folder = Folder::default();
    for _ in 0..num_coders {
        let flags = cur.read_u8()?;
        if flags & 0x80 != 0 {
            return Err(FormatError::Unsupported(
                "7z alternative coder methods".to_owned(),
            ));
        }
        let id = cur.read_bytes(usize::from(flags & 0x0F))?.to_vec();
        let (num_in, num_out) = if flags & 0x10 != 0 {
            (read_count(cur, MAX_CODERS)?, read_count(cur, MAX_CODERS)?)
        } else {
            (1, 1)
        };
        let props = if flags & 0x20 != 0 {
            let len = read_count(cur, cur.remaining() as u64)?;
            cur.read_bytes(len)?.to_vec()
        } else {
            Vec::new()
        };
        folder.coders.push(Coder {
            id,
            num_in,
            num_out,
            props,
        });
    }
    let (total_in, total_out) = (folder.total_in(), folder.total_out());
    if total_out == 0 || total_in < total_out - 1 {
        return Err(corrupt("inconsistent coder streams"));
    }
    for _ in 0..total_out - 1 {
        let in_index = read_count(cur, (total_in - 1) as u64)?;
        let out_index = read_count(cur, (total_out - 1) as u64)?;
        folder.bind_pairs.push((in_index, out_index));
    }
    let num_packed = total_in - (total_out - 1);
    if num_packed == 1 {
        let unbound = (0..total_in).find(|i| !folder.bind_pairs.iter().any(|&(b, _)| b == *i));
        folder
            .packed_streams
            .push(unbound.ok_or_else(|| corrupt("no packed stream"))?);
    } else {
        for _ in 0..num_packed {
            folder
                .packed_streams
                .push(read_count(cur, (total_in - 1) as u64)?);
        }
    }
    if folder.main_output().is_none() {
        return Err(corrupt("folder has no final output"));
    }
    Ok(folder)
}

type Substreams = (Option<Vec<u64>>, Option<Vec<Option<u32>>>);

fn parse_substreams(
    cur: &mut Cursor<'_>,
    folders: &mut [Folder],
) -> Result<Substreams, FormatError> {
    let mut id = cur.read_u8()?;
    if id == K_NUM_UNPACK_STREAM {
        for folder in folders.iter_mut() {
            folder.num_streams = read_bounded_count(cur)? as u64;
        }
        id = cur.read_u8()?;
    }
    let mut sizes = None;
    if id == K_SIZE {
        let mut all = Vec::new();
        for folder in folders.iter() {
            if folder.num_streams == 0 {
                continue;
            }
            let mut sum = 0u64;
            for _ in 1..folder.num_streams {
                let size = cur.read_7z_number()?;
                sum = sum
                    .checked_add(size)
                    .ok_or_else(|| corrupt("stream sizes overflow"))?;
                all.push(size);
            }
            let last = folder
                .unpack_size()
                .checked_sub(sum)
                .ok_or_else(|| corrupt("stream sizes exceed folder"))?;
            all.push(last);
        }
        sizes = Some(all);
        id = cur.read_u8()?;
    }
    let mut digests = None;
    if id == K_CRC {
        let count: u64 = folders
            .iter()
            .map(|f| {
                if f.num_streams == 1 && f.crc.is_some() {
                    0
                } else {
                    f.num_streams
                }
            })
            .sum();
        let count = usize::try_from(count).map_err(|_| corrupt("too many streams"))?;
        digests = Some(read_digests(cur, count)?);
        id = cur.read_u8()?;
    }
    if id != K_END {
        return Err(corrupt("bad substreams info"));
    }
    Ok((sizes, digests))
}

/// Combines folder data and `SubStreamsInfo` into one `(size, crc)` per file stream.
fn substream_table(
    folders: &[Folder],
    sizes: Option<Vec<u64>>,
    digests: Option<Vec<Option<u32>>>,
) -> Result<Vec<(u64, Option<u32>)>, FormatError> {
    let mut sizes = sizes.map(Vec::into_iter);
    let mut digests = digests.map(Vec::into_iter);
    let mut table = Vec::new();
    for folder in folders {
        let only_one = folder.num_streams == 1;
        for _ in 0..folder.num_streams {
            let size = match sizes.as_mut() {
                Some(it) => it.next().ok_or_else(|| corrupt("missing stream size"))?,
                None if only_one => folder.unpack_size(),
                None => return Err(corrupt("missing stream sizes")),
            };
            let crc = if only_one && folder.crc.is_some() {
                folder.crc
            } else {
                digests.as_mut().and_then(Iterator::next).flatten()
            };
            table.push((size, crc));
        }
    }
    Ok(table)
}

fn parse_files_info(cur: &mut Cursor<'_>) -> Result<Vec<FileRecord>, FormatError> {
    let count = read_bounded_count(cur)?;
    let mut files = vec![
        FileRecord {
            has_stream: true,
            ..FileRecord::default()
        };
        count
    ];
    let mut empty_streams: Vec<usize> = Vec::new();
    loop {
        let id = cur.read_u8()?;
        if id == K_END {
            break;
        }
        let size = read_count(cur, cur.remaining() as u64)?;
        let mut prop = Cursor::new(cur.read_bytes(size)?);
        match id {
            K_EMPTY_STREAM => {
                let bits = read_bits(&mut prop, count)?;
                empty_streams = bits
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b)
                    .map(|(i, _)| i)
                    .collect();
                for &i in &empty_streams {
                    files[i].has_stream = false;
                    files[i].is_dir = true;
                }
            }
            K_EMPTY_FILE | K_ANTI => {
                let bits = read_bits(&mut prop, empty_streams.len())?;
                for (&i, set) in empty_streams.iter().zip(bits) {
                    if set {
                        if id == K_EMPTY_FILE {
                            files[i].is_dir = false;
                        } else {
                            files[i].is_anti = true;
                        }
                    }
                }
            }
            K_NAME => {
                if prop.read_u8()? != 0 {
                    return Err(FormatError::Unsupported("7z external names".to_owned()));
                }
                let raw = prop.read_bytes(prop.remaining())?;
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&pair| u16::from_le_bytes(pair))
                    .collect();
                let mut names = units.split(|&u| u == 0);
                for file in &mut files {
                    let name = names.next().ok_or_else(|| corrupt("missing file names"))?;
                    file.name = String::from_utf16_lossy(name);
                }
            }
            K_CTIME | K_ATIME | K_MTIME => {
                let defined = read_defined(&mut prop, count)?;
                if prop.read_u8()? != 0 {
                    return Err(FormatError::Unsupported("7z external times".to_owned()));
                }
                for (file, is_set) in files.iter_mut().zip(defined) {
                    if is_set {
                        let time = Some(FileTime::from_windows_ticks(prop.read_u64_le()?));
                        match id {
                            K_CTIME => file.ctime = time,
                            K_ATIME => file.atime = time,
                            _ => file.mtime = time,
                        }
                    }
                }
            }
            K_WIN_ATTRIBUTES => {
                let defined = read_defined(&mut prop, count)?;
                if prop.read_u8()? != 0 {
                    return Err(FormatError::Unsupported(
                        "7z external attributes".to_owned(),
                    ));
                }
                for (file, is_set) in files.iter_mut().zip(defined) {
                    if is_set {
                        file.attributes = Some(prop.read_u32_le()?);
                    }
                }
            }
            // kComment, kStartPos, kDummy and unknown properties are skipped by size.
            _ => {}
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_vectors_are_msb_first() {
        let mut cur = Cursor::new(&[0b1010_0000, 0b1000_0000]);
        assert_eq!(
            read_bits(&mut cur, 9).unwrap(),
            [true, false, true, false, false, false, false, false, true]
        );
    }

    #[test]
    fn parses_bcj2_style_folder() {
        // BCJ2 (4 in, 1 out) fed by three LZMA coders; pack streams are LZMA inputs
        // 5, 6, 7 and BCJ2 input 3.
        let mut data = vec![4u8];
        data.extend([0x14, 0x03, 0x03, 0x01, 0x1B, 0x04, 0x01]); // BCJ2, complex 4 in / 1 out
        for _ in 0..3 {
            data.extend([0x23, 0x03, 0x01, 0x01, 0x05, 0x5D, 0, 0, 1, 0]); // LZMA + props
        }
        data.extend([0, 1, 1, 2, 2, 3]); // bind pairs: in0<-out1, in1<-out2, in2<-out3
        data.extend([4, 5, 6, 3]); // packed streams
        let folder = parse_folder(&mut Cursor::new(&data)).unwrap();
        assert_eq!(folder.coders.len(), 4);
        assert_eq!(folder.total_in(), 7);
        assert_eq!(folder.bind_pairs, [(0, 1), (1, 2), (2, 3)]);
        assert_eq!(folder.packed_streams, [4, 5, 6, 3]);
        assert_eq!(folder.main_output(), Some(0));
    }

    #[test]
    fn rejects_absurd_counts() {
        let data = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
        assert!(read_bounded_count(&mut Cursor::new(&data)).is_err());
    }
}
