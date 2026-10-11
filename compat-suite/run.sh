#!/usr/bin/env bash
# Windows-compatibility matrix, macOS/Linux side (06 §4). Creates every writer profile over the
# CJK + emoji + RTL + long-name corpus and verifies it with every independent opener available
# locally (unzip, 7z, tar, gzip, xz, bzip2, zstd). The Windows side (Explorer/WinRAR/PeaZip)
# consumes ./out/*.zip|7z|tar.* and ./out/manifest.txt on a Windows runner.
set -euo pipefail
FZ="$(realpath "${FZ:-$(dirname "$0")/../engine/fz-core/target/release/fz}")"
O="${1:-$(mktemp -d "${TMPDIR:-/tmp}/osz-compat.XXXXXX")}"
if [[ -d "$O" && -n "$(ls -A "$O")" ]]; then
  echo "Refusing to overwrite a nonempty compatibility output directory: $O" >&2; exit 1
fi
mkdir -p "$O/corpus"; cd "$O"
c=corpus
mkdir -p "$c/空格 dir" "$c/deep" "$c/emoji"
echo ascii > "$c/plain.txt"; echo sp > "$c/with space.txt"
echo cjk > "$c/中文文件名.txt"; echo jp > "$c/日本語ファイル.txt"; echo ko > "$c/한국어.txt"
echo emoji > "$c/emoji/😀🎉.txt"; echo rtl > "$c/עברית_مرحبا.txt"
printf 'e\xcc\x81' > "$c/combining-é.txt"
echo long > "$c/$(printf 'n%.0s' $(seq 1 200)).txt"
d="$c/deep"; for i in $(seq 1 40); do d="$d/l$i"; done; mkdir -p "$d"; echo deep > "$d/f.txt"
head -c 300000 /dev/urandom > "$c/photo.jpg"; seq 1 50000 > "$c/data.csv"
(cd "$c" && find . | LC_ALL=C sort) > manifest.txt
pass=0; fail=0; skip=0
check() { if "${@:2}" >/dev/null 2>&1; then echo "PASS $1"; pass=$((pass+1)); else echo "FAIL $1"; fail=$((fail+1)); fi; }
for lv in store fast normal max; do
  "$FZ" create zip-$lv.zip $c --level $lv >/dev/null
  check "zip-$lv unzip -t"  unzip -tq zip-$lv.zip
  check "zip-$lv 7z t"      7z t zip-$lv.zip
  check "zip-$lv fz test"   "$FZ" test zip-$lv.zip
done
"$FZ" create aes.zip $c --password pw >/dev/null;           check "zip-aes256 7z t" 7z t -ppw aes.zip
"$FZ" create aes128.zip $c --password pw --crypto aes128 >/dev/null; check "zip-aes128 7z t" 7z t -ppw aes128.zip
"$FZ" create zc.zip $c --password pw --crypto zipcrypto >/dev/null; check "zip-zipcrypto unzip -t" unzip -P pw -tq zc.zip
"$FZ" create span.zip $c --split 200KB >/dev/null;           check "zip-spanned 7z t" 7z t span.zip
"$FZ" create bz.zip $c --method bzip2 >/dev/null;            check "zip-bzip2 7z t" 7z t bz.zip
if unzip -tq bz.zip > bzip2-unzip.log 2>&1; then
  echo 'PASS zip-bzip2 unzip -t'; pass=$((pass+1))
else
  status=$?
  if [[ $status -eq 81 ]]; then echo 'SKIP zip-bzip2 unzip: this system opener lacks method support'; skip=$((skip+1))
  else echo "FAIL zip-bzip2 unzip: exit $status"; fail=$((fail+1)); fi
fi
"$FZ" create x.zipx $c >/dev/null;                           check "zipx-zstd fz test" "$FZ" test x.zipx
"$FZ" create n.7z $c >/dev/null;                             check "7z 7z t" 7z t n.7z
"$FZ" create ns.7z $c --solid off >/dev/null;                check "7z-nonsolid 7z t" 7z t ns.7z
"$FZ" create mx.7z $c --level max >/dev/null;                check "7z-max 7z t" 7z t mx.7z
"$FZ" create he.7z $c --password pw --encrypt-names >/dev/null; check "7z-header-enc 7z t" 7z t -ppw he.7z
"$FZ" create sp.7z $c --split 200KB >/dev/null;              check "7z-split 7z t" 7z t sp.7z.001
"$FZ" create t.tar $c >/dev/null;   check "tar tar tf" tar tf t.tar
"$FZ" create t.tgz $c >/dev/null;   check "tgz tar tzf" tar tzf t.tgz
"$FZ" create t.tbz $c >/dev/null;   check "tbz tar tjf" tar tjf t.tbz
"$FZ" create t.txz $c >/dev/null;   check "txz tar tJf" tar tJf t.txz
command -v zstd >/dev/null && { "$FZ" create t.tzst $c >/dev/null; check "tzst zstd -t" zstd -tq t.tzst; }
# hygiene: no macOS junk, no backslashes, '/' separators, no ZIP64 for small archives
mkdir -p junk; echo x > junk/._a; echo x > junk/.DS_Store; mkdir -p junk/__MACOSX; echo x > junk/__MACOSX/a; echo ok > junk/keep.txt
"$FZ" create junk.zip junk >/dev/null
check "no-apple-junk" bash -c "! unzip -Z1 junk.zip | grep -E '(^|/)(\._|\.DS_Store|__MACOSX)'"
check "no-backslash" bash -c "! unzip -Z1 zip-normal.zip | grep -F '\\'"
check "no-zip64-records" python3 -c 'import pathlib; assert bytes([80,75,6,6]) not in pathlib.Path("zip-normal.zip").read_bytes()'
# roundtrip byte-compare through the engine for the main profiles
for a in zip-normal.zip aes.zip n.7z t.tgz span.zip; do
  pw=(); [[ $a == aes.zip ]] && pw=(--password pw)
  rm -rf rt; "$FZ" extract $a -o rt ${pw[@]+"${pw[@]}"} >/dev/null 2>&1; check "roundtrip $a" diff -r rt/$c $c
done
echo "== $pass passed, $fail failed, $skip skipped =="; [ $fail -eq 0 ]
