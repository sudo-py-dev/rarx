use crate::{Codec, CodecError, Progress};

/// The "Store" method: output equals input (7z method `00`, ZIP method 0).
#[derive(Clone, Copy, Debug, Default)]
pub struct Copy;

impl Codec for Copy {
    fn process(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, CodecError> {
        let len = input.len().min(output.len());
        output[..len].copy_from_slice(&input[..len]);
        Ok(Progress {
            consumed: len,
            produced: len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_up_to_smaller_buffer() {
        let mut out = [0u8; 3];
        let progress = Copy.process(b"hello", &mut out).unwrap();
        assert_eq!(
            progress,
            Progress {
                consumed: 3,
                produced: 3
            }
        );
        assert_eq!(&out, b"hel");
    }
}
