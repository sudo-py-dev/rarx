# 7z corpus

No reference 7-Zip binary was available when these were made, so two independent
producers are used:

- `generate.py` — containers assembled by hand per `7zFormat.txt`; all compressed
  data from CPython's `lzma` module (liblzma). Regenerate: `python3 -I corpus/7z/generate.py`.
- `sevenz-gen.rs` — `main.rs` of a throwaway crate depending on `sevenz-rust2 0.24`
  (`features = ["deflate"]`); run it with this folder as its argument. Produces the
  `s7r-*` files.

| File | Producer | Purpose |
|---|---|---|
| `lzma2-solid.7z` | generate.py | solid LZMA2 block, folder, empty file, UTF-16 name, Unix symlink/modes |
| `lzma-encoded-header.7z` | generate.py | LZMA data + LZMA-compressed header |
| `bcj-lzma2.7z` | generate.py | BCJ x86 + LZMA2 coder chain (bind pair) |
| `delta-lzma.7z` | generate.py | Delta:4 + LZMA chain |
| `multi-folder.7z` | generate.py | two folders: Copy and LZMA2 |
| `corrupt-crc.7z` | generate.py | wrong CRC for one item in a solid block |
| `encrypted.7z` | generate.py | 7zAES coder marker → encrypted |
| `truncated.7z` | generate.py | first half of `lzma2-solid.7z` |
| `empty.7z` | generate.py | start header only |
| `s7r-solid.7z`, `s7r-nonsolid.7z` | sevenz-rust2 | default LZMA2, solid and per-file blocks |
| `s7r-bcj-lzma2.7z`, `s7r-deflate.7z` | sevenz-rust2 | BCJ + LZMA2, Deflate |
| `s7r-aes.7z` | sevenz-rust2 | AES-256 with encrypted header (password `secret`) |
| `s7r-ppmd.7z` | sevenz-rust2 | PPMd (unsupported for now) |

`sevenz-verify.rs` is the `main.rs` of a second throwaway crate (same dependency) that
extracts an archive with `sevenz-rust2`, used to check archives written by RarX.

Still wanted: archives from real 7-Zip (BCJ2, ARM64, multi-volume, SFX, `-mhc=off`,
`-mtc=on`, anti-items).
