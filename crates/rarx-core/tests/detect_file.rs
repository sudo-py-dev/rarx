use std::io::Write;

use rarx_core::{FormatId, detect_file};

#[test]
fn detects_7z_file_on_disk() {
    let dir = std::env::temp_dir().join(format!("rarx-detect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sample.bin");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(b"7z\xBC\xAF\x27\x1C\x00\x04")
        .unwrap();

    let detected = detect_file(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(detected, Some(FormatId::SevenZip));
}

#[test]
fn missing_file_is_io_error() {
    assert!(matches!(
        detect_file(std::path::Path::new("/nonexistent/rarx/archive.7z")),
        Err(rarx_core::Error::Io(_))
    ));
}
