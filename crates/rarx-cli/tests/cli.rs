#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use rarx_testkit::{TempDir, corpus_file};

const RARX: &str = env!("CARGO_BIN_EXE_rarx");
const RX7: &str = env!("CARGO_BIN_EXE_rx7");

fn zip(name: &str) -> PathBuf {
    corpus_file(format!("zip/{name}"))
}

fn run(bin: &str, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn code(output: &Output) -> Option<i32> {
    output.status.code()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn version_flags() {
    assert!(stdout(&run(RARX, &["--version"], "")).starts_with("rarx "));
    assert!(stdout(&run(RX7, &["--version"], "")).starts_with("rx7 "));
}

#[test]
fn unimplemented_commands_are_command_line_errors() {
    assert_eq!(code(&run(RARX, &["k", "x.rar"], "")), Some(7));
    assert_eq!(code(&run(RX7, &["rn", "x.7z"], "")), Some(7));
    assert_eq!(code(&run(RX7, &["x", "-bogus", "x.7z"], "")), Some(7));
}

/// A small folder to archive.
fn sample_tree(root: &Path) -> PathBuf {
    let src = root.join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), "alpha\n".repeat(50)).unwrap();
    std::fs::write(src.join("sub/b.log"), "log\n").unwrap();
    src
}

#[test]
fn rx7_adds_updates_and_deletes() {
    let tmp = TempDir::new("rx7-a");
    let src = sample_tree(tmp.path());
    let zip = tmp.path().join("out.zip");
    let added = run(RX7, &["a", arg(&zip), arg(&src), "-mx9", "-x!*.log"], "");
    assert_eq!(
        code(&added),
        Some(0),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let listing = stdout(&run(RX7, &["l", arg(&zip)], ""));
    assert!(listing.contains("src/a.txt") && !listing.contains("b.log"));
    assert_eq!(code(&run(RX7, &["t", arg(&zip)], "")), Some(0));

    assert_eq!(code(&run(RX7, &["u", arg(&zip), arg(&src)], "")), Some(0));
    assert!(stdout(&run(RX7, &["l", arg(&zip)], "")).contains("b.log"));
    assert_eq!(code(&run(RX7, &["d", arg(&zip), "*.log"], "")), Some(0));
    assert!(!stdout(&run(RX7, &["l", arg(&zip)], "")).contains("b.log"));

    assert_eq!(
        code(&run(
            RX7,
            &["a", "-t7z", arg(&tmp.path().join("x.7z")), arg(&src)],
            ""
        )),
        Some(2)
    );
    let missing = run(RX7, &["a", arg(&zip), arg(&tmp.path().join("nope"))], "");
    assert_eq!(code(&missing), Some(1));
}

#[test]
fn rarx_adds_moves_and_deletes() {
    let tmp = TempDir::new("rarx-a");
    let src = sample_tree(tmp.path());
    let zip = tmp.path().join("out.zip");
    let added = run(RARX, &["a", "-ep1", "-m5", arg(&zip), arg(&src)], "");
    assert_eq!(
        code(&added),
        Some(0),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(stdout(&added).contains("Adding      src/a.txt"));

    assert_eq!(
        code(&run(
            RARX,
            &["a", arg(&tmp.path().join("x.rar")), arg(&src)],
            ""
        )),
        Some(9)
    );
    assert_eq!(
        code(&run(
            RARX,
            &["a", arg(&zip), arg(&tmp.path().join("nope"))],
            ""
        )),
        Some(10)
    );
    assert_eq!(code(&run(RARX, &["d", arg(&zip), "*.none"], "")), Some(10));
    assert_eq!(code(&run(RARX, &["d", arg(&zip), "*.log"], "")), Some(0));

    let moved = tmp.path().join("moved.zip");
    assert_eq!(
        code(&run(
            RARX,
            &["m", "-ep1", arg(&moved), arg(&src.join("a.txt"))],
            ""
        )),
        Some(0)
    );
    assert!(!src.join("a.txt").exists());
    assert_eq!(stdout(&run(RARX, &["lb", arg(&moved)], "")).trim(), "a.txt");
}

#[test]
fn rx7_lists_and_extracts() {
    let listing = run(RX7, &["l", arg(&zip("infozip-basic.zip"))], "");
    assert_eq!(code(&listing), Some(0));
    assert!(stdout(&listing).contains("4 files, 2 folders"));

    let tmp = TempDir::new("rx7-x");
    let out = format!("-o{}", tmp.path().display());
    let result = run(RX7, &["x", arg(&zip("infozip-basic.zip")), &out, "-y"], "");
    assert_eq!(code(&result), Some(0));
    assert!(stdout(&result).contains("Everything is Ok"));
    assert!(tmp.path().join("dir/sub/empty.txt").is_file());
}

#[test]
fn rx7_technical_listing_and_stdout() {
    let listing = stdout(&run(
        RX7,
        &["l", "-slt", arg(&zip("infozip-basic.zip")), "a.txt"],
        "",
    ));
    assert!(listing.contains("Path = dir/a.txt"));
    assert!(listing.contains("Method = Deflate"));
    assert!(!listing.contains("Path = b.bin"));

    let data = run(
        RX7,
        &["e", "-so", arg(&zip("python-utf8.zip")), "日本語.txt"],
        "",
    );
    assert_eq!(stdout(&data), "japanese name\n");
}

#[test]
fn rx7_reports_crc_errors_as_fatal() {
    assert_eq!(
        code(&run(RX7, &["t", arg(&zip("corrupt-crc.zip"))], "")),
        Some(2)
    );
    assert_eq!(code(&run(RX7, &["t", "/nonexistent/a.zip"], "")), Some(2));
}

#[test]
fn rarx_exit_codes() {
    assert_eq!(
        code(&run(RARX, &["t", arg(&zip("corrupt-crc.zip"))], "")),
        Some(3)
    );
    assert_eq!(
        code(&run(RARX, &["t", arg(&zip("infozip-encrypted.zip"))], "")),
        Some(11)
    );
    assert_eq!(code(&run(RARX, &["t", "/nonexistent/a.zip"], "")), Some(6));
    assert_eq!(
        code(&run(
            RARX,
            &["t", arg(&zip("infozip-basic.zip")), "*.none"],
            ""
        )),
        Some(10)
    );
    assert_eq!(
        code(&run(RARX, &["t", arg(&zip("infozip-basic.zip"))], "")),
        Some(0)
    );
}

#[test]
fn rarx_extracts_to_trailing_slash_path_and_asks_before_overwrite() {
    let tmp = TempDir::new("rarx-x");
    let dest = format!("{}/", tmp.path().display());
    let archive = zip("infozip-basic.zip");
    assert_eq!(code(&run(RARX, &["x", arg(&archive), &dest], "")), Some(0));
    let a = tmp.path().join("dir/a.txt");
    std::fs::write(&a, "local").unwrap();

    // "n[E]ver" keeps every existing file.
    let kept = run(RARX, &["x", arg(&archive), &dest], "e\n");
    assert_eq!(code(&kept), Some(0));
    assert!(stdout(&kept).contains("Would you like to replace"));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "local");

    // -o+ overwrites without asking.
    assert_eq!(
        code(&run(RARX, &["x", "-o+", arg(&archive), &dest], "")),
        Some(0)
    );
    assert_ne!(std::fs::read_to_string(&a).unwrap(), "local");
}

#[test]
fn rarx_bare_listing_and_print() {
    let bare = stdout(&run(RARX, &["lb", arg(&zip("infozip-basic.zip"))], ""));
    assert_eq!(
        bare.lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with("RARX"))
            .count(),
        6
    );
    let printed = run(
        RARX,
        &["p", "-inul", arg(&zip("python-utf8.zip")), "*/naïve.txt"],
        "",
    );
    assert_eq!(stdout(&printed), "unicode folder\n");
}
