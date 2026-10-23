//! Writes 7z fixtures with sevenz-rust2 (an independent implementation) into argv[1].

use std::fs;
use std::path::Path;

use sevenz_rust2::encoder_options::{AesEncoderOptions, DeflateOptions, Lzma2Options};
use sevenz_rust2::{ArchiveWriter, EncoderConfiguration, EncoderMethod};

fn tree(root: &Path) {
    fs::create_dir_all(root.join("docs/nested")).unwrap();
    fs::create_dir_all(root.join("empty-dir")).unwrap();
    let text: String = (0..400).map(|i| format!("line {i}: sevenz-rust2 cross-check text\n")).collect();
    fs::write(root.join("docs/readme.txt"), &text).unwrap();
    fs::write(root.join("docs/nested/deep.txt"), text.repeat(3)).unwrap();
    fs::write(root.join("empty.txt"), "").unwrap();
    let mut code = Vec::new();
    for i in 0..3000i32 {
        code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5, 0xE8]);
        code.extend_from_slice(&(0x2000 - i * 12).to_le_bytes());
        code.extend_from_slice(&[0x5D, 0xC3]);
    }
    fs::write(root.join("program.bin"), code).unwrap();
}

fn write(out: &Path, name: &str, src: &Path, methods: Vec<EncoderConfiguration>, solid: bool, encrypt_header: bool) {
    let path = out.join(name);
    let mut w = ArchiveWriter::create(&path).unwrap();
    if !methods.is_empty() {
        w.set_content_methods(methods);
    }
    w.set_encrypt_header(encrypt_header);
    if solid {
        w.push_source_path(src, |_| true).unwrap();
    } else {
        w.push_source_path_non_solid(src, |_| true).unwrap();
    }
    w.finish().unwrap();
    println!("wrote {}", path.display());
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("output dir"));
    let src = std::env::temp_dir().join(format!("sevenz-gen-{}", std::process::id()));
    let _ = fs::remove_dir_all(&src);
    tree(&src);
    write(&out, "s7r-solid.7z", &src, vec![], true, false);
    write(&out, "s7r-nonsolid.7z", &src, vec![], false, false);
    write(&out, "s7r-bcj-lzma2.7z", &src,
        vec![EncoderMethod::BCJ_X86_FILTER.into(), Lzma2Options::from_level(9).into()], true, false);
    write(&out, "s7r-deflate.7z", &src, vec![DeflateOptions::default().into()], true, false);
    write(&out, "s7r-aes.7z", &src,
        vec![AesEncoderOptions::new("secret".into()).into(), Lzma2Options::default().into()], true, true);
    write(&out, "s7r-ppmd.7z", &src, vec![EncoderMethod::PPMD.into()], true, false);
    fs::remove_dir_all(&src).unwrap();
}
