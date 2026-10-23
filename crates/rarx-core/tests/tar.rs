//! TAR family: GNU tar / gzip / bzip2 / xz / zstd fixtures in, RarX output checked
//! by GNU tar when it is installed (AGENT.md §22).

#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test
#![allow(clippy::format_collect)] // fixtures mirror the generator's simple loops

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use rarx_core::select::Selection;
use rarx_core::{
    AddOptions, Archive, EntryKind, Error, ExtractOptions, FileTime, FormatId, ItemError,
    JobControl, OverwriteMode, Report, SilentHandler, SilentUpdate, UpdateReport, add, extract,
    test,
};
use rarx_testkit::{TempDir, corpus_file};

const DEEP: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff.txt";

fn open(name: &str) -> Archive {
    Archive::open(&corpus_file(format!("tar/{name}"))).unwrap()
}

/// Same text as `a.txt` in corpus/tar/generate.sh.
fn a_txt() -> String {
    (0..200)
        .map(|i| format!("line {i} of the tar fixture\n"))
        .collect()
}

fn extract_all(archive: &mut Archive, dest: &Path) -> Report {
    let options = ExtractOptions {
        overwrite: OverwriteMode::Overwrite,
        ..ExtractOptions::default()
    };
    extract(
        archive,
        dest,
        &options,
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap()
}

fn read_item(archive: &mut Archive, name: &str) -> String {
    let index = archive
        .entries()
        .iter()
        .position(|e| e.name == name)
        .unwrap();
    let mut out = String::new();
    archive
        .read_entry(index)
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    out
}

#[test]
fn lists_pax_archive() {
    let archive = open("pax.tar");
    assert_eq!(archive.format(), FormatId::Tar);
    let get = |name: &str| archive.entries().iter().find(|e| e.name == name).unwrap();
    assert_eq!(get("./dir/a.txt").size, 5490);
    assert_eq!(get("./dir/a.txt").unix_mode, Some(0o755));
    assert_eq!(get("./link").kind, EntryKind::Symlink);
    assert_eq!(get("./dir/empty").kind, EntryKind::Directory);
    assert_eq!(
        get("./hard.txt").size,
        12,
        "hard links report their target's size"
    );
    assert_eq!(
        get(&format!("./{DEEP}")).size,
        5,
        "pax path longer than 100 bytes"
    );
    assert!(
        archive
            .entries()
            .iter()
            .any(|e| e.name == "./naïve 日本.txt")
    );
    assert_eq!(
        get("./dir/b.txt").modified,
        Some(FileTime::Unix {
            seconds: 1_741_064_767,
            nanos: 123_456_789
        })
    );
}

#[test]
fn extracts_every_dialect_and_compressor() {
    for name in [
        "gnu.tar",
        "pax.tar",
        "pax.tar.gz",
        "pax.tgz",
        "pax.tar.bz2",
        "pax.tar.xz",
        "pax.tar.zst",
    ] {
        let tmp = TempDir::new("tar-x");
        let report = extract_all(&mut open(name), tmp.path());
        assert_eq!(report.errors, [], "{name}");
        assert_eq!(
            fs::read_to_string(tmp.path().join("dir/a.txt")).unwrap(),
            a_txt(),
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join(DEEP)).unwrap(),
            "deep\n",
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join("hard.txt")).unwrap(),
            "second file\n",
            "{name}"
        );
        assert!(tmp.path().join("dir/empty").is_dir(), "{name}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(tmp.path().join("dir/a.txt"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755, "{name}");
            assert_eq!(
                fs::read_link(tmp.path().join("link")).unwrap(),
                Path::new("dir/a.txt")
            );
        }
    }
    for name in ["ustar.tar", "v7.tar"] {
        let tmp = TempDir::new("tar-old");
        assert_eq!(
            extract_all(&mut open(name), tmp.path()).errors,
            [],
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join("dir/a.txt")).unwrap(),
            a_txt(),
            "{name}"
        );
    }
}

#[test]
fn random_access_in_compressed_tar() {
    let mut archive = open("pax.tar.xz");
    assert_eq!(read_item(&mut archive, "./dir/b.txt"), "second file\n");
    assert_eq!(
        read_item(&mut archive, "./dir/a.txt"),
        a_txt(),
        "going backwards restarts the decoder"
    );
    assert_eq!(read_item(&mut archive, "./hard.txt"), "second file\n");
}

#[test]
fn single_compressed_files() {
    for (name, item, content) in [
        ("a.txt.gz", "a.txt", a_txt()),
        ("b.txt.bz2", "b.txt", "second file\n".to_owned()),
        ("b.txt.xz", "b.txt", "second file\n".to_owned()),
        ("b.txt.zst", "b.txt", "second file\n".to_owned()),
        (
            "members.txt.gz",
            "members.txt",
            "first\nsecond\n".to_owned(),
        ),
    ] {
        let mut archive = open(name);
        assert_eq!(archive.entries().len(), 1, "{name}");
        assert_eq!(read_item(&mut archive, item), content, "{name}");
    }
}

#[test]
fn damaged_and_unsupported() {
    assert!(matches!(
        Archive::open(&corpus_file("tar/truncated.tar.gz")),
        Err(Error::Format(_))
    ));
    let report = test(
        &mut open("sparse.tar"),
        &Selection::all(),
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap();
    assert!(
        matches!(&report.errors[..], [(_, ItemError::Unsupported(what))] if what.contains("sparse"))
    );
}

/// A tree with a long path, a non-ASCII name, a symlink and an empty folder.
fn make_tree(root: &Path) -> PathBuf {
    let src = root.join("src");
    let long_dir = src.join("l".repeat(70)).join("m".repeat(70));
    fs::create_dir_all(&long_dir).unwrap();
    fs::create_dir_all(src.join("empty")).unwrap();
    fs::write(long_dir.join("deep.txt"), "deep\n").unwrap();
    fs::write(src.join("naïve 日本.txt"), "unicode\n").unwrap();
    fs::write(src.join("big.txt"), a_txt().repeat(50)).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("big.txt", src.join("link")).unwrap();
    src
}

fn add_with(archive: &Path, inputs: &[PathBuf]) -> UpdateReport {
    let options = AddOptions {
        store_symlinks: true,
        ..AddOptions::default()
    };
    add(
        archive,
        inputs,
        &options,
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap()
}

fn gnu_tar_lists(archive: &Path) -> Option<Vec<String>> {
    let out = Command::new("tar").arg("-tf").arg(archive).output().ok()?;
    assert!(
        out.status.success(),
        "GNU tar rejected {}: {}",
        archive.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
fn writes_tar_family_that_round_trips() {
    let tmp = TempDir::new("tar-w");
    let src = make_tree(tmp.path());
    let deep = format!("src/{}/{}/deep.txt", "l".repeat(70), "m".repeat(70));
    for name in [
        "out.tar",
        "out.tar.gz",
        "out.tgz",
        "out.tar.bz2",
        "out.tar.xz",
        "out.tar.zst",
    ] {
        let archive = tmp.path().join(name);
        let report = add_with(&archive, std::slice::from_ref(&src));
        assert_eq!(report.warnings, [], "{name}");

        let mut opened = Archive::open(&archive).unwrap();
        assert_eq!(opened.format(), FormatId::Tar, "{name}");
        let out = tmp.path().join(format!("x-{name}"));
        assert_eq!(extract_all(&mut opened, &out).errors, [], "{name}");
        assert_eq!(
            fs::read_to_string(out.join(&deep)).unwrap(),
            "deep\n",
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(out.join("src/naïve 日本.txt")).unwrap(),
            "unicode\n",
            "{name}"
        );
        assert_eq!(
            fs::read(out.join("src/big.txt")).unwrap(),
            fs::read(src.join("big.txt")).unwrap(),
            "{name}"
        );
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(out.join("src/link")).unwrap(),
            Path::new("big.txt"),
            "{name}"
        );

        if let Some(names) = gnu_tar_lists(&archive) {
            assert!(names.contains(&deep), "{name}: {names:?}");
            assert!(
                names.contains(&"src/naïve 日本.txt".to_owned()),
                "{name}: {names:?}"
            );
            assert!(
                names.contains(&"src/empty/".to_owned()),
                "{name}: {names:?}"
            );
        }
    }
}

#[test]
fn updating_compressed_tar_keeps_items() {
    let tmp = TempDir::new("tar-u");
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("u.tar.zst");
    add_with(&archive, std::slice::from_ref(&src));
    let extra = tmp.path().join("extra.txt");
    fs::write(&extra, "extra\n").unwrap();
    let report = add_with(&archive, &[extra]);
    assert_eq!(report.added, 1);
    let mut opened = Archive::open(&archive).unwrap();
    assert_eq!(read_item(&mut opened, "extra.txt"), "extra\n");
    assert_eq!(read_item(&mut opened, "src/big.txt"), a_txt().repeat(50));
}

#[test]
fn single_stream_files() {
    let tmp = TempDir::new("tar-s");
    let file = tmp.path().join("notes.txt");
    fs::write(&file, a_txt()).unwrap();
    for ext in ["gz", "bz2", "xz", "zst"] {
        let archive = tmp.path().join(format!("notes.txt.{ext}"));
        add_with(&archive, std::slice::from_ref(&file));
        let mut opened = Archive::open(&archive).unwrap();
        assert_eq!(opened.entries()[0].name, "notes.txt", "{ext}");
        assert_eq!(read_item(&mut opened, "notes.txt"), a_txt(), "{ext}");
    }
    let other = tmp.path().join("other.txt");
    fs::write(&other, "other\n").unwrap();
    let two = add(
        &tmp.path().join("two.gz"),
        &[file, other],
        &AddOptions::default(),
        &mut SilentUpdate,
        &JobControl::default(),
    );
    assert!(two.is_err(), "a .gz holds one file");
    assert!(!tmp.path().join("two.gz").exists());
}
