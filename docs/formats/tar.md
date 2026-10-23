# TAR and compressed streams

Implemented in `crates/rarx-formats/src/tar/` (`header.rs`, `mod.rs` reader,
`writer.rs`) and `crates/rarx-formats/src/stream.rs` (gzip, bzip2, xz, Zstandard).

## Reading

- Detection: `ustar` magic at offset 257, `.tar` extension for v7 archives, and for
  `.gz/.bz2/.xz/.zst` the first decoded block: a valid TAR header means a compressed
  TAR (one-step tree, like WinRAR), anything else a single compressed file named after
  the archive stem.
- Headers: checksum verified (unsigned and signed sums); octal and GNU base-256
  numbers; ustar `prefix`; GNU `L`/`K` long names (≤ 1 MiB); pax `x` (per item) and
  `g` (global) records for `path`, `linkpath`, `size`, `mtime`/`atime`/`ctime` with
  nanoseconds. Volume labels are skipped.
- Items: files, folders, symlinks (target from the header), hard links (read as a copy
  of the earlier target, never created as links), devices/FIFOs, sparse files (`S`,
  `GNU.sparse.*`) and multi-volume parts are listed but reported as unsupported.
- `./` folder entries map to the destination itself and are skipped on extraction.
- Plain archives read item data in place; compressed archives are decoded once for
  the listing and then through a kept decoder (restarted only to go backwards).
- Streams: every gzip member, bzip2 stream, xz stream and zstd frame is decoded
  (concatenated files work). Zstandard windows up to 2 GiB are accepted.

## Writing

- ustar headers; a pax `x` header is added when a name is non-ASCII or does not fit
  `prefix`/`name`, a link target is long or non-ASCII, the size is ≥ 8 GiB, or the
  mtime has a fraction or is out of range. Archives end with two zero blocks, padded
  to 10 KiB records like GNU tar. Owner and group are written as 0.
- `.tar.gz/.tgz`, `.tar.bz2/.tbz2`, `.tar.xz/.txz`, `.tar.zst/.tzst`: the whole TAR
  goes through the stream encoder (levels 1–9; Zstandard maps to 1–19 and uses all
  cores).
- `.gz/.bz2/.xz/.zst` alone hold exactly one file; gzip records its name and mtime.
- Updating re-encodes kept items. Output passes GNU tar `-t` (tests run it when present).

Not yet: sparse extraction, uid/gid/uname/gname restore, xattrs (`SCHILY.xattr`),
device creation, `--format` choice (gnu vs pax), LZ4/Brotli/Lzip/`.Z` streams,
parallel gzip/xz.
