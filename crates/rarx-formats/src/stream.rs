//! Single-stream compressors used on their own (`.gz`) or around TAR (`.tar.gz`)
//! (AGENT.md §5): gzip, bzip2, xz and Zstandard.

use std::io::{self, BufReader, Read, Write};

use flate2::bufread::MultiGzDecoder;
use flate2::write::GzEncoder;
use lzma_rust2::{XzOptions, XzReader, XzWriter};

use crate::FormatId;

/// Largest Zstandard window accepted when decoding (2 GiB, `--long=31`).
const ZSTD_WINDOW_LOG_MAX: u32 = 31;

/// A whole-stream compression format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamCodec {
    /// gzip (RFC 1952), all members.
    Gzip,
    /// bzip2, all streams.
    Bzip2,
    /// xz, all streams.
    Xz,
    /// Zstandard, all frames.
    Zstd,
}

impl StreamCodec {
    /// The codec of a stream format, if `format` is one.
    #[must_use]
    pub fn from_format(format: FormatId) -> Option<Self> {
        match format {
            FormatId::Gzip => Some(Self::Gzip),
            FormatId::Bzip2 => Some(Self::Bzip2),
            FormatId::Xz => Some(Self::Xz),
            FormatId::Zstd => Some(Self::Zstd),
            _ => None,
        }
    }

    /// The matching format identity.
    #[must_use]
    pub fn format(self) -> FormatId {
        match self {
            Self::Gzip => FormatId::Gzip,
            Self::Bzip2 => FormatId::Bzip2,
            Self::Xz => FormatId::Xz,
            Self::Zstd => FormatId::Zstd,
        }
    }

    /// Usual file extension, without the dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Gzip => "gz",
            Self::Bzip2 => "bz2",
            Self::Xz => "xz",
            Self::Zstd => "zst",
        }
    }

    /// Wraps `input` in a decoder for this codec.
    ///
    /// # Errors
    /// Any error from initialising the decoder.
    pub fn decoder<'a>(
        self,
        input: Box<dyn Read + Send + 'a>,
    ) -> io::Result<Box<dyn Read + Send + 'a>> {
        Ok(match self {
            Self::Gzip => Box::new(MultiGzDecoder::new(BufReader::new(input))),
            Self::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(input)),
            Self::Xz => Box::new(XzReader::new(input, true)),
            Self::Zstd => {
                let mut decoder = zstd::stream::read::Decoder::new(input)?;
                decoder.window_log_max(ZSTD_WINDOW_LOG_MAX)?;
                Box::new(decoder)
            }
        })
    }

    /// Starts compressing into `out` at a 7-Zip style level (1–9).
    ///
    /// # Errors
    /// Any error from initialising the encoder.
    pub fn encoder<W: Write + Send>(
        self,
        out: W,
        level: u32,
        name: Option<&str>,
        mtime: Option<u32>,
    ) -> io::Result<StreamEncoder<W>> {
        let level = level.clamp(1, 9);
        Ok(match self {
            Self::Gzip => {
                let mut builder = flate2::GzBuilder::new().mtime(mtime.unwrap_or(0));
                if let Some(name) = name {
                    builder = builder.filename(name.as_bytes());
                }
                StreamEncoder::Gzip(builder.write(out, flate2::Compression::new(level)))
            }
            Self::Bzip2 => StreamEncoder::Bzip2(bzip2::write::BzEncoder::new(
                out,
                bzip2::Compression::new(level),
            )),
            Self::Xz => {
                StreamEncoder::Xz(Box::new(XzWriter::new(out, XzOptions::with_preset(level))?))
            }
            Self::Zstd => {
                let mut encoder = zstd::stream::write::Encoder::new(out, zstd_level(level))?;
                encoder.include_checksum(true)?;
                let threads =
                    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
                encoder.multithread(u32::try_from(threads).unwrap_or(1))?;
                StreamEncoder::Zstd(encoder)
            }
        })
    }
}

/// Maps 7-Zip levels 1–9 onto Zstandard levels (as 7-Zip ZS does roughly).
fn zstd_level(level: u32) -> i32 {
    match level {
        0 | 1 => 1,
        2 => 2,
        3 => 3,
        4 => 5,
        5 => 7,
        6 => 9,
        7 => 12,
        8 => 16,
        _ => 19,
    }
}

/// An active stream compressor.
pub enum StreamEncoder<W: Write> {
    /// gzip.
    Gzip(GzEncoder<W>),
    /// bzip2.
    Bzip2(bzip2::write::BzEncoder<W>),
    /// xz.
    Xz(Box<XzWriter<W>>),
    /// Zstandard.
    Zstd(zstd::stream::write::Encoder<'static, W>),
}

impl<W: Write> StreamEncoder<W> {
    /// Ends the stream and returns the sink.
    ///
    /// # Errors
    /// Any error from flushing the compressor.
    pub fn finish(self) -> io::Result<W> {
        match self {
            Self::Gzip(e) => e.finish(),
            Self::Bzip2(e) => e.finish(),
            Self::Xz(e) => e.finish(),
            Self::Zstd(e) => e.finish(),
        }
    }
}

impl<W: Write> Write for StreamEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Gzip(e) => e.write(buf),
            Self::Bzip2(e) => e.write(buf),
            Self::Xz(e) => e.write(buf),
            Self::Zstd(e) => e.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Gzip(e) => e.flush(),
            Self::Bzip2(e) => e.flush(),
            Self::Xz(e) => e.flush(),
            Self::Zstd(e) => e.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codec_round_trips() {
        let data: Vec<u8> = (0..200_000u32)
            .flat_map(|i| (i % 1000).to_le_bytes())
            .collect();
        for codec in [
            StreamCodec::Gzip,
            StreamCodec::Bzip2,
            StreamCodec::Xz,
            StreamCodec::Zstd,
        ] {
            let mut encoder = codec.encoder(Vec::new(), 5, Some("x"), Some(7)).unwrap();
            encoder.write_all(&data).unwrap();
            let packed = encoder.finish().unwrap();
            assert!(packed.len() < data.len() / 4, "{codec:?}");
            assert_eq!(crate::detect(&packed), Some(codec.format()), "{codec:?}");
            let mut out = Vec::new();
            codec
                .decoder(Box::new(packed.as_slice()))
                .unwrap()
                .read_to_end(&mut out)
                .unwrap();
            assert_eq!(out, data, "{codec:?}");
        }
    }

    #[test]
    fn concatenated_streams_decode_fully() {
        for codec in [
            StreamCodec::Gzip,
            StreamCodec::Bzip2,
            StreamCodec::Xz,
            StreamCodec::Zstd,
        ] {
            let mut joined = Vec::new();
            for part in [&b"first "[..], b"second"] {
                let mut encoder = codec.encoder(Vec::new(), 1, None, None).unwrap();
                encoder.write_all(part).unwrap();
                joined.extend(encoder.finish().unwrap());
            }
            let mut out = String::new();
            codec
                .decoder(Box::new(joined.as_slice()))
                .unwrap()
                .read_to_string(&mut out)
                .unwrap();
            assert_eq!(out, "first second", "{codec:?}");
        }
    }
}
