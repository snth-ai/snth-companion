#!/usr/bin/env bash
# Builds and tests the native boundary, without installing or changing authdb.
# SNTH_TEAM_ID must be the real signing team for a distributable bundle.
# The default deliberately cannot match a production broker.
set -euo pipefail
cd "$(dirname "$0")/.."
team="${SNTH_TEAM_ID:-UNCONFIGURED}"
if [[ ! "$team" =~ ^[A-Z0-9]+$ ]]; then
    echo "SNTH_TEAM_ID must contain only uppercase letters and digits" >&2
    exit 2
fi
out="$PWD/target/locked-use"
src="$PWD/app/macos/locked-use"
bundle="$out/SNTHConnectAuthorizationPlugin.bundle"
guardian="$out/SNTHLockedUseGuardian.app"
validation="$out/SNTHLockedUseValidation.app"
mkdir -p "$bundle/Contents/MacOS"
mkdir -p "$guardian/Contents/MacOS"
mkdir -p "$validation/Contents/MacOS"
cp "$src/Info.plist" "$bundle/Contents/Info.plist"
cp "$src/Guardian-Info.plist" "$guardian/Contents/Info.plist"
cp "$src/Validation-Info.plist" "$validation/Contents/Info.plist"
flags=(-std=c11 -D_DARWIN_C_SOURCE -Wall -Wextra -Werror -fblocks -mmacosx-version-min=12.0 "-DSNTH_TEAM_ID=\"$team\"")
frameworks=(-framework Security -framework SystemConfiguration -framework CoreFoundation)
xcrun clang "${flags[@]}" -arch arm64 -arch x86_64 -fvisibility=hidden -bundle \
    "$src/AuthorizationPlugin.c" "${frameworks[@]}" \
    -o "$bundle/Contents/MacOS/SNTHConnectAuthorizationPlugin"
xcrun clang "${flags[@]}" "$src/AuthorizationPlugin.c" "$src/test_plugin.c" \
    "${frameworks[@]}" -o "$out/test-plugin"
"$out/test-plugin"
xcrun clang "${flags[@]}" "$src/Lease.c" "$src/test_lease.c" -o "$out/test-lease"
"$out/test-lease"
xcrun clang "${flags[@]}" "$src/GuardianState.c" "$src/test_guardian.c" -o "$out/test-guardian"
"$out/test-guardian"
xcrun clang "${flags[@]}" -fobjc-arc -arch arm64 -arch x86_64 \
    "$src/Guardian.m" "$src/GuardianState.c" "$src/Session.m" "$src/Watchdog.m" \
    -framework AppKit -framework ApplicationServices -framework Security \
    -o "$guardian/Contents/MacOS/SNTHLockedUseGuardian"
xcrun clang "${flags[@]}" -fobjc-arc "$src/Session.m" "$src/test_session.m" \
    -framework AppKit -framework ApplicationServices -framework Security -o "$out/test-session"
"$out/test-session"
xcrun clang "${flags[@]}" "$src/Watchdog.m" "$src/test_watchdog.c" -o "$out/test-watchdog"
python3 "$src/test_watchdog.py" "$out/test-watchdog"
xcrun clang "${flags[@]}" -fobjc-arc -arch arm64 -arch x86_64 \
    "$src/Validation.m" "$src/Session.m" -framework AppKit -framework ApplicationServices \
    -framework Security -framework LocalAuthentication \
    -o "$validation/Contents/MacOS/SNTHLockedUseValidation"
xcrun clang "${flags[@]}" -arch arm64 -arch x86_64 "$src/Broker.c" "$src/Lease.c" \
    "${frameworks[@]}" -o "$out/ai.snth.connect.locked-use-broker"
cp "$src/ai.snth.connect.locked-use-broker.plist" "$out/"
if [[ -n "${SNTH_CONNECT_SIGNING_IDENTITY:-}" ]]; then
    if [[ "$team" == UNCONFIGURED ]]; then
        echo "Set SNTH_TEAM_ID before signing a distribution bundle" >&2
        exit 2
    fi
    codesign --force --options runtime --timestamp --sign "$SNTH_CONNECT_SIGNING_IDENTITY" "$bundle"
    codesign --verify --strict "$bundle"
    codesign --force --options runtime --timestamp --identifier ai.snth.connect.locked-use-broker \
        --sign "$SNTH_CONNECT_SIGNING_IDENTITY" "$out/ai.snth.connect.locked-use-broker"
    codesign --verify --strict "$out/ai.snth.connect.locked-use-broker"
    codesign --force --options runtime --timestamp --sign "$SNTH_CONNECT_SIGNING_IDENTITY" "$guardian"
    codesign --verify --strict "$guardian"
    codesign --force --options runtime --timestamp --sign "$SNTH_CONNECT_SIGNING_IDENTITY" "$validation"
    codesign --verify --strict "$validation"
fi
echo "Built $bundle (not installed; native runtime integration is still required)"
