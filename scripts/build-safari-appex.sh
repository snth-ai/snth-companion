#!/usr/bin/env bash
# Build the Safari extension that ships inside SNTH Connect.app: an app
# extension (.appex) made of
#
#   - the native handler (Swift, app/safari): the relay between the
#     extension's JavaScript and the running app
#   - the SNTH Vault web extension itself (JavaScript, from its own
#     repository), with the manifest of the copy that lives in the app
#
#   scripts/build-safari-appex.sh <output folder> [--version <app version>]
#
# writes "<output folder>/SNTH Vault.appex", UNSIGNED. The release script
# (release-macos.sh) puts it into the app and signs it; nothing else does.
#
# The web extension comes from the COMMITTED tree of the extension's
# repository (git archive HEAD), like its own Safari edition
# (scripts/build-safari.sh there): what is not committed is not shipped.
#
# Needs: Xcode (xcodebuild), node, git.
#
# Settings (environment, optional):
#   SNTH_VAULT_EXTENSION   the extension's repository   (../snth-vault-extension)
#   SNTH_VAULT_REF         the commit to take           (HEAD)

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_REPO="${SNTH_VAULT_EXTENSION:-$ROOT/../snth-vault-extension}"
EXT_REF="${SNTH_VAULT_REF:-HEAD}"
SAFARI="$ROOT/app/safari"
PROJECT="$SAFARI/Extension/SNTHVault.xcodeproj"
TARGET="SNTH Vault"
APPEX_NAME="SNTH Vault.appex"
BUNDLE_ID="ai.snth.connect.safari"

OUT=""
VERSION=""
while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            VERSION="${2:-}"
            shift
            ;;
        -h | --help)
            sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "error: unknown option $1" >&2
            exit 2
            ;;
        *)
            [ -z "$OUT" ] || {
                echo "error: more than one output folder given" >&2
                exit 2
            }
            OUT="$1"
            ;;
    esac
    shift
done

note() { printf '    %s\n' "$*"; }
die() {
    printf '\nerror: %s\n' "$*" >&2
    exit 1
}

[ -n "$OUT" ] || die "give the output folder"
[ "$(uname -s)" = "Darwin" ] || die "the Safari extension is built on macOS only"
for tool in xcodebuild node git lipo plutil; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done
[ -n "$VERSION" ] || VERSION="$(node -p "require('$ROOT/app/package.json').version")"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like 1.2.3, got: $VERSION"
git -C "$EXT_REPO" rev-parse --git-dir >/dev/null 2>&1 ||
    die "the SNTH Vault extension repository is not at $EXT_REPO (set SNTH_VAULT_EXTENSION)"
EXT_COMMIT="$(git -C "$EXT_REPO" rev-parse --short=12 "$EXT_REF")" || die "no commit $EXT_REF in $EXT_REPO"
if [ -n "$(git -C "$EXT_REPO" status --porcelain -- manifest.json icons popup src scripts/connect-safari.js)" ]; then
    note "NOTE: $EXT_REPO has uncommitted changes. They are NOT in this build (commit $EXT_COMMIT is)."
fi

# Every Swift file of the relay must be in the Xcode project: a file the
# package builds and tests but the extension lacks would only show in Safari.
for file in "$SAFARI"/Sources/ConnectRelay/*.swift "$SAFARI"/Extension/*.swift; do
    grep -F "$(basename "$file") in Sources" "$PROJECT/project.pbxproj" >/dev/null ||
        die "$(basename "$file") is not in $PROJECT (add it to the Sources phase)"
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/snth-safari-appex.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# ------------------------------------------------------ the web extension

mkdir -p "$WORK/ext"
git -C "$EXT_REPO" archive "$EXT_REF" manifest.json icons popup src scripts/connect-safari.js | tar -x -C "$WORK/ext"
# The manifest of the copy that lives in the app (see that script).
EXT_LABEL="$(node "$WORK/ext/scripts/connect-safari.js" "$WORK/ext")" || die "could not write the manifest"
[ -n "$EXT_LABEL" ] || die "the manifest script wrote nothing (scripts/connect-safari.js in the extension's repository)"
EXT_VERSION="$(node -p "require('$WORK/ext/manifest.json').version")"
node -e '
    const m = require(process.argv[1]);
    const need = ["nativeMessaging", "cookies", "storage", "activeTab", "scripting"];
    const missing = need.filter((p) => !(m.permissions || []).includes(p));
    if (missing.length) { console.error("the manifest lacks: " + missing.join(", ")); process.exit(1); }
    if (m.key) { console.error("the manifest still has a Chromium key"); process.exit(1); }
    if ((m.host_permissions || []).some((h) => h.includes("*://") || h.includes("<all_urls>"))) {
        console.error("the manifest asks for sites at install"); process.exit(1);
    }
' "$WORK/ext/manifest.json" || die "the web extension's manifest is not the one for the app"
[ -f "$WORK/ext/src/safari.js" ] || die "the web extension has no Safari transport (src/safari.js): it is older than this app"
note "web extension: $EXT_LABEL, commit $EXT_COMMIT"

# ------------------------------------------------------ the native handler

# Built for both kinds of Mac whatever the app is built for: it is small,
# and a universal app needs it that way.
xcodebuild -project "$PROJECT" -target "$TARGET" -configuration Release \
    SYMROOT="$WORK/build" OBJROOT="$WORK/obj" \
    ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO \
    MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$VERSION" \
    CODE_SIGNING_ALLOWED=NO \
    build >"$WORK/xcodebuild.log" 2>&1 || {
    tail -40 "$WORK/xcodebuild.log"
    die "xcodebuild failed"
}
BUILT="$WORK/build/Release/$APPEX_NAME"
[ -d "$BUILT" ] || die "xcodebuild did not produce $APPEX_NAME"

# ------------------------------------------------------------- put together

RESOURCES="$BUILT/Contents/Resources"
mkdir -p "$RESOURCES"
for item in manifest.json icons popup src; do
    cp -R "$WORK/ext/$item" "$RESOURCES/"
done
# Which web extension this is, for whoever looks into the app later.
plutil -insert SNTHVaultExtensionVersion -string "$EXT_VERSION" "$BUILT/Contents/Info.plist"
plutil -insert SNTHVaultExtensionCommit -string "$EXT_COMMIT" "$BUILT/Contents/Info.plist"

plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$BUILT/Contents/Info.plist"; }
[ "$(plist CFBundleIdentifier)" = "$BUNDLE_ID" ] || die "the extension's bundle id is $(plist CFBundleIdentifier), not $BUNDLE_ID"
[ "$(plist CFBundleShortVersionString)" = "$VERSION" ] || die "the extension is version $(plist CFBundleShortVersionString), not $VERSION"
[ "$(plist NSExtension:NSExtensionPointIdentifier)" = "com.apple.Safari.web-extension" ] ||
    die "the extension does not declare itself a Safari web extension"
[ "$(plist NSExtension:NSExtensionPrincipalClass)" = "SNTHVault.SafariWebExtensionHandler" ] ||
    die "unexpected handler class: $(plist NSExtension:NSExtensionPrincipalClass)"
BINARY="$BUILT/Contents/MacOS/$(plist CFBundleExecutable)"
[ -x "$BINARY" ] || die "the extension has no executable"
ARCHS_BUILT="$(lipo -archs "$BINARY")"
for arch in arm64 x86_64; do
    case " $ARCHS_BUILT " in *" $arch "*) ;; *) die "the handler lacks $arch (has: $ARCHS_BUILT)" ;; esac
done
# The handler's class must really be in the binary under the name the
# Info.plist gives (as the Objective-C runtime spells "SNTHVault.Safari...").
HANDLER_MODULE="SNTHVault"
HANDLER_CLASS="SafariWebExtensionHandler"
grep -a -q "_TtC${#HANDLER_MODULE}${HANDLER_MODULE}${#HANDLER_CLASS}${HANDLER_CLASS}" "$BINARY" ||
    die "the handler class is not in the extension's binary"
if find "$BUILT" -name '._*' -o -name '.DS_Store' | grep . >/dev/null; then
    die "Finder metadata files inside the extension"
fi

mkdir -p "$OUT"
rm -rf "${OUT:?}/$APPEX_NAME"
ditto "$BUILT" "$OUT/$APPEX_NAME"
note "handler: $ARCHS_BUILT, version $VERSION"
note "$OUT/$APPEX_NAME (unsigned)"
