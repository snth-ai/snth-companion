#!/usr/bin/env bash
# build-dmg.sh — produce a signed + notarized + stapled .dmg containing
# SNTH Companion.app for macOS distribution outside the Mac App Store.
#
# Prerequisites (one-time):
#   1. Apple Developer Program membership (required for Developer ID certs).
#   2. Developer ID Application certificate installed in login keychain.
#      (Check with: `security find-identity -v -p codesigning` — look for
#      "Developer ID Application: <Team Name> (<TEAM_ID>)".)
#   3. App-specific password for notarytool stored with:
#      `xcrun notarytool store-credentials SNTH_NOTARY \
#            --apple-id "you@example.com" --team-id "XXXXXXXXXX" \
#            --password "app-specific-password"`
#   4. AppIcon.icns placed at packaging/macos/AppIcon.icns.
#
# Env vars consumed:
#   VERSION        — required, e.g. "0.1.0"
#   DEVELOPER_ID   — required, "Developer ID Application: Name (TEAMID)"
#   NOTARY_PROFILE — defaults to "SNTH_NOTARY"
#   OUT_DIR        — defaults to ./dist
#   SKIP_NOTARIZE  — set to "1" to skip notarize+staple (testing only;
#                    the resulting DMG will fail Gatekeeper on another Mac).
#
# Usage:
#   VERSION=0.1.0 DEVELOPER_ID="Developer ID Application: YourName (XXXXXXXXXX)" \
#     ./packaging/macos/build-dmg.sh
#
# The DMG lands at $OUT_DIR/SNTH-Companion-$VERSION.dmg.

set -euo pipefail

: "${VERSION:?VERSION is required}"
: "${DEVELOPER_ID:?DEVELOPER_ID is required}"
NOTARY_PROFILE="${NOTARY_PROFILE:-SNTH_NOTARY}"
OUT_DIR="${OUT_DIR:-$(pwd)/dist}"
SKIP_NOTARIZE="${SKIP_NOTARIZE:-0}"

# --- paths ---
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG_DIR="$REPO_ROOT/packaging/macos"
INFO_TEMPLATE="$PKG_DIR/Info.plist"
ENTITLEMENTS="$PKG_DIR/entitlements.plist"
ICON_FILE="$PKG_DIR/AppIcon.icns"
BUILD_DIR="$OUT_DIR/build"
APP_DIR="$BUILD_DIR/SNTH Companion.app"
DMG_PATH="$OUT_DIR/SNTH-Companion-$VERSION.dmg"

mkdir -p "$BUILD_DIR" "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
rm -f "$DMG_PATH"

# --- build React SPA bundle (embedded via embed.FS at Go compile time) ---
echo "[1a/7] building UI bundle…"
cd "$REPO_ROOT/ui"
if [[ ! -d node_modules ]]; then
  npm ci
fi
npm run build

# --- compile universal binary (arm64 + amd64) ---
echo "[1b/7] compiling Go binary (universal)…"
cd "$REPO_ROOT"
GOARCH=arm64 go build -trimpath -ldflags="-s -w" \
  -o "$BUILD_DIR/snth-companion.arm64" ./cmd/companion
GOARCH=amd64 go build -trimpath -ldflags="-s -w" \
  -o "$BUILD_DIR/snth-companion.amd64" ./cmd/companion
lipo -create -output "$APP_DIR/Contents/MacOS/snth-companion" \
  "$BUILD_DIR/snth-companion.arm64" "$BUILD_DIR/snth-companion.amd64"
chmod +x "$APP_DIR/Contents/MacOS/snth-companion"

# --- Info.plist with injected version ---
echo "[2/7] assembling bundle…"
sed "s|__VERSION__|$VERSION|g" "$INFO_TEMPLATE" > "$APP_DIR/Contents/Info.plist"
if [[ -f "$ICON_FILE" ]]; then
  cp "$ICON_FILE" "$APP_DIR/Contents/Resources/AppIcon.icns"
else
  echo "  warn: no AppIcon.icns at $ICON_FILE — bundle will have no icon."
fi

# --- sign ---
echo "[3/7] signing bundle with $DEVELOPER_ID…"
codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" \
  --sign "$DEVELOPER_ID" \
  "$APP_DIR/Contents/MacOS/snth-companion"
codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" \
  --sign "$DEVELOPER_ID" \
  "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

# --- build DMG ---
echo "[4/7] building DMG…"
# hdiutil occasionally fails to detach a previous ghost mount; prune first.
DMG_TMP="$BUILD_DIR/staging.dmg"
rm -f "$DMG_TMP"
hdiutil create -volname "SNTH Companion" \
  -srcfolder "$APP_DIR" \
  -ov -format UDZO "$DMG_TMP"

# --- sign DMG ---
echo "[5/7] signing DMG…"
codesign --force --timestamp --sign "$DEVELOPER_ID" "$DMG_TMP"

if [[ "$SKIP_NOTARIZE" == "1" ]]; then
  echo "[6/7] skipping notarize (SKIP_NOTARIZE=1)"
  mv "$DMG_TMP" "$DMG_PATH"
  echo "[7/7] done (unnotarized): $DMG_PATH"
  exit 0
fi

# --- notarize ---
echo "[6/7] submitting to notarytool (profile=$NOTARY_PROFILE)…"
xcrun notarytool submit "$DMG_TMP" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait

# --- staple ---
echo "[7/7] stapling ticket to DMG…"
xcrun stapler staple "$DMG_TMP"
xcrun stapler validate "$DMG_TMP"

mv "$DMG_TMP" "$DMG_PATH"
echo ""
echo "✓ DMG ready: $DMG_PATH"
echo "  Verify externally with:"
echo "    spctl -a -vvv -t install '$DMG_PATH'"
