//! Self-extracting archive stub (AGENT.md §12.4). Build with `--profile sfx`.

#![forbid(unsafe_code)]

fn main() {
    eprintln!(
        "rarx-sfx {}: SFX stubs arrive in milestone M6",
        env!("CARGO_PKG_VERSION")
    );
}
