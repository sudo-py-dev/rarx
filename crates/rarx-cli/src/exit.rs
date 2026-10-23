//! Process exit codes, identical to the reference tools (AGENT.md §21).

use std::process::ExitCode;

/// Exit codes of WinRAR's console `rar`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Rar {
    /// Successful operation.
    Success = 0,
    /// Non-fatal warnings occurred.
    Warning = 1,
    /// A fatal error occurred.
    Fatal = 2,
    /// Invalid checksum; data is damaged.
    Crc = 3,
    /// Attempt to modify a locked archive.
    Locked = 4,
    /// Write error.
    Write = 5,
    /// File open error.
    Open = 6,
    /// Wrong command line option.
    UserError = 7,
    /// Not enough memory.
    Memory = 8,
    /// File create error.
    Create = 9,
    /// No files matching the mask and options were found.
    NoFiles = 10,
    /// Wrong password.
    BadPassword = 11,
    /// Read error.
    Read = 12,
    /// User stopped the process.
    UserBreak = 255,
}

/// Exit codes of 7-Zip's `7z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SevenZip {
    /// No error.
    Success = 0,
    /// Non-fatal warning.
    Warning = 1,
    /// Fatal error.
    Fatal = 2,
    /// Command line error.
    CommandLineError = 7,
    /// Not enough memory.
    Memory = 8,
    /// User stopped the process.
    UserBreak = 255,
}

impl From<Rar> for ExitCode {
    fn from(code: Rar) -> Self {
        ExitCode::from(code as u8)
    }
}

impl From<SevenZip> for ExitCode {
    fn from(code: SevenZip) -> Self {
        ExitCode::from(code as u8)
    }
}
