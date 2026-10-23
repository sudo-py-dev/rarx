# Algorithm provenance

Every codec, filter, cipher and format parser records where its specification came from
(AGENT.md §2). Clean-room rule: **never** consult the UnRAR source code.

| Component | Crate | Source of specification | Notes |
|---|---|---|---|
| CRC-32/ISO-HDLC | rarx-checksum | ISO 3309 / IEEE 802.3 polynomial | table-driven |
| CRC-64/XZ | rarx-checksum | ECMA-182 polynomial, xz file format spec | table-driven |
| 7z `NUMBER` encoding | rarx-io | 7z format description (`7zFormat.txt`, LZMA SDK, public domain) | |
| RAR5 `vint` | rarx-io | RAR 5.0 archive format technote (rarlab.com) | |
| Format signatures | rarx-formats | respective public format specs | |
| Delta filter | rarx-filters | LZMA SDK / xz file format spec | |
