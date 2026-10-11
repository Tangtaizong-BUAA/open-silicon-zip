#!/bin/bash
# Build distributable preview artifacts. No installation, Finder restart or upload.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=${1:?Usage: scripts/package-release.sh VERSION}
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
  echo 'Invalid release version' >&2; exit 1
fi
OUT="$ROOT/release-artifacts/v$VERSION"
if [ -e "$OUT" ]; then
  echo "Output already exists: $OUT" >&2; exit 1
fi
# Keep builder home paths out of Rust panic strings and native codec diagnostics.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/build"
export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$HOME=/build"
export CXXFLAGS="${CXXFLAGS:-} -ffile-prefix-map=$HOME=/build"
bash "$ROOT/app/package-app.sh"
APP="$ROOT/app/dist-stage/Open Silicon Zip.app"
codesign --verify --deep --strict "$APP"
python3 - "$APP" <<'PYAPP'
import os, pathlib, sys
root = pathlib.Path(sys.argv[1])
home = os.fsencode(os.environ['HOME'])
for path in root.rglob('*'):
    if path.is_file() and not path.is_symlink():
        if home in path.read_bytes():
            raise SystemExit(f'Builder home path remains in {path.relative_to(root)}')
if list(root.rglob('*.provisionprofile')) or list(root.rglob('*.mobileprovision')):
    raise SystemExit('Do not publish a provisioning profile in this preview')
PYAPP
if [ "$(lipo -archs "$APP/Contents/MacOS/fz")" != arm64 ]; then
  echo 'Release engine must be native arm64' >&2; exit 1
fi
mkdir -p "$OUT"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/osz-release.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
NAME="OpenSiliconZip-$VERSION-macos-arm64"
ditto --noextattr --noqtn -c -k --keepParent "$APP" "$OUT/$NAME.zip"
unzip -tq "$OUT/$NAME.zip"
mkdir "$WORK/image"
ditto --noextattr --noqtn "$APP" "$WORK/image/Open Silicon Zip.app"
ln -s /Applications "$WORK/image/Applications"
hdiutil create -quiet -volname 'Open Silicon Zip' -srcfolder "$WORK/image" \
  -fs HFS+ -format UDZO "$OUT/$NAME.dmg"
hdiutil verify "$OUT/$NAME.dmg"
(
  cd "$OUT"
  shasum -a 256 "$NAME.dmg" "$NAME.zip" > SHA256SUMS.txt
  shasum -a 256 -c SHA256SUMS.txt
)
printf 'Release artifacts: %s\n' "$OUT"
