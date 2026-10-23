//! `rx7`: 7-Zip-style command line.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    rarx_cli::sevenzip::run(&args)
}
