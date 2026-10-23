//! 7z reading against hand-built (liblzma) and sevenz-rust2 fixtures (AGENT.md §22).

#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test
#![allow(clippy::format_collect)] // test fixtures mirror the generators' simple loops

use std::fs;
use std::io::Read;
use std::path::Path;

use rarx_core::select::Selection;
use rarx_core::{
    Archive, EntryKind, Error, ExtractOptions, FormatError, FormatId, ItemError, JobControl,
    OverwriteMode, Report, SilentHandler, extract, test,
};
use rarx_testkit::{TempDir, corpus_file};

fn open(name: &str) -> Archive {
    Archive::open(&corpus_file(format!("7z/{name}"))).unwrap()
}

fn test_all(name: &str) -> Report {
    test(
        &mut open(name),
        &Selection::all(),
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap()
}

fn extract_all(name: &str, dest: &Path) -> Report {
    let options = ExtractOptions {
        overwrite: OverwriteMode::Overwrite,
        ..ExtractOptions::default()
    };
    extract(
        &mut open(name),
        dest,
        &options,
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap()
}

/// Same text as `text()` in corpus/7z/generate.py.
fn generated_text(seed: &str, lines: usize) -> String {
    (0..lines)
        .map(|i| format!("{seed} line {i}: the quick brown fox jumps over the lazy dog\n"))
        .collect()
}

/// Same text as `tree()` in corpus/7z/sevenz-gen.rs.
fn s7r_text() -> String {
    (0..400)
        .map(|i| format!("line {i}: sevenz-rust2 cross-check text\n"))
        .collect()
}

fn read_item(archive: &mut Archive, name: &str) -> Vec<u8> {
    let index = archive
        .entries()
        .iter()
        .position(|e| e.name == name)
        .unwrap();
    let mut out = Vec::new();
    archive
        .read_entry(index)
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    out
}

#[test]
fn lists_solid_archive() {
    let archive = open("lzma2-solid.7z");
    assert_eq!(archive.format(), FormatId::SevenZip);
    let listing: Vec<_> = archive
        .entries()
        .iter()
        .map(|e| (e.name.as_str(), e.kind, e.size))
        .collect();
    assert_eq!(
        listing,
        [
            ("docs", EntryKind::Directory, 0),
            ("docs/a.txt", EntryKind::File, 17890),
            ("docs/b.txt", EntryKind::File, 6970),
            ("empty.txt", EntryKind::File, 0),
            ("naïve 日本.txt", EntryKind::File, 13),
            ("link", EntryKind::Symlink, 10),
        ]
    );
    assert_eq!(archive.entries()[1].method, "LZMA2:20");
    assert_eq!(archive.entries()[1].unix_mode, Some(0o100_644));
    assert!(archive.entries()[1].packed_size > 0);
    assert_eq!(
        archive.entries()[2].packed_size,
        0,
        "only the first item of a block carries its size"
    );
}

#[test]
fn extracts_solid_archive() {
    let tmp = TempDir::new("7z-solid");
    let report = extract_all("lzma2-solid.7z", tmp.path());
    assert_eq!(report.errors, []);
    assert_eq!(
        fs::read_to_string(tmp.path().join("docs/a.txt")).unwrap(),
        generated_text("alpha", 300)
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("docs/b.txt")).unwrap(),
        generated_text("beta", 120)
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("naïve 日本.txt")).unwrap(),
        "unicode name\n"
    );
    assert_eq!(fs::read(tmp.path().join("empty.txt")).unwrap(), b"");
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(tmp.path().join("link")).unwrap(),
        Path::new("docs/a.txt")
    );
}

#[test]
fn random_access_inside_a_solid_block() {
    let mut archive = open("lzma2-solid.7z");
    assert_eq!(
        read_item(&mut archive, "docs/b.txt"),
        generated_text("beta", 120).into_bytes()
    );
    // Going backwards restarts the decoder.
    assert_eq!(
        read_item(&mut archive, "docs/a.txt"),
        generated_text("alpha", 300).into_bytes()
    );

    // A partly read item must not shift the next one.
    let a = archive
        .entries()
        .iter()
        .position(|e| e.name == "docs/a.txt")
        .unwrap();
    let mut partial = [0u8; 10];
    archive
        .read_entry(a)
        .unwrap()
        .read_exact(&mut partial)
        .unwrap();
    assert_eq!(
        read_item(&mut archive, "docs/b.txt"),
        generated_text("beta", 120).into_bytes()
    );
}

#[test]
fn filters_and_encoded_headers() {
    for name in [
        "lzma-encoded-header.7z",
        "bcj-lzma2.7z",
        "delta-lzma.7z",
        "multi-folder.7z",
    ] {
        let report = test_all(name);
        assert_eq!(report.errors, [], "{name}");
        assert!(report.files > 0, "{name}");
    }
    let mut encoded = open("lzma-encoded-header.7z");
    assert_eq!(
        read_item(&mut encoded, "b.txt"),
        generated_text("beta", 120).into_bytes()
    );
    assert_eq!(open("bcj-lzma2.7z").entries()[0].method, "LZMA2:20 BCJ");
    assert_eq!(open("delta-lzma.7z").entries()[0].method, "LZMA:16 Delta:4");
}

#[test]
fn checksum_errors_are_per_item() {
    let report = test_all("corrupt-crc.7z");
    assert_eq!(
        report.errors,
        [("docs/a.txt".to_owned(), ItemError::Checksum)]
    );
    assert_eq!(
        (report.files, report.links),
        (3, 1),
        "items after the damaged one still verify"
    );
}

#[test]
fn encryption_is_detected() {
    let archive = open("encrypted.7z");
    assert!(archive.entries()[0].encrypted);
    assert_eq!(
        test_all("encrypted.7z").errors,
        [("secret.txt".to_owned(), ItemError::Encrypted)]
    );
    let header_encrypted = Archive::open(&corpus_file("7z/s7r-aes.7z"));
    assert!(matches!(
        header_encrypted,
        Err(Error::Format(FormatError::Encrypted))
    ));
}

#[test]
fn damaged_and_empty_archives() {
    let truncated = Archive::open(&corpus_file("7z/truncated.7z"));
    assert!(matches!(
        truncated,
        Err(Error::Format(FormatError::Corrupt(_)))
    ));
    assert_eq!(open("empty.7z").entries().len(), 0);
}

#[test]
fn sevenz_rust2_archives() {
    for name in [
        "s7r-solid.7z",
        "s7r-nonsolid.7z",
        "s7r-bcj-lzma2.7z",
        "s7r-deflate.7z",
    ] {
        let tmp = TempDir::new("7z-s7r");
        let report = extract_all(name, tmp.path());
        assert_eq!(report.errors, [], "{name}");
        assert_eq!(report.files, 4, "{name}");
        let deep = fs::read_to_string(tmp.path().join("docs/nested/deep.txt")).unwrap();
        assert_eq!(deep, s7r_text().repeat(3), "{name}");
        assert_eq!(
            fs::read(tmp.path().join("program.bin")).unwrap().len(),
            33000,
            "{name}"
        );
    }
}

#[test]
fn unsupported_methods_are_named() {
    let report = test_all("s7r-ppmd.7z");
    assert_ne!(report.errors.len(), 0);
    assert!(
        report
            .errors
            .iter()
            .all(|(_, e)| matches!(e, ItemError::Unsupported(m) if m.contains("PPMD")))
    );
}
