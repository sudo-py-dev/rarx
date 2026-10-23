/// Identity of a container or stream format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FormatId {
    /// 7z archive.
    SevenZip,
    /// RAR 5.0 and later.
    Rar5,
    /// RAR 1.5 – 4.x.
    Rar4,
    /// ZIP / ZIP64 (local header or empty-archive end record).
    Zip,
    /// gzip stream.
    Gzip,
    /// bzip2 stream.
    Bzip2,
    /// xz stream.
    Xz,
    /// Zstandard frame.
    Zstd,
    /// LZ4 frame.
    Lz4,
    /// POSIX/GNU TAR.
    Tar,
}

impl FormatId {
    /// Short type name as 7-Zip prints it (`7z`, `zip`, `rar`, …).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::SevenZip => "7z",
            Self::Rar5 => "rar5",
            Self::Rar4 => "rar",
            Self::Zip => "zip",
            Self::Gzip => "gzip",
            Self::Bzip2 => "bzip2",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
            Self::Lz4 => "lz4",
            Self::Tar => "tar",
        }
    }
}

/// Magic bytes found at a fixed offset from the start of a file.
#[derive(Clone, Copy, Debug)]
pub struct Signature {
    /// Format the magic identifies.
    pub format: FormatId,
    /// Offset of the magic from the start of the data.
    pub offset: usize,
    /// The magic itself.
    pub magic: &'static [u8],
}

/// Signatures checked by [`detect`], ordered so longer, more specific magics win
/// (RAR5 before RAR4, which share a 6-byte prefix).
const SIGNATURES: &[Signature] = &[
    sig(FormatId::SevenZip, b"7z\xBC\xAF\x27\x1C"),
    sig(FormatId::Rar5, b"Rar!\x1A\x07\x01\x00"),
    sig(FormatId::Rar4, b"Rar!\x1A\x07\x00"),
    sig(FormatId::Zip, b"PK\x03\x04"),
    sig(FormatId::Zip, b"PK\x05\x06"),
    sig(FormatId::Zip, b"PK\x07\x08"),
    sig(FormatId::Gzip, b"\x1F\x8B"),
    sig(FormatId::Bzip2, b"BZh"),
    sig(FormatId::Xz, b"\xFD7zXZ\x00"),
    sig(FormatId::Zstd, b"\x28\xB5\x2F\xFD"),
    sig(FormatId::Lz4, b"\x04\x22\x4D\x18"),
    Signature {
        format: FormatId::Tar,
        offset: 257,
        magic: b"ustar",
    },
];

const fn sig(format: FormatId, magic: &'static [u8]) -> Signature {
    Signature {
        format,
        offset: 0,
        magic,
    }
}

/// Identifies the format of `prefix` (the first bytes of a file) by magic bytes.
///
/// Returns `None` when no signature matches; callers then fall back to the file
/// extension and SFX/embedded-archive scanning (AGENT.md §4).
#[must_use]
pub fn detect(prefix: &[u8]) -> Option<FormatId> {
    SIGNATURES
        .iter()
        .find(|s| {
            prefix
                .get(s.offset..)
                .is_some_and(|rest| rest.starts_with(s.magic))
        })
        .map(|s| s.format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_formats() {
        let cases: &[(&[u8], FormatId)] = &[
            (b"7z\xBC\xAF\x27\x1C\x00\x04", FormatId::SevenZip),
            (b"Rar!\x1A\x07\x01\x00\x33", FormatId::Rar5),
            (b"Rar!\x1A\x07\x00\xCF", FormatId::Rar4),
            (b"PK\x03\x04\x14\x00", FormatId::Zip),
            (b"PK\x05\x06\x00\x00", FormatId::Zip),
            (b"\x1F\x8B\x08\x00", FormatId::Gzip),
            (b"BZh91AY&SY", FormatId::Bzip2),
            (b"\xFD7zXZ\x00\x00\x04", FormatId::Xz),
            (b"\x28\xB5\x2F\xFD\x24", FormatId::Zstd),
            (b"\x04\x22\x4D\x18\x64", FormatId::Lz4),
        ];
        for (bytes, expected) in cases {
            assert_eq!(detect(bytes), Some(*expected), "input {bytes:02X?}");
        }
    }

    #[test]
    fn unknown_and_short_inputs() {
        assert_eq!(detect(b""), None);
        assert_eq!(detect(b"Rar!"), None);
        assert_eq!(detect(b"hello world"), None);
    }
}
