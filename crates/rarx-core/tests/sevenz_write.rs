//! 7z creation and update round trips (AGENT.md §22).

#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rarx_core::select::Selection;
use rarx_core::{
    AddOptions, Archive, EntryKind, ExtractOptions, FormatId, JobControl, OverwriteMode,
    SilentHandler, SilentUpdate, UpdateMode, UpdateReport, add, delete, extract,
};
use rarx_testkit::TempDir;

const MTIME: u64 = 1_700_000_123;

fn make_tree(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(src.join("docs/empty-dir")).unwrap();
    fs::write(src.join("docs/a.txt"), "seven zip\n".repeat(800)).unwrap();
    fs::write(src.join("docs/b.txt"), "bravo\n".repeat(90)).unwrap();
    fs::write(src.join("empty.txt"), "").unwrap();
    fs::write(src.join("naïve 日本.txt"), "unicode\n").unwrap();
    for file in ["docs/a.txt", "docs/b.txt", "empty.txt", "naïve 日本.txt"] {
        let f = fs::File::options()
            .write(true)
            .open(src.join(file))
            .unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(MTIME))
            .unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("docs/a.txt", src.join("link")).unwrap();
    src
}

fn add_with(archive: &Path, inputs: &[PathBuf], options: &AddOptions) -> UpdateReport {
    add(
        archive,
        inputs,
        options,
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap()
}

fn extract_to(archive: &Path, dest: &Path) {
    let mut archive = Archive::open(archive).unwrap();
    let options = ExtractOptions {
        overwrite: OverwriteMode::Overwrite,
        ..ExtractOptions::default()
    };
    let report = extract(
        &mut archive,
        dest,
        &options,
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(report.errors, []);
}

fn assert_tree_matches(src: &Path, out: &Path) {
    for file in ["docs/a.txt", "docs/b.txt", "empty.txt", "naïve 日本.txt"] {
        let (a, b) = (src.join(file), out.join(file));
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap(), "{file}");
        let mtime = fs::metadata(&b).unwrap().modified().unwrap();
        assert_eq!(
            mtime,
            SystemTime::UNIX_EPOCH + Duration::from_secs(MTIME),
            "{file}"
        );
    }
    assert!(out.join("docs/empty-dir").is_dir());
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(out.join("link")).unwrap(),
        Path::new("docs/a.txt")
    );
}

fn store_links() -> AddOptions {
    AddOptions {
        store_symlinks: true,
        ..AddOptions::default()
    }
}

#[test]
fn round_trips_in_every_mode() {
    let tmp = TempDir::new("7zw-modes");
    let src = make_tree(tmp.path());
    let modes = [
        (5, true, true),
        (9, false, true),
        (1, true, false),
        (0, false, false),
    ];
    for (level, solid, compress_header) in modes {
        let name = format!("l{level}-s{solid}-h{compress_header}.7z");
        let archive = tmp.path().join(&name);
        let options = AddOptions {
            level,
            solid,
            compress_header,
            ..store_links()
        };
        let report = add_with(&archive, std::slice::from_ref(&src), &options);
        assert_eq!(report.warnings, [], "{name}");

        let opened = Archive::open(&archive).unwrap();
        assert_eq!(opened.format(), FormatId::SevenZip);
        let method = &opened
            .entries()
            .iter()
            .find(|e| e.name == "src/docs/a.txt")
            .unwrap()
            .method;
        assert_eq!(method.starts_with("LZMA2"), level > 0, "{name}: {method}");
        let out = tmp.path().join(format!("out-{name}"));
        extract_to(&archive, &out);
        assert_tree_matches(&src, &out.join("src"));
    }
}

#[test]
fn solid_archive_is_smaller_than_per_file_blocks() {
    let tmp = TempDir::new("7zw-solid");
    let src = make_tree(tmp.path());
    let solid = tmp.path().join("solid.7z");
    let split = tmp.path().join("split.7z");
    add_with(&solid, std::slice::from_ref(&src), &AddOptions::default());
    add_with(
        &split,
        std::slice::from_ref(&src),
        &AddOptions {
            solid: false,
            ..AddOptions::default()
        },
    );
    let size = |p: &Path| fs::metadata(p).unwrap().len();
    assert!(
        size(&solid) < size(&split),
        "{} vs {}",
        size(&solid),
        size(&split)
    );
    let kinds: Vec<_> = Archive::open(&solid)
        .unwrap()
        .entries()
        .iter()
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&EntryKind::Directory));
}

#[test]
fn update_and_delete_keep_other_items() {
    let tmp = TempDir::new("7zw-update");
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("u.7z");
    add_with(&archive, std::slice::from_ref(&src), &store_links());

    let extra = tmp.path().join("extra.txt");
    fs::write(&extra, "added later\n").unwrap();
    let report = add_with(
        &archive,
        &[extra],
        &AddOptions {
            mode: UpdateMode::Update,
            ..AddOptions::default()
        },
    );
    assert_eq!(report.added, 1);
    assert!(report.kept >= 7);

    let report = delete(
        &archive,
        &Selection::all().include("b.txt"),
        None,
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(report.deleted, 1);

    let out = tmp.path().join("out");
    extract_to(&archive, &out);
    assert_eq!(
        fs::read_to_string(out.join("extra.txt")).unwrap(),
        "added later\n"
    );
    assert!(!out.join("src/docs/b.txt").exists());
    assert_eq!(
        fs::read(out.join("src/docs/a.txt")).unwrap(),
        fs::read(src.join("docs/a.txt")).unwrap()
    );
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(out.join("src/link")).unwrap(),
        Path::new("docs/a.txt")
    );
}

#[test]
fn large_file_streams_through_lzma2() {
    let tmp = TempDir::new("7zw-large");
    let big = tmp.path().join("big.bin");
    let data: Vec<u8> = (0..6_000_000u32)
        .map(|i| u8::try_from((i / 7) % 251).unwrap())
        .collect();
    fs::write(&big, &data).unwrap();
    let archive = tmp.path().join("big.7z");
    add_with(
        &archive,
        &[big],
        &AddOptions {
            level: 1,
            ..AddOptions::default()
        },
    );
    assert!(fs::metadata(&archive).unwrap().len() < 1_000_000);
    extract_to(&archive, &tmp.path().join("out"));
    assert_eq!(fs::read(tmp.path().join("out/big.bin")).unwrap(), data);
}
