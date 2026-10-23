//! ZIP creation and update: round trips through RarX and, when installed, Info-ZIP
//! and `CPython` (AGENT.md §22.3).

#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use rarx_core::select::Selection;
use rarx_core::{
    AddOptions, Archive, EntryKind, Error, ExtractOptions, JobControl, OverwriteMode,
    SilentHandler, SilentUpdate, StorePaths, UpdateMode, UpdateReport, add, delete, extract,
};
use rarx_testkit::{TempDir, corpus_file};

const OLD: u64 = 1_700_000_000;

fn set_mtime(path: &Path, secs: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

/// Builds `src/` with text, binary, empty, nested and non-ASCII files.
fn make_tree(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(src.join("docs/deep")).unwrap();
    fs::write(src.join("docs/a.txt"), "hello\n".repeat(500)).unwrap();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let noise: Vec<u8> = (0..3000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    fs::write(src.join("noise.bin"), noise).unwrap();
    fs::write(src.join("docs/deep/empty"), "").unwrap();
    fs::write(src.join("naïve 日本.txt"), "unicode\n").unwrap();
    for file in [
        "docs/a.txt",
        "noise.bin",
        "docs/deep/empty",
        "naïve 日本.txt",
    ] {
        set_mtime(&src.join(file), OLD);
    }
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

fn names(archive: &Path) -> Vec<String> {
    Archive::open(archive)
        .unwrap()
        .entries()
        .iter()
        .map(|e| e.name.clone())
        .collect()
}

fn extract_all(archive: &Path, dest: &Path) {
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

fn assert_same_file(a: &Path, b: &Path) {
    assert_eq!(
        fs::read(a).unwrap(),
        fs::read(b).unwrap(),
        "{}",
        a.display()
    );
    let secs = |p: &Path| {
        fs::metadata(p)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };
    assert_eq!(secs(a), secs(b), "mtime of {}", a.display());
}

/// Runs an external checker if it is installed; returns `None` when it is not.
fn external(program: &str, args: &[&str]) -> Option<bool> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .map(|out| out.status.success())
}

fn python_check(archive: &Path) -> Option<bool> {
    let script =
        "import sys, zipfile\nz = zipfile.ZipFile(sys.argv[1])\nsys.exit(1 if z.testzip() else 0)";
    external("python3", &["-I", "-c", script, archive.to_str().unwrap()])
}

#[test]
fn creates_archive_that_round_trips() {
    let tmp = TempDir::new("write-roundtrip");
    let src = make_tree(tmp.path());
    let zip = tmp.path().join("out.zip");
    let report = add_with(&zip, std::slice::from_ref(&src), &AddOptions::default());
    assert_eq!((report.added, report.warnings.len()), (7, 0));

    let archive = Archive::open(&zip).unwrap();
    let a = archive
        .entries()
        .iter()
        .find(|e| e.name == "src/docs/a.txt")
        .unwrap();
    assert_eq!(a.method, "Deflate");
    let noise = archive
        .entries()
        .iter()
        .find(|e| e.name == "src/noise.bin")
        .unwrap();
    assert_eq!(
        noise.method, "Store",
        "incompressible data falls back to Store"
    );
    assert!(
        archive
            .entries()
            .iter()
            .any(|e| e.name == "src/docs/deep" && e.kind == EntryKind::Directory)
    );

    let out = tmp.path().join("out");
    extract_all(&zip, &out);
    for file in [
        "docs/a.txt",
        "noise.bin",
        "docs/deep/empty",
        "naïve 日本.txt",
    ] {
        assert_same_file(&src.join(file), &out.join("src").join(file));
    }
}

#[test]
fn created_archives_pass_external_tools() {
    let tmp = TempDir::new("write-compat");
    let src = make_tree(tmp.path());
    for (name, force_zip64) in [("plain.zip", false), ("zip64.zip", true)] {
        let zip = tmp.path().join(name);
        add_with(
            &zip,
            std::slice::from_ref(&src),
            &AddOptions {
                force_zip64,
                ..AddOptions::default()
            },
        );
        let path = zip.to_str().unwrap();
        if let Some(ok) = external("unzip", &["-tq", path]) {
            assert!(ok, "unzip -t rejected {name}");
        }
        if let Some(ok) = python_check(&zip) {
            assert!(ok, "python zipfile rejected {name}");
        }
        assert_eq!(names(&zip).len(), 7);
    }
}

#[test]
fn large_files_stream_through_deflate() {
    let tmp = TempDir::new("write-large");
    let big = tmp.path().join("big.txt");
    fs::write(&big, "0123456789abcdef".repeat(400_000)).unwrap();
    let zip = tmp.path().join("big.zip");
    add_with(
        &zip,
        std::slice::from_ref(&big),
        &AddOptions {
            level: 1,
            ..AddOptions::default()
        },
    );
    let archive = Archive::open(&zip).unwrap();
    assert!(archive.entries()[0].packed_size < 200_000);
    extract_all(&zip, &tmp.path().join("out"));
    assert_eq!(
        fs::read(&big).unwrap(),
        fs::read(tmp.path().join("out/big.txt")).unwrap()
    );
}

#[test]
fn path_modes() {
    let tmp = TempDir::new("write-paths");
    let src = make_tree(tmp.path());
    let input = src.join("docs");

    let relative = tmp.path().join("relative.zip");
    add_with(
        &relative,
        std::slice::from_ref(&input),
        &AddOptions::default(),
    );
    assert!(names(&relative).contains(&"docs/deep/empty".to_owned()));

    let flat = tmp.path().join("flat.zip");
    add_with(
        &flat,
        std::slice::from_ref(&input),
        &AddOptions {
            paths: StorePaths::None,
            ..AddOptions::default()
        },
    );
    assert_eq!(names(&flat), ["a.txt", "empty"]);

    let given = tmp.path().join("given.zip");
    add_with(
        &given,
        &[input],
        &AddOptions {
            paths: StorePaths::AsGiven,
            ..AddOptions::default()
        },
    );
    // Roots and drive letters are dropped; the rest of the path is kept.
    let mut parts: Vec<String> = src
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    parts.extend(["docs".to_owned(), "a.txt".to_owned()]);
    let expected = parts.join("/");
    assert!(names(&given).contains(&expected), "{:?}", names(&given));
}

#[test]
fn update_freshen_and_synchronize() {
    let tmp = TempDir::new("write-update");
    let src = make_tree(tmp.path());
    let zip = tmp.path().join("u.zip");
    add_with(&zip, std::slice::from_ref(&src), &AddOptions::default());

    // Newer a.txt, unchanged noise.bin, a brand-new file, and a deleted file.
    fs::write(src.join("docs/a.txt"), "changed\n").unwrap();
    fs::write(src.join("new.txt"), "new\n").unwrap();
    fs::remove_file(src.join("naïve 日本.txt")).unwrap();

    let freshen = AddOptions {
        mode: UpdateMode::Freshen,
        ..AddOptions::default()
    };
    let report = add_with(&zip, std::slice::from_ref(&src), &freshen);
    assert_eq!((report.replaced, report.added), (1, 0));
    assert!(!names(&zip).contains(&"src/new.txt".to_owned()));

    let update = AddOptions {
        mode: UpdateMode::Update,
        ..AddOptions::default()
    };
    let report = add_with(&zip, std::slice::from_ref(&src), &update);
    assert_eq!((report.replaced, report.added), (0, 1));
    assert!(names(&zip).contains(&"src/naïve 日本.txt".to_owned()));

    let sync = AddOptions {
        mode: UpdateMode::Synchronize,
        ..AddOptions::default()
    };
    let report = add_with(&zip, std::slice::from_ref(&src), &sync);
    assert_eq!(report.deleted, 1);
    assert!(!names(&zip).contains(&"src/naïve 日本.txt".to_owned()));

    let out = tmp.path().join("out");
    extract_all(&zip, &out);
    assert_eq!(
        fs::read_to_string(out.join("src/docs/a.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        fs::read_to_string(out.join("src/new.txt")).unwrap(),
        "new\n"
    );
}

#[test]
fn delete_by_mask() {
    let tmp = TempDir::new("write-delete");
    let src = make_tree(tmp.path());
    let zip = tmp.path().join("d.zip");
    add_with(&zip, &[src], &AddOptions::default());
    let report = delete(
        &zip,
        &Selection::all().include("docs"),
        None,
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(report.deleted, 4);
    assert_eq!(names(&zip), ["src", "src/naïve 日本.txt", "src/noise.bin"]);

    let none = delete(
        &zip,
        &Selection::all().include("*.zzz"),
        None,
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap();
    assert!(!none.written);
}

#[test]
fn raw_copy_keeps_encrypted_items_intact() {
    let tmp = TempDir::new("write-rawcopy");
    let zip = tmp.path().join("enc.zip");
    fs::copy(corpus_file("zip/infozip-encrypted.zip"), &zip).unwrap();
    let extra = tmp.path().join("plain.txt");
    fs::write(&extra, "plain\n").unwrap();
    let report = add_with(&zip, &[extra], &AddOptions::default());
    assert_eq!((report.kept, report.added), (1, 1));
    let archive = Archive::open(&zip).unwrap();
    assert!(archive.entries()[0].encrypted);
    if let Some(ok) = external("unzip", &["-tq", "-P", "secret", zip.to_str().unwrap()]) {
        assert!(ok, "unzip could not decrypt the copied item");
    }
}

#[test]
fn archive_does_not_include_itself_and_can_move_sources() {
    let tmp = TempDir::new("write-move");
    let src = make_tree(tmp.path());
    let zip = src.join("inside.zip");
    let options = AddOptions {
        delete_sources: true,
        ..AddOptions::default()
    };
    let report = add_with(&zip, std::slice::from_ref(&src), &options);
    assert_eq!(report.added, 7);
    assert!(!names(&zip).iter().any(|n| {
        Path::new(n)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    }));
    let left: Vec<_> = fs::read_dir(&src)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["inside.zip"]);
}

#[cfg(unix)]
#[test]
fn symlinks_followed_or_stored() {
    let tmp = TempDir::new("write-links");
    let src = make_tree(tmp.path());
    std::os::unix::fs::symlink("docs/a.txt", src.join("link")).unwrap();

    let followed = tmp.path().join("followed.zip");
    add_with(
        &followed,
        std::slice::from_ref(&src),
        &AddOptions::default(),
    );
    let archive = Archive::open(&followed).unwrap();
    let link = archive
        .entries()
        .iter()
        .find(|e| e.name == "src/link")
        .unwrap();
    assert_eq!((link.kind, link.size), (EntryKind::File, 3000));

    let stored = tmp.path().join("stored.zip");
    add_with(
        &stored,
        &[src],
        &AddOptions {
            store_symlinks: true,
            ..AddOptions::default()
        },
    );
    let out = tmp.path().join("out");
    extract_all(&stored, &out);
    assert_eq!(
        fs::read_link(out.join("src/link")).unwrap(),
        Path::new("docs/a.txt")
    );
}

#[test]
fn unsupported_targets_and_missing_inputs() {
    let tmp = TempDir::new("write-errors");
    let src = make_tree(tmp.path());
    let err = add(
        &tmp.path().join("x.rar"),
        &[src],
        &AddOptions::default(),
        &mut SilentUpdate,
        &JobControl::default(),
    );
    assert!(matches!(err, Err(Error::Unsupported(_))));

    let zip = tmp.path().join("none.zip");
    let report = add_with(&zip, &[tmp.path().join("missing")], &AddOptions::default());
    assert_eq!((report.written, report.warnings.len()), (false, 1));
    assert!(!zip.exists());

    let cancelled = JobControl::default();
    cancelled.cancel();
    let src = tmp.path().join("src");
    let result = add(
        &zip,
        &[src],
        &AddOptions::default(),
        &mut SilentUpdate,
        &cancelled,
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    let leftovers: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        ["src"],
        "no archive or temp file is left after cancelling"
    );
}
