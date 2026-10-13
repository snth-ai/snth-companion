#!/usr/bin/env bash
# The macOS half of a SNTH Connect release. Run on the release Mac AFTER the
# CI workflow (connect-release.yml) has created the draft GitHub release for
# the tag with the Windows and Linux files and latest.json.
#
#   scripts/release-macos.sh <version> [--dry-run] [--no-upload] [--universal]
#
# What it does:
#   1. builds the app (arm64) signed with the Developer ID, hardened runtime
#   2. notarizes it with the keychain profile, staples the ticket, and checks
#      it with codesign and Gatekeeper (spctl)
#   3. makes the first-install download (a signed, notarized, stapled DMG)
#      and the updater bundle (.app.tar.gz of the STAPLED app) with its
#      signature from the updater key
#   4. uploads them to the draft release for connect-v<version> and adds the
#      darwin entry to that release's latest.json
#
# It stops with an error if the release or its latest.json is missing, if the
# built app is not version <version>, or if any check fails. It can be run
# again: every upload replaces what is there.
#
#   --dry-run    build, sign and package, but no notarization, no upload and
#                no latest.json edit; prints what it would have done
#   --no-upload  everything including notarization, but nothing goes to GitHub
#   --universal  one build for Apple silicon and Intel (needs
#                `rustup target add x86_64-apple-darwin`; about twice the
#                build time; not the default yet and never tried)
#
# Needs: Xcode command line tools, node + npm, rustup/cargo, gh (signed in),
# the Developer ID certificate and the notary profile in the login keychain,
# the updater key file, and its password in the login keychain.
#
# Settings (environment, all optional):
#   SNTH_CONNECT_REPO              GitHub repo           (snth-ai/snth-companion)
#   SNTH_CONNECT_SIGNING_IDENTITY  codesign identity     (Developer ID Application: Aleksandr Sabri (CZAP38JFAE))
#   SNTH_CONNECT_NOTARY_PROFILE    notarytool profile    (SNTH_NOTARY)
#   SNTH_CONNECT_UPDATER_KEY       updater key file      (~/.tauri/snth-connect.key)
#   SNTH_CONNECT_KEYCHAIN_ACCOUNT  keychain account of the key password ($USER)
#   CARGO_BUILD_JOBS               passed on to cargo

set -euo pipefail

REPO="${SNTH_CONNECT_REPO:-snth-ai/snth-companion}"
IDENTITY="${SNTH_CONNECT_SIGNING_IDENTITY:-Developer ID Application: Aleksandr Sabri (CZAP38JFAE)}"
NOTARY_PROFILE="${SNTH_CONNECT_NOTARY_PROFILE:-SNTH_NOTARY}"
KEY_FILE="${SNTH_CONNECT_UPDATER_KEY:-$HOME/.tauri/snth-connect.key}"
KEYCHAIN_SERVICE="snth-connect-updater-key-password"
KEYCHAIN_ACCOUNT="${SNTH_CONNECT_KEYCHAIN_ACCOUNT:-$USER}"
APP_NAME="SNTH Connect.app"

VERSION=""
DRY_RUN=0
NO_UPLOAD=0
UNIVERSAL=0

usage() {
    sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'
}

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        --no-upload) NO_UPLOAD=1 ;;
        --universal) UNIVERSAL=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        -*)
            echo "error: unknown option $arg" >&2
            exit 2
            ;;
        *)
            if [ -n "$VERSION" ]; then
                echo "error: more than one version given" >&2
                exit 2
            fi
            VERSION="$arg"
            ;;
    esac
done

step() { printf '\n==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
would() { printf '    [dry run] would run: %s\n' "$*"; }
die() {
    printf '\nerror: %s\n' "$*" >&2
    exit 1
}

[ -n "$VERSION" ] || {
    usage >&2
    exit 2
}
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like 1.2.3, got: $VERSION"
[ "$(uname -s)" = "Darwin" ] || die "this script runs on macOS only"

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="connect-v$VERSION"
if [ "$UNIVERSAL" = "1" ]; then
    TARGET="universal-apple-darwin"
    ARCH_LABEL="universal"
    FEED_TARGETS=("darwin-aarch64" "darwin-x86_64")
else
    TARGET="aarch64-apple-darwin"
    ARCH_LABEL="aarch64"
    FEED_TARGETS=("darwin-aarch64")
fi
APP="$ROOT/target/$TARGET/release/bundle/macos/$APP_NAME"
OUT="$ROOT/target/release-macos/$VERSION"
DMG="$OUT/SNTH-Connect_${VERSION}_${ARCH_LABEL}.dmg"
TARBALL="$OUT/SNTH-Connect_${VERSION}_${ARCH_LABEL}.app.tar.gz"
SIGNATURE="$TARBALL.sig"
TARBALL_URL="https://github.com/$REPO/releases/download/$TAG/$(basename "$TARBALL")"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/snth-connect-release.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

UPLOAD=1
if [ "$DRY_RUN" = "1" ] || [ "$NO_UPLOAD" = "1" ]; then
    UPLOAD=0
fi

# ---------------------------------------------------------------- preflight

step "Preflight ($TAG, $TARGET$([ "$DRY_RUN" = "1" ] && echo ", dry run"))"

for tool in node npm cargo xcrun codesign hdiutil ditto tar security; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done

PACKAGE_VERSION="$(node -p "require('$ROOT/app/package.json').version")"
[ "$PACKAGE_VERSION" = "$VERSION" ] ||
    die "app/package.json is version $PACKAGE_VERSION, not $VERSION. Bump it (and tag that commit) first."

if grep -q "REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY" "$ROOT/app/src-tauri/tauri.conf.json"; then
    die "app/src-tauri/tauri.conf.json still has the placeholder updater public key"
fi

security find-identity -v -p codesigning | grep -F "\"$IDENTITY\"" >/dev/null ||
    die "the signing identity is not in the keychain: $IDENTITY"
[ -f "$KEY_FILE" ] || die "the updater key file is missing: $KEY_FILE"

if [ "$UNIVERSAL" = "1" ]; then
    rustup target list --installed | grep -x "x86_64-apple-darwin" >/dev/null ||
        die "a universal build needs: rustup target add x86_64-apple-darwin"
fi

if [ "$UPLOAD" = "1" ]; then
    command -v gh >/dev/null 2>&1 || die "gh is not installed"
    RELEASE_ASSETS="$(gh release view "$TAG" --repo "$REPO" --json assets --jq '.assets[].name' 2>/dev/null)" ||
        die "there is no release for $TAG in $REPO. Push the tag and wait for the connect-release workflow first."
    printf '%s\n' "$RELEASE_ASSETS" | grep -x "latest.json" >/dev/null ||
        die "the $TAG release has no latest.json yet. Wait for the connect-release workflow to finish (it must be green)."
    note "the $TAG release exists and has latest.json"
fi
note "version $VERSION, identity and updater key are in place"

# -------------------------------------------------------------------- build

step "Build and sign the app"
(
    cd "$ROOT/app"
    [ -d node_modules ] || npm ci
    # Tauri signs with the identity (hardened runtime, secure timestamp).
    # Its own notarization stays off: this script does that step itself,
    # so the Apple account variables are kept away from the build.
    env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID \
        -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
        -u TAURI_SIGNING_PRIVATE_KEY -u TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
        APPLE_SIGNING_IDENTITY="$IDENTITY" \
        npm run tauri build -- --target "$TARGET" --bundles app
)
[ -d "$APP" ] || die "the build did not produce $APP"

BUILT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
[ "$BUILT_VERSION" = "$VERSION" ] || die "the built app is version $BUILT_VERSION, not $VERSION"

step "Check the signature"
codesign --verify --deep --strict --verbose=2 "$APP" || die "codesign --verify failed for the built app"
# The helper the browsers start for the SNTH Vault extension ships inside
# the app and must carry the same signature, or macOS refuses to run it.
HOST_HELPER="$APP/Contents/MacOS/snth-connect-host"
[ -x "$HOST_HELPER" ] || die "the browser helper is missing from the app (expected $HOST_HELPER)"
codesign --verify --strict "$HOST_HELPER" || die "the browser helper inside the app is not signed"
note "the browser helper is in the app and signed"
SIGN_INFO="$(codesign -dvv "$APP" 2>&1)"
printf '%s\n' "$SIGN_INFO" | grep -F "Authority=$IDENTITY" >/dev/null ||
    die "the app is not signed by: $IDENTITY"
printf '%s\n' "$SIGN_INFO" | grep -E '^CodeDirectory .*flags=.*runtime' >/dev/null ||
    die "the app is signed without the hardened runtime (notarization would refuse it)"
printf '%s\n' "$SIGN_INFO" | grep -E '^Timestamp=' >/dev/null ||
    die "the signature has no secure timestamp (notarization would refuse it)"
note "signed by the Developer ID, hardened runtime, timestamped"

# The Apps page (Calendar, Contacts, Reminders, Notes, Messages) sends
# commands to other apps. Under the hardened runtime macOS refuses that
# without this entitlement, and without the usage sentence it does not
# even ask. Both come from app/src-tauri (Entitlements.plist, Info.plist).
codesign -d --entitlements - --xml "$APP" 2>/dev/null | grep -F "com.apple.security.automation.apple-events" >/dev/null ||
    die "the signed app lacks the com.apple.security.automation.apple-events entitlement (app/src-tauri/Entitlements.plist, bundle.macOS.entitlements in tauri.conf.json)"
/usr/libexec/PlistBuddy -c 'Print :NSAppleEventsUsageDescription' "$APP/Contents/Info.plist" >/dev/null 2>&1 ||
    die "the built app has no NSAppleEventsUsageDescription (app/src-tauri/Info.plist)"
note "entitled to ask for control of other apps (Automation), with its usage sentence"

mkdir -p "$OUT"

# ---------------------------------------------------------------- notarize

# Submit one file to the notary service and wait for the verdict.
notarize() {
    local file="$1" log="$WORK/notary-$(basename "$1").log"
    xcrun notarytool submit "$file" --keychain-profile "$NOTARY_PROFILE" --wait 2>&1 | tee "$log" || true
    grep -q "status: Accepted" "$log" ||
        die "notarization of $(basename "$file") was not accepted. See the log above; details: xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE"
}

step "Notarize the app and staple the ticket"
if [ "$DRY_RUN" = "1" ]; then
    would "ditto -c -k --keepParent \"$APP\" <zip>"
    would "xcrun notarytool submit <zip> --keychain-profile $NOTARY_PROFILE --wait"
    would "xcrun stapler staple \"$APP\""
    would "spctl -a -vv \"$APP\"   (expects: accepted, source=Notarized Developer ID)"
else
    APP_ZIP="$WORK/SNTH-Connect-notarize.zip"
    ditto -c -k --keepParent "$APP" "$APP_ZIP"
    notarize "$APP_ZIP"
    xcrun stapler staple "$APP" || die "could not staple the ticket to the app"
    xcrun stapler validate "$APP" || die "the stapled ticket does not validate"
    GATEKEEPER="$(spctl -a -vv "$APP" 2>&1)" || die "Gatekeeper refuses the app: $GATEKEEPER"
    printf '%s\n' "$GATEKEEPER" | grep "source=Notarized Developer ID" >/dev/null ||
        die "Gatekeeper does not see a notarized Developer ID app: $GATEKEEPER"
    codesign --verify --deep --strict "$APP" || die "codesign --verify failed after stapling"
    note "notarized, stapled, accepted by Gatekeeper"
fi

# ---------------------------------------------- first-install download (DMG)

step "Make the disk image (first install)"
DMG_STAGE="$WORK/dmg"
mkdir -p "$DMG_STAGE"
ditto "$APP" "$DMG_STAGE/$APP_NAME"
# The shortcut people drag the app onto. An app that is opened straight
# from the download instead runs from a read-only copy and cannot update.
ln -s /Applications "$DMG_STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "SNTH Connect" -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG" >/dev/null
codesign --sign "$IDENTITY" --timestamp "$DMG" || die "could not sign the disk image"
codesign --verify --strict "$DMG" || die "codesign --verify failed for the disk image"
if [ "$DRY_RUN" = "1" ]; then
    would "xcrun notarytool submit \"$DMG\" --keychain-profile $NOTARY_PROFILE --wait"
    would "xcrun stapler staple \"$DMG\""
    would "spctl -a -t open --context context:primary-signature -vv \"$DMG\""
else
    notarize "$DMG"
    xcrun stapler staple "$DMG" || die "could not staple the ticket to the disk image"
    xcrun stapler validate "$DMG" || die "the disk image's ticket does not validate"
    DMG_GATEKEEPER="$(spctl -a -t open --context context:primary-signature -vv "$DMG" 2>&1)" ||
        die "Gatekeeper refuses the disk image: $DMG_GATEKEEPER"
fi
note "$(basename "$DMG")"

# ------------------------------------------------------- updater bundle

step "Make the updater bundle and sign it with the updater key"
rm -f "$TARBALL" "$SIGNATURE"
# The archive holds the app folder at its top level, which is what the
# updater expects. No Finder metadata files: they would end up inside the
# installed app and break its signature.
COPYFILE_DISABLE=1 tar -czf "$TARBALL" -C "$(dirname "$APP")" "$APP_NAME"
if tar -tzf "$TARBALL" | grep '/\._' >/dev/null; then
    die "the updater bundle contains ._ metadata files"
fi
# Unpack it the way a tester's app would get it, and check the result.
UNPACKED="$WORK/unpacked"
mkdir -p "$UNPACKED"
tar -xzf "$TARBALL" -C "$UNPACKED"
codesign --verify --deep --strict "$UNPACKED/$APP_NAME" || die "the app inside the updater bundle fails codesign --verify"
if [ "$DRY_RUN" != "1" ]; then
    xcrun stapler validate "$UNPACKED/$APP_NAME" || die "the app inside the updater bundle has no valid notarization ticket"
fi

# The key's password comes from the login keychain and goes straight to the
# signer's environment. It is never printed or written anywhere.
KEY_PASSWORD="$(security find-generic-password -a "$KEYCHAIN_ACCOUNT" -s "$KEYCHAIN_SERVICE" -w 2>/dev/null)" ||
    die "the updater key password is not in the keychain (service $KEYCHAIN_SERVICE, account $KEYCHAIN_ACCOUNT)"
(
    cd "$ROOT/app"
    env -u TAURI_SIGNING_PRIVATE_KEY \
        TAURI_SIGNING_PRIVATE_KEY_PATH="$KEY_FILE" \
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$KEY_PASSWORD" \
        npx tauri signer sign "$TARBALL" >/dev/null
) || die "tauri signer could not sign the updater bundle"
unset KEY_PASSWORD
[ -s "$SIGNATURE" ] || die "the signer did not write $SIGNATURE"
note "$(basename "$TARBALL") + .sig"

# ------------------------------------------------------------------ upload

# Add (or replace) the darwin entries in a latest.json file.
add_to_feed() {
    node -e '
        const fs = require("fs");
        const [file, version, url, sigFile, ...targets] = process.argv.slice(1);
        const feed = JSON.parse(fs.readFileSync(file, "utf8"));
        const feedVersion = String(feed.version || "").replace(/^v/, "");
        if (feedVersion !== version) {
            console.error(`latest.json is for version ${feedVersion}, not ${version}`);
            process.exit(1);
        }
        feed.platforms = feed.platforms || {};
        const signature = fs.readFileSync(sigFile, "utf8").trim();
        for (const target of targets) feed.platforms[target] = { signature, url };
        fs.writeFileSync(file, JSON.stringify(feed, null, 2) + "\n");
        console.log("    platforms now in the feed: " + Object.keys(feed.platforms).sort().join(", "));
    ' "$@"
}

step "Upload to the $TAG release and add macOS to latest.json"
if [ "$UPLOAD" = "1" ]; then
    gh release upload "$TAG" "$DMG" "$TARBALL" "$SIGNATURE" --repo "$REPO" --clobber
    gh release download "$TAG" --repo "$REPO" --pattern latest.json --dir "$WORK" --clobber ||
        die "could not download latest.json from the $TAG release"
    add_to_feed "$WORK/latest.json" "$VERSION" "$TARBALL_URL" "$SIGNATURE" "${FEED_TARGETS[@]}" ||
        die "latest.json was not changed"
    gh release upload "$TAG" "$WORK/latest.json" --repo "$REPO" --clobber
    note "uploaded. The release is still a draft: check it, then publish."
else
    [ "$DRY_RUN" = "1" ] && label="[dry run] would run" || label="[--no-upload] skipped"
    note "$label: gh release upload $TAG <dmg> <app.tar.gz> <sig> --repo $REPO --clobber"
    note "$label: gh release download $TAG --pattern latest.json, then add to it:"
    for target in "${FEED_TARGETS[@]}"; do
        note "    platforms[\"$target\"] = { url: \"$TARBALL_URL\", signature: <contents of $(basename "$SIGNATURE")> }"
    done
    note "$label: gh release upload $TAG latest.json --repo $REPO --clobber"
fi

step "Done"
note "first install: $DMG"
note "updater:       $TARBALL"
note "signature:     $SIGNATURE"
if [ "$DRY_RUN" = "1" ]; then
    note "DRY RUN: these files are signed but NOT notarized. Do not hand them out."
fi
