#!/bin/sh
# Regenerates the TAR corpus with GNU tar and the reference gzip/bzip2/xz/zstd tools.
# Usage: sh corpus/tar/generate.sh   (needs GNU tar >= 1.30, gzip, bzip2, xz, zstd)
set -eu
OUT=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
export TZ=UTC LC_ALL=C.UTF-8

cd "$WORK"
mkdir -p tree/dir/empty "tree/$(printf 'd%.0s' $(seq 1 60))/$(printf 'e%.0s' $(seq 1 60))"
i=0; while [ $i -lt 200 ]; do echo "line $i of the tar fixture"; i=$((i+1)); done > tree/dir/a.txt
printf 'second file\n' > tree/dir/b.txt
printf 'unicode name\n' > "tree/naïve 日本.txt"
printf 'deep\n' > "tree/$(printf 'd%.0s' $(seq 1 60))/$(printf 'e%.0s' $(seq 1 60))/$(printf 'f%.0s' $(seq 1 80)).txt"
ln -s dir/a.txt tree/link
ln tree/dir/b.txt tree/hard.txt
chmod 755 tree/dir/a.txt
find tree -exec touch -h -d '2025-03-04 05:06:07.123456789' {} +

common="--sort=name --owner=0 --group=0 --numeric-owner"
tar $common --format=gnu -cf "$OUT/gnu.tar" -C tree .
tar $common --format=pax -cf "$OUT/pax.tar" -C tree .
tar $common --format=ustar -cf "$OUT/ustar.tar" -C tree dir link
tar $common --format=v7 -cf "$OUT/v7.tar" -C tree dir

gzip -9 -n -c "$OUT/pax.tar" > "$OUT/pax.tar.gz"
bzip2 -9 -c "$OUT/pax.tar" > "$OUT/pax.tar.bz2"
xz -6 -c "$OUT/pax.tar" > "$OUT/pax.tar.xz"
zstd -q -19 -c "$OUT/pax.tar" > "$OUT/pax.tar.zst"
cp "$OUT/pax.tar.gz" "$OUT/pax.tgz"

# Single compressed files (not TAR), including a two-member gzip.
gzip -9 -N -c tree/dir/a.txt > "$OUT/a.txt.gz"
{ printf 'first\n' | gzip -c; printf 'second\n' | gzip -c; } > "$OUT/members.txt.gz"
xz -c tree/dir/b.txt > "$OUT/b.txt.xz"
zstd -q -c tree/dir/b.txt > "$OUT/b.txt.zst"
bzip2 -c tree/dir/b.txt > "$OUT/b.txt.bz2"

# Damaged and unusual archives.
head -c 600 "$OUT/pax.tar.gz" > "$OUT/truncated.tar.gz"
dd if=/dev/zero of=sparse.bin bs=1 count=0 seek=1048576 2>/dev/null
printf 'tail' >> sparse.bin
tar $common --format=gnu --sparse -cf "$OUT/sparse.tar" sparse.bin
