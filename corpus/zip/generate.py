"""Regenerates the ZIP test corpus. Run from anywhere: python3 -I corpus/zip/generate.py

Outputs are committed; rerun only when adding cases. Requires Info-ZIP `zip`.
"""

import os
import shutil
import struct
import subprocess
import tempfile
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
MTIME = 1767323045  # 2026-01-02T03:04:05Z
DOS_TIME = (2026, 1, 2, 3, 4, 6)


def out(name):
    return os.path.join(HERE, name)


def infozip_basic():
    """Info-ZIP 3.0: deflated + stored files, empty file, folders, a symlink."""
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.join(tmp, "src")
        os.makedirs(os.path.join(root, "dir", "sub"))
        with open(os.path.join(root, "dir", "a.txt"), "w") as f:
            f.write("hello from a\n" * 100)
        open(os.path.join(root, "dir", "sub", "empty.txt"), "w").close()
        with open(os.path.join(root, "b.bin"), "wb") as f:
            f.write(bytes((i * 7) % 256 for i in range(1000)))
        os.symlink("dir/a.txt", os.path.join(root, "link"))
        for dirpath, dirnames, filenames in os.walk(root):
            for name in filenames + dirnames:
                os.utime(os.path.join(dirpath, name), (MTIME, MTIME), follow_symlinks=False)
        os.utime(root, (MTIME, MTIME))
        target = out("infozip-basic.zip")
        if os.path.exists(target):
            os.remove(target)
        subprocess.run(
            ["zip", "-q", "-r", "-y", "-n", ".bin", target, "dir", "b.bin", "link"],
            cwd=root, check=True, env={**os.environ, "TZ": "UTC"},
        )
        enc = out("infozip-encrypted.zip")
        if os.path.exists(enc):
            os.remove(enc)
        subprocess.run(
            ["zip", "-q", "-P", "secret", enc, "dir/a.txt"],
            cwd=root, check=True, env={**os.environ, "TZ": "UTC"},
        )


def info(name, mode=None):
    zi = zipfile.ZipInfo(name, DOS_TIME)
    if mode is not None:
        zi.create_system = 3
        zi.external_attr = mode << 16
    return zi


def python_utf8():
    with zipfile.ZipFile(out("python-utf8.zip"), "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(info("café/naïve.txt"), "unicode folder\n")
        z.writestr(info("日本語.txt"), "japanese name\n")


def evil_traversal():
    with zipfile.ZipFile(out("evil-traversal.zip"), "w") as z:
        for name in ["../evil.txt", "/abs.txt", "a/../../up.txt", "C:\\win.txt", "ok.txt"]:
            zi = info("x")
            zi.filename = name  # bypass zipfile's own sanitising
            z.writestr(zi, f"payload {name}\n")


def evil_symlink():
    link = 0o120777
    with zipfile.ZipFile(out("evil-symlink.zip"), "w") as z:
        z.writestr(info("abs", link), "/tmp")
        z.writestr(info("dot", link), ".")
        z.writestr(info("esc", link), "dot/..")
        z.writestr(info("up", link), "../outside")
        z.writestr(info("dir/file.txt", 0o100644), "real file\n")
        z.writestr(info("good", link), "dir/file.txt")


def corrupt_crc():
    data = b"this data will not match its CRC\n" * 10
    path = out("corrupt-crc.zip")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(info("broken.txt"), data)
        z.writestr(info("fine.txt"), "fine\n")
    good = struct.pack("<I", zlib.crc32(data))
    bad = struct.pack("<I", zlib.crc32(data) ^ 0xFFFFFFFF)
    raw = open(path, "rb").read()
    assert raw.count(good) == 2
    open(path, "wb").write(raw.replace(good, bad))


def sfx_prefixed():
    stub = b"MZ" + bytes(4094)
    raw = open(out("infozip-basic.zip"), "rb").read()
    open(out("sfx-prefixed.exe"), "wb").write(stub + raw)


def zip64_stored():
    """Hand-built ZIP64 archive: every size/offset lives in ZIP64 fields."""
    files = [("big/one.txt", b"first zip64 file\n"), ("two.txt", b"second\n" * 3)]
    dos_time = (3 << 11) | (4 << 5) | 3
    dos_date = ((2026 - 1980) << 9) | (1 << 5) | 2
    body, central = b"", b""
    for name, data in files:
        name_b = name.encode()
        crc = zlib.crc32(data)
        offset = len(body)
        local_extra = struct.pack("<HHQQ", 1, 16, len(data), len(data))
        body += struct.pack(
            "<IHHHHHIIIHH", 0x04034B50, 45, 0, 0, dos_time, dos_date, crc,
            0xFFFFFFFF, 0xFFFFFFFF, len(name_b), len(local_extra),
        ) + name_b + local_extra + data
        cextra = struct.pack("<HHQQQ", 1, 24, len(data), len(data), offset)
        central += struct.pack(
            "<IHHHHHHIIIHHHHHII", 0x02014B50, 45, 45, 0, 0, dos_time, dos_date, crc,
            0xFFFFFFFF, 0xFFFFFFFF, len(name_b), len(cextra), 0, 0, 0, 0, 0xFFFFFFFF,
        ) + name_b + cextra
    cd_offset = len(body)
    zip64_eocd_pos = cd_offset + len(central)
    zip64_eocd = struct.pack(
        "<IQHHIIQQQQ", 0x06064B50, 44, 45, 45, 0, 0, len(files), len(files),
        len(central), cd_offset,
    )
    locator = struct.pack("<IIQI", 0x07064B50, 0, zip64_eocd_pos, 1)
    eocd = struct.pack(
        "<IHHHHIIH", 0x06054B50, 0, 0, 0xFFFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0,
    )
    open(out("zip64-stored.zip"), "wb").write(body + central + zip64_eocd + locator + eocd)


if __name__ == "__main__":
    if shutil.which("zip") is None:
        raise SystemExit("Info-ZIP `zip` is required")
    infozip_basic()
    python_utf8()
    evil_traversal()
    evil_symlink()
    corrupt_crc()
    sfx_prefixed()
    zip64_stored()
