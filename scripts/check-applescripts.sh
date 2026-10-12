#!/usr/bin/env bash
# check-applescripts.sh: compile every AppleScript the Apps page uses
# (crates/connect-core/src/apps/mac.rs) against the dictionaries of the
# real apps on this Mac, WITHOUT running any of them.
#
# Compiling checks the grammar and every app term (that Calendar still has
# "allday event", that Messages still has "participant"...). It is worth
# running after a macOS upgrade, before a release.
#
# Nothing is sent to any app: the compiler runs inside a sandbox that
# forbids sending Apple events and opening apps, so this cannot raise a
# permission window, start an app or read anything of yours. If macOS
# ever needed to ask an app for its dictionary, the compile would fail
# here instead of asking.
#
# Usage: scripts/check-applescripts.sh        (macOS only)

set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$HERE/../crates/connect-core/src/apps/mac.rs"
[ "$(uname -s)" = "Darwin" ] || { echo "macOS only"; exit 2; }
[ -f "$SRC" ] || { echo "not found: $SRC"; exit 2; }
for t in osacompile sandbox-exec python3; do
  command -v "$t" >/dev/null 2>&1 || { echo "missing tool: $t"; exit 2; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/snth-applescripts.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
cat > "$WORK/profile.sb" <<'SB'
(version 1)
(allow default)
(deny appleevent-send)
(deny lsopen)
(deny process-fork)
SB

# Cut the script constants out of the Rust source: PRELUDE + each script,
# and the one-line questions used to ask for a permission.
python3 -I - "$SRC" "$WORK" <<'PY'
import re, sys
src = open(sys.argv[1], encoding="utf-8").read()
out = sys.argv[2]
consts = dict(re.findall(r'const ([A-Z_]+): &str = r#"(.*?)"#;', src, re.S))
prelude = consts.pop("PRELUDE")
for name, body in consts.items():
    open(f"{out}/{name}.applescript", "w", encoding="utf-8").write(prelude + "\n" + body)
for i, probe in enumerate(re.findall(r'=> "(tell application \\"[A-Za-z]+\\" to count [a-z]+)\\n"', src)):
    open(f"{out}/PROBE_{i}.applescript", "w", encoding="utf-8").write(probe.replace('\\"', '"') + "\n")
PY

fails=0; count=0
for f in "$WORK"/*.applescript; do
  name="$(basename "$f" .applescript)"
  count=$((count+1))
  if err="$(sandbox-exec -f "$WORK/profile.sb" /usr/bin/osacompile -o "$WORK/$name.scpt" "$f" 2>&1)"; then
    echo "OK    $name"
  else
    fails=$((fails+1)); echo "FAIL  $name: $err"
  fi
done
echo "== $count scripts, $fails failed (macOS $(sw_vers -productVersion))"
[ "$count" -ge 16 ] || { echo "expected at least 16 scripts: the source layout changed"; exit 1; }
[ "$fails" -eq 0 ]
