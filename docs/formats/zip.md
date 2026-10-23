# ZIP (read side)

Implemented in `crates/rarx-formats/src/zip/`.

- End record: last `PK\5\6` within the final 64 KiB + 22 bytes; ZIP64 locator directly
  before it switches to the ZIP64 end record (tried at the stated offset, then right
  before the locator).
- SFX / prepended data: `shift = end_of_directory - dir_size - dir_offset`, added to the
  directory and every local-header offset.
- Central directory is authoritative for names, sizes, CRC; local header only gives
  name/extra lengths to locate data (so data descriptors need no special handling).
- Names: UTF-8 if flag bit 11, else Info-ZIP Unicode Path (`0x7075`) if its CRC matches,
  else ASCII, else UTF-8 for Unix/macOS hosts when valid, else CP437.
- Times: NTFS (`0x000A`) > extended timestamp (`0x5455`, mtime only) > DOS (local time).
- Kind: Unix mode `S_IFLNK`/`S_IFDIR` for Unix/macOS hosts, else trailing `/` or DOS
  directory attribute.
- Methods: Store, Deflate. Others are listed by name and reported as unsupported.
- Not yet: multi-volume/split ZIP, encryption, Deflate64/BZip2/LZMA/Zstd/XZ/PPMd.
