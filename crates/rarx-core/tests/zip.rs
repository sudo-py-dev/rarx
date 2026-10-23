//! End-to-end ZIP extraction against the committed corpus (AGENT.md §22).

#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test

use std::fs;
use std::path::Path;

use rarx_core::select::Selection;
use rarx_core::{
    Archive, EntryKind, ExtractOptions, FormatId, ItemError, JobControl, OverwriteMode, PathMode,
    Report, SilentHandler, extract, test,
};
use rarx_testkit::{TempDir, corpus_file};

fn open(name: &str) -> Archive {
    Archive::open(&corpus_file(format!("zip/{name}"))).unwrap()
}

fn extract_with(name: &str, dest: &Path, options: &ExtractOptions) -> Report {
    let mut archive = open(name);
    extract(
        &mut archive,
        dest,
        options,
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap()
}

fn overwrite_all() -> ExtractOptions {
    ExtractOptions {
        overwrite: OverwriteMode::Overwrite,
        ..ExtractOptions::default()
    }
}

fn errors(report: &Report) -> Vec<(&str, &ItemError)> {
    report.errors.iter().map(|(n, e)| (n.as_str(), e)).collect()
}

#[test]
fn lists_infozip_archive() {
    let archive = open("infozip-basic.zip");
    assert_eq!(archive.format(), FormatId::Zip);
    let listing: Vec<_> = archive
        .entries()
        .iter()
        .map(|e| (e.name.as_str(), e.kind, e.size))
        .collect();
    assert_eq!(
        listing,
        [
            ("dir", EntryKind::Directory, 0),
            ("dir/a.txt", EntryKind::File, 1300),
            ("dir/sub", EntryKind::Directory, 0),
            ("dir/sub/empty.txt", EntryKind::File, 0),
            ("b.bin", EntryKind::File, 1000),
            ("link", EntryKind::Symlink, 9),
        ]
    );
    assert_eq!(archive.entries()[1].method, "Deflate");
    assert_eq!(archive.entries()[4].method, "Store");
}

#[test]
fn extracts_infozip_archive_with_metadata() {
    let tmp = TempDir::new("basic");
    let report = extract_with("infozip-basic.zip", tmp.path(), &overwrite_all());
    assert_eq!(errors(&report), []);
    assert_eq!((report.files, report.folders, report.links), (3, 2, 1));

    let a = tmp.path().join("dir/a.txt");
    assert_eq!(
        fs::read_to_string(&a).unwrap(),
        "hello from a\n".repeat(100)
    );
    assert_eq!(fs::read(tmp.path().join("dir/sub/empty.txt")).unwrap(), b"");
    let bin: Vec<u8> = (0..1000u32)
        .map(|i| u8::try_from(i * 7 % 256).unwrap())
        .collect();
    assert_eq!(fs::read(tmp.path().join("b.bin")).unwrap(), bin);

    // Info-ZIP's extended-timestamp field holds the exact UTC mtime.
    let mtime = fs::metadata(&a).unwrap().modified().unwrap();
    let secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(secs, 1_767_323_045);

    #[cfg(unix)]
    {
        let link = tmp.path().join("link");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("dir/a.txt"));
        assert_eq!(
            fs::read_to_string(link).unwrap(),
            "hello from a\n".repeat(100)
        );
    }
}

#[test]
fn tests_archive_integrity() {
    let mut archive = open("infozip-basic.zip");
    let report = test(
        &mut archive,
        &Selection::all(),
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(errors(&report), []);
    assert_eq!(report.bytes, 1300 + 1000 + 9);
}

#[test]
fn reads_utf8_names_and_zip64() {
    let tmp = TempDir::new("utf8");
    let report = extract_with("python-utf8.zip", tmp.path(), &overwrite_all());
    assert_eq!(errors(&report), []);
    assert_eq!(
        fs::read_to_string(tmp.path().join("café/naïve.txt")).unwrap(),
        "unicode folder\n"
    );
    assert!(tmp.path().join("日本語.txt").is_file());

    let report = extract_with("zip64-stored.zip", tmp.path(), &overwrite_all());
    assert_eq!(errors(&report), []);
    assert_eq!(
        fs::read_to_string(tmp.path().join("big/one.txt")).unwrap(),
        "first zip64 file\n"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("two.txt")).unwrap(),
        "second\n".repeat(3)
    );
}

#[test]
fn opens_sfx_with_prefixed_stub() {
    let archive = open("sfx-prefixed.exe");
    assert_eq!(archive.entries().len(), 6);
    let tmp = TempDir::new("sfx");
    let report = extract_with("sfx-prefixed.exe", tmp.path(), &overwrite_all());
    assert_eq!(errors(&report), []);
    assert!(tmp.path().join("dir/a.txt").is_file());
}

#[test]
fn traversal_names_stay_inside_destination() {
    let outer = TempDir::new("traversal");
    let dest = outer.path().join("dest");
    let report = extract_with("evil-traversal.zip", &dest, &overwrite_all());
    assert_eq!(errors(&report), []);
    for inside in ["evil.txt", "abs.txt", "a/up.txt", "win.txt", "ok.txt"] {
        assert!(dest.join(inside).is_file(), "{inside} missing");
    }
    let leaked: Vec<_> = fs::read_dir(outer.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leaked, ["dest"]);
}

#[cfg(unix)]
#[test]
fn dangerous_symlinks_are_refused() {
    let outer = TempDir::new("symlink");
    let dest = outer.path().join("dest");
    let report = extract_with("evil-symlink.zip", &dest, &overwrite_all());
    let mut failed: Vec<_> = report
        .errors
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    failed.sort_unstable();
    assert_eq!(failed, ["abs", "esc", "up"]);
    assert!(
        report
            .errors
            .iter()
            .all(|(_, e)| matches!(e, ItemError::UnsafeLink(_)))
    );
    assert_eq!(fs::read_link(dest.join("dot")).unwrap(), Path::new("."));
    assert_eq!(
        fs::read_to_string(dest.join("good")).unwrap(),
        "real file\n"
    );
}

#[test]
fn corrupt_data_is_reported_and_removed() {
    let mut archive = open("corrupt-crc.zip");
    let report = test(
        &mut archive,
        &Selection::all(),
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(errors(&report), [("broken.txt", &ItemError::Checksum)]);
    assert_eq!(report.files, 1);

    let tmp = TempDir::new("corrupt");
    let report = extract_with("corrupt-crc.zip", tmp.path(), &overwrite_all());
    assert_eq!(errors(&report), [("broken.txt", &ItemError::Checksum)]);
    assert!(!tmp.path().join("broken.txt").exists());
    assert!(tmp.path().join("fine.txt").is_file());

    let keep = ExtractOptions {
        keep_broken: true,
        ..overwrite_all()
    };
    extract_with("corrupt-crc.zip", tmp.path(), &keep);
    assert!(tmp.path().join("broken.txt").is_file());
}

#[test]
fn encrypted_items_are_reported() {
    let tmp = TempDir::new("encrypted");
    let report = extract_with("infozip-encrypted.zip", tmp.path(), &overwrite_all());
    assert_eq!(errors(&report), [("dir/a.txt", &ItemError::Encrypted)]);
    assert!(!tmp.path().join("dir/a.txt").exists());
}

#[test]
fn overwrite_modes() {
    let tmp = TempDir::new("overwrite");
    let only_a = |overwrite| ExtractOptions {
        overwrite,
        selection: Selection::all().include("a.txt"),
        ..ExtractOptions::default()
    };
    let a = tmp.path().join("dir/a.txt");
    extract_with(
        "infozip-basic.zip",
        tmp.path(),
        &only_a(OverwriteMode::Skip),
    );
    fs::write(&a, "local edit").unwrap();

    let report = extract_with(
        "infozip-basic.zip",
        tmp.path(),
        &only_a(OverwriteMode::Skip),
    );
    assert_eq!(report.skipped, 1);
    assert_eq!(fs::read_to_string(&a).unwrap(), "local edit");

    extract_with(
        "infozip-basic.zip",
        tmp.path(),
        &only_a(OverwriteMode::RenameNew),
    );
    assert_eq!(fs::read_to_string(&a).unwrap(), "local edit");
    assert!(tmp.path().join("dir/a_1.txt").is_file());

    extract_with(
        "infozip-basic.zip",
        tmp.path(),
        &only_a(OverwriteMode::RenameExisting),
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("dir/a_2.txt")).unwrap(),
        "local edit"
    );
    assert_ne!(fs::read_to_string(&a).unwrap(), "local edit");

    fs::write(&a, "again").unwrap();
    extract_with(
        "infozip-basic.zip",
        tmp.path(),
        &only_a(OverwriteMode::Overwrite),
    );
    assert_ne!(fs::read_to_string(&a).unwrap(), "again");
}

#[test]
fn extract_without_paths_and_with_selection() {
    let tmp = TempDir::new("flat");
    let options = ExtractOptions {
        paths: PathMode::None,
        selection: Selection::all().include("*.txt"),
        ..overwrite_all()
    };
    let report = extract_with("infozip-basic.zip", tmp.path(), &options);
    assert_eq!(errors(&report), []);
    let mut names: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["a.txt", "empty.txt"]);
}

#[test]
fn non_archive_is_unknown_format() {
    let tmp = TempDir::new("unknown");
    let path = tmp.path().join("notes.txt");
    fs::write(&path, "plain text").unwrap();
    assert!(matches!(
        Archive::open(&path),
        Err(rarx_core::Error::UnknownFormat(_))
    ));
    let exe = tmp.path().join("program.exe");
    fs::write(&exe, b"MZ not an sfx at all, just bytes........").unwrap();
    assert!(matches!(
        Archive::open(&exe),
        Err(rarx_core::Error::UnknownFormat(_))
    ));
}
