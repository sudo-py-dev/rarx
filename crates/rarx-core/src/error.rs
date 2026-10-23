use std::path::PathBuf;
use std::{fmt, io};

use rarx_formats::FormatError;

/// Result alias for engine operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Engine-level failure. Front-ends map these to messages and exit codes.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An operating-system I/O call failed.
    Io(io::Error),
    /// The archive could not be parsed.
    Format(FormatError),
    /// The file is not an archive in any format RarX recognises.
    UnknownFormat(PathBuf),
    /// The operation needs a feature RarX does not implement yet.
    Unsupported(String),
    /// The user or caller cancelled the job.
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Format(err) => err.fmt(f),
            Self::UnknownFormat(path) => {
                write!(f, "cannot open the file as an archive: {}", path.display())
            }
            Self::Unsupported(what) => write!(f, "not supported yet: {what}"),
            Self::Cancelled => f.write_str("operation cancelled"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Format(err) => Some(err),
            Self::UnknownFormat(_) | Self::Unsupported(_) | Self::Cancelled => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<FormatError> for Error {
    fn from(err: FormatError) -> Self {
        Self::Format(err)
    }
}
