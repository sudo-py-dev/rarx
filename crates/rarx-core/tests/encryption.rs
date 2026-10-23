//! Encrypted 7z and ZIP archives: reading, writing, wrong passwords, rewrites
//! (AGENT.md §8).

#![allow(clippy::unwrap_used)] // helpers outside #[test] fns; failures should abort the test

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rarx_core::select::Selection;
use rarx_core::{
    AddOptions, Archive, Error, ExtractOptions, FormatError, ItemError, JobControl, OverwriteMode,
    Password, Report, SilentHandler, SilentUpdate, ZipCipher, add, delete, extract, test,
};
use rarx_testkit::{TempDir, corpus_file};

#[allow(clippy::unnecessary_wraps)] // reads naturally at the call sites
fn pw(text: &str) -> Option<Password> {
    Some(Password::from(text))
}

fn make_tree(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(src.join("docs/empty-dir")).unwrap();
    fs::write(src.join("docs/a.txt"), "secret text\n".repeat(700)).unwrap();
    fs::write(src.join("docs/tiny.txt"), "tiny\n").unwrap();
    fs::write(src.join("empty.txt"), "").unwrap();
    src
}

fn add_with(archive: &Path, inputs: &[PathBuf], options: &AddOptions) {
    let report = add(
        archive,
        inputs,
        options,
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(report.warnings, []);
}

fn extract_with(archive: &Path, password: Option<Password>, dest: &Path) -> Report {
    let mut archive = Archive::open_with_password(archive, password).unwrap();
    let options = ExtractOptions {
        overwrite: OverwriteMode::Overwrite,
        ..ExtractOptions::default()
    };
    extract(
        &mut archive,
        dest,
        &options,
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap()
}

fn test_with(archive: &Path, password: Option<Password>) -> Report {
    let mut archive = Archive::open_with_password(archive, password).unwrap();
    test(
        &mut archive,
        &Selection::all(),
        &mut SilentHandler,
        &JobControl::default(),
    )
    .unwrap()
}

fn assert_tree_matches(src: &Path, out: &Path) {
    for file in ["docs/a.txt", "docs/tiny.txt", "empty.txt"] {
        assert_eq!(
            fs::read(src.join(file)).unwrap(),
            fs::read(out.join("src").join(file)).unwrap(),
            "{file}"
        );
    }
    assert!(out.join("src/docs/empty-dir").is_dir());
}

/// Every error is a wrong password (or a missing one with `missing`).
fn all_password_errors(report: &Report, missing: bool) -> bool {
    let wanted = if missing {
        ItemError::Encrypted
    } else {
        ItemError::WrongPassword
    };
    !report.errors.is_empty() && report.errors.iter().all(|(_, e)| *e == wanted)
}

#[test]
fn sevenz_rust2_aes_archive_with_encrypted_header() {
    let path = corpus_file("7z/s7r-aes.7z");
    assert!(matches!(
        Archive::open(&path),
        Err(Error::Format(FormatError::Encrypted))
    ));
    assert!(matches!(
        Archive::open_with_password(&path, pw("guess")),
        Err(Error::Format(FormatError::WrongPassword))
    ));
    let tmp = TempDir::new("enc-s7r");
    let report = extract_with(&path, pw("secret"), tmp.path());
    assert_eq!(report.errors, []);
    assert_eq!(report.files, 4);
    assert_eq!(
        fs::read(tmp.path().join("program.bin")).unwrap().len(),
        33000
    );
}

#[test]
fn infozip_zipcrypto_archive() {
    let path = corpus_file("zip/infozip-encrypted.zip");
    let archive = Archive::open(&path).unwrap();
    assert_eq!(archive.entries()[0].method, "ZipCrypto Deflate");
    let tmp = TempDir::new("enc-infozip");
    let report = extract_with(&path, pw("secret"), tmp.path());
    assert_eq!(report.errors, []);
    let plain = TempDir::new("enc-infozip-plain");
    extract_with(&corpus_file("zip/infozip-basic.zip"), None, plain.path());
    assert_eq!(
        fs::read(tmp.path().join("dir/a.txt")).unwrap(),
        fs::read(plain.path().join("dir/a.txt")).unwrap()
    );
    assert!(all_password_errors(&test_with(&path, pw("nope")), false));
    assert!(all_password_errors(&test_with(&path, None), true));
}

#[test]
fn sevenz_round_trips_with_and_without_header_encryption() {
    for encrypt_header in [false, true] {
        for level in [0, 5] {
            let tmp = TempDir::new("enc-7z");
            let src = make_tree(tmp.path());
            let archive = tmp.path().join("out.7z");
            let options = AddOptions {
                level,
                password: pw("päss wörd"),
                encrypt_header,
                ..AddOptions::default()
            };
            add_with(&archive, std::slice::from_ref(&src), &options);
            let case = format!("header {encrypt_header}, level {level}");

            let out = tmp.path().join("out");
            let report = extract_with(&archive, pw("päss wörd"), &out);
            assert_eq!(report.errors, [], "{case}");
            assert_tree_matches(&src, &out);

            let raw = fs::read(&archive).unwrap();
            assert!(
                !raw.windows(11).any(|w| w == b"secret text"),
                "{case}: plaintext leaked"
            );
            if encrypt_header {
                assert!(
                    matches!(
                        Archive::open(&archive),
                        Err(Error::Format(FormatError::Encrypted))
                    ),
                    "{case}"
                );
                assert!(
                    matches!(
                        Archive::open_with_password(&archive, pw("wrong")),
                        Err(Error::Format(FormatError::WrongPassword))
                    ),
                    "{case}"
                );
            } else {
                let listed = Archive::open(&archive).unwrap();
                assert!(listed.has_encrypted_items(), "{case}");
                assert!(
                    all_password_errors(&test_with(&archive, None), true),
                    "{case}"
                );
                assert!(
                    all_password_errors(&test_with(&archive, pw("wrong")), false),
                    "{case}"
                );
            }
        }
    }
}

#[test]
fn deleting_from_encrypted_7z_keeps_it_encrypted() {
    let tmp = TempDir::new("enc-7z-del");
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("d.7z");
    let options = AddOptions {
        password: pw("k"),
        encrypt_header: true,
        ..AddOptions::default()
    };
    add_with(&archive, std::slice::from_ref(&src), &options);
    let report = delete(
        &archive,
        &Selection::all().include("src/docs/tiny.txt"),
        pw("k"),
        &mut SilentUpdate,
        &JobControl::default(),
    )
    .unwrap();
    assert_eq!(report.deleted, 1);
    assert!(matches!(
        Archive::open(&archive),
        Err(Error::Format(FormatError::Encrypted))
    ));
    let opened = Archive::open_with_password(&archive, pw("k")).unwrap();
    assert!(
        !opened
            .entries()
            .iter()
            .any(|e| e.name == "src/docs/tiny.txt")
    );
    assert_eq!(test_with(&archive, pw("k")).errors, []);

    let refused = delete(
        &archive,
        &Selection::all().include("src/empty.txt"),
        None,
        &mut SilentUpdate,
        &JobControl::default(),
    );
    assert!(matches!(
        refused,
        Err(Error::Format(FormatError::Encrypted))
    ));
}

#[test]
fn zip_round_trips_with_both_ciphers() {
    for cipher in [ZipCipher::Aes256, ZipCipher::ZipCrypto] {
        let tmp = TempDir::new("enc-zip");
        let src = make_tree(tmp.path());
        let archive = tmp.path().join("out.zip");
        let options = AddOptions {
            password: pw("zip pass"),
            zip_cipher: cipher,
            ..AddOptions::default()
        };
        add_with(&archive, std::slice::from_ref(&src), &options);

        let listed = Archive::open(&archive).unwrap();
        let entry = |name: &str| {
            listed
                .entries()
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .clone()
        };
        assert!(entry("src/docs/a.txt").encrypted);
        assert!(!entry("src/docs").encrypted, "folders stay plain");
        if cipher == ZipCipher::Aes256 {
            assert_eq!(entry("src/docs/a.txt").method, "AES-256 Deflate");
            assert!(
                entry("src/docs/a.txt").crc32.is_some(),
                "AE-1 keeps the CRC"
            );
            assert_eq!(
                entry("src/docs/tiny.txt").crc32,
                None,
                "AE-2 for tiny files"
            );
        }

        let out = tmp.path().join("out");
        let report = extract_with(&archive, pw("zip pass"), &out);
        assert_eq!(report.errors, [], "{cipher:?}");
        assert_tree_matches(&src, &out);
        assert!(
            all_password_errors(&test_with(&archive, pw("other")), false),
            "{cipher:?}"
        );
        if cipher == ZipCipher::ZipCrypto {
            unzip_agrees(&archive, &src);
        }
    }
}

/// Info-ZIP `unzip` decrypts our ZipCrypto items (skipped where it is missing).
fn unzip_agrees(archive: &Path, src: &Path) {
    let Ok(out) = Command::new("unzip")
        .args(["-p", "-P", "zip pass"])
        .arg(archive)
        .arg("src/docs/a.txt")
        .output()
    else {
        return;
    };
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, fs::read(src.join("docs/a.txt")).unwrap());
}

#[test]
fn large_zip_items_stream_through_the_cipher() {
    // Above the in-memory limit, so ZipCrypto needs a data descriptor.
    let tmp = TempDir::new("enc-zip-big");
    let big = tmp.path().join("big.bin");
    let data: Vec<u8> = (0..5_000_000u32)
        .map(|i| (i % 253).to_le_bytes()[0])
        .collect();
    fs::write(&big, &data).unwrap();
    for cipher in [ZipCipher::Aes256, ZipCipher::ZipCrypto] {
        let archive = tmp.path().join(format!("{cipher:?}.zip"));
        let options = AddOptions {
            password: pw("p"),
            zip_cipher: cipher,
            level: 1,
            ..AddOptions::default()
        };
        add_with(&archive, std::slice::from_ref(&big), &options);
        let out = tmp.path().join(format!("out-{cipher:?}"));
        assert_eq!(extract_with(&archive, pw("p"), &out).errors, []);
        assert_eq!(fs::read(out.join("big.bin")).unwrap(), data, "{cipher:?}");
    }
}

#[test]
fn tar_refuses_passwords() {
    let tmp = TempDir::new("enc-tar");
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("x.tar.gz");
    let result = add(
        &archive,
        &[src],
        &AddOptions {
            password: pw("p"),
            ..AddOptions::default()
        },
        &mut SilentUpdate,
        &JobControl::default(),
    );
    assert!(matches!(result, Err(Error::Unsupported(_))));
    assert!(!archive.exists());
}
