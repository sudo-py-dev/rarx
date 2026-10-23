//! Converting archive timestamps for display and for restoring on disk.

use std::time::SystemTime;

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use rarx_formats::{CivilTime, FileTime};

fn to_timestamp(time: FileTime) -> Option<Timestamp> {
    match time {
        FileTime::Unix { seconds, nanos } => {
            Timestamp::new(seconds, i32::try_from(nanos).ok()?).ok()
        }
        FileTime::Local(c) => DateTime::new(c.year, c.month, c.day, c.hour, c.minute, c.second, 0)
            .ok()?
            .to_zoned(TimeZone::system())
            .ok()
            .map(|zoned| zoned.timestamp()),
    }
}

/// Converts an archive timestamp to a [`SystemTime`], interpreting zone-less times
/// in the local time zone (as WinRAR and 7-Zip do).
#[must_use]
pub fn to_system_time(time: FileTime) -> Option<SystemTime> {
    to_timestamp(time).map(SystemTime::from)
}

/// Formats an archive timestamp in local time using a `strftime` pattern.
#[must_use]
pub fn format_local(time: FileTime, pattern: &str) -> Option<String> {
    let zoned = to_timestamp(time)?.to_zoned(TimeZone::system());
    Some(zoned.strftime(pattern).to_string())
}

/// Converts a file-system time to an absolute archive timestamp.
#[must_use]
pub fn from_system_time(time: SystemTime) -> Option<FileTime> {
    let ts = Timestamp::try_from(time).ok()?;
    Some(FileTime::Unix {
        seconds: ts.as_second(),
        nanos: u32::try_from(ts.subsec_nanosecond()).ok()?,
    })
}

/// Local wall-clock fields of a file-system time, for formats that store local time.
#[must_use]
pub fn to_local_civil(time: SystemTime) -> Option<CivilTime> {
    let dt = Timestamp::try_from(time)
        .ok()?
        .to_zoned(TimeZone::system())
        .datetime();
    Some(CivilTime {
        year: dt.year(),
        month: dt.month(),
        day: dt.day(),
        hour: dt.hour(),
        minute: dt.minute(),
        second: dt.second(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_time_round_trips() {
        let t = to_system_time(FileTime::Unix {
            seconds: 1_000,
            nanos: 5,
        })
        .unwrap();
        let since = t.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!((since.as_secs(), since.subsec_nanos()), (1_000, 5));
    }

    #[test]
    fn local_time_formats_back_unchanged() {
        let civil = CivilTime {
            year: 2026,
            month: 10,
            day: 23,
            hour: 10,
            minute: 47,
            second: 42,
        };
        let text = format_local(FileTime::Local(civil), "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(text, "2026-10-23 10:47:42");
    }

    #[test]
    fn system_time_conversions_agree() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::new(1_767_323_045, 7);
        assert_eq!(
            from_system_time(now),
            Some(FileTime::Unix {
                seconds: 1_767_323_045,
                nanos: 7
            })
        );
        let civil = to_local_civil(now).unwrap();
        assert_eq!(
            to_system_time(FileTime::Local(civil)),
            Some(now - std::time::Duration::new(0, 7))
        );
    }
}
