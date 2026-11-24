#!/usr/bin/env bash
# Builds an installable BarBuds disk image.
#
#   scripts/package.sh            # Release build
#   CONFIG=Debug scripts/package.sh   # Debug build (keeps the Debug menu)
#
# Output: dist/BarBuds-<version>-<git sha>-<config>.dmg
#
# Signing uses the project's automatic signing (an Apple Development
# certificate). The build is not notarized, so on first launch testers must
# allow it once in System Settings › Privacy & Security › Open Anyway.
set -euo pipefail

CONFIG="${CONFIG:-Release}"
case "$CONFIG" in
  Release | Debug) ;;
  *) echo "CONFIG must be Release or Debug, got '$CONFIG'" >&2; exit 1 ;;
esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$ROOT/build"
DIST="$ROOT/dist"
PROJECT="$ROOT/app/BarBuds/BarBuds.xcodeproj"

step() { printf '\n==> %s\n' "$*"; }

step "Building the Rust core (make core)"
make -C "$ROOT" core

step "Building BarBuds ($CONFIG, arm64 + x86_64)"
mkdir -p "$BUILD"
LOG="$BUILD/xcodebuild-$CONFIG.log"
if ! xcodebuild \
  -project "$PROJECT" \
  -scheme BarBuds \
  -configuration "$CONFIG" \
  -destination 'generic/platform=macOS' \
  -derivedDataPath "$BUILD/DerivedData" \
  ARCHS="arm64 x86_64" \
  ONLY_ACTIVE_ARCH=NO \
  build >"$LOG" 2>&1; then
  grep -E "error:" "$LOG" | sort -u | head -20 >&2
  echo "Build failed. Full log: $LOG" >&2
  exit 1
fi

APP="$BUILD/DerivedData/Build/Products/$CONFIG/BarBuds.app"
[[ -d "$APP" ]] || { echo "Build produced no app at $APP" >&2; exit 1; }

step "Verifying the app"
codesign --verify --deep --strict "$APP"
ARCHS_BUILT="$(lipo -archs "$APP/Contents/MacOS/BarBuds")"
for arch in arm64 x86_64; do
  [[ " $ARCHS_BUILT " == *" $arch "* ]] || { echo "Missing $arch slice (got: $ARCHS_BUILT)" >&2; exit 1; }
done
PLIST="$APP/Contents/Info.plist"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
MIN_OS="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$PLIST")"
SIGNER="$(codesign -dvv "$APP" 2>&1 | sed -n 's/^Authority=//p' | head -1)"
echo "Version $VERSION, architectures: $ARCHS_BUILT, requires macOS $MIN_OS"
echo "Signed by: ${SIGNER:-ad hoc}"

step "Creating the disk image"
SHA="$(git -C "$ROOT" rev-parse --short HEAD)"
DIRTY="$(git -C "$ROOT" status --porcelain --untracked-files=no | grep -q . && echo "-dirty" || true)"
NAME="BarBuds-$VERSION-$SHA$DIRTY-$(echo "$CONFIG" | tr '[:upper:]' '[:lower:]')"
STAGING="$BUILD/dmg"
rm -rf "$STAGING"
mkdir -p "$STAGING" "$DIST"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
cat > "$STAGING/How to install.txt" <<EOF
BarBuds $VERSION ($CONFIG build, $SHA$DIRTY). Requires macOS $MIN_OS or later.

1. Drag BarBuds into the Applications folder.
2. Open BarBuds from Applications. macOS will say it can't verify the app,
   because this test build isn't notarized by Apple. Click Done.
3. Open System Settings › Privacy & Security, scroll down to the message
   about BarBuds, and click "Open Anyway". Confirm, and BarBuds starts.
4. Allow Bluetooth access when asked.

BarBuds has no Dock icon: look for the earbuds icon in the menu bar. Pair
your earbuds in System Settings › Bluetooth first; supported ones appear
automatically.
EOF
rm -f "$DIST/$NAME.dmg"
hdiutil create -quiet -volname "BarBuds" -srcfolder "$STAGING" -ov -format UDZO "$DIST/$NAME.dmg"
rm -rf "$STAGING"

step "Done"
echo "$DIST/$NAME.dmg"
