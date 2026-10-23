//! `rarx`: WinRAR-style command line.

use std::process::ExitCode;

use rarx_cli::{Dialect, run};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(Dialect::Rar, &args)
}
