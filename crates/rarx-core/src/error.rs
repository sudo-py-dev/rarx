use std::{fmt, io};

/// Result alias for engine operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Engine-level failure. Front-ends map these to messages and exit codes.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An operating-system I/O call failed.
    Io(io::Error),
    /// The user or caller cancelled the job.
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Cancelled => f.write_str("operation cancelled"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Cancelled => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
