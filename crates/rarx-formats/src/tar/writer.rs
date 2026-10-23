//! TAR writer: POSIX ustar headers, with a pax extended header whenever a value
//! does not fit (long or non-ASCII names, sizes ≥ 8 GiB, times outside 1970–2242).

use std::io::{self, Read, Write};

use super::header::BLOCK;
use crate::{EntryKind, FileTime, NewItem};

/// GNU tar pads archives to 20 blocks.
const RECORD: u64 = 20 * BLOCK as u64;
const MAX_OCTAL_11: u64 = 0o777_7777_7777;

/// Writes a TAR archive to any sink.
pub struct TarWriter<W: Write> {
    out: W,
    written: u64,
}

impl<W: Write> TarWriter<W> {
    /// Starts an archive.
    pub fn new(out: W) -> Self {
        Self { out, written: 0 }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn pad(&mut self) -> io::Result<()> {
        let rest = self.written % BLOCK as u64;
        if rest != 0 {
            let zeros = vec![0u8; BLOCK - usize::try_from(rest).unwrap_or(0)];
            self.put(&zeros)?;
        }
        Ok(())
    }

    /// Adds a regular file of exactly `item.size_hint` bytes read from `data`.
    ///
    /// # Errors
    /// Any I/O error, or `InvalidData` if `data` is shorter than `size_hint`
    /// (the file changed while being archived).
    pub fn add_file(&mut self, item: &NewItem, data: &mut dyn Read) -> io::Result<()> {
        self.header(item, b'0', item.size_hint, "")?;
        let copied = io::copy(&mut data.take(item.size_hint), &mut self.out)?;
        self.written += copied;
        if copied != item.size_hint {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} shrank while being archived", item.name),
            ));
        }
        self.pad()
    }

    /// Adds a directory entry.
    ///
    /// # Errors
    /// Any I/O error.
    pub fn add_directory(&mut self, item: &NewItem) -> io::Result<()> {
        self.header(item, b'5', 0, "")
    }

    /// Adds a symbolic link pointing to `target`.
    ///
    /// # Errors
    /// Any I/O error.
    pub fn add_symlink(&mut self, item: &NewItem, target: &str) -> io::Result<()> {
        self.header(item, b'2', 0, target)
    }

    /// Writes the end-of-archive blocks and returns the sink.
    ///
    /// # Errors
    /// Any I/O error.
    pub fn finish(mut self) -> io::Result<W> {
        self.put(&[0u8; 2 * BLOCK])?;
        let rest = self.written % RECORD;
        if rest != 0 {
            let zeros = vec![0u8; usize::try_from(RECORD - rest).unwrap_or(0)];
            self.put(&zeros)?;
        }
        self.out.flush()?;
        Ok(self.out)
    }

    /// Writes the header (preceded by a pax header when needed).
    fn header(&mut self, item: &NewItem, typeflag: u8, size: u64, link: &str) -> io::Result<()> {
        let mut name = item.name.trim_matches('/').to_owned();
        if item.kind == EntryKind::Directory {
            name.push('/');
        }
        let (seconds, nanos) = match item.modified {
            Some(FileTime::Unix { seconds, nanos }) => (seconds, nanos),
            _ => (0, 0),
        };
        let split = split_ustar_name(&name);
        let mut pax = Vec::new();
        if split.is_none() {
            pax_record(&mut pax, "path", &name);
        }
        if link.len() > 100 || !link.is_ascii() {
            pax_record(&mut pax, "linkpath", link);
        }
        if size > MAX_OCTAL_11 {
            pax_record(&mut pax, "size", &size.to_string());
        }
        let mtime_fits = u64::try_from(seconds).is_ok_and(|s| s <= MAX_OCTAL_11);
        if !mtime_fits || nanos != 0 {
            let frac = if nanos == 0 {
                String::new()
            } else {
                format!(".{nanos:09}")
            };
            pax_record(
                &mut pax,
                "mtime",
                &format!("{seconds}{}", frac.trim_end_matches('0')),
            );
        }

        if !pax.is_empty() {
            let base = name
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("item");
            let pax_name: String = format!("PaxHeaders/{base}")
                .chars()
                .filter(char::is_ascii)
                .take(99)
                .collect();
            let block = ustar_block(
                &pax_name,
                "",
                0o644,
                pax.len() as u64,
                seconds.max(0),
                b'x',
                "",
            );
            self.put(&block)?;
            self.put(&pax)?;
            self.pad()?;
        }
        let (prefix, short) = split.unwrap_or_else(|| (String::new(), ascii_tail(&name, 100)));
        let mode = item
            .unix_mode
            .unwrap_or(if item.kind == EntryKind::Directory {
                0o755
            } else {
                0o644
            });
        let link = if link.len() <= 100 && link.is_ascii() {
            link.to_owned()
        } else {
            ascii_tail(link, 100)
        };
        let block = ustar_block(
            &short,
            &prefix,
            mode & 0o7777,
            size.min(MAX_OCTAL_11),
            seconds.clamp(0, i64::try_from(MAX_OCTAL_11).unwrap_or(i64::MAX)),
            typeflag,
            &link,
        );
        self.put(&block)
    }
}

/// Splits a name into ustar `(prefix, name)` fields, or `None` if it does not fit
/// or is not ASCII.
fn split_ustar_name(name: &str) -> Option<(String, String)> {
    if !name.is_ascii() {
        return None;
    }
    if name.len() <= 100 {
        return Some((String::new(), name.to_owned()));
    }
    // The prefix must end at a '/' and hold at most 155 bytes; the rest at most 100.
    let body = name.trim_end_matches('/');
    name.char_indices()
        .filter(|&(i, c)| c == '/' && i <= 155 && i < body.len())
        .map(|(i, _)| i)
        .find(|&i| name.len() - i - 1 <= 100)
        .map(|i| (name[..i].to_owned(), name[i + 1..].to_owned()))
}

/// Last `max` bytes of `text` restricted to ASCII (fallback field for pax readers).
fn ascii_tail(text: &str, max: usize) -> String {
    let ascii: String = text
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    ascii[ascii.len().saturating_sub(max)..].to_owned()
}

fn pax_record(buf: &mut Vec<u8>, key: &str, value: &str) {
    // The length prefix counts itself, so iterate until it is stable.
    let body = format!(" {key}={value}\n");
    let mut len = body.len() + 1;
    while len.to_string().len() + body.len() != len {
        len = len.to_string().len() + body.len();
    }
    buf.extend_from_slice(format!("{len}{body}").as_bytes());
}

fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&text.as_bytes()[text.len() - digits..]);
    field[digits] = 0;
}

fn ustar_block(
    name: &str,
    prefix: &str,
    mode: u32,
    size: u64,
    mtime: i64,
    typeflag: u8,
    link: &str,
) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    block[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
    octal(&mut block[100..108], u64::from(mode));
    octal(&mut block[108..116], 0);
    octal(&mut block[116..124], 0);
    octal(&mut block[124..136], size);
    octal(&mut block[136..148], u64::try_from(mtime).unwrap_or(0));
    block[156] = typeflag;
    block[157..157 + link.len().min(100)].copy_from_slice(&link.as_bytes()[..link.len().min(100)]);
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    octal(&mut block[329..337], 0);
    octal(&mut block[337..345], 0);
    block[345..345 + prefix.len().min(155)]
        .copy_from_slice(&prefix.as_bytes()[..prefix.len().min(155)]);
    block[148..156].fill(b' ');
    let sum: u64 = block.iter().map(|&b| u64::from(b)).sum();
    octal(&mut block[148..155], sum);
    block[155] = b' ';
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pax_lengths_count_themselves() {
        let mut buf = Vec::new();
        pax_record(&mut buf, "path", "a");
        assert_eq!(buf, b"9 path=a\n");
        let mut long = Vec::new();
        pax_record(&mut long, "path", &"x".repeat(95));
        assert_eq!(long.len(), 105);
        assert!(long.starts_with(b"105 path="));
    }

    #[test]
    fn ustar_name_splitting() {
        assert_eq!(
            split_ustar_name("short"),
            Some((String::new(), "short".to_owned()))
        );
        let long = format!("{}/{}", "d".repeat(120), "f".repeat(90));
        assert_eq!(
            split_ustar_name(&long),
            Some(("d".repeat(120), "f".repeat(90)))
        );
        assert_eq!(split_ustar_name(&"x".repeat(150)), None);
        assert_eq!(split_ustar_name("café"), None);
    }
}
