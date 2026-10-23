"""Regenerates the 7z test corpus: python3 -I corpus/7z/generate.py

Containers are assembled by hand following 7zFormat.txt (LZMA SDK); all
compressed data comes from CPython's lzma module, i.e. liblzma, so the codec
side is an independent reference implementation.
"""

import lzma
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
FILETIME = 133_800_000_000_000_000  # 2025-01-01T00:00:00Z as 100 ns ticks since 1601

K_END, K_HEADER, K_MAIN, K_FILES = 0x00, 0x01, 0x04, 0x05
K_PACK, K_UNPACK, K_SUBSTREAMS, K_SIZE, K_CRC = 0x06, 0x07, 0x08, 0x09, 0x0A
K_FOLDER, K_CODERS_UNPACK_SIZE, K_NUM_UNPACK = 0x0B, 0x0C, 0x0D
K_EMPTY_STREAM, K_EMPTY_FILE, K_NAME, K_MTIME, K_ATTR, K_ENCODED = 0x0E, 0x0F, 0x11, 0x14, 0x15, 0x17

ID_COPY, ID_LZMA, ID_LZMA2 = b"\x00", b"\x03\x01\x01", b"\x21"
ID_DELTA, ID_BCJ, ID_AES = b"\x03", b"\x03\x03\x01\x03", b"\x06\xf1\x07\x01"

UNIX = 0x8000
FILE_ATTR = 0x20 | UNIX | (0o100644 << 16)
DIR_ATTR = 0x10 | UNIX | (0o040755 << 16)
LINK_ATTR = 0x20 | UNIX | (0o120777 << 16)


def num(n):
    """7z NUMBER: leading one bits of the first byte count the extra bytes."""
    for extra in range(8):
        if n < 1 << (8 * extra + 7 - extra):
            first = ((0xFF << (8 - extra)) & 0xFF) | (n >> (8 * extra))
            return bytes([first]) + (n & ((1 << (8 * extra)) - 1)).to_bytes(extra, "little")
    return b"\xff" + n.to_bytes(8, "little")


def bits(flags):
    out = bytearray((len(flags) + 7) // 8)
    for i, f in enumerate(flags):
        if f:
            out[i // 8] |= 0x80 >> (i % 8)
    return bytes(out)


def coder(method, props=b""):
    flags = len(method) | (0x20 if props else 0)
    return bytes([flags]) + method + (num(len(props)) + props if props else b"")


def lzma1(data, dict_size=1 << 16):
    raw = lzma.compress(data, format=lzma.FORMAT_RAW,
                        filters=[{"id": lzma.FILTER_LZMA1, "dict_size": dict_size, "lc": 3, "lp": 0, "pb": 2}])
    return raw, bytes([0x5D]) + struct.pack("<I", dict_size)


def lzma2(data, filters=()):
    chain = list(filters) + [{"id": lzma.FILTER_LZMA2, "dict_size": 1 << 20}]
    return lzma.compress(data, format=lzma.FORMAT_RAW, filters=chain), bytes([16])


class Folder:
    """One folder: coder records, bind pairs, per-output sizes and its file streams."""

    def __init__(self, coders, outputs, packs, streams, bind_pairs=()):
        self.coders, self.outputs, self.packs = coders, outputs, packs
        self.streams, self.bind_pairs = streams, list(bind_pairs)

    def record(self):
        out = num(len(self.coders)) + b"".join(self.coders)
        return out + b"".join(num(i) + num(o) for i, o in self.bind_pairs)


def chain(filter_id, filter_props, codec_id, codec_props, packed, size, streams):
    """Filter coder 0 fed by codec coder 1 (how 7-Zip writes e.g. BCJ + LZMA2)."""
    return Folder([coder(filter_id, filter_props), coder(codec_id, codec_props)],
                  [size, size], [packed], streams, bind_pairs=[(0, 1)])


def streams_info(pack_pos, folders, folder_crcs=None):
    packs = [p for f in folders for p in f.packs]
    out = bytes([K_PACK]) + num(pack_pos) + num(len(packs)) + bytes([K_SIZE])
    out += b"".join(num(len(p)) for p in packs) + bytes([K_END])
    out += bytes([K_UNPACK, K_FOLDER]) + num(len(folders)) + b"\x00"
    out += b"".join(f.record() for f in folders) + bytes([K_CODERS_UNPACK_SIZE])
    out += b"".join(num(s) for f in folders for s in f.outputs)
    if folder_crcs:
        out += bytes([K_CRC, 1]) + b"".join(struct.pack("<I", c) for c in folder_crcs)
    out += bytes([K_END])
    if not folder_crcs:
        out += bytes([K_SUBSTREAMS, K_NUM_UNPACK]) + b"".join(num(len(f.streams)) for f in folders)
        out += bytes([K_SIZE]) + b"".join(num(len(s)) for f in folders for s in f.streams[:-1])
        out += bytes([K_CRC, 1]) + b"".join(struct.pack("<I", crc_of(s)) for f in folders for s in f.streams)
        out += bytes([K_END])
    return out + bytes([K_END])


def crc_of(data):
    return zlib.crc32(data) ^ CRC_FLIP.get(data, 0)


CRC_FLIP = {}


def prop(pid, data):
    return bytes([pid]) + num(len(data)) + data


def files_info(files):
    out = bytes([K_FILES]) + num(len(files))
    empty = [f["data"] is None or f.get("empty") for f in files]
    if any(empty):
        out += prop(K_EMPTY_STREAM, bits(empty))
        out += prop(K_EMPTY_FILE, bits([f.get("empty", False) for f, e in zip(files, empty) if e]))
    names = b"".join(f["name"].encode("utf-16-le") + b"\0\0" for f in files)
    out += prop(K_NAME, b"\x00" + names)
    out += prop(K_MTIME, b"\x01\x00" + b"".join(struct.pack("<Q", FILETIME + i * 10_000_000) for i in range(len(files))))
    out += prop(K_ATTR, b"\x01\x00" + b"".join(struct.pack("<I", f["attr"]) for f in files))
    return out + bytes([K_END])


def write(name, folders, files, encode_header=False):
    packed = b"".join(p for f in folders for p in f.packs)
    header = bytes([K_HEADER, K_MAIN]) + streams_info(0, folders) + files_info(files) + bytes([K_END])
    if encode_header:
        hpacked, hprops = lzma1(header)
        hfolder = Folder([coder(ID_LZMA, hprops)], [len(header)], [hpacked], [header])
        header = bytes([K_ENCODED]) + streams_info(len(packed), [hfolder], [zlib.crc32(header)])
        packed += hpacked
    start = struct.pack("<QQI", len(packed), len(header), zlib.crc32(header))
    with open(os.path.join(HERE, name), "wb") as f:
        f.write(b"7z\xbc\xaf\x27\x1c\x00\x04" + struct.pack("<I", zlib.crc32(start)) + start + packed + header)


def text(seed, lines):
    return "".join(f"{seed} line {i}: the quick brown fox jumps over the lazy dog\n" for i in range(lines)).encode()


A, B = text("alpha", 300), text("beta", 120)
LINK = b"docs/a.txt"
UNICODE = "unicode name\n".encode()


def solid_files():
    return [
        {"name": "docs", "data": None, "attr": DIR_ATTR},
        {"name": "docs/a.txt", "data": A, "attr": FILE_ATTR},
        {"name": "docs/b.txt", "data": B, "attr": FILE_ATTR},
        {"name": "empty.txt", "data": b"", "empty": True, "attr": FILE_ATTR},
        {"name": "naïve 日本.txt", "data": UNICODE, "attr": FILE_ATTR},
        {"name": "link", "data": LINK, "attr": LINK_ATTR},
    ]


def lzma2_solid(name="lzma2-solid.7z"):
    streams = [A, B, UNICODE, LINK]
    packed, props = lzma2(b"".join(streams))
    size = sum(map(len, streams))
    write(name, [Folder([coder(ID_LZMA2, props)], [size], [packed], streams)], solid_files())


def lzma_encoded_header():
    streams = [A, B]
    packed, props = lzma1(b"".join(streams))
    files = [{"name": "a.txt", "data": A, "attr": 0x20}, {"name": "b.txt", "data": B, "attr": 0x20}]
    write("lzma-encoded-header.7z", [Folder([coder(ID_LZMA, props)], [len(A) + len(B)], [packed], streams)], files,
          encode_header=True)


def x86_code():
    out = bytearray()
    for i in range(4000):
        out += b"\x55\x48\x89\xe5\xe8" + struct.pack("<i", 0x1000 - i * 16) + b"\x5d\xc3\x90"
    return bytes(out)


def bcj_lzma2():
    code = x86_code()
    packed, props = lzma2(code, [{"id": lzma.FILTER_X86}])
    folder = chain(ID_BCJ, b"", ID_LZMA2, props, packed, len(code), [code])
    write("bcj-lzma2.7z", [folder], [{"name": "prog.bin", "data": code, "attr": 0x20}])


def delta_lzma():
    samples = b"".join(struct.pack("<hh", 600 * (i % 50), -800 * (i % 37)) for i in range(20000))
    raw = lzma.compress(samples, format=lzma.FORMAT_RAW, filters=[
        {"id": lzma.FILTER_DELTA, "dist": 4},
        {"id": lzma.FILTER_LZMA1, "dict_size": 1 << 16, "lc": 3, "lp": 0, "pb": 2}])
    props = bytes([0x5D]) + struct.pack("<I", 1 << 16)
    folder = chain(ID_DELTA, b"\x03", ID_LZMA, props, raw, len(samples), [samples])
    write("delta-lzma.7z", [folder], [{"name": "samples.raw", "data": samples, "attr": 0x20}])


def multi_folder():
    raw = bytes(range(256)) * 8
    packed, props = lzma2(B)
    folders = [Folder([coder(ID_COPY)], [len(raw)], [raw], [raw]),
               Folder([coder(ID_LZMA2, props)], [len(B)], [packed], [B])]
    files = [{"name": "raw.bin", "data": raw, "attr": 0x20}, {"name": "b.txt", "data": B, "attr": 0x20}]
    write("multi-folder.7z", folders, files)


def corrupt_crc():
    CRC_FLIP[A] = 0xFFFFFFFF
    lzma2_solid("corrupt-crc.7z")
    CRC_FLIP.clear()


def encrypted_marker():
    blob = bytes(64)
    folder = Folder([coder(ID_AES, b"\x13\x00"), coder(ID_LZMA2, b"\x10")], [64, 64], [blob], [bytes(64)],
                    bind_pairs=[(1, 0)])
    write("encrypted.7z", [folder], [{"name": "secret.txt", "data": bytes(64), "attr": 0x20}])


def truncated():
    data = open(os.path.join(HERE, "lzma2-solid.7z"), "rb").read()
    open(os.path.join(HERE, "truncated.7z"), "wb").write(data[: len(data) // 2])


def empty():
    start = struct.pack("<QQI", 0, 0, 0)
    with open(os.path.join(HERE, "empty.7z"), "wb") as f:
        f.write(b"7z\xbc\xaf\x27\x1c\x00\x04" + struct.pack("<I", zlib.crc32(start)) + start)


if __name__ == "__main__":
    lzma2_solid()
    lzma_encoded_header()
    bcj_lzma2()
    delta_lzma()
    multi_folder()
    corrupt_crc()
    encrypted_marker()
    truncated()
    empty()
