//! Converting archive timestamps for display and for restoring on disk.

use std::time::SystemTime;

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use rarx_formats::FileTime;

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

#[cfg(test)]
mod tests {
    use rarx_formats::CivilTime;

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
}
