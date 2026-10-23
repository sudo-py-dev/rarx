# AGENT.md — RarX

> **RarX** is a cross-platform (Linux, Windows, macOS) archive manager written in Rust that replicates
> the complete feature set of **WinRAR** and **7-Zip** (CLI + file manager + shell integration + SFX),
> with a single consistent engine, memory safety, and byte-for-byte format compatibility.
>
> This file is the **single source of truth** for any agent (human or AI) working on this repository.
> Read it fully before writing code. When this document and the code disagree, fix one of them in the
> same change — never leave them diverged.

---

## Table of contents

1. [Mission, goals and non-goals](#1-mission-goals-and-non-goals)
2. [Legal & licensing constraints (read first)](#2-legal--licensing-constraints-read-first)
3. [Workspace architecture](#3-workspace-architecture)
4. [Core engine abstractions](#4-core-engine-abstractions)
5. [Format support matrix](#5-format-support-matrix)
6. [Format specifications (writer-grade)](#6-format-specifications-writer-grade)
7. [Codecs & filters](#7-codecs--filters)
8. [Encryption & password handling](#8-encryption--password-handling)
9. [Hashing & integrity](#9-hashing--integrity)
10. [Archive operations (feature catalogue)](#10-archive-operations-feature-catalogue)
11. [File-system metadata fidelity](#11-file-system-metadata-fidelity)
12. [Multi-volume, solid, SFX, recovery](#12-multi-volume-solid-sfx-recovery)
13. [CLI — `rarx` (RAR-style) and `rx7` (7z-style)](#13-cli)
14. [GUI — file manager](#14-gui--file-manager)
15. [Dialogs (every one, every tab)](#15-dialogs-every-one-every-tab)
16. [Shell / desktop integration](#16-shell--desktop-integration)
17. [Settings, profiles, configuration files](#17-settings-profiles-configuration-files)
18. [Security requirements](#18-security-requirements)
19. [Performance requirements](#19-performance-requirements)
20. [Internationalisation & accessibility](#20-internationalisation--accessibility)
21. [Error handling, logging, exit codes](#21-error-handling-logging-exit-codes)
22. [Testing strategy & compatibility gates](#22-testing-strategy--compatibility-gates)
23. [Coding standards for agents](#23-coding-standards-for-agents)
24. [Milestones & definition of done](#24-milestones--definition-of-done)
25. [Master feature checklist](#25-master-feature-checklist)

---

## 1. Mission, goals and non-goals

### Goals

- **Feature parity** with WinRAR 7.x and 7-Zip 24.x/25.x — every command, switch, dialog option, and
  format listed in this document.
- **Interoperability**: archives produced by RarX must open in the reference tools (WinRAR, 7-Zip,
  Info-ZIP, bsdtar, GNU tar, xz-utils, zstd, etc.) and vice versa. Round-trip tests enforce this.
- **Safety**: no `unsafe` outside audited, isolated modules; hostile archives must never cause
  memory corruption, path traversal, or unbounded resource use.
- **Speed**: compression/decompression throughput within **±10%** of the reference implementation
  per codec on the same machine and thread count (see §19).
- **One engine, three front-ends**: library (`rarx-core`), CLI, GUI. The GUI never implements archive
  logic; it calls the same API the CLI does.
- **Cross-platform** with native integration on each OS (Explorer, Finder, Nautilus/Dolphin/Thunar/Nemo).

### Non-goals

- Re-implementing WinRAR's licence nag / trial expiry.
- Using or deriving from the proprietary **UnRAR source** (see §2).
- Running arbitrary code from archives (SFX setup programs only run on explicit user action, and only
  on the SFX side — never during extraction by RarX itself).

---

## 2. Legal & licensing constraints (read first)

| Topic | Rule |
|---|---|
| UnRAR source code (RARLAB) | **Must not be used, read, ported, or linked.** Its licence forbids using it to build a RAR-compatible archiver. All RAR decoding is a **clean-room** implementation from the public RAR 5.0 technote and independent documentation. |
| RAR **creation** (RAR5 writer) | Implemented behind the cargo feature `rar-write`, **off by default**. The format is documented publicly but the maintainers must obtain legal sign-off before enabling it in release builds. RAR4 creation is never implemented (WinRAR 7 itself dropped it). |
| 7-Zip code (LGPL-2.1 + unRAR restriction + BSD parts) | Do not copy code. Algorithms (LZMA, LZMA2, PPMd, BCJ2) are public-domain specs (LZMA SDK is public domain); porting from the **LZMA SDK** is allowed. |
| Third-party crates | Only licences compatible with **MIT OR Apache-2.0** distribution. `cargo deny` enforces this in CI. GPL-only crates are forbidden. |
| Trademarks | Product name is **RarX**. Never use "WinRAR", "7-Zip", their logos or icons in UI, binaries, or marketing. Compatibility statements ("opens RAR and 7z archives") are fine. |
| Patents | Deflate64, Implode, LZX, etc. are out of patent; still record origin of every algorithm in `docs/provenance.md`. |

---

## 3. Workspace architecture

Cargo workspace, edition 2024, MSRV pinned in `rust-toolchain.toml`.

```
rarx/
├─ AGENT.md                      ← this file
├─ Cargo.toml                    ← workspace
├─ deny.toml, rustfmt.toml, clippy.toml
├─ crates/
│  ├─ rarx-io/          Seekable/volume-spanning readers & writers, buffered I/O, mmap, temp files
│  ├─ rarx-checksum/    CRC32, CRC64, BLAKE2sp, BLAKE3, SHA-*, XXH*, MD5 … (SIMD dispatch)
│  ├─ rarx-crypto/      AES (CBC/CTR), ZipCrypto, KDFs, HMAC, zeroizing secrets, password store
│  ├─ rarx-codecs/      Pure codecs: lzma, lzma2, ppmd7/8, bzip2, deflate/64, zstd, lz4, brotli,
│  │                    lzx, quantum, xpress, rar15/20/29/50 unpack, rar50 pack, implode, shrink …
│  ├─ rarx-filters/     BCJ, BCJ2, ARM, ARMT, ARM64, PPC, SPARC, IA64, RISCV, Delta, RAR VM/filters
│  ├─ rarx-formats/     One module per container format (§5) implementing the `Format` traits
│  ├─ rarx-core/        Engine: operations, update planner, solid grouping, multithreading,
│  │                    progress, cancellation, path sanitising, metadata application
│  ├─ rarx-sfx/         SFX stub binaries (console + GUI) per platform, SFX script parser
│  ├─ rarx-cli/         `rarx` (WinRAR-style CLI) and `rx7` (7-Zip-style CLI) binaries
│  ├─ rarx-gui/         Desktop file manager (Slint UI), dialogs, viewer, themes
│  ├─ rarx-shell/       Explorer (COM IExplorerCommand), Finder Sync ext, Linux service menus
│  ├─ rarx-ffi/         Stable C ABI (`rarx.h`) + 7z-plugin-compatible codec/format DLL interface
│  └─ rarx-testkit/     Corpus loaders, reference-tool runners, fuzz harness helpers
├─ fuzz/                cargo-fuzz targets (one per format parser and per codec decoder)
├─ corpus/              Test archives (git-lfs), each with provenance + expected listing JSON
├─ i18n/                Fluent (.ftl) translation files
├─ docs/                Format notes, provenance.md, ADRs (docs/adr/NNNN-*.md)
└─ packaging/           MSI/MSIX, .dmg/.pkg, .deb/.rpm/Flatpak/AppImage, winget/homebrew manifests
```

### Dependency rules

- `rarx-codecs` and `rarx-filters` depend on nothing except `rarx-checksum` (pure, `no_std + alloc`
  capable behind a feature).
- `rarx-formats` depends on codecs/filters/crypto/io — **never** on `rarx-core`.
- `rarx-cli`, `rarx-gui`, `rarx-shell` depend on `rarx-core` only (plus UI crates). No format code in UI.
- Async runtime: **none** in the engine. Parallelism uses `rayon` + explicit worker threads with
  bounded channels (`crossbeam-channel`). GUI uses its own event loop and talks to the engine through a
  job queue.

### Recommended crates (verify licence + current version via Context7 / crates.io before adding)

`clap` (CLI parsing, but RAR-style switch parser is hand-written — see §13), `slint` (GUI),
`rayon`, `crossbeam-channel`, `memmap2`, `aes`, `cbc`, `ctr`, `sha1`, `sha2`, `sha3`, `hmac`,
`pbkdf2`, `blake2`, `blake3`, `md-5`, `crc32fast`, `xxhash-rust`, `zeroize`, `secrecy`,
`zstd` (or pure-Rust `ruzstd` for decode), `lz4_flex`, `brotli`, `flate2` with `zlib-rs` backend
(our own Deflate encoder is still required for 7-Zip-grade "Ultra" deflate), `filetime`, `xattr`,
`windows` (Win32/COM), `objc2` (macOS), `fluent`, `tracing`, `thiserror`, `anyhow` (bins only),
`proptest`, `insta`, `criterion`.

---

## 4. Core engine abstractions

```rust
/// A container format (7z, zip, rar, tar, iso…).
pub trait Format: Send + Sync {
    fn id(&self) -> FormatId;
    fn extensions(&self) -> &'static [&'static str];
    fn signatures(&self) -> &'static [Signature];          // offset + magic bytes
    fn capabilities(&self) -> FormatCaps;                   // READ|WRITE|UPDATE|ENCRYPT|HEADER_ENCRYPT|
                                                            // SOLID|MULTIVOLUME|SFX|COMMENT|RECOVERY|LOCK|…
    fn open(&self, src: VolumeSet, opts: &OpenOptions) -> Result<Box<dyn ArchiveReader>>;
    fn create(&self, dst: VolumeSink, opts: &CreateOptions) -> Result<Box<dyn ArchiveWriter>>;
}

pub trait ArchiveReader {
    fn properties(&self) -> &ArchiveProps;                  // type, solid, volumes, comment, locked, RR %, …
    fn entries(&self) -> EntryIter<'_>;                     // lazily streamed for huge archives
    fn extract(&mut self, sel: &Selection, sink: &mut dyn ExtractSink, ctl: &JobControl) -> Result<Report>;
    fn test(&mut self, sel: &Selection, ctl: &JobControl) -> Result<Report>;
    fn open_nested(&mut self, idx: EntryIndex) -> Result<Option<Box<dyn ArchiveReader>>>; // archive-in-archive
}

pub trait ArchiveWriter {
    fn add(&mut self, item: SourceItem, opts: &ItemOptions) -> Result<()>;
    fn copy_from(&mut self, old: &mut dyn ArchiveReader, idx: EntryIndex) -> Result<()>; // update w/o recompress
    fn set_comment(&mut self, c: Option<&str>) -> Result<()>;
    fn finish(self: Box<Self>, ctl: &JobControl) -> Result<Report>;
}
```

### Engine services (all in `rarx-core`)

- **Format detection**: by signature (scanning first 1 MiB + tail for EOCD/SFX payloads), then by
  extension, then by "open as" user choice. Supports `-t` type forcing and `-t#` "raw parse" (7-Zip `#`
  handler: find embedded archives at any offset).
- **VolumeSet**: transparent spanning across `.partN.rar`, `.rNN`, `.7z.001`, `.zNN/.zip`, `.001`
  generic split; asks the user (callback) for missing volumes; supports "removable media" prompts.
- **Update planner**: computes the action per file for `add/update/freshen/sync/move/delete` from
  (disk item, archive item, update policy) — exact replication of 7-Zip's `-u` state machine
  (states `p q r x y z w`, actions `0 1 2 3`) and RAR's `a/u/f` + `-as` semantics.
- **Job model**: every operation is a `Job` with `JobControl` (pause/resume/cancel, priority,
  background mode, thread cap, memory cap), emitting `ProgressEvent`s (bytes in/out, ratio, current
  file, ETA, speed, errors). CLI and GUI subscribe to the same events.
- **Callbacks**: `AskPassword`, `AskOverwrite`, `AskNextVolume`, `AskContinueOnError`, `ShowWarning` —
  front-ends implement these; non-interactive mode resolves them from switches.
- **Path sanitiser** (see §18) applied to **every** extracted path, no opt-out except the explicit
  "absolute paths" switch which still blocks `..` traversal.
- **Temp & atomic writes**: archives are built in a temp file in the destination folder (or the
  configured temp folder, `-w`) and atomically renamed; the original is kept until success.

---

## 5. Format support matrix

Legend: **R** read/list/extract/test · **W** create · **U** update in place (add/delete/rename) ·
**E** encryption · **H** header (filename) encryption · **S** solid · **V** multi-volume · **X** SFX ·
**C** comment · **RR** recovery record.

| Format | Ext. | R | W | U | E | H | S | V | X | C | RR | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **7z** | .7z | ✔ | ✔ | ✔ | ✔ AES-256 | ✔ | ✔ | ✔ | ✔ | ✔¹ | – | All coders/filters §7 |
| **RAR5** (incl. RAR 7 large-dict) | .rar | ✔ | ✔² | ✔² | ✔ AES-256 | ✔ | ✔ | ✔ | ✔ | ✔ | ✔ | Clean-room |
| **RAR 1.5–4.x** | .rar, .rNN | ✔ | ✘ | ✘ | ✔ (read: AES-128, RAR2 cipher, RAR1.5 cipher) | ✔ | ✔ | ✔ | ✔ (read) | ✔ | ✔ (read/repair) | |
| **ZIP / ZIP64** | .zip, .zipx, .jar, .apk, .xpi, .odt, .docx … | ✔ | ✔ | ✔ | ✔ ZipCrypto, AES-128/192/256 | ✘ | ✘ | ✔ split/spanned | ✔ | ✔ | – | |
| **TAR** (v7, ustar, GNU, pax, star, sparse) | .tar | ✔ | ✔ | ✔ | – | – | – | – | – | – | – | Long names, pax headers |
| **GZIP** | .gz, .tgz, .taz | ✔ | ✔ | – | – | – | – | – | – | – | – | multi-member |
| **BZIP2** | .bz2, .tbz2, .tbz | ✔ | ✔ | – | – | – | – | – | – | – | – | multi-stream, MT |
| **XZ** | .xz, .txz | ✔ | ✔ | – | – | – | – | – | – | – | – | multi-block MT, index, all filters |
| **LZMA (alone)** | .lzma | ✔ | ✔ | – | – | – | – | – | – | – | – | |
| **LZMA86** | .lzma86 | ✔ | ✔ | – | – | – | – | – | – | – | – | |
| **Zstandard** | .zst, .tzst | ✔ | ✔ | – | – | – | – | – | – | – | – | long-range, dictionaries, skippable frames |
| **LZ4** | .lz4 | ✔ | ✔ | – | – | – | – | – | – | – | – | frame + legacy |
| **Brotli** | .br | ✔ | ✔ | – | – | – | – | – | – | – | – | |
| **Lzip** | .lz | ✔ | ✔ | – | – | – | – | – | – | – | – | |
| **Lzop** | .lzo | ✔ | – | – | – | – | – | – | – | – | – | |
| **Unix compress** | .Z | ✔ | ✔ | – | – | – | – | – | – | – | – | LZW |
| **WIM** | .wim, .swm, .esd | ✔ | ✔ | ✔ | – | – | ✔ | ✔ | – | – | – | XPRESS, LZX, LZMS |
| **CAB** | .cab | ✔ | – | – | – | – | ✔ | ✔ | – | – | – | MSZIP, Quantum, LZX |
| **ARJ** | .arj | ✔ | – | – | – | – | – | ✔ | – | ✔ | – | methods 1–4 |
| **LZH/LHA** | .lzh, .lha | ✔ | – | – | – | – | – | – | – | – | – | lh0–lh7, lzs, lz5 |
| **ACE** | .ace | ✔ (opt-in feature `ace`) | – | – | – | – | ✔ | ✔ | – | ✔ | – | v1/v2, clean-room |
| **ISO 9660** (+Joliet, Rock Ridge, El Torito) | .iso | ✔ | ✔ | – | – | – | – | – | – | – | – | |
| **UDF** (1.02–2.60) | .iso, .udf | ✔ | – | – | – | – | – | – | – | – | – | |
| **CPIO** (bin, odc, newc, crc) | .cpio | ✔ | ✔ | – | – | – | – | – | – | – | – | |
| **AR / a / lib / deb** | .ar, .a, .lib, .deb | ✔ | ✔ (ar) | – | – | – | – | – | – | – | – | |
| **RPM** | .rpm | ✔ | – | – | – | – | – | – | – | – | – | payload cpio + any compressor |
| **XAR / PKG** | .xar, .pkg | ✔ | – | – | – | – | – | – | – | – | – | |
| **DMG** (UDIF: zlib, bzip2, ADC, LZFSE, LZMA) | .dmg | ✔ | – | – | – | – | – | – | – | – | – | |
| **HFS / HFS+** | .hfs, .hfsx | ✔ | – | – | – | – | – | – | – | – | – | incl. compressed files |
| **APFS** | .apfs | ✔ | – | – | – | – | – | – | – | – | – | read-only, unencrypted |
| **NTFS image** | .ntfs, .img | ✔ | – | – | – | – | – | – | – | – | – | ADS, compressed attrs |
| **FAT12/16/32, exFAT image** | .fat, .img | ✔ | – | – | – | – | – | – | – | – | – | |
| **ext2/3/4 image** | .ext, .img | ✔ | – | – | – | – | – | – | – | – | – | |
| **SquashFS** | .squashfs | ✔ | – | – | – | – | – | – | – | – | – | gzip/lzma/lzo/xz/lz4/zstd |
| **CramFS** | .cramfs | ✔ | – | – | – | – | – | – | – | – | – | |
| **MBR / GPT** partition tables | .mbr, .gpt, .img | ✔ | – | – | – | – | – | – | – | – | – | partitions as entries |
| **VHD / VHDX / VMDK / VDI / QCOW2** | | ✔ | – | – | – | – | – | – | – | – | – | expose raw disk → nested open |
| **CHM** | .chm | ✔ | – | – | – | – | – | – | – | – | – | LZX |
| **MSI / Compound File (CFB/OLE)** | .msi, .msp, .doc, .xls | ✔ | – | – | – | – | – | – | – | – | – | |
| **NSIS installer** | .exe | ✔ | – | – | – | – | ✔ | – | – | – | – | zlib/bzip2/lzma, script decompile to .nsi |
| **Inno Setup** (read-only extraction of data) | .exe | ✔ (opt-in) | – | – | – | – | – | – | – | – | – | |
| **PE / ELF / Mach-O / TE** | .exe, .dll, .so, .dylib | ✔ | – | – | – | – | – | – | – | – | – | sections, resources, version info |
| **UEFI capsule / firmware volume** | .scap, .bin | ✔ | – | – | – | – | – | – | – | – | – | |
| **SWF / FLV** | .swf, .flv | ✔ | – | – | – | – | – | – | – | – | – | streams as entries |
| **MSLZ** (`COMPRESS.EXE`) | .??_ | ✔ | – | – | – | – | – | – | – | – | – | |
| **Split** (generic) | .001 | ✔ | ✔ | – | – | – | – | ✔ | – | – | – | 7-Zip "split" handler |
| **Hash files** | .sha256, .sha1, .md5, .sfv, .b3 … | ✔ (verify) | ✔ | – | – | – | – | – | – | – | – | 7-Zip "Hash" handler |
| **Base64 / uuencode / xxencode / yEnc** | .uue, .b64, .xxe, .yenc | ✔ | – | – | – | – | – | – | – | – | – | WinRAR supports UUE/XXE |
| **Zipx extras** (JPEG/WavPack recompression) | .zipx | ✔ (opt-in) | – | – | – | – | – | – | – | – | – | detect and report unsupported if disabled |
| **GZ/BZ2/XZ/ZST wrapped TAR** | .tar.gz etc. | ✔ | ✔ | – | – | – | – | – | – | – | – | "one-step" open as a single tree (WinRAR behaviour) |

¹ 7z archive comment via `kComment` (supported by spec; few tools write it — we write only when asked).
² Behind feature `rar-write` (§2).

**Unsupported input** must produce a precise message: "Unsupported compression method `<name>` in
`<file>`" — never a generic "corrupt archive".

---

## 6. Format specifications (writer-grade)

Each format module must ship `docs/formats/<fmt>.md` with field-level notes. Minimum requirements:

### 6.1 7z

- Signature `37 7A BC AF 27 1C`, version `0.4`; 32-byte start header (StartHeaderCRC, NextHeaderOffset,
  NextHeaderSize, NextHeaderCRC).
- Full property-ID set: `kEnd 0x00` … `kDummy 0x19` (Header, ArchiveProperties, AdditionalStreamsInfo,
  MainStreamsInfo, FilesInfo, PackInfo, UnPackInfo, SubStreamsInfo, Size, CRC, Folder,
  CodersUnPackSize, NumUnPackStream, EmptyStream, EmptyFile, Anti, Name, CTime, ATime, MTime,
  WinAttributes, Comment, EncodedHeader, StartPos, Dummy). Unknown properties: skip by size.
- Folders with **multiple coders and bind pairs** (e.g. BCJ2 with 4 in-streams + LZMA ×3).
- **Encoded (compressed) header** (LZMA by default), **encrypted header** (`-mhe=on`).
- **Header compression** options: `-mhc=on|off`, `-mhcf` (filter for header).
- **Anti-items** (deletion markers for incremental update archives).
- **Empty streams / empty files / directories**, `kStartPos`, `kDummy` alignment padding.
- Attributes: Windows attrs + high 16 bits Unix mode (`0x8000` flag `FILE_ATTRIBUTE_UNIX_EXTENSION`).
- Timestamps: 100-ns FILETIME for M/C/A time; `-mtm -mtc -mta -mtr` switches control storage.
- Solid block rules: `-ms=on|off|<N>f<M>b<K>e` (files per block, bytes per block, per-extension).
- File ordering for solid: sort by extension then name (7-Zip `qs` mode `-mqs`), or by type analysis.
- **Multithreaded** LZMA2 block encoding and parallel multi-folder decoding.
- Update without recompressing unchanged folders; repack only affected solid blocks.
- SFX: stub + archive; reading finds signature at any offset ≤ configurable search limit.
- Method IDs (hex): Copy `00`, Delta `03`, x86 BCJ `03030103`, BCJ2 `0303011B`, PPC `03030205`,
  IA64 `03030401`, ARM `03030501`, ARMT `03030701`, SPARC `03030805`, ARM64 `0A`, RISCV `0B`,
  LZMA `030101`, LZMA2 `21`, PPMD `030401`, BZip2 `040202`, Deflate `040108`, Deflate64 `040109`,
  AES-256+SHA-256 `06F10701`, Zstd `04F71101`, Brotli `04F71102`, LZ4 `04F71104`, LZ5/Lizard read-only.
  Unknown IDs → listed with hex method name, extraction reports "unsupported method".

### 6.2 RAR5 / RAR 7

- Signature `52 61 72 21 1A 07 01 00`. vint encoding. Header CRC32 on every block.
- Header types: Main (1), File (2), Service (3), Archive encryption (4), End of archive (5).
- Main header flags: volume, volume number present, solid, recovery record present, locked.
  Extra: locator record (quick open offset, recovery record offset), metadata record (original
  archive name and creation time — RAR 7 `-ams`).
- File/service extra records: Encryption (1), Hash/BLAKE2sp (2), High-precision time (3, Unix ns
  or Windows FILETIME), Version (4, file versioning `-ver`), Redirection (5: Unix symlink, Windows
  symlink, junction, hardlink, file copy/dedup), Unix owner (6), Service data (7).
- Service headers: `CMT` (comment), `QO` (quick open), `ACL` (NTFS ACL), `STM` (NTFS alternate
  stream), `RR` (recovery record).
- Compression: RAR5 LZ + Huffman with dictionary **128 KiB – 4 GiB** (RAR5) and up to **64 GiB**
  (RAR 7 extended, must refuse gracefully if memory cap is lower and report needed memory).
  Filters: DELTA, E8, E8E9, ARM. Compression methods 0–5 (store, fastest … best).
- Encryption: AES-256-CBC, PBKDF2-HMAC-SHA256 with `2^Count` iterations (default Count=15),
  salt 16 bytes, IV per file, 12-byte password check value (+4 byte checksum), hash MAC conversion
  of CRC/BLAKE2 when encrypted. Header encryption with archive encryption header.
- Recovery record: Reed–Solomon, size in % (`-rr5%`) or in records, applicable per volume; recovery
  volumes `.rev` (RAR5 RS over volumes).
- Volumes: `name.partN.rar`; old style `name.rar, .r00, .r01…` for RAR4 read.
- Locking (`k` command) sets the lock flag; locked archives refuse modification in all front-ends.
- Quick open record: duplicate copy of file headers near the end for fast listing.

### 6.3 RAR 1.5 – 4.x (read only)

- Signature `52 61 72 21 1A 07 00`; block types `0x72–0x7B` (marker, archive, file, old comment,
  old auth, old subblock, old recovery, old auth, new subblock, end).
- Unpack algorithms: v1.5, v2.0 (incl. audio/multimedia compression), v2.9/3.x (LZ + PPMd
  variant H + **RarVM standard filters** — recognise by CRC: E8, E8E9, ITANIUM, DELTA, RGB, AUDIO;
  custom VM bytecode is **not executed**; report unsupported).
- Encryption: RAR 1.5 cipher, RAR 2.0 cipher, RAR 3.x AES-128 with SHA-1-based KDF (2^18 rounds).
- Unicode name encoding (compressed high-byte scheme), extended time fields, NTFS streams/ACL
  subblocks, old & new recovery records (repair support), `.rNN` volumes, old SFX modules.

### 6.4 ZIP

- Records: Local header `PK\3\4`, Central dir `PK\1\2`, EOCD `PK\5\6`, ZIP64 EOCD `PK\6\6`, ZIP64
  locator `PK\6\7`, data descriptor `PK\7\8` (with/without signature), split marker `PK\7\8`/`PK00`.
- Methods: Store 0, Shrink 1, Reduce 2–5, Implode 6, Deflate 8, Deflate64 9, BZip2 12, LZMA 14,
  Zstd 93 (and legacy 20), MP3 94 (read-optional), XZ 95, JPEG 96 (read-optional), WavPack 97
  (read-optional), PPMd I r1 98, AES marker 99.
- Extra fields: ZIP64 `0x0001`, NTFS times `0x000A`, Unix `0x000D`, Extended timestamp `0x5455`,
  Info-ZIP Unix UID/GID `0x7875`, Info-ZIP Unicode Path `0x7075`, Unicode Comment `0x6375`,
  WinZip AES `0x9901`, alignment padding `0xD935` (Android). Unknown → preserved on update.
- Encodings: UTF-8 flag (bit 11), OEM code page (CP437/CP850/CP866/… by locale), ANSI; `-mcp=<cp>`
  and `-scs` overrides; auto-detect heuristics with user override in GUI ("Name encoding").
- Encryption: ZipCrypto (traditional PKWARE), WinZip AE-1/AE-2 (PBKDF2-HMAC-SHA1 1000 iters,
  AES-CTR LE counter, 10-byte HMAC-SHA1, 2-byte verifier). Strong Encryption (PKWARE SES) → detect
  and report unsupported.
- Split (`.z01 … .zip`) and spanned archives; ZIP64 auto when >65535 entries or >4 GiB.
- Archive comment (≤65535 bytes) + per-file comments (GUI shows and edits both).
- Self-extracting ZIP (offsets adjusted, `-sfx`).
- Multithreaded deflate (per-file parallel; per-block for single huge file at the cost of ratio).
- Directory entries, empty files, symlinks (Unix mode `0xA000`), zero-length streams.

### 6.5 TAR family

- v7, ustar, GNU (long names `L`/`K`, sparse `S`, multi-volume `M`), pax (`x`/`g`, `path`,
  `linkpath`, `size`, `uid/gid/uname/gname`, `mtime` sub-second, `SCHILY.xattr.*`, `LIBARCHIVE.*`,
  `GNU.sparse.*` 0.0/0.1/1.0), star. Hardlinks, symlinks, devices, FIFOs (extract only on Unix).
- Writer formats selectable: `-mf=gnu|pax|posix|ustar`; pax by default when needed.
- Compressed tar: single-step "open as tree" with streaming (no temp file) when possible.

### 6.6 Other formats

Every read-only format in §5 must:
1. List entries with name, size, packed size, times, attributes, method, CRC where available.
2. Extract and test (CRC verify where the format has it).
3. Support nested open (e.g. `.deb` → `data.tar.xz` → tree).
4. Pass its fuzz target for 24 CPU-hours without crash before release.

---

## 7. Codecs & filters

### 7.1 Compression codecs

| Codec | Encode | Decode | Levels / key params | MT |
|---|---|---|---|---|
| Copy/Store | ✔ | ✔ | – | – |
| LZMA | ✔ | ✔ | `d` dict 4 KiB–1.5 GiB (3.8 GiB on 64-bit), `lc lp pb`, `fb` 5–273, `mc`, `mf` bt2/bt3/bt4/hc4/hc5, `a` 0/1 | 2 (match finder) |
| LZMA2 | ✔ | ✔ | as LZMA + `c` chunk size, block size | N (blocks) |
| PPMd var. H (7z) | ✔ | ✔ | `o` order 2–32, `mem` 2^11..2^32-? | – |
| PPMd var. I rev.1 (zip) | ✔ | ✔ | `o`, `mem`, restore method | – |
| BZip2 | ✔ | ✔ | block 100–900 KB, `pass` | N |
| Deflate | ✔ (7-Zip-grade optimal parsing) | ✔ | `fb` 3–258, `pass` 1–15, `mc` | per-file |
| Deflate64 | ✔ | ✔ | as Deflate | per-file |
| Zstandard | ✔ | ✔ | level 1–22 (+ negative fast), `long`, window log, dictionaries | N |
| LZ4 | ✔ | ✔ | fast/HC levels | N |
| Brotli | ✔ | ✔ | 0–11, window 10–24 | – |
| Lzip | ✔ | ✔ | LZMA-based | – |
| LZW (`.Z`) | ✔ | ✔ | 9–16 bits | – |
| RAR 1.5 / 2.0 / 2.9 / 5.0 / 7.0 | ✔ RAR5/7 only² | ✔ all | dict, method 0–5 | ✔ |
| Implode / Shrink / Reduce | – | ✔ | | |
| LZX (CAB/CHM/WIM) | ✔ (WIM) | ✔ | window 15–21 (CAB) | |
| Quantum (CAB) | – | ✔ | | |
| MSZIP | – | ✔ | | |
| XPRESS / XPRESS-Huffman (WIM, NTFS) | ✔ | ✔ | | |
| LZMS (WIM/ESD) | ✔ | ✔ | | |
| LZFSE / LZVN (DMG/APFS) | – | ✔ | | |
| ADC (DMG) | – | ✔ | | |
| LZO (lzop, squashfs) | – | ✔ | | |
| ARJ m1–m4, LH0–LH7, ACE LZ77/V2 | – | ✔ | | |

### 7.2 Filters (pre-processing)

| Filter | Encode | Decode | Notes |
|---|---|---|---|
| BCJ x86 | ✔ | ✔ | auto for executables (`-mf=on`) |
| BCJ2 | ✔ | ✔ | 4-stream, used with LZMA in 7z "exe" profile |
| ARM, ARMT (Thumb), ARM64, PPC, SPARC, IA64, RISCV | ✔ | ✔ | auto-selected by PE/ELF/Mach-O machine type |
| Delta (1–256) | ✔ | ✔ | auto for WAV/BMP (7z `-mf` analysis) |
| RAR5 DELTA/E8/E8E9/ARM | ✔² | ✔ | |
| RAR3 standard VM filters (E8, E8E9, ITANIUM, DELTA, RGB, AUDIO) | – | ✔ | recognised by fingerprint |
| Swap2/Swap4 | ✔ | ✔ | 7-Zip `swap` filters |

**Automatic filter selection** (`-mf=on`, 7-Zip "analysis"): inspect first bytes of each file
(PE/ELF/Mach-O headers, WAV, BMP) and group files by filter in separate solid blocks.

### 7.3 Codec conformance

- Every decoder: fuzzed, bounded memory (`max_dict` from memory cap), streaming (no full-file buffer).
- Every encoder: round-trip property tests + decode by the reference tool.
- Output of our LZMA/LZMA2/PPMd/Deflate encoders at equal settings should be **within 0.5%** of
  7-Zip's compressed size; exact byte equality is not required.

---

## 8. Encryption & password handling

| Format | Algorithm | KDF |
|---|---|---|
| 7z | AES-256-CBC | SHA-256 iterated `2^NumCyclesPower` (default 19) over salt ‖ UTF-16LE password ‖ 64-bit counter |
| RAR5 | AES-256-CBC | PBKDF2-HMAC-SHA256, `2^15` default, salt 16 B, check value |
| RAR3/4 | AES-128-CBC | SHA-1 based, 2^18 iterations |
| RAR 2.0 / 1.5 | proprietary stream ciphers | read only |
| ZIP | ZipCrypto | – (read+write, warn "weak" in UI) |
| ZIP | WinZip AES-128/192/256 (AE-1, AE-2) | PBKDF2-HMAC-SHA1, 1000 |

Requirements:

- **Header encryption**: 7z `-mhe=on`, RAR `-hp`. Listing such archive prompts for password first.
- **Wrong password detection**: 7z (CRC failure after decode / LZMA error → "Wrong password?"),
  RAR5 (check value, immediate), ZIP AES (verifier), ZipCrypto (1-byte check; warn on false positive).
- **Password manager** ("Organize passwords" in WinRAR): store named passwords encrypted at rest with
  OS keychain (Windows DPAPI / Credential Manager, macOS Keychain, Linux Secret Service). Optional
  "try saved passwords automatically on extraction".
- **Default password** for a session ("Set default password", WinRAR `Ctrl+P`), cleared on exit.
- Passwords held in `secrecy::SecretString` + `zeroize`; never logged, never in argv of child
  processes, never in crash dumps (`-p` on CLI is accepted but `-p` with no value prompts with echo off).
- Unicode passwords: normalise to NFC; ZIP legacy passwords encoded per code page with UTF-8 fallback.
- "Show password" toggle, "Enter password twice for verification", "Encrypt file names" checkbox,
  "ZIP legacy encryption" checkbox, encryption method combo (ZipCrypto / AES-256 for ZIP).
- Encrypted archives refuse to be "repaired" into unencrypted ones silently.

---

## 9. Hashing & integrity

- Archive-native checksums: CRC32 (7z/zip/rar4), BLAKE2sp (RAR5 optional `-htb`), CRC32 (RAR5 `-htc`
  default).
- **Hash calculator** (7-Zip `h` command / Explorer "CRC SHA" submenu / WinRAR "Show information"):
  CRC32, CRC64, XXH32, XXH64, XXH3-64, XXH3-128, MD5, SHA-1, SHA-224, SHA-256, SHA-384, SHA-512,
  SHA3-256, BLAKE2sp, BLAKE3, `*` (all). Output: per-file, per-folder sum, data+names hash
  (7-Zip semantics), copy to clipboard, save to `.sha256` etc.
- **Hash-file verify**: open `.sha256/.md5/.sfv/...` lists, verify all, report missing/mismatch.
- SIMD dispatch (SSE4.2/AVX2/AVX-512/SHA-NI/ARMv8 crypto/CRC) at runtime with scalar fallback;
  identical results asserted in tests.

---

## 10. Archive operations (feature catalogue)

Every operation below must be available from **CLI, GUI and shell menu** (unless noted).

### 10.1 Create / add

- Create new archive from files/folders/wildcards/list files (`@listfile`), recursive or not.
- Archive formats selectable: 7z, RAR5², ZIP, TAR, GZ, BZ2, XZ, ZST, LZ4, BR, LZ, WIM, ISO, CPIO, Z.
- Compression level presets: **Store, Fastest, Fast, Normal, Good/Maximum, Best/Ultra** — mapping
  table per format kept in `rarx-core/src/presets.rs` and mirrored from 7-Zip/WinRAR defaults.
- Method, dictionary size, word size/fast bytes, solid block size, threads, memory usage estimate
  shown live ("Memory for compressing / decompressing").
- **Add to "name.ext"** quick action; **Add to archive…** full dialog; **Compress and email**.
- Archive each file/folder to a **separate archive** (WinRAR "Put each file to separate archive",
  7-Zip "Create separate archives").
- **Delete files after archiving** (to recycle bin optional) / **Move to archive** (`m`, `-sdel`).
- **Test archived files** after creation; delete source only if the test passes.
- **Lock archive**, **add recovery record**, **create SFX**, **create solid**, **add comment**.
- **Archive name generation by mask** (`-ag`, e.g. `backup-YYYYMMDD-HHMMSS`) — full WinRAR mask syntax
  (`Y M D H I S W A` + quoted text, `+` prefix to append).
- **File paths mode**: store relative paths (default), store full paths (`-ep3` incl. drive),
  no paths (`-ep`), exclude base folder (`-ep1`), absolute (`-spf` 7z / `-ep3`), path prefix inside
  archive (`-ap`).
- **Exclusions/inclusions**: wildcard masks, regex optional, recursive masks, exclude by attributes,
  by size (`-sl/-sm`), by time (`-ta/-tb/-tn/-to`), by archive bit (`-ao`), empty folders (`-ed`),
  exclude mask lists (`-x@`).
- **Store without compressing** extensions list (WinRAR "Files to store without compression",
  default `*.rar *.zip *.cab *.7z *.ace *.arj *.bz2 *.gz *.lha *.lzh *.taz *.tgz *.xz *.txz *.zst *.jpg *.png *.mp3 *.mp4 *.mkv …`).
- **Solid sorting/grouping** list (WinRAR `rarfiles.lst` order; 7-Zip sort by type `-mqs`).
- **Identical files** detection (RAR5 `-oi`: store as references when ≥ min size), **hardlinks** stored as links (`-oh`/`-snh`), **symlinks** stored as links (`-ol`/`-snl`).
- **Open shared/locked files** (Windows VSS snapshot `-ssw`/"Open shared files"); otherwise skip with warning.
- **Clear archive attribute** after archiving (`-ac`), archive only files with attribute set (`-ao`).
- **Process files from stdin** (`-si`) and **write archive to stdout** (`-so`) for stream formats.
- **Background / low-priority** mode (`-ri`, GUI "Background archiving"), **wait if other copies
  active** (`-ri` / WinRAR "Wait if other WinRAR copies are active"), **shut down / sleep / hibernate
  when done** (WinRAR `-ioff`), **turn off the PC** option in progress dialog.
- **Archive time** set to latest file time (`-tl`), keep original archive time (`-tk`).
- **Compression profiles** (save/load named setting sets, default profile, profile for context menu).

### 10.2 Update

- `update` (add new + replace older), `freshen` (replace older only), `synchronize` (add/update + delete
  archived files missing on disk), `add & replace`, `add & skip existing`.
- 7-Zip `-u` policy syntax fully supported: `-u[-][p#][q#][r#][x#][y#][z#][w#][!newArchive]`.
- Update-copy mode: write changed archive to a different name, keep original (`!newArchive`).
- **Incremental / differential backups**: archive bit logic, `-ac -ao`, file versions (`-ver[N]`, RAR5
  stores old versions as `name;N`), anti-items in 7z.
- Solid archive update: re-pack only affected solid blocks; warn about cost.

### 10.3 Extract

- **Extract files…** (dialog), **Extract here**, **Extract to "<name>\"**, **Extract each archive to
  separate folder**, **Smart extract** ("Extract here" that creates a folder only if the archive has
  more than one root item — WinRAR "Remove redundant folders from extraction path").
- With full paths (`x`), without paths (`e`), relative to a sub-path inside the archive (`-ap`, "extract
  from folder"), absolute paths (`-ep3`/`-spf`, dangerous; behind confirmation).
- **Overwrite modes**: ask, overwrite all (`-o+`/`-aoa`), skip existing (`-o-`/`-aos`), rename extracted
  (`-or`/`-aou`), rename existing (`-aot`), update (newer only, `-u`), freshen (existing only, `-f`).
  Ask dialog offers: Yes, Yes to all, No, No to all, Rename (manual), Auto-rename, Cancel; shows size
  and date of both files.
- **Keep broken files** (`-kb`): keep partially extracted files after CRC error.
- **Display files in Explorer** after extraction; **Delete archive after extraction**
  (never / ask / always, applies to all volumes).
- **Set file times**: modification / creation / last access, each "do not set / set / set original"
  (`-tsm -tsc -tsa`).
- **Set file security** (NTFS ACLs, `-oc`... RAR `-ow`), **owners** (Unix uid/gid when root),
  **alternate streams** (`-os`/`-sns`), **extended attributes** (`-sxattr`), **file attributes**
  (Windows: archive, hidden, readonly, system; Unix permissions with umask unless `-slp`-like
  preserve option).
- **Mark of the Web propagation** (Windows): copy `Zone.Identifier` ADS from archive to extracted
  files — modes: never, for Office/executable types, for all files (7-Zip `-snz`, WinRAR setting).
- **Symlinks / hardlinks / junctions**: create links (with safety checks §18) or extract target copies;
  Windows: fall back to copy if no privilege; option "Allow absolute paths in symbolic links".
- **Extract to stdout** (`p` / `-so`), **pipe from stdin** (`-si` for stream formats).
- **Nested archives**: "Extract archives inside archive" option (unpack `.tar.gz` in one step),
  configurable recursion depth.
- **Selective extraction** by mask, list file, sub-folder, index; **exclude** masks.
- Free disk space check before extraction (compare with total unpacked size; warn).
- Extraction of **multi-volume** sets starting from any volume (auto-locate first volume).
- Long path support (Windows `\\?\` prefix), invalid name sanitisation per target FS (§18).

### 10.4 Test

- Test integrity of whole archive or selection; test all volumes; report per-file status; summary
  with error count; test after creation; test SFX; test encrypted files (needs password).

### 10.5 List / view / info

- Technical listing (`-slt`, `lt`, `vta`): all properties per entry.
- Bare listing (`lb`/`vb`), verbose (`v`), JSON listing (`--json`, RarX extension for scripting).
- **Archive information dialog**: type, version, solid, SFX, lock, recovery record %, volumes count,
  total/packed size, ratio, dictionary, files/folders count, encryption, comment, BLAKE2/CRC, created
  by host OS, physical size, offset, warnings ("Headers error", "Unexpected end of archive",
  "There are data after the end of archive"). Ratio bar visualisation.
- **Internal viewer**: text (ANSI/OEM/UTF-8/UTF-16 auto + manual), hex, image preview, word-wrap,
  find, print. **External viewer/editor** configurable; **edit file in archive and update** on save
  (watch temp file; prompt "File modified, update in archive?").
- **Find files** (WinRAR Alt+F7): search by name mask, containing text (case, whole words, hex,
  encodings ANSI/UTF-8/UTF-16), inside archives (incl. nested), in disks/folders/archive types, skip
  encrypted, save results to list.

### 10.6 Modify archive contents

- Delete (`d`), rename (`rn`, incl. pattern pairs), move inside archive (drag-and-drop in GUI),
  create folder in archive, add files to sub-folder (drop into folder), change comment (`c`, `cw` to
  write comment to file), change per-file comment (zip), change attributes (WinRAR "Edit time and
  attributes"? → set attributes/times on archived entries for formats that support in-place
  metadata rewrite).
- **Convert archives** (WinRAR "Convert archives"): any readable format → any writable format,
  batch, keep/delete originals, recompress, encrypt, SFX, convert SFX ↔ archive, interactive mode.
- **Repair** (`r`, `rc`): rebuild from recovery record (RAR), reconstruct missing volumes from `.rev`
  (`rc`), "Treat corrupt archive as RAR/ZIP" option, scan for headers and rebuild 7z/zip directory
  (best-effort salvage: "rebuilt.<name>").
- **Lock** archive (`k`), **Protect** (add recovery record `rr`), create recovery volumes (`rv`).
- **SFX**: convert archive to SFX (`s`), remove SFX module (`s-`), replace SFX module, SFX options
  (§12.4).
- **Benchmark** (§19.3), **Estimate** (WinRAR "Estimate" — predicts ratio and time by sampling).
- **Split** any file into parts (7-Zip Alt+F5) with size presets (10M, 100M, 640M CD, 700M CD,
  4092M FAT, 4480M DVD, 8128M DVD DL, 23040M BD, custom with units b/k/m/g) and **Combine** parts.
- **Virus scan** archive contents via configurable scanner (Windows AMSI / command line template with
  `%f` placeholder), "Scan archive for viruses" command.
- **Generate report** (WinRAR): HTML / plain text / CSV listing of archive contents with selectable
  columns, archive totals, sorting.
- **Email archive** (Compress and email): create archive then open default mail client with attachment
  (MAPI on Windows, `xdg-email` Linux, `NSSharingService` macOS).
- **Copy full names to clipboard**, **copy archive to clipboard**, **paste files into archive**
  (clipboard paste adds), **drag-and-drop out** (lazy extraction to temp, delayed rendering on Windows).

---

## 11. File-system metadata fidelity

| Metadata | Windows | Linux/BSD | macOS | Formats storing it |
|---|---|---|---|---|
| mtime (ns) | ✔ | ✔ | ✔ | all |
| ctime/birth time | ✔ | btime when available | ✔ | 7z, RAR5, zip(NTFS/0x5455), pax |
| atime | ✔ | ✔ | ✔ | 7z, RAR5, zip, pax |
| DOS attributes | ✔ | mapped | mapped | 7z, RAR, zip |
| Unix mode incl. setuid/sticky | mapped | ✔ (setuid stripped unless `--keep-setuid`) | ✔ | 7z, RAR5, zip, tar |
| uid/gid + names | – | ✔ (root only on restore) | ✔ | RAR5, tar, zip 0x7875 |
| Symlinks | ✔ | ✔ | ✔ | RAR5, 7z (`-snl`), zip, tar |
| Hardlinks | ✔ | ✔ | ✔ | RAR5, 7z (`-snh`), tar |
| Junctions | ✔ | – | – | RAR5 |
| NTFS ADS | ✔ | – | – | RAR (`-os`), 7z (`-sns`), WIM |
| NTFS ACL / owner | ✔ | – | – | RAR (`-ow`), WIM |
| xattrs / POSIX ACL | – | ✔ | ✔ | RAR5 (planned via service hdr), pax, 7z (`-snx`-style RarX ext) |
| macOS resource fork / Finder info | – | – | ✔ (AppleDouble `._` handling, `__MACOSX` ignore option) | zip, xar |
| Sparse files | ✔ | ✔ | ✔ | tar (GNU/pax sparse) |
| Case-sensitivity conflicts | detect & prompt | – | detect | all |

Rules:
- Time precision preserved to the best the format allows; display in UI in local time with option for UTC.
- "Store file times" / "Restore file times" toggles per kind (M/C/A) in add & extract dialogs.

---

## 12. Multi-volume, solid, SFX, recovery

### 12.1 Volumes
- Volume size with units `b, k(=1024), K(=1000)`, `m/M`, `g/G`, plus **autodetect** (fill removable
  media), multiple different sizes (`-v100m -v200m` 7-Zip semantics: last size repeats).
- Pause after each volume (`-vp`), old-style naming (`-vn`), delete previous files on removable disk
  (`-vd`), volume number in name format (`.part01`, `.part001` zero-padding by volume count).
- Create, extract, test, list multi-volume; resume on missing volume prompt; "Next volume not found"
  dialog with browse.
- Recovery volumes `.rev` (RAR5) — create (`rv[N]`/`%`) and reconstruct (`rc`).

### 12.2 Solid
- 7z: block size by bytes/files/extension; RAR: solid groups by extension (`-se`), by volume (`-sv`,
  `-sv-`), file count (`-s<N>`); `-s-` disable. Solid sort order via `rarfiles.lst`.

### 12.3 Recovery record
- RAR5: size given in % of archive size (`-rr<N>%`; the upper limit must be **verified against current
  WinRAR docs** and clamped accordingly), added on create (`-rr`) or later (`rr` command), shown in Info dialog.
- Repair flow: detect damaged sectors via CRC, rebuild, write `fixed.<name>` / `rebuilt.<name>`.

### 12.4 SFX (self-extracting archives)
- Modules per platform: **Windows GUI** (Win32, x64/ARM64), **Windows console**, **Linux ELF**
  (x86_64/aarch64, console + optional GTK-less minimal UI), **macOS** (universal, console).
- Formats with SFX: 7z, RAR5², ZIP.
- **SFX options** (WinRAR "Advanced SFX options" — full set):
  - *General*: extraction path (absolute / relative to `%TEMP%`, `%ProgramFiles%`), create in current
    folder, "Run after extraction", "Run before extraction", setup program, save and restore paths.
  - *Setup*: `Setup=` (multiple), `Presetup=`, run as administrator (`RequireAdmin`/manifest),
    wait and delete temp files (`TempMode`).
  - *Modes*: unpack to temporary folder, silent mode (hide all / hide start dialog), overwrite mode
    (ask, overwrite all, skip existing).
  - *Update*: update mode (extract and replace / extract and update / freshen), overwrite mode.
  - *Text and icon*: title, text (HTML-capable in GUI module: `Text=`), logo image, custom icon.
  - *License*: license title and text (must accept).
  - *Module*: choose SFX module.
  - *Shortcuts*: create desktop/start menu/startup/program shortcuts (`Shortcut=` syntax D/M/P/S/T).
- Script language: WinRAR SFX comment command syntax (`Path=`, `Setup=`, `Silent=`, `Overwrite=`,
  `Title=`, `Text`, `License`, `Shortcut=`, `Delete=`, `TempMode`, `Update=`, `SavePath`, `Presetup=`,
  `Logo=`) and 7-Zip SFX `config.txt` (`;!@Install@!UTF-8!` … `RunProgram`, `ExecuteFile`,
  `ExecuteParameters`, `Directory`, `Title`, `BeginPrompt`, `Progress`). Both parsers fuzzed.
- SFX stub is code-signed in release pipelines (Windows Authenticode, macOS notarisation), small
  (<500 KiB target for console), statically linked, and refuses path traversal like the main app.
- Converting SFX → normal archive and normal → SFX without recompression.

---

## 13. CLI

Two binaries share one engine:

- **`rarx`** — WinRAR `rar.exe`/`unrar` compatible syntax: `rarx <command> -<switch1> … <archive> [files…] [@listfiles…] [path_to_extract\]`
- **`rx7`** — 7-Zip `7z` compatible syntax: `rx7 <command> [<switches>…] <archive_name> [<file_names>…] [@listfile]`

Both support: response files `@file`, `--` end of switches, environment variable default switches
(`RARX_RAR` like `RAR` env var; `RARX_7Z_SWITCHES`), config file defaults (§17), `--json` output
(RarX extension), `--help`, `--version`, shell completions (`rarx completions bash|zsh|fish|pwsh`),
man pages generated at build time.

Switch parsing rules: case-insensitive, `-` or `/` prefix on Windows, `-` only elsewhere; switches may
appear anywhere before `--`; `-switch-` disables a switch; wildcards `*` `?` with 7-Zip and RAR
matching semantics (RAR: `*` matches dots, `*.*` = all; 7-Zip: `*` = all, `*.*` = names with dot on
non-Windows — replicate per binary).

### 13.1 `rarx` commands (all of RAR)

| Cmd | Meaning |
|---|---|
| `a` | Add files to archive |
| `c` | Add archive comment |
| `ch` | Change archive parameters (e.g. add RR, convert) |
| `cw` | Write archive comment to file |
| `d` | Delete files from archive |
| `e` | Extract files without paths |
| `f` | Freshen files |
| `i[par]=<str>` | Find string in archives (`i`, `ic`, `ih`, `it`, `il`) |
| `k` | Lock archive |
| `l[t[a],b]` | List archive (technical, all, bare) |
| `m[f]` | Move to archive (files only with `mf`) |
| `p` | Print file to stdout |
| `r` | Repair archive |
| `rc` | Reconstruct missing volumes |
| `rn` | Rename archived files |
| `rr[N]` | Add recovery record |
| `rv[N]` | Create recovery volumes |
| `s[name\|-]` | Convert to SFX / remove SFX |
| `t` | Test archive |
| `u` | Update files |
| `v[t[a],b]` | Verbose list |
| `x` | Extract with full paths |

### 13.2 `rarx` switches (all of RAR)

`-` stop switches · `-@[+]` disable/enable file lists · `-ac` clear archive attr · `-ad[1|2]` append
archive name to dest path · `-ag[format]` name by date · `-ai` ignore attributes · `-am[s,r]`
archive name and time · `-ap<path>` path inside archive · `-as` synchronize · `-c-` disable comments
show · `-cfg-` ignore config · `-cl` / `-cu` lower/upper case names · `-df` delete after archiving ·
`-dh` open shared files · `-dr` delete to recycle bin · `-ds` no sort for solid · `-dw` wipe after
archiving · `-ed` no empty dirs · `-ep` exclude paths · `-ep1` exclude base dir · `-ep2` full paths ·
`-ep3` full paths with drive · `-ep4<path>` exclude path prefix · `-e[+]<attr>` attribute filters ·
`-f` freshen · `-hp[pwd]` encrypt data+headers · `-ht[b|c]` hash type BLAKE2/CRC32 ·
`-id[c,d,n,p,q]` disable messages · `-ierr` errors to stderr · `-ilog[name]` log errors ·
`-inul` disable all messages · `-ioff[n]` turn PC off/sleep/hibernate · `-isnd[-]` sound ·
`-iver` version · `-k` lock · `-kb` keep broken · `-log[f][=name]` write names to log ·
`-m<0..5>` method · `-ma[4|5]` format version · `-mc<par>` advanced compression params ·
`-md<n>[k,m,g]` dictionary (incl. RAR7 up to 64g) · `-me[par]` encryption params ·
`-ms[list]` store file types · `-mt<threads>` threads · `-n<file>` include · `-n@[list]` include list ·
`-o[+|-]` overwrite all / skip · `-oc` set compressed attr · `-oh` hardlinks as links ·
`-oi[0-4][:min]` identical files as refs · `-ol[a]` symlinks as links (`a`: absolute allowed) ·
`-om[-|1][=lst]` propagate Mark of the Web · `-op<path>` output path · `-or` auto-rename ·
`-os` NTFS streams · `-ow` owner/group/security · `-p[pwd]` password · `-p-` no password ·
`-qo[-|+]` quick open info · `-r` recurse · `-r-` no recurse · `-r0` recurse wildcard dirs ·
`-ri<P>[:<S>]` priority & sleep · `-rr[N]` recovery record · `-rv[N]` recovery volumes ·
`-s[<N>,v[-],e]` solid options · `-s-` non solid · `-sc<chr>[obj]` charset for list/comment/redir ·
`-sfx[name]` create SFX · `-si[name]` stdin · `-sl<size>` / `-sm<size>` size limits ·
`-sv` / `-sv-` independent solid volumes · `-t` test after · `-ta[mcao]<date>` / `-tb` / `-tn<time>`
/ `-to<time>` time filters · `-ts[m,c,a,p][+,-,1]` save/restore times · `-u` update ·
`-v<size>[k,b]` volumes · `-vd` erase disk · `-ver[n]` file versions · `-vn` old naming ·
`-vp` pause between volumes · `-w<path>` work dir · `-x<file>` exclude · `-x@[list]` exclude list ·
`-y` yes to all · `-z[file]` comment from file.

### 13.3 `rx7` commands (all of 7-Zip)

| Cmd | Meaning |
|---|---|
| `a` | Add |
| `b` | Benchmark (`b [iterations] [-mm=* -mmt=* -md=*]`) |
| `d` | Delete |
| `e` | Extract without paths |
| `h` | Calculate hashes (`-scrc<method>`) |
| `i` | Show information about supported formats/codecs/hashers |
| `l` | List |
| `rn` | Rename (pairs old new) |
| `t` | Test |
| `u` | Update |
| `x` | Extract with full paths |

### 13.4 `rx7` switches (all of 7-Zip)

`--` stop parsing · `-ai[r[-|0]]{@listfile|!wildcard}` include archives · `-ax[…]` exclude archives ·
`-ao{a|s|t|u}` overwrite mode · `-an` no archive name · `-bb[0-3]` log level ·
`-bd` no progress · `-bs{o|e|p}{0|1|2}` output streams · `-bt` timing stats · `-i[r[-|0]]{@|!}`
include · `-m{Parameters}` compression method/params (full syntax: `-m0=LZMA2:d=64m:fb=64
-m1=BCJ2 -mb0s0:1 -mx=9 -mmt=on|N -mhe=on -mhc=on -mtc=on -mta=on -mtm=on -mtr=on -ms=on|e|<N>f<M>b
-mf=on|off|FILTER -mqs -mcu=on -mcp=<cp> -mem=AES256|ZipCrypto -mmemuse=<size>|p<N> -myx=<level>
-mtp=<level> -mpass=<n> -mfb=<n> -mmc=<n> -mo=<order> -mmem=<size> -md=<size> -mc=<chunk>
-mlc/-mlp/-mpb -mmf=<mf> -ma=<0|1> -mx0..9` etc.) · `-o{dir}` output dir · `-p{password}` ·
`-r[-|0]` recurse · `-sa{a|e|s}` archive name mode · `-scc{UTF-8|WIN|DOS}` console charset ·
`-scs{UTF-8|UTF-16LE|UTF-16BE|WIN|DOS|<id>}` list file charset · `-scrc[CRC32|CRC64|SHA1|SHA256|…|*]`
hash function · `-sdel` delete after compression · `-seml[.]` send email · `-sfx[{name}]` SFX ·
`-si[{name}]` stdin · `-slp` large pages · `-slt` technical listing · `-snh` hardlinks as links ·
`-snl` symlinks as links · `-sni` NT security · `-sns[-]` alternate streams · `-snr` replace invalid
chars · `-snz` propagate Zone.Identifier · `-snt` (RarX: xattrs) · `-so` stdout · `-spd` disable
wildcard for names · `-spe` eliminate root folder duplication · `-spf[2]` full/absolute paths ·
`-ssc[-]` case sensitivity · `-sse` stop if can't open input file · `-ssp` don't change last access time
of sources · `-ssw` compress shared files · `-stl` archive time from newest file · `-stm{mask}`
thread affinity · `-stx{type}` exclude archive type · `-t{type}` archive type (`-t7z -tzip -ttar …`,
`-t#`, `-t*`, `-t#:e`, `-tsplit`, multi `-ttar.gz`) · `-u[-][p#][q#][r#][x#][y#][z#][w#][!{file}]`
update options · `-v{size}[b|k|m|g]` volumes · `-w[{path}]` work dir · `-x[r[-|0]]{@|!}` exclude ·
`-y` yes to all.

### 13.5 Unified extras (both binaries)

- `--json` structured output for `l/v/t/i/h` and progress events (NDJSON with `--progress=json`).
- `--memory-limit <size>` hard cap for decoders (default: 75% physical RAM; refuse > cap).
- `--threads <n>`, `--priority low|normal|high`.
- `--dry-run` for add/update/delete/extract (prints planned actions).
- `--sanitize=strict|compat|none` (path policy; `none` still blocks traversal outside dest).
- `rarx-legacy` symlink names: invoking as `rar`, `unrar`, `7z`, `7za`, `7zr` selects behaviour
  automatically (packagers may install these aliases **only** under non-trademarked names unless legally cleared).

---

## 14. GUI — file manager

Built with **Slint** (native-looking, accessible, cross-platform). The GUI process hosts the engine
in worker threads; long jobs run in a separate **job window** so the browser stays responsive.

### 14.1 Main window
- **Address bar** with path, breadcrumb, history drop-down, "Up one level" (Backspace).
- **Toolbar** (customisable, large/small icons, text labels toggle): Add, Extract To, Test, View,
  Delete, Find, Wizard, Info, VirusScan, Comment, Protect, SFX, Repair, Convert, Benchmark, Hash.
- **File list**: virtualised (≥1 M entries smooth), columns Name, Size, Packed, Ratio, Type, Modified,
  Created, Accessed, Attributes, CRC32, Method, Encrypted, Comment, Host OS, Version, Folders, Files,
  Offset, Block, Links, Owner/Group, Characteristics (7-Zip) — user-chooseable, reorderable, sortable,
  width persisted per format.
- **View modes**: details, list, small icons, large icons, thumbnails (images inside archives);
  **flat view** (all files recursively, 7-Zip "Flat View").
- **Folder tree** panel (WinRAR "Show folder tree"), **two-panel mode** (7-Zip F9) with independent
  panels and copy/move between them.
- **Status bar**: selected count/size, total, archive properties (encrypted lock icon, solid, RR,
  locked), drive free space.
- **Navigation into archives** like folders, nested archives (Ctrl+PgDn open inside, Ctrl+PgUp up,
  "Open inside *", "Open inside #" raw parser), **"Open outside"** (Shift+Enter → system handler).
- **Drag & drop**: from Explorer/Finder/Files into list (add), from archive out (extract), between
  panels; modifiers to choose copy/move; drop on archive file in the list to add to it.
- **Favorites** (bookmarks for folders and archives, organise dialog, Alt+Shift+0–9 to set, Alt+0–9 to go).
- **Folder history** (Alt+F12), **Recently opened archives** in File menu (count configurable).
- **Quick search/filter** (type-to-select, filter box with wildcards), **Select / Deselect by mask**
  (Gray +/−, Gray *  invert), Select all.
- **Themes** (light, dark, high-contrast, system; user theme packs: toolbar icon sets in SVG),
  font/size, row height, grid lines, full-row select, single-click open option, show hidden/system
  files, show real file icons, "Archive files first" ordering.
- **File operations on disk** (7-Zip FM-like): Copy (F5), Move (F6), New folder (F7), Delete (Del →
  recycle bin; Shift+Del permanent with confirm), Rename (F2), Properties (Alt+Enter), Comment,
  Create file, Create hard/symbolic link ("Link…"), Alternate streams listing,
  Compare files (diff tool launch), Calculate checksum, Split / Combine, Open in terminal.
- **Wizard** (WinRAR): step-by-step "Unpack an archive / Create new archive / Add to existing archive"
  for novice users.
- **Job queue window**: shows all running/queued jobs, pause/resume/cancel, background ("Background"
  button sends to tray), priority, "Shut down when done", elapsed/remaining, speed graph, ratio,
  per-file error log with "Copy to clipboard" and "Save".
- **Diagnostic messages window** (WinRAR): accumulates warnings/errors with "Break operation",
  "Copy", "Save to file".
- **Tray icon** for background jobs with notifications on completion (OS-native notifications).
- **Single-instance** forwarding (open second archive in new window or tab; configurable).

### 14.2 Keyboard shortcuts (defaults, all rebindable in Settings)

| Action | Key |
|---|---|
| Add files | Alt+A |
| Extract to | Alt+E |
| Extract without confirmation | Alt+W |
| Test | Alt+T |
| View file | Alt+V / F3 |
| Edit file | F4 |
| Delete | Del / Alt+D |
| Repair | Alt+R |
| Add comment | Alt+M |
| Protect (recovery record) | Alt+P |
| Lock | Alt+L |
| Convert to SFX | Alt+X |
| Show information | Alt+I |
| Find files | F3 in panel / Alt+F7 |
| Wizard | Alt+Z |
| Rename | F2 |
| Copy / Move / New folder | F5 / F6 / F7 |
| Two panels | F9 |
| Split / Combine | Alt+F5? — final mapping tracked in `keymap.toml` |
| Benchmark | Alt+B |
| Set default password | Ctrl+P |
| Organize favorites | Ctrl+F |
| Add to favorites | Ctrl+F? (resolve conflict in keymap) |
| Settings | Ctrl+S |
| Select all / Invert / by mask | Ctrl+A / Gray* / Gray+ Gray− |
| Up one level | Backspace / Ctrl+PgUp |
| Open inside | Ctrl+PgDn |
| Refresh | Ctrl+R / F5? |
| Properties | Alt+Enter |
| Quit | Alt+Q / Ctrl+Q |

Conflicts marked “?” must be resolved by an ADR (`docs/adr/`) before the GUI milestone closes; the
keymap file is the source of truth.

---

## 15. Dialogs (every one, every tab)

### 15.1 "Archive name and parameters" (Add)
- **General**: archive name (+ Browse), Profiles button, Update mode (add & replace, add & update,
  freshen, synchronize, ask before overwrite, skip existing, rename automatically), archive format
  (RAR5²/ZIP/7z/TAR/… — no RAR4 creation), compression method, dictionary size, split to volumes
  size (combo with presets + custom), archiving options checkboxes: Delete files after archiving,
  Create SFX archive, Create solid archive, Add recovery record, Test archived files, Lock archive.
  **Set password…** button.
- **Advanced**: NTFS options (save file security, save file streams), recovery record %, compression
  parameters… (threads, filters: delta/x86/ARM, solid by ext, dictionary), SFX options…, Volumes
  (pause after each volume, old style names, create recovery volumes count), background archiving,
  turn PC off when done, Save symbolic links as links / hard links as links / identical files as references.
- **Options**: delete mode (delete/recycle/wipe), file-time precision, "Files to store without compression".
- **Files**: files to add (with Append), files to exclude, files to store without compression, file paths
  (store relative / full / absolute / none), "Put each file to separate archive", "Archive file/folder
  in subfolders", "Exclude base folder from names".
- **Backup**: erase destination disk contents, clear attribute "Archive" after compression, open shared
  files, add only files with "Archive" attribute set, generate archive name by mask (+ mask), keep
  previous file versions (N).
- **Time**: high-precision mtime, store creation time, store last access time, include files by time
  (newer than / older than / before / after date), archiving time (set to current / original /
  latest file time).
- **Comment**: load from file, edit text.
- 7z/ZIP specific panel (7-Zip "Add to archive" fields): Compression level, method, dictionary, word
  size, solid block size, CPU threads, memory usage for compressing / decompressing (live), split to
  volumes, parameters (free text `-m`), update mode, path mode (relative / full / absolute), options
  (create SFX, compress shared files, delete after compression), encryption (password twice, show
  password, encrypt file names, method), NTFS (store symlinks, hardlinks, alternate data streams, file
  security), Time (store modification / creation / last access, set archive time to latest file,
  do not change source files last access time, timestamp precision).

### 15.2 "Extraction path and options"
- **General**: destination path + folder tree browser with "New folder", update mode (extract and
  replace / extract and update / fresh existing), overwrite mode (ask / overwrite / skip / rename
  automatically / rename existing), miscellaneous (extract archives to subfolders, keep broken files,
  display files in Explorer), Save settings, Display / Remove redundant folder.
- **Advanced**: file time (set modification / creation / last access), delete archive (never /
  ask / always), file security, alternate streams, Mark of the Web propagation, file paths
  (extract relative / no paths / absolute), "Allow potentially incompatible names", wait if other
  copies active, background extraction, turn PC off when done, ignore missing volumes?.
- **Password**: field + "Use for all archives", "Show password", "Organize passwords" button.

### 15.3 Others (each has a spec in `docs/ui/dialogs/*.md` with mock-ups)
Overwrite confirmation · Password entry (with saved passwords drop-down) · Next volume needed ·
Archive information (General, Options, Comment, SFX tabs) · Find files + results · Benchmark (CPU
usage, ratings, dictionary, passes, threads, MIPS like 7-Zip; WinRAR-style single KB/s meter with
"Multithreading" checkbox) · Convert archives · Repair archive (path, "treat as RAR/ZIP") · Lock
confirmation · Protect (recovery record %) · SFX options (all tabs from §12.4) · Comment editor ·
Organize favorites · Organize passwords · Compression profiles (Save current as…, set default, add
to context menu) · Generate report · Wizard · Split / Combine · Hash results (copy/save) · Settings
(all tabs §17) · Diagnostic messages · Job queue · About (licences of third-party crates).

---

## 16. Shell / desktop integration

### 16.1 Windows
- **Windows 11 modern context menu** via packaged `IExplorerCommand` (sparse MSIX identity) +
  **classic menu** entries (cascaded or flat, user-selectable items), icons, multi-select support.
- Items: Open with RarX · Extract files… · Extract here · Extract to "<name>\" · Smart extract ·
  Extract each archive to separate folder · Test archive · Add to archive… · Add to "name.7z/.rar/.zip" ·
  Compress and email… · Compress to "name.rar" and email · Convert to … · CRC SHA submenu
  (CRC32, CRC64, SHA1, SHA256, BLAKE2sp, BLAKE3, `*`) · Open archive as (type submenu).
- **Drag-and-drop handler** (right-drag menu: "Extract here", "Add to archive…").
- **File associations** with per-extension checkboxes, "Associate RarX with:" list, icons per type,
  "Integrate into shell" toggle, "Cascaded context menus", "Icons in context menus", "Context menu
  items…" selector.
- **Property sheet** handler (archive info tab) and **thumbnail/preview handler** (list contents) optional.
- Long path aware manifest, per-monitor DPI v2, dark title bar.

### 16.2 macOS
- Finder Sync / Share extension + Services menu items (Compress with RarX, Extract here, Extract to…),
  Quick Look preview plug-in listing archive contents, UTI declarations for every supported format,
  dock icon drop target, notarised, sandbox-friendly security-scoped bookmarks.

### 16.3 Linux
- `.desktop` entry with MIME types for every format (`shared-mime-info` XML for missing types),
  Nautilus Python extension or `nautilus-scripts`, Dolphin/KDE service menus (`ServiceMenus/*.desktop`),
  Thunar custom actions (`uca.xml`), Nemo actions, Caja; Freedesktop thumbnailer for previews; Flatpak
  portal-aware file access; XDG base dirs.

---

## 17. Settings, profiles, configuration files

- Storage: TOML at `$XDG_CONFIG_HOME/rarx/` / `%APPDATA%\RarX\` / `~/Library/Application Support/RarX/`.
  **Portable mode**: if `rarx.portable` exists next to the binary, store settings there.
- Import/export settings (WinRAR "Settings → Save/Restore"), reset to defaults.
- Files: `settings.toml`, `profiles/*.toml` (compression profiles), `extract-profiles/*.toml`,
  `favorites.toml`, `keymap.toml`, `passwords.enc` (if keychain unavailable, encrypted with master
  password), `solid-order.lst` (like `rarfiles.lst`), `cli.toml` (default switches per command, like
  `rar.ini` `switches=` / `switches_a=` etc.).
- **Settings tabs**:
  - *General*: language, theme, toolbar, low priority, multithreading, keep archives history (count),
    log errors to file, beep, show comment, wizard on start, reuse windows, single click.
  - *Compression*: default profile, "Create default", "Files to store without compression",
    solid sort list, default format for "Add to" quick command.
  - *Paths*: temp folder (+ "Use only for removable disks"), default start folder, default extraction
    path (+ append archive name), restore last working folder.
  - *File list*: list type, sort, show hidden, show seconds, file time format, real icons, flat view,
    full row, grid, archives first, highlight encrypted/folders, size units (bytes/KB/MB, IEC/SI).
  - *Viewer*: internal/external viewer, viewer type autodetect, unpack everything for Exe/Bat/HTML
    ("Unpack everything for: *.exe *.msi *.bat *.cmd *.com *.lnk *.htm *.html"), ask before
    updating modified file, "Ignore modifications" list.
  - *Security*: file types to exclude from extraction (e.g. `*.exe *.vbs`—empty by default), Zone.Identifier
    propagation mode, delete temp files (always/never/ask), wipe temp files, virus scanner command line +
    parameters + file types, propose to delete archive after extraction.
  - *Integration*: associations, shell menu items, cascade, icons, "Show in context menu" per item, Explorer
    preview/thumbnail handlers, compression profiles shown in context menu.
  - *Keyboard*: keymap editor.

---

## 18. Security requirements

These are **release blockers**. Each has a dedicated regression test in `rarx-core/tests/security/`.

1. **Path traversal (Zip-Slip)**: normalise every entry path; reject/strip `..`, absolute paths, drive
   letters, UNC (`\\server\`), `\\?\`, device names (`CON`, `NUL`, `COM1`…, `LPT1`…), trailing dots and
   spaces on Windows, NTFS ADS separators (`:`) unless ADS extraction enabled, alternate separators (`\`
   in Unix-created zip names, `/` on Windows), Unicode look-alike slashes (`U+2215`, `U+FF0F`),
   empty components, `.` components.
2. **Symlink escape**: never write *through* an extracted symlink; create links last; a link whose
   resolved target escapes the destination is skipped (or extracted as a plain file under compat
   mode) unless the user passes the explicit "allow absolute/outside links" switch. Check every path
   component with `openat`-style directory handles (`O_NOFOLLOW`, Windows `FILE_FLAG_OPEN_REPARSE_POINT`).
   Hardlinks must target files already extracted inside the destination.
3. **Decompression bombs**: enforce limits — max unpacked total (default: free disk space + warning),
   max ratio alert (default 1:1000 for nested/recursive), max nesting depth (default 8), max entries
   (configurable), max dictionary vs memory cap. Overlapping ZIP entries (ZIP-in-ZIP "non-recursive
   bomb") detected by checking local header ranges overlap.
4. **Memory**: every allocation sized from archive data is checked against `MemoryBudget`; use
   `try_reserve`. No `Vec::with_capacity(untrusted)`.
5. **Integer safety**: all offset/size arithmetic uses checked ops; `overflow-checks = true` in release.
6. **`unsafe` policy**: `#![forbid(unsafe_code)]` in every crate except `rarx-checksum` (SIMD),
   `rarx-shell` (COM/ObjC FFI), `rarx-ffi`, `rarx-io` (mmap). Each `unsafe` block carries a
   `// SAFETY:` comment and is covered by Miri tests where possible.
7. **Crypto hygiene**: constant-time MAC compare (`subtle`), CSPRNG (`getrandom`) for salts/IVs,
   zeroize keys, no ECB, no IV reuse, minimum KDF cost not configurable below defaults for writing.
8. **Mark of the Web** propagation honoured by default for executables/Office types on Windows.
9. **Executable launches**: viewing a file from an archive extracts it to a private temp dir
   (0700 / user-only ACL); executables get the whole archive ("unpack everything for") only after
   confirmation; temp files wiped on close.
10. **SFX**: stub verifies its own archive CRC before running anything; never auto-elevates without
    the manifest-declared request; refuses traversal like the main engine.
11. **Supply chain**: `cargo deny` (licences, advisories, bans, sources), `cargo audit` in CI,
    reproducible builds, signed releases, SBOM (CycloneDX) per release.
12. **Fuzzing**: every parser and decoder has a `cargo-fuzz` target; OSS-Fuzz style continuous fuzzing;
    any crash corpus file goes into `corpus/regressions/` with a test.
13. **Filename display**: escape control characters and bidi overrides (`U+202E`) in UI and CLI output
    so names can't spoof extensions.

---

## 19. Performance requirements

### 19.1 Targets (same machine, same threads, warm cache)

| Operation | Target vs reference |
|---|---|
| LZMA2 -mx5/-mx9 compress | ≤ 110% of 7-Zip time, ≤ 100.5% size |
| LZMA/LZMA2 decompress | ≤ 110% of 7-Zip time |
| Deflate compress (Normal/Ultra) | ≤ 110% of 7-Zip time, ≤ 100.5% size |
| Deflate decompress | ≥ zlib-ng speed |
| RAR5 decompress | ≤ 115% of UnRAR time |
| RAR5 compress² | ≤ 115% of WinRAR time, ≤ 101% size |
| Zstd / LZ4 / Brotli | ≈ reference C libraries (FFI or pure Rust within 10%) |
| List 1 M-entry archive (7z / zip / rar w/ QO) | < 2 s |
| GUI open of 1 M-entry archive | first paint < 300 ms, scrolling 60 fps |

### 19.2 Techniques
- Streaming pipelines (reader → decoder → hasher → writer) with bounded buffers; zero-copy where possible.
- Multi-threaded: LZMA2 blocks, BZip2 blocks, Zstd workers, XZ blocks, parallel per-file for zip/rar
  non-solid, parallel folders for 7z, parallel hashing.
- Large pages (`-slp`), NUMA-aware thread affinity (`-stm`), runtime CPU feature dispatch.
- Avoid seeking over network shares (sequential-friendly writers).
- Memory usage reported before job start; `-mmemuse` honoured.

### 19.3 Benchmark feature
- 7-Zip-compatible `b` command: LZMA compress/decompress rating (MIPS), per-thread usage, CPU freq
  estimation, hash speed tests (`-mm=*`), dictionary sweep, `-mmt` sweep, output table matching 7-Zip
  format (`Ratings`, `Usage`, `R/U`, `Rating`).
- WinRAR-style GUI benchmark: continuous run, KB/s, processed, errors (also stress-tests RAM: detect
  errors → hardware fault message), multithreading toggle, dictionary selection, RAR 7 large dict.
- Criterion micro-benchmarks for every codec in CI with regression alerts (>5%).

---

## 20. Internationalisation & accessibility

- All UI strings in Fluent (`.ftl`); no string literals in UI code. Plural rules, RTL layout (Arabic,
  Hebrew) mirrored, locale-aware number/date/size formatting. Ship ≥ 40 languages via community
  translations; English is the source.
- CLI messages translatable too; `--lang` override; machine-readable output (`--json`) never translated.
- Filenames: full Unicode, NFC/NFD handling on macOS, legacy code pages for ZIP/RAR4/ARJ/LZH (CP437,
  CP850, CP866, CP932 Shift-JIS, CP936 GBK, CP949, CP950 Big5, CP1250–1258, KOI8-R…) with user-selectable
  "name encoding" when opening an archive.
- Accessibility: full keyboard navigation, screen-reader names/roles (UIA / NSAccessibility / AT-SPI via
  Slint accessibility), high-contrast themes, scalable fonts to 200%, no information by colour alone,
  focus indicators, progress announced to screen readers.

---

## 21. Error handling, logging, exit codes

- Library errors: `thiserror` enums per crate, converted to `rarx_core::Error` with kind, path,
  entry name, offset. Never `panic!`/`unwrap()` on input-derived data (clippy `unwrap_used`,
  `expect_used`, `panic` denied in library crates).
- Messages identify **what, where, why, what to do** ("CRC failed in `docs/a.txt` (encrypted). Wrong
  password? — re-enter password or use `-p`").
- Logging via `tracing`; GUI log viewer; `-ilog` / `--log-file`; no secrets in logs.
- **Exit codes** — each binary replicates its reference tool exactly:

| `rarx` (RAR) | Meaning | `rx7` (7-Zip) | Meaning |
|---|---|---|---|
| 0 | Success | 0 | No error |
| 1 | Non-fatal warning | 1 | Warning (non-fatal) |
| 2 | Fatal error | 2 | Fatal error |
| 3 | CRC error | 7 | Command line error |
| 4 | Locked archive modification attempt | 8 | Not enough memory |
| 5 | Write error | 255 | User stopped the process |
| 6 | File open error | | |
| 7 | Wrong command line option | | |
| 8 | Not enough memory | | |
| 9 | File create error | | |
| 10 | No files matching mask/options | | |
| 11 | Wrong password | | |
| 12 | Read error | | |
| 255 | User break | | |

- Ctrl+C: graceful cancel → remove partial archive / temp files, keep original, exit 255.

---

## 22. Testing strategy & compatibility gates

1. **Unit tests** per module; **property tests** (`proptest`) for every codec (encode→decode identity,
   arbitrary input never panics).
2. **Golden corpus** (`corpus/`): for each format and each method/option combination, archives produced
   by reference tools (WinRAR 3.x–7.x, 7-Zip 9.20–25.x, Info-ZIP, WinZip, macOS Archive Utility,
   Android `zipalign`, GNU tar, bsdtar, xz, zstd, cabarc, mkisofs, etc.) with expected listing JSON
   (`insta` snapshots) and file hashes.
3. **Cross-tool round-trip** (CI job `compat`, Linux + Windows runners): create with RarX → verify with
   `7z t`, `unrar t` (only invoking the binary, never its source), `unzip -t`, `tar -t`, `xz -t`,
   `zstd -t`; create with reference → extract with RarX → compare tree, contents, times, modes, links.
4. **Metadata tests** per OS (times precision, attrs, ACLs, ADS, xattrs, symlinks, hardlinks).
5. **Security suite** (§18): malicious archives corpus (zip-slip, symlink escape, overlapping zip bomb,
   42.zip, huge dictionary claims, truncated headers, CRC collisions, bidi names).
6. **Fuzzing**: nightly 1 h per target in CI, continuous on dedicated infra; coverage tracked.
7. **CLI conformance**: table-driven tests that run every command/switch in §13 against the expected
   behaviour; exit codes asserted; `--help` snapshot tests.
8. **GUI tests**: Slint testing backend for dialogs/state; screenshot tests per theme; accessibility
   tree assertions; manual exploratory checklist per release (`docs/qa/checklist.md`).
9. **Performance**: criterion + a `bench/compare.sh` harness vs reference binaries, results in CI artifacts.
10. **Miri** on pure crates, **sanitizers** (ASan/TSan via nightly) on FFI crates.
11. Coverage ≥ 85% lines for `rarx-formats`, `rarx-codecs`, `rarx-core`.

---

## 23. Coding standards for agents

- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings` (pedantic in library crates,
  with justified `allow`s), `cargo test --workspace`, `cargo deny check` must pass before any commit.
- Small, focused functions; no module > ~800 lines without reason; public items documented (`#![deny(missing_docs)]`
  in library crates) with examples that compile.
- **No format logic in front-ends**; **no UI strings in the engine** (engine returns typed errors/events).
- Binary parsing: explicit little-endian readers (`u32::from_le_bytes`), bounds-checked cursor type
  (`rarx_io::Cursor`); never transmute structs.
- Every new format/codec PR includes: spec notes in `docs/formats/`, provenance entry, fuzz target,
  corpus files with provenance, compat test, update of the §5/§7 tables and the §25 checklist.
- Before using any library API, fetch current docs (Context7) — APIs change.
- Public API changes require an ADR in `docs/adr/`. Commit messages: Conventional Commits.
- Never weaken a security check to make a test pass; never add `#[ignore]` without a linked issue.
- Feature flags: `rar-write`, `ace`, `inno`, `zipx-extras`, `gui`, `shell`, `ffi`, `simd` (default on),
  `pure-rust` (avoid C deps: replace zstd/brotli C bindings with pure-Rust impls).

---

## 24. Milestones & definition of done

| # | Milestone | Scope | Exit criteria |
|---|---|---|---|
| M0 | Skeleton | workspace, CI, deny, fuzz infra, io/checksum crates | CI green on 3 OSes |
| M1 | Core formats R/W | 7z (LZMA/LZMA2/PPMd/BZip2/Deflate + BCJ/BCJ2/Delta + AES), ZIP/ZIP64 (+AES/ZipCrypto), TAR, GZ/BZ2/XZ/ZST/LZ4 | compat gate passes; security suite passes |
| M2 | RAR read | RAR 1.5–7 decode, encryption, volumes, RR repair, `.rev` reconstruct | full RAR corpus extracts bit-exact |
| M3 | CLI parity | `rarx` + `rx7` all commands/switches, exit codes, JSON | CLI conformance suite 100% |
| M4 | Read-only formats | everything else in §5 | each format: corpus + 24 CPU-h fuzz clean |
| M5 | GUI | main window, all dialogs §15, job queue, viewer, find, wizard, benchmark | GUI tests + a11y audit pass |
| M6 | Integration | shell menus all OSes, associations, SFX modules, installers | manual QA checklist signed |
| M7 | Performance | meet §19 targets | benchmark report attached to release |
| M8 | RAR write (gated) | RAR5/RAR7 creation behind `rar-write` | legal sign-off + compat gate |
| 1.0 | Release | all §25 boxes ticked | — |

**Definition of done for any feature**: implemented in engine → exposed in CLI → exposed in GUI (+
shell if applicable) → documented (user docs + this file) → tested (unit + compat + security where
relevant) → translated strings extracted → checklist ticked.

---

## 25. Master feature checklist

Agents: tick (`[x]`) items only when the Definition of Done (§24) is fully met. Do not delete items;
if an item is dropped, strike it through and link the ADR.

### Formats
- [ ] 7z read · [ ] 7z write · [ ] 7z update · [ ] 7z AES + header encryption · [ ] 7z SFX · [ ] 7z volumes
- [ ] RAR5/RAR7 read · [ ] RAR1.5–4 read · [ ] RAR encryption (all versions) read · [ ] RAR volumes (both naming styles)
- [ ] RAR recovery record repair · [ ] RAR `.rev` reconstruct · [ ] RAR comments/QO/locked/versions/links/ACL/streams
- [ ] RAR5 write² · [ ] RAR recovery record create² · [ ] RAR `.rev` create² · [ ] RAR SFX create²
- [ ] ZIP/ZIP64 read/write/update · [ ] ZipCrypto · [ ] WinZip AES · [ ] split/spanned ZIP · [ ] ZIP SFX · [ ] ZIP code pages
- [ ] TAR all dialects R/W · [ ] GZ · [ ] BZ2 · [ ] XZ · [ ] LZMA · [ ] LZMA86 · [ ] ZST · [ ] LZ4 · [ ] BR · [ ] LZ · [ ] LZO · [ ] Z
- [ ] WIM/SWM/ESD · [ ] CAB · [ ] ARJ · [ ] LZH · [ ] ACE (opt) · [ ] ISO/Joliet/RR/El Torito · [ ] UDF · [ ] CPIO · [ ] AR/DEB · [ ] RPM
- [ ] XAR/PKG · [ ] DMG · [ ] HFS/HFS+ · [ ] APFS · [ ] NTFS · [ ] FAT/exFAT · [ ] ext2/3/4 · [ ] SquashFS · [ ] CramFS
- [ ] MBR/GPT · [ ] VHD/VHDX/VMDK/VDI/QCOW2 · [ ] CHM · [ ] MSI/CFB · [ ] NSIS · [ ] Inno (opt) · [ ] PE/ELF/Mach-O/TE · [ ] UEFI
- [ ] SWF/FLV · [ ] MSLZ · [ ] Split .001 · [ ] Hash files · [ ] UUE/XXE/B64/yEnc · [ ] Zipx extras (opt) · [ ] raw `#` parser

### Codecs & filters
- [ ] LZMA · [ ] LZMA2 (MT) · [ ] PPMd H · [ ] PPMd I · [ ] BZip2 (MT) · [ ] Deflate · [ ] Deflate64 · [ ] Zstd · [ ] LZ4 · [ ] Brotli
- [ ] RAR15 · [ ] RAR20 (+audio) · [ ] RAR29 (+PPMd, VM std filters) · [ ] RAR50 · [ ] RAR70 large dict
- [ ] Implode · [ ] Shrink · [ ] Reduce · [ ] LZX · [ ] Quantum · [ ] MSZIP · [ ] XPRESS · [ ] LZMS · [ ] LZFSE/LZVN · [ ] ADC · [ ] LZW · [ ] ARJ/LH/ACE decoders
- [ ] BCJ · [ ] BCJ2 · [ ] ARM · [ ] ARMT · [ ] ARM64 · [ ] PPC · [ ] SPARC · [ ] IA64 · [ ] RISCV · [ ] Delta · [ ] Swap · [ ] auto filter analysis

### Operations
- [ ] add · [ ] move · [ ] update · [ ] freshen · [ ] synchronize · [ ] 7z `-u` state machine · [ ] delete · [ ] rename · [ ] create folder in archive
- [ ] extract (paths/no paths/absolute/sub-path) · [ ] extract here / to folder / smart / each separately · [ ] all overwrite modes · [ ] keep broken
- [ ] test · [ ] list (all modes + JSON) · [ ] print to stdout · [ ] stdin/stdout streams · [ ] info dialog · [ ] comments (archive + per file)
- [ ] lock · [ ] protect · [ ] repair · [ ] reconstruct · [ ] convert (batch) · [ ] SFX convert/remove/options · [ ] benchmark · [ ] estimate
- [ ] hash calculator (all algos) · [ ] hash-file verify · [ ] split/combine · [ ] find in archives · [ ] virus scan · [ ] report · [ ] email
- [ ] archive name by mask · [ ] separate archive per item · [ ] delete/recycle/wipe after archiving · [ ] test before delete
- [ ] include/exclude masks, lists, attributes, size, time filters · [ ] store-without-compression list · [ ] solid sort list
- [ ] file versions · [ ] identical files refs · [ ] incremental via archive bit · [ ] anti-items · [ ] VSS shared files
- [ ] volumes (sizes, autodetect, pause, naming) · [ ] background/priority/wait for other copies · [ ] shutdown when done
- [ ] nested archive open/extract · [ ] edit-in-archive with auto-update · [ ] drag & drop both directions · [ ] clipboard paste

### Metadata
- [ ] mtime/ctime/atime precision · [ ] DOS attrs · [ ] Unix mode · [ ] owners · [ ] symlinks · [ ] hardlinks · [ ] junctions
- [ ] NTFS ADS · [ ] NTFS ACL · [ ] xattrs · [ ] macOS resource forks / `__MACOSX` · [ ] sparse files · [ ] Mark of the Web

### Security
- [ ] path sanitiser (all rules §18.1) · [ ] symlink-safe extraction · [ ] bomb limits · [ ] memory budget · [ ] bidi/control-char escaping
- [ ] password zeroize + keychain · [ ] fuzz targets for every parser/decoder · [ ] cargo-deny/audit · [ ] signed reproducible releases + SBOM

### Front-ends
- [ ] `rarx` all commands · [ ] `rarx` all switches · [ ] `rx7` all commands · [ ] `rx7` all switches · [ ] exit codes · [ ] completions + man pages
- [ ] main window · [ ] two-panel · [ ] tree · [ ] flat view · [ ] thumbnails · [ ] favorites · [ ] history · [ ] themes · [ ] keymap editor
- [ ] Add dialog (all tabs) · [ ] Extract dialog (all tabs) · [ ] all dialogs §15.3 · [ ] wizard · [ ] viewer · [ ] job queue · [ ] tray + notifications
- [ ] Windows shell (modern + classic) · [ ] associations · [ ] macOS Finder/Quick Look · [ ] Linux Nautilus/Dolphin/Thunar/Nemo/Caja · [ ] thumbnailers
- [ ] SFX modules (Win GUI/console, Linux, macOS) · [ ] SFX script parsers (RAR + 7z syntax)
- [ ] settings (all tabs) · [ ] profiles · [ ] portable mode · [ ] import/export settings
- [ ] i18n (Fluent, RTL, code pages) · [ ] accessibility audit · [ ] installers (MSI/MSIX, dmg/pkg, deb/rpm/Flatpak/AppImage)
- [ ] C ABI (`rarx.h`) · [ ] performance targets met · [ ] user documentation complete

---

*Footnote ²: RAR creation features are compiled only with the `rar-write` feature after legal sign-off (§2).*

*Items marked "verify" or "?" in this document are open questions: resolve them against current
official documentation of the reference tools and record the decision in an ADR, then update this file.*
