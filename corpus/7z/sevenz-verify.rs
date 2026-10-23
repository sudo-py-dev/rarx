//! Extracts argv[1] (a .7z) into argv[2] with sevenz-rust2, using argv[3] as the
//! password if given; `list` mode prints entries.
fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().unwrap();
    if first == "list" {
        let path = args.next().unwrap();
        let archive = sevenz_rust2::Archive::open(&path).unwrap();
        for (i, f) in archive.files.iter().enumerate() {
            println!("{i}: {:?} has_stream={} dir={} size={}", f.name(), f.has_stream(), f.is_directory(), f.size());
        }
        println!("folders={}", archive.blocks.len());
        return;
    }
    let dest = args.next().unwrap();
    match args.next() {
        Some(pw) => sevenz_rust2::decompress_file_with_password(&first, &dest, pw.as_str().into()),
        None => sevenz_rust2::decompress_file(&first, &dest),
    }
    .unwrap_or_else(|e| panic!("{first}: {e}"));
}
