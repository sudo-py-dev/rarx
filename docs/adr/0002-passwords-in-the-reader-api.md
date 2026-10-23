# 2. Passwords in the reader and update APIs

- Status: accepted
- Date: 2026-10-23

## Context

Slice 5 adds 7z AES-256 and ZIP ZipCrypto / WinZip AES (AGENT.md §8). A 7z archive
with an encrypted header cannot even be listed without the password, while item-level
encryption only matters when data is read. The engine must never hold passwords in
plain `String`s or print them.

## Decision

- `rarx_crypto::Password` (zeroized on drop, redacted `Debug`) is the only password
  type; `rarx-formats` and `rarx-core` re-export it.
- `rarx_formats::open` takes `password: Option<Password>`; `ArchiveReader` gains
  `set_password` (default: ignore) so callers can supply one after listing.
- `FormatError::WrongPassword` and `ItemError::WrongPassword` are new; extraction
  reclassifies checksum/data failures of encrypted items as wrong passwords, since most
  ciphers give no earlier signal.
- `rarx_core::Archive::open_with_password`, `Archive::set_password`,
  `Archive::has_encrypted_items`; `AddOptions` gains `password`, `encrypt_header` and
  `zip_cipher`; `delete` takes the password (7z rewrites must stay encrypted).
- `SevenZOptions` is no longer `Copy`/`Eq` (it holds a password).
- Prompting is the front end's job: the CLIs ask (without echo on a terminal) when an
  archive turns out to need a password, unless `-p-` was given.

## Consequences

Every caller of `rarx_formats::open` and `rarx_core::delete` changed. Password
normalisation to NFC and code-page encoding of ZipCrypto passwords (AGENT.md §8) are
not done yet: passwords are used as typed, UTF-8 for ZIP and UTF-16LE for 7z, which
matches 7-Zip for ASCII and for UTF-8 ZIP tools.
