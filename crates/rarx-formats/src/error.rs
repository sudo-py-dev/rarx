use std::{fmt, io};

use rarx_io::ParseError;

/// Failure while opening or reading an archive.
#[derive(Debug)]
#[non_exhaustive]
pub enum FormatError {
    /// The underlying file could not be read.
    Io(io::Error),
    /// A header ended early or held an out-of-range value.
    Parse(ParseError),
    /// The archive structure is invalid.
    Corrupt(String),
    /// The archive uses a feature RarX does not implement yet.
    Unsupported(String),
    /// The item is encrypted and no usable password was supplied.
    Encrypted,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "read error: {err}"),
            Self::Parse(err) => write!(f, "headers error: {err}"),
            Self::Corrupt(what) => write!(f, "corrupt archive: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Encrypted => f.write_str("encrypted data needs a password"),
        }
    }
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Parse(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for FormatError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<ParseError> for FormatError {
    fn from(err: ParseError) -> Self {
        Self::Parse(err)
    }
}

/// Integrity failure detected while streaming an item's data. Carried inside an
/// [`io::Error`] of kind [`io::ErrorKind::InvalidData`]; use [`DataError::find`] to
/// recover it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataError {
    /// The data's checksum does not match the stored one.
    ChecksumMismatch,
    /// The decoded data is shorter or longer than the stored size.
    SizeMismatch,
}

impl DataError {
    /// Extracts a [`DataError`] from an I/O error produced by an entry stream.
    #[must_use]
    pub fn find(err: &io::Error) -> Option<Self> {
        err.get_ref()?.downcast_ref::<Self>().copied()
    }
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ChecksumMismatch => "CRC failed",
            Self::SizeMismatch => "unpacked size does not match header",
        })
    }
}

impl std::error::Error for DataError {}

impl From<DataError> for io::Error {
    fn from(err: DataError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, err)
    }
}
