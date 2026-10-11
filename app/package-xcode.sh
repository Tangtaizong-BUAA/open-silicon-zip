#!/bin/bash
# Reproducible ARM64 build. No deployment, Finder restart, or guessed DerivedData.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(cd .. && pwd)
IDENT=${SIGN_IDENTITY:--}
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --locked --release --manifest-path "$ROOT/engine/fz-core/Cargo.toml"
xcodegen generate --spec XcodeGen/project.yml
xcodebuild -project XcodeGen/FastZip.xcodeproj -scheme FastZipApp -configuration Release \
  -derivedDataPath build/DerivedData CODE_SIGNING_ALLOWED=NO ARCHS=arm64 build
STAGE="$PWD/dist-stage/Open Silicon Zip.app"
mkdir -p dist-stage
# Replace only this generated bundle.
rm -rf "$STAGE"
ditto build/DerivedData/Build/Products/Release/FastZip.app "$STAGE"
cp "$ROOT/engine/fz-core/target/release/fz" "$STAGE/Contents/MacOS/fz"
bash scripts/make-icon.sh "$STAGE"
mkdir -p "$STAGE/Contents/Resources/Licenses"
cp "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$STAGE/Contents/Resources/Licenses/"
cp "$ROOT"/licenses/* "$STAGE/Contents/Resources/Licenses/"
cp "$ROOT/engine/vendor/sevenz-rust2/LICENSE" "$STAGE/Contents/Resources/Licenses/sevenz-rust2-APACHE-2.0.txt"
# Bundle the exact native liblzma used at build time; no Homebrew is needed at runtime.
LZMA=$(otool -L "$STAGE/Contents/MacOS/fz" | awk '/liblzma.*dylib/ {print $1; exit}')
if [[ "$LZMA" == /opt/* || "$LZMA" == /usr/local/* ]]; then
  cp -L "$LZMA" "$STAGE/Contents/Frameworks/liblzma.5.dylib"
  install_name_tool -change "$LZMA" '@executable_path/../Frameworks/liblzma.5.dylib' "$STAGE/Contents/MacOS/fz"
  # Never advertise support below the deployment target of a bundled native library.
  LZMA_MIN=$(otool -l "$STAGE/Contents/Frameworks/liblzma.5.dylib" | awk '/minos / {print $2; exit} /LC_VERSION_MIN_MACOSX/ {old=1} old && /version / {print $2; exit}')
  python3 - "$STAGE/Contents/Info.plist" "$LZMA_MIN" <<'PYPLIST'
import plistlib,sys
from pathlib import Path
p=Path(sys.argv[1]);info=plistlib.loads(p.read_bytes());version=sys.argv[2]
parse=lambda v: tuple(map(int,v.split('.')))
if version and parse(version)>parse(info.get('LSMinimumSystemVersion','13.0')):
    info['LSMinimumSystemVersion']=version
    p.write_bytes(plistlib.dumps(info))
PYPLIST
  codesign --force --sign "$IDENT" "$STAGE/Contents/Frameworks/liblzma.5.dylib"
fi
APPEX="$STAGE/Contents/PlugIns/FastZipFinderSync.appex"
# Swift linkers retain absolute module paths in debug symbol entries. Remove
# those before final signing; keep ordinary symbols for useful crash reports.
for BIN in "$STAGE/Contents/MacOS/FastZip" \
  "$STAGE/Contents/Frameworks/FastZipKit.framework/Versions/A/FastZipKit" \
  "$APPEX/Contents/MacOS/FastZipFinderSync" \
  "$APPEX/Contents/Frameworks/FastZipKit.framework/Versions/A/FastZipKit"; do
  if [ -f "$BIN" ]; then strip -S "$BIN"; fi
done
# Nested code is signed from the inside out, including the Rust helper.
if [ -d "$APPEX/Contents/Frameworks/FastZipKit.framework" ]; then
  codesign --force --sign "$IDENT" "$APPEX/Contents/Frameworks/FastZipKit.framework"
fi
codesign --force --sign "$IDENT" --entitlements scripts/appex-xcode.entitlements "$APPEX"
codesign --force --sign "$IDENT" "$STAGE/Contents/Frameworks/FastZipKit.framework"
codesign --force --sign "$IDENT" "$STAGE/Contents/MacOS/fz"
codesign --force --sign "$IDENT" "$STAGE/Contents/MacOS/FastZip"
codesign --force --sign "$IDENT" "$STAGE"
codesign --verify --deep --strict "$STAGE"
if otool -L "$STAGE/Contents/MacOS/fz" | grep -q /opt/homebrew; then
  echo 'ERROR: runtime dependency on Homebrew' >&2; exit 1
fi
printf 'Built: %s\n' "$STAGE"
