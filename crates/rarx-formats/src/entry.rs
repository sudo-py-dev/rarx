/// What an archive item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link; its data is the link target.
    Symlink,
}

/// Calendar date and time with no time zone (e.g. ZIP's MS-DOS timestamps, which
/// are in the creating machine's local time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilTime {
    /// Full year, e.g. 2026.
    pub year: i16,
    /// Month 1–12.
    pub month: i8,
    /// Day 1–31.
    pub day: i8,
    /// Hour 0–23.
    pub hour: i8,
    /// Minute 0–59.
    pub minute: i8,
    /// Second 0–59.
    pub second: i8,
}

/// A timestamp as stored by an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTime {
    /// Absolute time: seconds and nanoseconds since the Unix epoch, UTC.
    Unix {
        /// Whole seconds since 1970-01-01T00:00:00Z.
        seconds: i64,
        /// Sub-second part, 0–999 999 999.
        nanos: u32,
    },
    /// Wall-clock time in the archiver's (unknown) local time zone.
    Local(CivilTime),
}

impl FileTime {
    /// Seconds between 1601-01-01 (Windows `FILETIME` epoch) and 1970-01-01.
    const FILETIME_UNIX_DIFF: i64 = 11_644_473_600;

    /// Converts a Windows `FILETIME` (100 ns ticks since 1601-01-01 UTC).
    #[must_use]
    pub fn from_windows_ticks(ticks: u64) -> Self {
        let seconds = i64::try_from(ticks / 10_000_000).unwrap_or(i64::MAX);
        let nanos = u32::try_from(ticks % 10_000_000).unwrap_or(0) * 100;
        Self::Unix {
            seconds: seconds.saturating_sub(Self::FILETIME_UNIX_DIFF),
            nanos,
        }
    }

    /// Decodes an MS-DOS date/time pair, or `None` if it is not a valid date.
    #[must_use]
    pub fn from_dos(date: u16, time: u16) -> Option<Self> {
        let field = |value: u16, shift: u16, mask: u16| {
            i8::try_from((value >> shift) & mask).unwrap_or(i8::MAX)
        };
        let civil = CivilTime {
            year: i16::try_from(1980 + (date >> 9)).unwrap_or(i16::MAX),
            month: field(date, 5, 0x0F),
            day: field(date, 0, 0x1F),
            hour: field(time, 11, 0x1F),
            minute: field(time, 5, 0x3F),
            second: field(time, 0, 0x1F) * 2,
        };
        let valid = (1..=12).contains(&civil.month)
            && (1..=31).contains(&civil.day)
            && civil.hour < 24
            && civil.minute < 60
            && civil.second < 60;
        valid.then_some(Self::Local(civil))
    }
}

/// One item of an archive, as listed in its directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Path inside the archive, `/`-separated, without a trailing slash. Untrusted:
    /// sanitise before using it on disk.
    pub name: String,
    /// File, directory or link.
    pub kind: EntryKind,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Compressed size in bytes.
    pub packed_size: u64,
    /// Last modification time.
    pub modified: Option<FileTime>,
    /// Creation time.
    pub created: Option<FileTime>,
    /// Last access time.
    pub accessed: Option<FileTime>,
    /// CRC-32 of the uncompressed data, when the format stores one.
    pub crc32: Option<u32>,
    /// Human-readable compression method, e.g. `Deflate`.
    pub method: String,
    /// Whether the data is encrypted.
    pub encrypted: bool,
    /// Unix permission and type bits, when stored.
    pub unix_mode: Option<u32>,
    /// MS-DOS / Windows attribute bits, when stored.
    pub dos_attributes: Option<u32>,
    /// Per-item comment.
    pub comment: Option<String>,
}

impl Entry {
    /// Creates an entry with the given name and kind and every other field empty.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: EntryKind) -> Self {
        Self {
            name: name.into(),
            kind,
            size: 0,
            packed_size: 0,
            modified: None,
            created: None,
            accessed: None,
            crc32: None,
            method: String::new(),
            encrypted: false,
            unix_mode: None,
            dos_attributes: None,
            comment: None,
        }
    }

    /// Whether this entry is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_time_decodes() {
        // 2026-10-23 10:47:42
        let date: u16 = ((2026 - 1980) << 9) | (10 << 5) | 0x17;
        let time: u16 = (10 << 11) | (47 << 5) | 0x15;
        let expected = CivilTime {
            year: 2026,
            month: 10,
            day: 23,
            hour: 10,
            minute: 47,
            second: 42,
        };
        assert_eq!(
            FileTime::from_dos(date, time),
            Some(FileTime::Local(expected))
        );
    }

    #[test]
    fn invalid_dos_time_is_none() {
        assert_eq!(FileTime::from_dos(0, 0), None);
    }

    #[test]
    fn windows_ticks_convert_to_unix() {
        assert_eq!(
            FileTime::from_windows_ticks(116_444_736_000_000_000 + 15),
            FileTime::Unix {
                seconds: 0,
                nanos: 1500
            }
        );
    }
}
