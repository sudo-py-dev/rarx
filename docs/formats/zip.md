# ZIP

Implemented in `crates/rarx-formats/src/zip/` (`mod.rs` reader, `writer.rs` writer).

## Reading

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
- ZipCrypto: 12-byte header checked against the CRC's high byte (the DOS time's high
  byte when bit 3 is set); 1 wrong password in 256 passes and then fails the CRC,
  which is reported as "wrong password".
- WinZip AES (method 99, extra `0x9901`): AES-128/192/256, PBKDF2-HMAC-SHA1 (1000
  rounds), little-endian CTR from 1, 2-byte verifier, 10-byte HMAC-SHA1 checked at
  the end of the item (a mismatch reads as a CRC error). AE-2 items carry no CRC.
  Listed as e.g. `AES-256 Deflate` / `ZipCrypto Deflate`.
- PKWARE strong encryption (flag bit 6) is reported as unsupported.
- Not yet: multi-volume/split ZIP, Deflate64/BZip2/LZMA/Zstd/XZ/PPMd.

## Writing

- New items: local header written first, then patched with CRC and sizes (no data
  descriptors). Files ≤ 4 MiB are deflated in memory and stored instead when Deflate
  does not shrink them; larger files stream through Deflate.
- Fields: UTF-8 flag for non-ASCII names, DOS time in local time, UT extra (`0x5455`)
  with the exact UTC mtime, Unix mode in the external attributes (host = Unix) when
  known, Deflate level bits in the general-purpose flags.
- ZIP64 extra fields only where a value needs them (local header gets them up front when
  the file is ≥ ~4 GiB); ZIP64 end record + locator when entry count, directory size or
  offset need them. `set_force_zip64` forces all of this for tests.
- Updating copies untouched items byte for byte (`ZipReader::copy_raw`), including
  encrypted ones, rebuilding only the ZIP64 field and any data descriptor.
- Engine (`rarx-core/src/update.rs`) writes to a hidden temp file next to the target and
  renames it into place only on success.
- Encryption (`ZipWriter::set_encryption`): every item except folders.
  - AES-256: method 99 with the `0x9901` extra; AE-1 (CRC kept) unless the file is
    under 20 bytes, then AE-2 (CRC zeroed), as WinZip does. Needs version 5.1.
  - ZipCrypto: buffered items use the CRC check byte; streamed (> 4 MiB) items set
    bit 3, use the DOS-time check byte and append a data descriptor.
  - Cross-checked with the `zip` crate in both directions (AES-128/256, AE-1/AE-2,
    ZipCrypto) and with Info-ZIP `unzip -P` for ZipCrypto.
- Not yet: Deflate64/BZip2/LZMA/Zstd/PPMd methods, comments, split archives,
  7-Zip-grade optimal Deflate (we use zlib-rs levels).
