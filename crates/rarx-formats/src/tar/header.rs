//! TAR header blocks (POSIX ustar, GNU, v7) and pax extended records.

use std::collections::HashMap;

pub(super) const BLOCK: usize = 512;

/// One parsed 512-byte header.
#[derive(Clone, Debug, Default)]
pub(super) struct Header {
    pub name: Vec<u8>,
    pub mode: u64,
    pub size: u64,
    pub mtime: i64,
    pub typeflag: u8,
    pub linkname: Vec<u8>,
}

/// Result of reading one header block.
pub(super) enum Block {
    /// An all-zero block: end of archive.
    End,
    /// A valid header.
    Header(Header),
    /// The checksum does not match: not TAR, or damaged.
    Invalid,
}

/// Field bytes up to the first NUL.
fn cstr(field: &[u8]) -> &[u8] {
    field
        .iter()
        .position(|&b| b == 0)
        .map_or(field, |end| &field[..end])
}

/// Parses an octal or GNU base-256 number field. Malformed fields read as `None`.
fn number(field: &[u8]) -> Option<u64> {
    if let Some((&first, rest)) = field.split_first()
        && first & 0x80 != 0
    {
        if first & 0x40 != 0 {
            return None; // negative base-256 values are not meaningful here
        }
        let mut value = u64::from(first & 0x3F);
        for &b in rest {
            value = value.checked_mul(256)?.checked_add(u64::from(b))?;
        }
        return Some(value);
    }
    let text = cstr(field);
    let digits = text
        .iter()
        .copied()
        .skip_while(|b| *b == b' ')
        .take_while(|b| (b'0'..=b'7').contains(b));
    let mut value = 0u64;
    let mut any = false;
    for b in digits {
        value = value.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
        any = true;
    }
    if !any && text.iter().any(|b| !matches!(b, b' ' | 0)) {
        return None;
    }
    Some(value)
}

/// Parses one header block.
pub(super) fn parse(block: &[u8; BLOCK]) -> Block {
    if block.iter().all(|&b| b == 0) {
        return Block::End;
    }
    let Some(stored) = number(&block[148..156]) else {
        return Block::Invalid;
    };
    let unsigned: u64 = block
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                u64::from(b)
            }
        })
        .sum();
    let signed: i64 = block
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                i64::from(b.cast_signed())
            }
        })
        .sum();
    if stored != unsigned && i64::try_from(stored).ok() != Some(signed) {
        return Block::Invalid;
    }

    let magic = &block[257..265];
    let is_ustar = magic == b"ustar\x0000";
    let mut name = cstr(&block[0..100]).to_vec();
    if is_ustar {
        let prefix = cstr(&block[345..500]);
        if !prefix.is_empty() {
            let mut full = prefix.to_vec();
            full.push(b'/');
            full.extend_from_slice(&name);
            name = full;
        }
    }
    let mtime = number(&block[136..148])
        .and_then(|t| i64::try_from(t).ok())
        .unwrap_or(0);
    Block::Header(Header {
        name,
        mode: number(&block[100..108]).unwrap_or(0),
        size: number(&block[124..136]).unwrap_or(0),
        mtime,
        typeflag: block[156],
        linkname: cstr(&block[157..257]).to_vec(),
    })
}

/// Parses pax extended header records (`"<len> <key>=<value>\n"`). Malformed
/// trailing data is ignored.
pub(super) fn parse_pax(mut data: &[u8], into: &mut HashMap<String, Vec<u8>>) {
    while !data.is_empty() {
        let Some(space) = data.iter().position(|&b| b == b' ') else {
            return;
        };
        let Some(len) = std::str::from_utf8(&data[..space])
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
        else {
            return;
        };
        if len <= space + 1 || len > data.len() {
            return;
        }
        let record = &data[space + 1..len];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(eq) = record.iter().position(|&b| b == b'=') {
            let key = String::from_utf8_lossy(&record[..eq]).into_owned();
            into.insert(key, record[eq + 1..].to_vec());
        }
        data = &data[len..];
    }
}

/// Parses a pax time value such as `1700000000.123456789`.
pub(super) fn pax_time(value: &[u8]) -> Option<(i64, u32)> {
    let text = std::str::from_utf8(value).ok()?;
    let (secs, frac) = text.split_once('.').unwrap_or((text, ""));
    let secs = secs.parse::<i64>().ok()?;
    let digits: String = frac.chars().take(9).collect();
    let nanos = if digits.is_empty() {
        0
    } else {
        format!("{digits:0<9}").parse::<u32>().ok()?
    };
    Some((secs, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number(b"0000644\0"), Some(0o644));
        assert_eq!(number(b"   755 \0"), Some(0o755));
        assert_eq!(number(b"\0\0\0\0"), Some(0));
        assert_eq!(
            number(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0x02, 0, 0]),
            Some(0x0002_0000)
        );
        assert_eq!(number(b"12x"), Some(0o12));
        assert_eq!(number(b"zz"), None);
    }

    #[test]
    fn pax_records() {
        let mut map = HashMap::new();
        parse_pax(
            b"30 mtime=1700000000.123456789\n19 path=dir/f\xc3\xa9.txt\n",
            &mut map,
        );
        assert_eq!(map["path"], "dir/fé.txt".as_bytes());
        assert_eq!(pax_time(&map["mtime"]), Some((1_700_000_000, 123_456_789)));
        assert_eq!(pax_time(b"5.5"), Some((5, 500_000_000)));
    }

    #[test]
    fn zero_block_and_bad_checksum() {
        assert!(matches!(parse(&[0; BLOCK]), Block::End));
        let mut block = [0u8; BLOCK];
        block[0] = b'a';
        block[148..156].copy_from_slice(b"0000000\0");
        assert!(matches!(parse(&block), Block::Invalid));
    }
}
