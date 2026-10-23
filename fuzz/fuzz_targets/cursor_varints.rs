#![no_main]

use libfuzzer_sys::fuzz_target;
use rarx_io::Cursor;

fuzz_target!(|data: &[u8]| {
    let mut rar = Cursor::new(data);
    while rar.read_rar_vint().is_ok() {}
    let mut sevenzip = Cursor::new(data);
    while sevenzip.read_7z_number().is_ok() {}
});
