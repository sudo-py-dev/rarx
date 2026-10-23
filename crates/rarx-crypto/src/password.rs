use std::fmt;

use zeroize::Zeroizing;

/// A password, wiped from memory when dropped and never printed by `Debug`.
#[derive(Clone)]
pub struct Password(Zeroizing<String>);

impl Password {
    /// Wraps `text`.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }

    /// The password as UTF-8 bytes (ZIP AES, ZipCrypto with UTF-8 names).
    #[must_use]
    pub fn utf8(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// The password as UTF-16LE bytes (7z, RAR).
    #[must_use]
    pub fn utf16le(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(
            self.0
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>(),
        )
    }

    /// Whether the password is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for Password {
    fn from(text: &str) -> Self {
        Self::new(text.to_owned())
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_and_redaction() {
        let pw = Password::from("aé");
        assert_eq!(pw.utf8(), "aé".as_bytes());
        assert_eq!(pw.utf16le().as_slice(), [0x61, 0, 0xE9, 0]);
        assert_eq!(format!("{pw:?}"), "Password(***)");
    }
}
