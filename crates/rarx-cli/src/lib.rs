//! Shared plumbing for the `rarx` (RAR-style) and `rx7` (7-Zip-style) binaries
//! (AGENT.md §13). Each binary keeps its reference tool's command syntax and exit codes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod exit;

use std::process::ExitCode;

/// Which command-line dialect a binary speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// WinRAR `rar` syntax and exit codes.
    Rar,
    /// 7-Zip `7z` syntax and exit codes.
    SevenZip,
}

impl Dialect {
    fn binary(self) -> &'static str {
        match self {
            Self::Rar => "rarx",
            Self::SevenZip => "rx7",
        }
    }

    fn usage(self) -> &'static str {
        match self {
            Self::Rar => "Usage: rarx <command> -<switch 1> ... <archive> [files...] [path\\]",
            Self::SevenZip => {
                "Usage: rx7 <command> [<switches>...] <archive_name> [<file_names>...]"
            }
        }
    }
}

/// Entry point shared by both binaries until the real parsers land in milestone M3.
#[must_use]
pub fn run(dialect: Dialect, args: &[String]) -> ExitCode {
    let name = dialect.binary();
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "-?") => {
            println!("{}", dialect.usage());
            ExitCode::SUCCESS
        }
        Some("--version" | "-iver") => {
            println!("{name} {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("{name}: command `{command}` is not implemented yet (milestone M3)");
            match dialect {
                Dialect::Rar => exit::Rar::UserError.into(),
                Dialect::SevenZip => exit::SevenZip::CommandLineError.into(),
            }
        }
    }
}
