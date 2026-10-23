use crate::Checksum;

/// Reflected form of the IEEE 802.3 polynomial `0x04C11DB7`.
const POLY: u32 = 0xEDB8_8320;

static TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ POLY
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// CRC-32/ISO-HDLC, the checksum used by 7z, ZIP, RAR, gzip and xz.
#[derive(Clone, Copy, Debug)]
pub struct Crc32 {
    state: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self { state: u32::MAX }
    }
}

impl Checksum for Crc32 {
    type Output = u32;

    fn update(&mut self, data: &[u8]) {
        let mut crc = self.state;
        for &byte in data {
            crc = TABLE[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8);
        }
        self.state = crc;
    }

    fn finalize(&self) -> u32 {
        !self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_value() {
        assert_eq!(Crc32::checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_input() {
        assert_eq!(Crc32::checksum(b""), 0);
    }

    #[test]
    fn streaming_matches_one_shot() {
        let mut crc = Crc32::default();
        crc.update(b"1234");
        crc.update(b"56789");
        assert_eq!(crc.finalize(), Crc32::checksum(b"123456789"));
    }
}
