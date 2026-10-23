use crate::Checksum;

/// Reflected form of the ECMA-182 polynomial `0x42F0E1EBA9EA3693`.
const POLY: u64 = 0xC96C_5795_D787_0F42;

static TABLE: [u64; 256] = build_table();

const fn build_table() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u64;
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

/// CRC-64/XZ, used by xz and the 7-Zip hash calculator.
#[derive(Clone, Copy, Debug)]
pub struct Crc64 {
    state: u64,
}

impl Default for Crc64 {
    fn default() -> Self {
        Self { state: u64::MAX }
    }
}

impl Checksum for Crc64 {
    type Output = u64;

    fn update(&mut self, data: &[u8]) {
        let mut crc = self.state;
        for &byte in data {
            crc = TABLE[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8);
        }
        self.state = crc;
    }

    fn finalize(&self) -> u64 {
        !self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_value() {
        assert_eq!(Crc64::checksum(b"123456789"), 0x995D_C9BB_DF19_39FA);
    }

    #[test]
    fn reset_restores_initial_state() {
        let mut crc = Crc64::default();
        crc.update(b"garbage");
        crc.reset();
        crc.update(b"123456789");
        assert_eq!(crc.finalize(), 0x995D_C9BB_DF19_39FA);
    }
}
