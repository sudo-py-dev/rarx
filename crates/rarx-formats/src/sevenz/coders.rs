//! Turning a 7z folder (a graph of coders) into one decoded stream.

use std::fmt::Write as _;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use flate2::bufread::DeflateDecoder;
use lzma_rust2::filter::bcj::BcjReader;
use lzma_rust2::filter::bcj2::Bcj2Reader;
use lzma_rust2::filter::delta::DeltaReader;
use lzma_rust2::{Lzma2Reader, LzmaReader};

use super::header::{Coder, Folder};
use crate::{FormatError, ReadSeek};

/// The archive file, shared by every pack-stream reader of a folder.
pub(super) type Shared = Arc<Mutex<Box<dyn ReadSeek>>>;
pub(super) type Stream = Box<dyn Read + Send>;

const REGION_BUFFER: usize = 64 * 1024;
const MIN_DICTIONARY: u64 = 4096;

const COPY: &[u8] = &[0x00];
const DELTA: &[u8] = &[0x03];
const BCJ_X86: &[u8] = &[0x03, 0x03, 0x01, 0x03];
const BCJ2: &[u8] = &[0x03, 0x03, 0x01, 0x1B];
const PPC: &[u8] = &[0x03, 0x03, 0x02, 0x05];
const IA64: &[u8] = &[0x03, 0x03, 0x04, 0x01];
const ARM: &[u8] = &[0x03, 0x03, 0x05, 0x01];
const ARMT: &[u8] = &[0x03, 0x03, 0x07, 0x01];
const SPARC: &[u8] = &[0x03, 0x03, 0x08, 0x05];
const ARM64: &[u8] = &[0x0A];
const RISCV: &[u8] = &[0x0B];
const LZMA: &[u8] = &[0x03, 0x01, 0x01];
const LZMA2: &[u8] = &[0x21];
const PPMD: &[u8] = &[0x03, 0x04, 0x01];
const BZIP2: &[u8] = &[0x04, 0x02, 0x02];
const DEFLATE: &[u8] = &[0x04, 0x01, 0x08];
const DEFLATE64: &[u8] = &[0x04, 0x01, 0x09];
const ZSTD: &[u8] = &[0x04, 0xF7, 0x11, 0x01];
pub(super) const AES: &[u8] = &[0x06, 0xF1, 0x07, 0x01];

/// Reads one byte range of the shared archive file.
struct Region {
    src: Shared,
    pos: u64,
    end: u64,
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

/// Builds the decoded stream of `folder`. `packs` holds the absolute `(offset, size)`
/// of each of the folder's pack streams.
pub(super) fn folder_stream(
    src: &Shared,
    folder: &Folder,
    packs: &[(u64, u64)],
) -> Result<Stream, FormatError> {
    let main = folder
        .main_output()
        .ok_or_else(|| FormatError::Corrupt("7z folder has no output".to_owned()))?;
    let mut graph = Graph {
        src,
        folder,
        packs,
        depth: 0,
    };
    graph.output(main)
}

struct Graph<'a> {
    src: &'a Shared,
    folder: &'a Folder,
    packs: &'a [(u64, u64)],
    depth: usize,
}

impl Graph<'_> {
    /// Decoded stream of coder output `out_index`.
    fn output(&mut self, out_index: usize) -> Result<Stream, FormatError> {
        self.depth += 1;
        if self.depth > self.folder.coders.len() {
            return Err(FormatError::Corrupt(
                "7z coder graph has a cycle".to_owned(),
            ));
        }
        let (mut first_in, mut first_out) = (0, 0);
        let coder = self
            .folder
            .coders
            .iter()
            .find(|c| {
                if out_index < first_out + c.num_out {
                    return true;
                }
                first_in += c.num_in;
                first_out += c.num_out;
                false
            })
            .ok_or_else(|| FormatError::Corrupt("7z bind pair points nowhere".to_owned()))?;
        let mut inputs = Vec::with_capacity(coder.num_in);
        for in_index in first_in..first_in + coder.num_in {
            inputs.push(self.input(in_index)?);
        }
        let size = self
            .folder
            .unpack_sizes
            .get(out_index)
            .copied()
            .unwrap_or(0);
        self.depth -= 1;
        decoder(coder, inputs, size)
    }

    /// Stream feeding coder input `in_index`: a pack stream or another coder.
    fn input(&mut self, in_index: usize) -> Result<Stream, FormatError> {
        if let Some(p) = self
            .folder
            .packed_streams
            .iter()
            .position(|&i| i == in_index)
        {
            let &(offset, size) = self
                .packs
                .get(p)
                .ok_or_else(|| FormatError::Corrupt("7z pack stream missing".to_owned()))?;
            let region = Region {
                src: Arc::clone(self.src),
                pos: offset,
                end: offset + size,
            };
            return Ok(Box::new(BufReader::with_capacity(REGION_BUFFER, region)));
        }
        let &(_, out) = self
            .folder
            .bind_pairs
            .iter()
            .find(|&&(i, _)| i == in_index)
            .ok_or_else(|| FormatError::Corrupt("7z coder input is unconnected".to_owned()))?;
        self.output(out)
    }
}

/// Creates the decoder for one coder over its input streams.
fn decoder(coder: &Coder, mut inputs: Vec<Stream>, size: u64) -> Result<Stream, FormatError> {
    let unsupported = || FormatError::Unsupported(format!("7z method {}", method_label(coder)));
    let bad_props =
        || FormatError::Corrupt(format!("7z {} properties are invalid", method_label(coder)));
    let id = coder.id.as_slice();
    if id == BCJ2 {
        if inputs.len() != 4 {
            return Err(bad_props());
        }
        return Ok(Box::new(Bcj2Reader::new(inputs, size)));
    }
    let input = match inputs.len() {
        1 => inputs.remove(0),
        _ => return Err(unsupported()),
    };
    // A dictionary larger than the output is never used, so do not allocate it.
    let dictionary =
        |declared: u64| u32::try_from(declared.min(size.max(MIN_DICTIONARY))).unwrap_or(u32::MAX);
    Ok(match id {
        COPY => input,
        LZMA => {
            let [props, d0, d1, d2, d3] = coder.props[..] else {
                return Err(bad_props());
            };
            let dict = dictionary(u64::from(u32::from_le_bytes([d0, d1, d2, d3])));
            Box::new(
                LzmaReader::new_with_props(input, size, props, dict, None)
                    .map_err(FormatError::Io)?,
            )
        }
        LZMA2 => {
            let [code] = coder.props[..] else {
                return Err(bad_props());
            };
            Box::new(Lzma2Reader::new(
                input,
                dictionary(lzma2_dictionary(code).ok_or_else(bad_props)?),
                None,
            ))
        }
        DELTA => {
            let distance = usize::from(coder.props.first().copied().unwrap_or(0)) + 1;
            Box::new(DeltaReader::new(input, distance))
        }
        BCJ_X86 => Box::new(BcjReader::new_x86(input, 0)),
        ARM => Box::new(BcjReader::new_arm(input, 0)),
        ARMT => Box::new(BcjReader::new_arm_thumb(input, 0)),
        ARM64 => Box::new(BcjReader::new_arm64(input, 0)),
        PPC => Box::new(BcjReader::new_ppc(input, 0)),
        SPARC => Box::new(BcjReader::new_sparc(input, 0)),
        IA64 => Box::new(BcjReader::new_ia64(input, 0)),
        RISCV => Box::new(BcjReader::new_riscv(input, 0)),
        DEFLATE => Box::new(DeflateDecoder::new(BufReader::new(input))),
        AES => return Err(FormatError::Encrypted),
        _ => return Err(unsupported()),
    })
}

/// Dictionary size encoded in the LZMA2 property byte.
pub(super) fn lzma2_dictionary(code: u8) -> Option<u64> {
    match code {
        0..=39 => Some((2 | u64::from(code & 1)) << (code / 2 + 11)),
        40 => Some(u64::from(u32::MAX)),
        _ => None,
    }
}

/// Human-readable method name in 7-Zip style, e.g. `LZMA2:24` or `Delta:4`.
pub(super) fn method_label(coder: &Coder) -> String {
    let dict = |size: u64| {
        if size.is_power_of_two() {
            size.trailing_zeros().to_string()
        } else if size.is_multiple_of(1 << 20) {
            format!("{}m", size >> 20)
        } else {
            format!("{}k", size >> 10)
        }
    };
    match coder.id.as_slice() {
        COPY => "Copy".to_owned(),
        LZMA => match coder.props[..] {
            [_, a, b, c, d] => {
                format!("LZMA:{}", dict(u64::from(u32::from_le_bytes([a, b, c, d]))))
            }
            _ => "LZMA".to_owned(),
        },
        LZMA2 => match coder.props.first().copied().and_then(lzma2_dictionary) {
            Some(size) => format!("LZMA2:{}", dict(size)),
            None => "LZMA2".to_owned(),
        },
        DELTA => format!(
            "Delta:{}",
            u16::from(coder.props.first().copied().unwrap_or(0)) + 1
        ),
        BCJ_X86 => "BCJ".to_owned(),
        BCJ2 => "BCJ2".to_owned(),
        PPC => "PPC".to_owned(),
        IA64 => "IA64".to_owned(),
        ARM => "ARM".to_owned(),
        ARMT => "ARMT".to_owned(),
        SPARC => "SPARC".to_owned(),
        ARM64 => "ARM64".to_owned(),
        RISCV => "RISCV".to_owned(),
        PPMD => "PPMD".to_owned(),
        BZIP2 => "BZip2".to_owned(),
        DEFLATE => "Deflate".to_owned(),
        DEFLATE64 => "Deflate64".to_owned(),
        ZSTD => "ZSTD".to_owned(),
        AES => "7zAES".to_owned(),
        other => other.iter().fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02X}");
            hex
        }),
    }
}

/// Decodes a whole folder into memory (used for compressed headers).
pub(super) fn read_folder_to_vec(
    src: &Shared,
    folder: &Folder,
    packs: &[(u64, u64)],
    limit: u64,
) -> Result<Vec<u8>, FormatError> {
    let size = folder.unpack_size();
    if size > limit {
        return Err(FormatError::Corrupt(
            "7z compressed header is implausibly large".to_owned(),
        ));
    }
    let mut out = Vec::new();
    folder_stream(src, folder, packs)?
        .take(size)
        .read_to_end(&mut out)?;
    if out.len() as u64 != size {
        return Err(FormatError::Corrupt(
            "7z compressed header is truncated".to_owned(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coder(id: &[u8], props: &[u8]) -> Coder {
        Coder {
            id: id.to_vec(),
            num_in: 1,
            num_out: 1,
            props: props.to_vec(),
        }
    }

    #[test]
    fn lzma2_dictionary_sizes() {
        assert_eq!(lzma2_dictionary(0), Some(4 << 10));
        assert_eq!(lzma2_dictionary(1), Some(6 << 10));
        assert_eq!(lzma2_dictionary(24), Some(16 << 20));
        assert_eq!(lzma2_dictionary(40), Some(u64::from(u32::MAX)));
        assert_eq!(lzma2_dictionary(41), None);
    }

    #[test]
    fn labels() {
        assert_eq!(method_label(&coder(LZMA2, &[24])), "LZMA2:24");
        assert_eq!(method_label(&coder(LZMA2, &[25])), "LZMA2:24m");
        assert_eq!(method_label(&coder(LZMA, &[0x5D, 0, 0, 0, 1])), "LZMA:24");
        assert_eq!(method_label(&coder(DELTA, &[3])), "Delta:4");
        assert_eq!(method_label(&coder(&[0x12, 0x34], &[])), "1234");
    }
}
