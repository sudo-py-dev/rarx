#![no_main]

use std::io::{Cursor, Read};

use libfuzzer_sys::fuzz_target;
use rarx_formats::{FormatId, Password};

// Opens the input as every readable format and drains every item, with a password so
// encrypted headers and items reach the decryption code.
fuzz_target!(|data: &[u8]| {
    for format in [FormatId::Zip, FormatId::SevenZip, FormatId::Tar, FormatId::Gzip, FormatId::Xz] {
        let src = Box::new(Cursor::new(data.to_vec()));
        if let Ok(mut archive) = rarx_formats::open(format, src, "item", Some(Password::from("fuzz"))) {
            for index in 0..archive.entries().len().min(64) {
                if let Ok(mut stream) = archive.open_entry(index) {
                    let mut sink = Vec::new();
                    let _ = stream.by_ref().take(1 << 20).read_to_end(&mut sink);
                }
            }
        }
    }
});
