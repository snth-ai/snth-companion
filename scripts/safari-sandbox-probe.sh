#!/usr/bin/env bash
# Does the way the Safari extension reaches the app work on this Mac?
#
# The extension's native handler is sandboxed and signed with the Developer
# ID, without a provisioning profile. The app is not sandboxed, has the
# hardened runtime and the same signature. The two meet at a Unix socket in
# the folder of an app group whose id starts with the team id. This script
# runs exactly that pair, without Safari and without the app:
#
#   app side     a copy of `connectd` signed like the app (Developer ID,
#                hardened runtime, the app group entitlement), serving the
#                Safari door in the group's folder
#   handler side a copy of `connect-relay-harness` (the same relay code the
#                extension is built from) signed like the extension
#                (Developer ID, hardened runtime, app sandbox + the app group
#                and nothing else)
#
# and lets `connectd selftest safari --group` play the extension. It uses an
# app group of its own (<team>.ai.snth.connect.probe), never the app's.
#
#   scripts/safari-sandbox-probe.sh
#
# Needs: `cargo build -p connectd` done before, Xcode (it builds the test host
# itself, with `swift build`, about half a minute) and the Developer ID in the
# keychain.
#
# What it leaves behind: ONE small folder. The signed copies live in a temp
# folder and the probe group's folder is removed at the end, but macOS makes a
# sandbox container for the test host (~/Library/Containers/
# ai.snth.connect.probe.relay, shown in Finder as "connect-relay-probe") and
# lets no program delete a container, not even one of the same team (observed
# on macOS 26: "Operation not permitted"). Its contents are emptied; the empty
# folder can be moved to the Trash in Finder. A second run reuses it. Nothing
# is registered with LaunchServices or with Safari.
#
# Settings (environment, optional):
#   SNTH_CONNECT_SIGNING_IDENTITY  codesign identity (Developer ID Application: Aleksandr Sabri (CZAP38JFAE))
#   CONNECTD                       the connectd to copy   (target/debug/connectd)

set -euo pipefail

IDENTITY="${SNTH_CONNECT_SIGNING_IDENTITY:-Developer ID Application: Aleksandr Sabri (CZAP38JFAE)}"
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONNECTD="${CONNECTD:-$ROOT/target/debug/connectd}"
RELAY_ID="ai.snth.connect.probe.relay"

step() { printf '\n==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die() {
    printf '\nerror: %s\n' "$*" >&2
    exit 1
}

[ "$(uname -s)" = "Darwin" ] || die "this script runs on macOS only"
[ -x "$CONNECTD" ] || die "no connectd at $CONNECTD (cargo build -p connectd)"
command -v swift >/dev/null 2>&1 || die "swift is not installed (Xcode)"
security find-identity -v -p codesigning | grep -F "\"$IDENTITY\"" >/dev/null ||
    die "the signing identity is not in the keychain: $IDENTITY"

TEAM="$(printf '%s' "$IDENTITY" | sed -n 's/.*(\([A-Z0-9]\{10\}\))$/\1/p')"
[ -n "$TEAM" ] || die "cannot read the team id from the identity: $IDENTITY"
GROUP="$TEAM.ai.snth.connect.probe"
GROUP_DIR="$HOME/Library/Group Containers/$GROUP"
CONTAINER_DIR="$HOME/Library/Containers/$RELAY_ID"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/snth-safari-probe.XXXXXX")"
cleanup() {
    step "Clean up"
    # The signed copy of connectd removes both folders itself at the end of
    # its run (it is signed by the team they belong to). This is the second
    # try, for a run that ended early.
    if [ -x "$WORK/connectd" ]; then
        "$WORK/connectd" selftest safari --only-remove --group "$GROUP" --remove-group-folder \
            --remove-container "$RELAY_ID" >/dev/null 2>&1 || true
    fi
    rm -rf "$WORK"
    if [ -e "$GROUP_DIR" ]; then
        note "COULD NOT REMOVE: $GROUP_DIR"
    else
        note "gone: $GROUP_DIR"
    fi
    if [ -e "$CONTAINER_DIR" ]; then
        rm -rf "$CONTAINER_DIR/Data" 2>/dev/null || true
        note "left (macOS lets no program delete a sandbox container): $CONTAINER_DIR"
        note "to remove it: Finder > Go > Go to Folder > ~/Library/Containers, move \"connect-relay-probe\" to the Trash"
    else
        note "gone: $CONTAINER_DIR"
    fi
}
trap cleanup EXIT

step "Build the test host as a sandboxed program can be started"
# A sandboxed program needs a bundle id for its container. The extension has
# one in its Info.plist; a plain program carries it inside the binary.
cat >"$WORK/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$RELAY_ID</string>
<key>CFBundleName</key><string>connect-relay-probe</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
</dict></plist>
PLIST
swift build -j "${CARGO_BUILD_JOBS:-4}" --package-path "$ROOT/app/safari" --scratch-path "$WORK/build" \
    --product connect-relay-harness \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$WORK/Info.plist" >"$WORK/swift-build.log" 2>&1 ||
    { tail -30 "$WORK/swift-build.log"; die "swift build failed"; }
RELAY="$WORK/build/debug/connect-relay-harness"
[ -x "$RELAY" ] || die "swift build did not produce the test host"

step "Sign the two programs the way the app and its extension are signed"
plist() { # plist <file> <sandboxed 0|1>
    {
        echo '<?xml version="1.0" encoding="UTF-8"?>'
        echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
        echo '<plist version="1.0"><dict>'
        [ "$2" = "1" ] && echo '<key>com.apple.security.app-sandbox</key><true/>'
        [ "$2" = "0" ] && echo '<key>com.apple.security.automation.apple-events</key><true/>'
        echo "<key>com.apple.security.application-groups</key><array><string>$GROUP</string></array>"
        echo '</dict></plist>'
    } >"$1"
}
plist "$WORK/app.entitlements" 0
plist "$WORK/appex.entitlements" 1
cp "$CONNECTD" "$WORK/connectd"
cp "$RELAY" "$WORK/relay"
codesign --force --options runtime --timestamp=none --sign "$IDENTITY" \
    --entitlements "$WORK/app.entitlements" --identifier ai.snth.connect.probe.app "$WORK/connectd"
codesign --force --options runtime --timestamp=none --sign "$IDENTITY" \
    --entitlements "$WORK/appex.entitlements" --identifier "$RELAY_ID" "$WORK/relay"
for bin in connectd relay; do
    codesign --verify --strict "$WORK/$bin" || die "$bin: the signature does not verify"
done
codesign -d --entitlements - --xml "$WORK/relay" 2>/dev/null | grep -F "com.apple.security.app-sandbox" >/dev/null ||
    die "the test host is not sandboxed"
note "app side:     Developer ID, hardened runtime, group $GROUP, not sandboxed"
note "handler side: Developer ID, hardened runtime, group $GROUP, SANDBOXED, no other entitlement"
note "no provisioning profile on either"

step "Run the Safari door between them, in the group's folder"
# --app-first: the app side makes the group's folder before the sandboxed
# side ever runs, as on a computer where the app was opened before Safari
# started the extension.
"$WORK/connectd" selftest safari --relay "$WORK/relay" --group "$GROUP" --app-first --remove-group-folder \
    --remove-container "$RELAY_ID"

step "Result"
note "a sandboxed, Developer-ID-signed program reached a non-sandboxed one"
note "over a Unix socket in ~/Library/Group Containers/$GROUP/run"
