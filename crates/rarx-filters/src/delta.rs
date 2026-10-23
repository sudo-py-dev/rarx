use crate::Filter;

/// Byte-wise delta filter with a distance of 1–256 (7z method `03`, xz filter `0x03`).
#[derive(Clone, Debug)]
pub struct Delta {
    distance: usize,
    /// The last `distance` bytes seen, as a ring buffer indexed by `pos`.
    history: [u8; 256],
    pos: usize,
}

impl Delta {
    /// Creates a delta filter, or `None` if `distance` is outside 1–256.
    #[must_use]
    pub fn new(distance: usize) -> Option<Self> {
        (1..=256).contains(&distance).then_some(Self {
            distance,
            history: [0; 256],
            pos: 0,
        })
    }

    fn advance(&mut self) {
        self.pos = (self.pos + 1) % self.distance;
    }
}

impl Filter for Delta {
    fn encode(&mut self, buf: &mut [u8]) {
        for byte in buf {
            let original = *byte;
            *byte = original.wrapping_sub(self.history[self.pos]);
            self.history[self.pos] = original;
            self.advance();
        }
    }

    fn decode(&mut self, buf: &mut [u8]) {
        for byte in buf {
            *byte = byte.wrapping_add(self.history[self.pos]);
            self.history[self.pos] = *byte;
            self.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_distance() {
        assert!(Delta::new(0).is_none());
        assert!(Delta::new(257).is_none());
        assert!(Delta::new(256).is_some());
    }

    #[test]
    fn encodes_distance_two() {
        let mut buf = [10, 20, 13, 24, 16, 28];
        Delta::new(2).unwrap().encode(&mut buf);
        assert_eq!(buf, [10, 20, 3, 4, 3, 4]);
    }

    #[test]
    fn round_trips_across_split_buffers() {
        let original: [u8; 64] = core::array::from_fn(|i| u8::try_from(i * 37 % 251).unwrap());
        let mut data = original;
        let mut enc = Delta::new(3).unwrap();
        let (a, b) = data.split_at_mut(17);
        enc.encode(a);
        enc.encode(b);
        let mut dec = Delta::new(3).unwrap();
        let (a, b) = data.split_at_mut(40);
        dec.decode(a);
        dec.decode(b);
        assert_eq!(data, original);
    }
}
