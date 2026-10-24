#!/usr/bin/env bash
# p1-local.sh: end-to-end proof for SNTH Connect P1 (the Rust core) against
# the Go `connect-spike` peer on loopback. Prints PASS/FAIL per line.
#
# It drives the headless `connectd` CLI (one link per instance) through a
# control FIFO and the spike's admin API. The owner's apps (instances 4 and
# 5) run against connectd's in-memory stand-in (--apps mac / windows): no
# real app, clipboard or notification is touched. Nothing is deployed; nothing
# leaves this machine except the ipify probe. It binds 127.0.0.1 only,
# kills only the PIDs it starts, and uses the user's Cua daemon READ-ONLY
# (list_windows) when one is running, never touching its configuration.
#
# Usage:   scripts/p1-local.sh
# Env:
#   OPENPAW_DIR     openpaw worktree with ./cmd/connect-spike
#                   (default: ../openpaw-connect-p0)
#   P1_SIZE_MB      proxy transfer size (default 4)
#   P1_SKIP_BUILD=1 reuse existing binaries
#   P1_SKIP_CUA=1   skip the Cua MCP checks
#   P1_SKIP_INTERNET=1  skip the one check that reaches the internet
#   P1_KEEP=1       keep the work dir
#
# Safety: strictly one build at a time; -j 4.

set -u
set -o pipefail

# cua-driver installs to ~/.local/bin, which a non-login shell may not have.
export PATH="$HOME/.local/bin:$PATH"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
OPENPAW_DIR="${OPENPAW_DIR:-$(dirname "$REPO")/openpaw-connect-p0}"
SIZE_MB="${P1_SIZE_MB:-4}"
CONNECTD="$REPO/target/release/connectd"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/snth-connect-p1.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
SPIKE="$WORK/connect-spike"
PIDS=()
FD_OPEN=0
PASSES=0
FAILS=0

say()  { printf '%s\n' "$*"; }
pass() { PASSES=$((PASSES+1)); say "PASS  $*"; }
fail() { FAILS=$((FAILS+1)); say "FAIL  $*"; }
info() { say "      $*"; }

cleanup() {
  [ "$FD_OPEN" = "1" ] && exec 3>&- 2>/dev/null
  for pid in "${PIDS[@]:-}"; do [ -n "$pid" ] && kill -INT "$pid" 2>/dev/null; done
  sleep 0.3
  for pid in "${PIDS[@]:-}"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null; done
  wait 2>/dev/null
  if [ "${P1_KEEP:-0}" = "1" ] || [ "$FAILS" -gt 0 ]; then
    rm -f "$WORK/blob.bin" "$WORK"/*-copy-* 2>/dev/null
    say "logs kept in $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

need() { command -v "$1" >/dev/null 2>&1 || { say "missing tool: $1"; exit 2; }; }
for t in go cargo curl jq shasum openssl nc python3; do need "$t"; done
[ -d "$OPENPAW_DIR/cmd/connect-spike" ] || { say "OPENPAW_DIR=$OPENPAW_DIR has no cmd/connect-spike"; exit 2; }

free_port() { local p; while :; do p=$((20000 + RANDOM % 30000)); nc -z 127.0.0.1 "$p" >/dev/null 2>&1 || { echo "$p"; return; }; done; }

# ---------------------------------------------------------------- build
if [ "${P1_SKIP_BUILD:-0}" != "1" ]; then
  say "== build (one at a time, -j4)"
  ( cd "$OPENPAW_DIR" && go build -o "$SPIKE" ./cmd/connect-spike ) || { say "go build failed"; exit 2; }
  ( cd "$REPO" && cargo build -p connectd --release -j 4 2>&1 | tail -1 ) || { say "cargo build failed"; exit 2; }
else
  ( cd "$OPENPAW_DIR" && go build -o "$SPIKE" ./cmd/connect-spike ) || { say "go build failed"; exit 2; }
fi
[ -x "$CONNECTD" ] || { say "no connectd at $CONNECTD"; exit 2; }

# auto-detect the Cua daemon
CUA_BIN="${CUA_BIN:-$(command -v cua-driver 2>/dev/null || echo "$HOME/.local/bin/cua-driver")}"
SKIP_CUA="${P1_SKIP_CUA:-0}"
if [ "$SKIP_CUA" != "1" ]; then
  if [ -x "$CUA_BIN" ] && "$CUA_BIN" status 2>/dev/null | grep -q "daemon is running"; then
    CUA_BEFORE="$("$CUA_BIN" status 2>/dev/null)"
    info "Cua daemon detected ($CUA_BIN); MCP checks attach read-only to the user daemon"
  else
    SKIP_CUA=1
    info "no Cua daemon; skipping MCP checks"
  fi
fi

# ---------------------------------------------------------------- setup
export CONNECT_SPIKE_TOKEN="$(openssl rand -hex 24)"
PORT_WS="$(free_port)"; PORT_PROXY="$(free_port)"; PORT_HTTP="$(free_port)"
SERVER="http://127.0.0.1:$PORT_WS"
PROXY="http://127.0.0.1:$PORT_PROXY"
ORIGIN="http://127.0.0.1:$PORT_HTTP"
WS_URL="ws://127.0.0.1:$PORT_WS/api/connect/ws"

ROOT="$WORK/shared"; mkdir -p "$ROOT/docs"; OUTSIDE="$WORK/outside"; mkdir -p "$OUTSIDE"
echo "hello from the shared folder" > "$ROOT/docs/note.txt"
echo "must not be readable" > "$OUTSIDE/secret.txt"
ln -s "$OUTSIDE" "$ROOT/escape"
ROOT_P="$(cd "$ROOT" && pwd -P)"
NOTE_SHA="$(shasum -a 256 "$ROOT/docs/note.txt" | awk '{print $1}')"

# cookie fixture: a Chromium DB with a known key
COOKIE_DB="$WORK/Cookies"; COOKIE_PW="test-safe-storage-pw"
"$CONNECTD" make-cookie-fixture --db "$COOKIE_DB" --key "$COOKIE_PW" \
  --domain .youtube.com --name SID --value secret-sid >/dev/null

say "== payload: ${SIZE_MB} MB"
head -c "$((SIZE_MB*1024*1024))" /dev/urandom > "$WORK/blob.bin"
BLOB_SHA="$(shasum -a 256 "$WORK/blob.bin" | awk '{print $1}')"

"$SPIKE" serve -listen "127.0.0.1:$PORT_WS" -proxy "127.0.0.1:$PORT_PROXY" > "$WORK/spike.log" 2>&1 &
PIDS+=("$!")
"$SPIKE" testhttp -listen "127.0.0.1:$PORT_HTTP" -file "$WORK/blob.bin" > "$WORK/testhttp.log" 2>&1 &
PIDS+=("$!")
for _ in $(seq 1 50); do nc -z 127.0.0.1 "$PORT_WS" >/dev/null 2>&1 && break; sleep 0.1; done

spike() { CONNECT_SPIKE_TOKEN="$CONNECT_SPIKE_TOKEN" "$SPIKE" "$@" -server "$SERVER" 2>/dev/null; }

CD_PID=""; CD_LOG=""; CD_FIFO=""
start_connectd() { # $1 tag, rest = extra flags
  local tag="$1"; shift
  CD_LOG="$WORK/connectd-$tag.log"; CD_FIFO="$WORK/ctl-$tag"
  mkfifo "$CD_FIFO"
  "$CONNECTD" run --url "$WS_URL" --token "$CONNECT_SPIKE_TOKEN" \
    --secret-file "$WORK/secret-$tag.json" --journal "$WORK/journal-$tag.jsonl" \
    --cua-attach user --cua-bin "$CUA_BIN" --cookie-chromium chrome="$COOKIE_DB" --cookie-key "$COOKIE_PW" \
    "$@" < "$CD_FIFO" > "$CD_LOG" 2>&1 &
  CD_PID=$!; PIDS+=("$CD_PID")
  exec 3> "$CD_FIFO"; FD_OPEN=1
}
ctl() { echo "$1" >&3; }
stop_connectd() { exec 3>&- 2>/dev/null; FD_OPEN=0; kill -INT "$CD_PID" 2>/dev/null; sleep 0.4; }
wait_log() { # $1 pattern $2 timeout_s
  local t=0; while [ "$t" -lt "${2:-5}0" ]; do grep -q "$1" "$CD_LOG" 2>/dev/null && return 0; sleep 0.1; t=$((t+1)); done; return 1; }

# ================================================================ INSTANCE 1
say "== INSTANCE 1: balanced + allow-local + approve all"
start_connectd A --preset balanced --allow-local --approve '*' --root "${ROOT_P}:rw"

# 1. hello v1
if OUT="$(spike wait -timeout 15s)"; then
  CAPS="$(echo "$OUT" | jq -r '.devices[0].capabilities|join(",")')"
  REV1="$(echo "$OUT" | jq -r '.devices[0].settings_rev')"
  MODE="$(echo "$OUT" | jq -r '.devices[0].settings.net.mode')"
  # The balanced preset turns apps on too; this instance offers no app
  # (connectd's default), so the list in the hello is empty.
  if [ "$CAPS" = "net,cookies,computer,files,apps" ] && [ "$MODE" = "all" ] &&
     [ "$(echo "$OUT" | jq -r '.devices[0].settings.apps.apps // [] | length')" = "0" ]; then
    pass "1  protocol v1 hello: caps=$CAPS net.mode=$MODE rev=$REV1"
    info "$(echo "$OUT" | jq -c '.devices[0]|{os,arch,app_version,online}')"
  else
    fail "1  unexpected hello: caps=$CAPS mode=$MODE"
  fi
else
  fail "1  no hello within 15 s"; tail -8 "$CD_LOG" | sed 's/^/      /'
fi

# 2. proxy both directions (loopback origin; allow-local on)
DOWN_SHA="$(curl -sS --noproxy '' -x "$PROXY" -m 120 "$ORIGIN/file" | shasum -a 256 | awk '{print $1}')"
[ "$DOWN_SHA" = "$BLOB_SHA" ] && pass "2a download ${SIZE_MB} MB through the proxy: sha256 equal" || fail "2a download sha256 mismatch"
UP="$(curl -sS --noproxy '' -x "$PROXY" -m 120 -T "$WORK/blob.bin" "$ORIGIN/upload")"
[ "$(echo "$UP" | jq -r '.sha256' 2>/dev/null)" = "$BLOB_SHA" ] && pass "2b upload ${SIZE_MB} MB through the proxy: sha256 equal" || fail "2b upload sha256 mismatch: $UP"
info "proxy counters: $(spike devices | jq -c '.proxy')"

# 3. files read + write round trip
RD="$(spike files -op read -path "$ROOT_P/docs/note.txt")"
[ "$(echo "$RD" | jq -r '.sha256' 2>/dev/null)" = "$NOTE_SHA" ] && pass "3a files.read: sha256 equal, $(echo "$RD" | jq -r '.bytes') bytes" || fail "3a files.read mismatch: $RD"
WR="$(spike files -op write -mode replace -path "$ROOT_P/docs/written.txt" -data 'written by the synth')"
if echo "$WR" | jq -e '.size' >/dev/null 2>&1; then
  BACK="$(spike files -op read -path "$ROOT_P/docs/written.txt")"
  WANT="$(printf 'written by the synth' | shasum -a 256 | awk '{print $1}')"
  [ "$(echo "$BACK" | jq -r '.sha256')" = "$WANT" ] && pass "3b files.write round trip: sha256 equal" || fail "3b write read-back mismatch"
else
  fail "3b files.write failed: $WR"
fi
# escapes refused
ESC="$(spike files -op read -path "$ROOT_P/escape/secret.txt")"
case "$(echo "$ESC" | jq -r '.error' 2>/dev/null)" in
  denied:*) pass "3c symlink escape refused: \"$(echo "$ESC" | jq -r '.error' | sed "s#$WORK#<work>#g")\"" ;;
  *) fail "3c symlink escape not refused: $ESC" ;;
esac

# 4. cookies via the fixture (values never printed)
CK="$(spike cookies -domain youtube.com -reason 'watch an age-gated video')"
if [ "$(echo "$CK" | jq -r '.count' 2>/dev/null)" = "1" ] && [ "$(echo "$CK" | jq -r '.source')" = "chrome" ]; then
  pass "4  cookies.get via fixture: count=1 names=$(echo "$CK" | jq -c '.names') source=chrome (no values)"
else
  fail "4  cookies.get unexpected: $CK"
fi

# 5. MCP: catalog needs no approval; a normal stream (auto mode) is served
if [ "$SKIP_CUA" != "1" ]; then
  ML="$(spike mcp-list)"
  N="$(echo "$ML" | jq -r '.tool_count' 2>/dev/null)"
  if [ "${N:-0}" -ge 20 ]; then
    pass "5a catalog mcp stream served without approval: $N tools, $(echo "$ML" | jq -r '.server_name') $(echo "$ML" | jq -r '.server_version')"
  else
    fail "5a catalog mcp-list failed: $ML"
  fi
  MC="$(spike mcp-call -tool list_windows)"
  [ "$(echo "$MC" | jq -r '.is_error' 2>/dev/null)" = "false" ] && pass "5b normal mcp stream (auto mode) tools/call ok" || fail "5b normal mcp-call failed: $MC"
else
  info "5  MCP checks skipped (no Cua daemon)"
fi

# 7. pause / resume
ctl "pause"; wait_log "OK paused" 3
CODE="$(curl -sS --noproxy '' -x "$PROXY" -m 10 -o /dev/null -w '%{http_code}' "$ORIGIN/hello")"
# presence travels the control stream; give it a moment to land.
PAUSED=false
for _ in $(seq 1 20); do
  PAUSED="$(spike devices | jq -r '.devices[0].paused')"
  [ "$PAUSED" = "true" ] && break; sleep 0.2
done
[ "$CODE" = "403" ] && [ "$PAUSED" = "true" ] && pass "7a pause: proxy 403, presence paused=true" || fail "7a pause: proxy=$CODE paused=$PAUSED"
ctl "resume"; wait_log "OK resumed" 3; sleep 0.3
BODY="$(curl -sS --noproxy '' -x "$PROXY" -m 10 "$ORIGIN/hello")"
[ "$BODY" = "hello from testhttp" ] && pass "7b resume: proxy works again" || fail "7b resume: got \"$BODY\""

# 8. real internet through the proxy (the one request that leaves this
#    machine; P1_SKIP_INTERNET=1 leaves it out)
if [ "${P1_SKIP_INTERNET:-0}" = "1" ]; then
  info "8  real internet check skipped (P1_SKIP_INTERNET=1)"
else
  IP="$(curl -sS --noproxy '' -x "$PROXY" -m 20 https://api.ipify.org 2>/dev/null)"
  echo "$IP" | grep -Eq '^[0-9a-fA-F:.]+$' && pass "8  real internet via proxy -> $IP" || fail "8  no IP via proxy: ${IP:-empty}"
fi

# 6. settings change propagates (balanced -> careful flips net.mode all->ask).
#    Done after the proxy checks: applying a preset also resets allow-local.
ctl "preset careful"; sleep 0.8
OUT="$(spike devices)"
REV2="$(echo "$OUT" | jq -r '.devices[0].settings_rev')"
MODE2="$(echo "$OUT" | jq -r '.devices[0].settings.net.mode')"
if [ "$REV2" -gt "$REV1" ] && [ "$MODE2" = "ask" ]; then
  pass "6  settings change propagated: rev $REV1 -> $REV2, net.mode all -> ask"
else
  fail "6  settings did not propagate: rev=$REV2 mode=$MODE2"
fi

# 9. unpair: synth acks, the app wipes the token and stops
ctl "unpair"; wait_log "OK unpaired" 8
if spike wait -offline -timeout 10s >/dev/null; then
  TOKLEFT="$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print("yes" if "pairing_token" in d else "no")' "$WORK/secret-A.json" 2>/dev/null || echo "no")"
  ACK="$(grep -c "unpaired (ack)\|unpaired" "$CD_LOG")"
  [ "$TOKLEFT" = "no" ] && pass "9  unpair: synth acked, token wiped (401 on reconnect needs a real synth; the spike has no token store)" || fail "9  unpair: token still present ($TOKLEFT)"
else
  fail "9  device still online after unpair"
fi
stop_connectd

# ================================================================ INSTANCE 2
say "== INSTANCE 2: balanced, NO allow-local (private ranges refused)"
start_connectd B --preset balanced --approve '*' --root "${ROOT_P}:rw"
spike wait -timeout 15s >/dev/null || fail "B  no hello"
deny_check() { # $1 label $2 url
  local hdr code why
  hdr="$(curl -sS --noproxy '' -x "$PROXY" -m 15 -o /dev/null -D - "$2" 2>/dev/null)"
  code="$(echo "$hdr" | head -1 | awk '{print $2}')"
  why="$(echo "$hdr" | grep -i '^x-connect-error:' | head -1 | sed 's/^[^:]*: //' | tr -d '\r')"
  case "$why" in
    denied:*) [ "$code" = "403" ] && pass "$1 -> 403 \"$why\"" || fail "$1 -> $code \"$why\"" ;;
    *) fail "$1 -> $code \"$why\" (want 403 denied:)" ;;
  esac
}
deny_check "10a loopback 127.0.0.1"          "$ORIGIN/hello"
deny_check "10b cloud metadata 169.254.169.254" "http://169.254.169.254/latest/meta-data/"
deny_check "10c RFC1918 10.0.0.1"            "http://10.0.0.1/"
CC="$(curl -sS --noproxy '' -x "$PROXY" -m 15 -o /dev/null -w '%{http_connect}' "https://192.168.1.1/" 2>/dev/null)"
[ "$CC" = "403" ] && pass "10d CONNECT 192.168.1.1:443 -> 403" || fail "10d CONNECT -> $CC"
stop_connectd

# ================================================================ INSTANCE 3
say "== INSTANCE 3: careful + deny-all (ask-mode denial, link survives)"
start_connectd C --preset careful --deny-all --root "${ROOT_P}:rw"
spike wait -timeout 15s >/dev/null || fail "C  no hello"
# a normal mcp stream in ask mode, with every ask denied -> labeled denial
MC="$(spike mcp-call -tool list_windows 2>/dev/null)"
case "$(echo "$MC" | jq -r '.error' 2>/dev/null)" in
  *denied:*) pass "11a normal mcp stream in ask mode denied: \"$(echo "$MC" | jq -r '.error' | cut -c1-90)\"" ;;
  *) fail "11a ask-mode mcp was not denied: $MC" ;;
esac
# the link survives the denial
if spike ping >/dev/null 2>&1; then
  pass "11b link survived the denial (ping ok)"
else
  fail "11b link did not survive the denial"
fi
# a catalog stream is still served without approval
if [ "$SKIP_CUA" != "1" ]; then
  ML="$(spike mcp-list)"
  [ "$(echo "$ML" | jq -r '.tool_count' 2>/dev/null)" -ge 20 ] 2>/dev/null && pass "11c catalog stream served even under deny-all" || fail "11c catalog refused under deny-all: $ML"
fi
stop_connectd

# ================================================================ INSTANCE 4
# The owner's apps over the real link, against connectd's in-memory
# stand-in for a Mac (--apps mac): no real app, clipboard or screen is
# touched. The approver allows exactly two things: adding calendar events,
# and messages to one recipient.
say "== INSTANCE 4: apps (stand-in for a Mac), balanced, two approvals"
start_connectd D --preset balanced --apps mac --root "${ROOT_P}" \
  --approve 'apps:calendar.create' --approve 'apps:messages.send:+79161234567'
spike wait -timeout 15s >/dev/null || fail "D  no hello"
apps() { # $1 app $2 action $3 params-json ; prints the JSON answer (result shown)
  local params="${3:-}"
  [ -n "$params" ] || params='{}'
  spike apps -app "$1" -action "$2" -params "$params" -show-result
}
aerr() { echo "$1" | jq -r '.error // empty' 2>/dev/null; }

OUT="$(spike devices)"
A="$(echo "$OUT" | jq -c '.devices[0].settings.apps')"
IDS="$(echo "$A" | jq -r '[.apps[].id]|join(",")')"
MSG_ON="$(echo "$A" | jq -r '.apps[]|select(.id=="messages")|.on')"
if [ "$(echo "$A" | jq -r '.enabled')" = "true" ] && [ "$(echo "$A" | jq -r '.mode')" = "ask_new" ] &&
   [ "$IDS" = "messages,contacts,calendar,reminders,notes,shortcuts,notify,clipboard,open" ] && [ "$MSG_ON" = "false" ]; then
  pass "13a hello carries apps: mode=ask_new, 9 apps, Messages off by default"
else
  fail "13a apps in the hello: $A"
fi

R="$(apps calendar list '{"start":"2026-10-13","end":"2026-10-20"}')"
[ "$(echo "$R" | jq -r '.result.events|length' 2>/dev/null)" = "2" ] &&
  pass "13b a read (calendar.list) needs no approval: 2 events" || fail "13b calendar.list: $R"

R="$(apps contacts search '{"query":"ann"}')"
N="$(echo "$R" | jq -r '.result.contacts|length' 2>/dev/null)"
R2="$(apps contacts search '{"query":"a"}')"
case "$(aerr "$R2")" in
  "bad request:"*) [ "$N" = "2" ] && pass "13c contacts.search: 2 matches for a real query; a one-letter query is refused" || fail "13c contacts.search returned $N: $R" ;;
  *) fail "13c a one-letter contact query was not refused: $R2" ;;
esac

R="$(apps calendar create '{"title":"Dentist","start":"2026-10-21T15:00","end":"2026-10-21T16:00","reason":"from the email"}')"
[ "$(echo "$R" | jq -r '.result.id' 2>/dev/null)" = "ev-new" ] && grep -q "ask cap=apps scope=calendar.create" "$CD_LOG" &&
  pass "13d a change the owner allows (calendar.create) was asked about, then done" || fail "13d calendar.create: $R"

R="$(apps notes create '{"title":"Plan","text":"NOTE-TEXT-e2e"}')"
case "$(aerr "$R")" in
  "denied: the owner did not allow it") pass "13e a change the owner refuses (notes.create) -> \"$(aerr "$R")\"" ;;
  *) fail "13e notes.create was not refused: $R" ;;
esac

R="$(apps messages send '{"to":"+79161234567","text":"hi"}')"
case "$(aerr "$R")" in
  "denied: Messages is turned off"*) pass "13f Messages is off until switched on: \"$(aerr "$R" | cut -c1-60)...\"" ;;
  *) fail "13f messages.send with Messages off: $R" ;;
esac

ctl "apps app messages on"; wait_log "OK apps" 3; sleep 0.6
R="$(apps messages send '{"to":"+7 916 123-45-67","text":"MESSAGE-BODY-e2e","reason":"you asked"}')"
R2="$(apps messages send '{"to":"+79160000000","text":"MESSAGE-BODY-e2e"}')"
if [ "$(echo "$R" | jq -r '.result.sent' 2>/dev/null)" = "true" ] && [ "$(aerr "$R2")" = "denied: the owner did not allow it" ] &&
   grep -q "ask cap=apps scope=messages.send:+79161234567" "$CD_LOG" && grep -q "ask cap=apps scope=messages.send:+79160000000" "$CD_LOG"; then
  pass "13g a message is asked about per recipient: the allowed one is sent, another one is refused"
else
  fail "13g messages.send: allowed=$R other=$R2"
fi

# Without -show-result the spike prints the shape of an answer only.
R="$(spike apps -app messages -action recent)"
if [ "$(echo "$R" | jq -r '.shape.messages' 2>/dev/null)" = "4 items" ] && ! echo "$R" | grep -q "Thursday"; then
  pass "13h messages.recent: 4 items; the spike prints the shape, not the texts"
else
  fail "13h messages.recent shape: $R"
fi

echo "%PDF-1.4" > "$ROOT/docs/paper.pdf"; printf '#!/bin/sh\n' > "$ROOT/docs/run.command"; echo "%PDF-1.4" > "$OUTSIDE/outside.pdf"
ctl "apps on auto"; wait_log "OK apps" 3; sleep 0.6
R="$(apps open file "{\"path\":\"$ROOT_P/docs/paper.pdf\"}")"
R2="$(apps open file "{\"path\":\"$ROOT_P/docs/run.command\"}")"
R3="$(apps open file "{\"path\":\"$ROOT_P/escape/outside.pdf\"}")"
R4="$(apps open url '{"url":"file:///etc/passwd"}')"
if [ "$(echo "$R" | jq -r '.result.opened' 2>/dev/null)" = "true" ]; then
  case "$(aerr "$R2")|$(aerr "$R3")|$(aerr "$R4")" in
    "denied: files of type"*"|denied: "*"|denied: "*) pass "13i open: a document in a shared folder opens; a script, a file behind a link out of the folder and a file: address are refused" ;;
    *) fail "13i open refusals: script=$(aerr "$R2") outside=$(aerr "$R3") url=$(aerr "$R4")" ;;
  esac
else
  fail "13i open.file: $R"
fi

# In auto mode a change is not asked about, but a message still is.
ASKS_BEFORE="$(grep -c "ask cap=apps" "$CD_LOG")"
apps notes create '{"title":"Plan"}' >/dev/null
apps messages send '{"to":"anna@example.com","text":"hi"}' >/dev/null
ASKS_AFTER="$(grep -c "ask cap=apps" "$CD_LOG")"
[ "$((ASKS_AFTER-ASKS_BEFORE))" = "1" ] && grep -q "scope=messages.send:anna@example.com" "$CD_LOG" &&
  pass "13j auto mode: a note is written without asking, a message still asks" || fail "13j auto mode asked $((ASKS_AFTER-ASKS_BEFORE)) times"

# The journal: one line per call, never what a message, a contact or a note says.
JF="$WORK/journal-D.jsonl"
if grep -q '"capability":"apps"' "$JF" && ! grep -Eq 'MESSAGE-BODY-e2e|NOTE-TEXT-e2e|Petrova|Thursday|Dentist|9161234567|anna@example' "$JF"; then
  pass "13k journal: $(grep -c '"capability":"apps"' "$JF") apps lines, no message, contact, note or event text in them"
else
  fail "13k journal holds content (or no apps lines): $(grep -Ec 'MESSAGE-BODY-e2e|NOTE-TEXT-e2e|Petrova|Thursday|Dentist' "$JF") hits"
fi

ctl "apps off"; wait_log "OK apps rev" 3; sleep 0.6
R="$(apps clipboard read)"
[ "$(spike devices | jq -r '.devices[0].settings.apps.enabled')" = "false" ] && [ "$(aerr "$R")" = "denied: apps are turned off on this computer" ] &&
  pass "13l apps switched off: the synth is told, and a call is refused" || fail "13l apps off: $R"
stop_connectd

# ================================================================ INSTANCE 5
say "== INSTANCE 5: apps (stand-in for a Windows computer): no Mac app anywhere"
start_connectd E --preset full --apps windows
spike wait -timeout 15s >/dev/null || fail "E  no hello"
A="$(spike devices | jq -c '.devices[0].settings.apps')"
IDS="$(echo "$A" | jq -r '[.apps[].id]|join(",")')"
R="$(apps calendar list '{"start":"2026-10-13","end":"2026-10-20"}')"
R2="$(apps messages send '{"to":"+79161234567","text":"hi"}')"
case "$IDS|$(aerr "$R")|$(aerr "$R2")" in
  "notify,clipboard,open|unavailable: "*"|unavailable: "*) pass "14a a Windows computer offers notify, clipboard, open; Calendar and Messages are unavailable" ;;
  *) fail "14a windows apps: ids=$IDS calendar=$(aerr "$R") messages=$(aerr "$R2")" ;;
esac
apps clipboard write '{"text":"clipboard text e2e"}' >/dev/null
R="$(apps clipboard read)"
R2="$(apps notify show '{"title":"Done","body":"all set"}')"
[ "$(echo "$R" | jq -r '.result.text' 2>/dev/null)" = "clipboard text e2e" ] && [ "$(echo "$R2" | jq -r '.result.shown' 2>/dev/null)" = "true" ] &&
  pass "14b clipboard round trip and a notification through the rpc path" || fail "14b clipboard=$R notify=$R2"
stop_connectd

# ---------------------------------------------------------------- Cua unchanged
if [ "$SKIP_CUA" != "1" ]; then
  CUA_AFTER="$(cua-driver status 2>/dev/null)"
  [ "$CUA_BEFORE" = "$CUA_AFTER" ] && pass "12 user's Cua daemon unchanged (status identical before/after)" || fail "12 user's Cua daemon changed"
fi

say "== summary: $PASSES passed, $FAILS failed"
if [ "$FAILS" -gt 0 ]; then
  say "--- last connectd log"; tail -20 "$WORK"/connectd-*.log 2>/dev/null
  exit 1
fi
exit 0
