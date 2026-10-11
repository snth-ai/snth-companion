#!/usr/bin/env bash
# p0-local.sh: local end-to-end proof for SNTH Connect phase P0a.
#
# Builds the Go spike server (openpaw worktree, cmd/connect-spike) and the
# Rust client (this crate), runs both on loopback and prints PASS/FAIL per
# check. Nothing is deployed, nothing leaves this machine except check 8
# and the latency probe (plain HTTPS requests to api.ipify.org).
#
# Usage:
#   connectd/scripts/p0-local.sh
#
# Environment:
#   OPENPAW_DIR     openpaw worktree with ./connect and ./cmd/connect-spike
#                   (default: ../openpaw-connect-p0 next to this repo)
#   P0_SIZE_MB      payload size for checks 2 and 3 (default 100)
#   P0_SLOW_MBPS    reader speed for check 3, MB/s (default 8)
#   P0_SKIP_BUILD=1 reuse existing binaries
#   P0_SKIP_CUA=1   skip check 6 (no Cua Driver daemon on this machine)
#   P0_HALFOPEN=1   extra check 9: freeze the client (SIGSTOP) and wait for
#                   the server to drop the silent link (takes up to 2 min)
#   P0_SOAK_MIN=N   extra check 10: keep the link up for N minutes with one
#                   small request every 30 s, expect zero reconnects
#   P0_ABORT=1      extra check 11: kill a synth-side download midway and
#                   expect the app to release the stream (about 30 s)
#   P0_KEEP=1       keep the work dir (logs) after the run
#
# Safety: binds 127.0.0.1 only, kills only the PIDs it started, calls only
# read-only Cua tools (list_windows) and never touches the Cua daemon.

set -u
set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$CRATE_DIR/.." && pwd)"
OPENPAW_DIR="${OPENPAW_DIR:-$(dirname "$REPO_DIR")/openpaw-connect-p0}"
SIZE_MB="${P0_SIZE_MB:-100}"
SLOW_MBPS="${P0_SLOW_MBPS:-8}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/snth-connect-p0.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
SPIKE="$WORK/connect-spike"
CONNECTD="$CRATE_DIR/target/release/connectd"
PIDS=()
FAILS=0
PASSES=0

say()  { printf '%s\n' "$*"; }
pass() { PASSES=$((PASSES + 1)); say "PASS  $*"; }
fail() { FAILS=$((FAILS + 1)); say "FAIL  $*"; }
info() { say "      $*"; }

kill_pid() { # kill one PID we started, then wait for it
  local pid="$1" sig="${2:-TERM}"
  [ -n "$pid" ] || return 0
  kill "-$sig" "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

cleanup() {
  for pid in "${PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -CONT "$pid" 2>/dev/null
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  wait 2>/dev/null
  if [ "${P0_KEEP:-0}" = "1" ] || [ "$FAILS" -gt 0 ]; then
    rm -f "$WORK/blob.bin"
    say "logs kept in $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

need() { command -v "$1" >/dev/null 2>&1 || { say "missing tool: $1"; exit 2; }; }
for t in go cargo curl jq shasum openssl nc perl; do need "$t"; done
[ -d "$OPENPAW_DIR/connect" ] || { say "OPENPAW_DIR=$OPENPAW_DIR has no connect/ package"; exit 2; }

free_port() {
  local p
  while :; do
    p=$((20000 + RANDOM % 30000))
    nc -z 127.0.0.1 "$p" >/dev/null 2>&1 || { echo "$p"; return; }
  done
}

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }

median() { sort -n | awk '{a[NR]=$1} END {if (NR==0) print "nan"; else print a[int((NR+1)/2)]}'; }

# ---------------------------------------------------------------- build
if [ "${P0_SKIP_BUILD:-0}" != "1" ]; then
  say "== build (one at a time)"
  (cd "$OPENPAW_DIR" && go build -o "$SPIKE" ./cmd/connect-spike) || { say "go build failed"; exit 2; }
  (cd "$CRATE_DIR" && cargo build -j 4 --release 2>&1 | tail -2) || { say "cargo build failed"; exit 2; }
else
  (cd "$OPENPAW_DIR" && go build -o "$SPIKE" ./cmd/connect-spike) || { say "go build failed"; exit 2; }
fi
[ -x "$CONNECTD" ] || { say "no connectd binary at $CONNECTD"; exit 2; }

# ---------------------------------------------------------------- setup
export CONNECT_SPIKE_TOKEN="$(openssl rand -hex 24)"
PORT_WS="$(free_port)"; PORT_PROXY="$(free_port)"; PORT_HTTP="$(free_port)"
SERVER="http://127.0.0.1:$PORT_WS"
PROXY="http://127.0.0.1:$PORT_PROXY"
ORIGIN="http://127.0.0.1:$PORT_HTTP"
WS_URL="ws://127.0.0.1:$PORT_WS/api/connect/ws"

ROOT="$WORK/shared"; OUTSIDE="$WORK/outside"
mkdir -p "$ROOT/docs" "$OUTSIDE"
echo "hello from the shared folder" > "$ROOT/docs/note.txt"
echo "must not be readable" > "$OUTSIDE/secret.txt"
ln -s "$OUTSIDE" "$ROOT/escape"

say "== payload: ${SIZE_MB} MB of random bytes"
head -c "$((SIZE_MB * 1024 * 1024))" /dev/urandom > "$WORK/blob.bin"
BLOB_SHA="$(shasum -a 256 "$WORK/blob.bin" | awk '{print $1}')"

"$SPIKE" serve -listen "127.0.0.1:$PORT_WS" -proxy "127.0.0.1:$PORT_PROXY" > "$WORK/spike.log" 2>&1 &
SPIKE_PID=$!; PIDS+=("$SPIKE_PID")
"$SPIKE" testhttp -listen "127.0.0.1:$PORT_HTTP" -file "$WORK/blob.bin" > "$WORK/testhttp.log" 2>&1 &
HTTP_PID=$!; PIDS+=("$HTTP_PID")

CLIENT_PID=""
start_client() { # extra connectd flags as arguments
  CONNECTD_TOKEN="$CONNECT_SPIKE_TOKEN" "$CONNECTD" run --url "$WS_URL" \
    --root "$ROOT" --mcp-tool list_windows "$@" >> "$WORK/connectd.log" 2>&1 &
  CLIENT_PID=$!; PIDS+=("$CLIENT_PID")
}

spike() { "$SPIKE" "$@" -server "$SERVER" 2>/dev/null; }

for _ in $(seq 1 50); do nc -z 127.0.0.1 "$PORT_WS" >/dev/null 2>&1 && break; sleep 0.1; done

rss_kb() { ps -o rss= -p "$1" 2>/dev/null | tr -d ' '; }
SAMPLER_PID=""
start_sampler() { # writes "name rss_kb" lines until stopped
  : > "$WORK/rss.txt"
  ( while :; do
      echo "server $(rss_kb "$SPIKE_PID")" >> "$WORK/rss.txt"
      echo "client $(rss_kb "$CLIENT_PID")" >> "$WORK/rss.txt"
      sleep 0.2
    done ) &
  SAMPLER_PID=$!; PIDS+=("$SAMPLER_PID")
}
stop_sampler() { kill_pid "$SAMPLER_PID"; SAMPLER_PID=""; }
peak_mb() { awk -v n="$1" '$1==n && $2+0>m {m=$2+0} END {printf "%.1f", m/1024}' "$WORK/rss.txt"; }

# ---------------------------------------------------------------- 1
say "== 1. connect + hello"
start_client --allow-private
if OUT="$(spike wait -timeout 15s)"; then
  DEV="$(echo "$OUT" | jq -r '.devices[0] | "\(.device_id) os=\(.os)/\(.arch) app=\(.app_version) caps=\(.capabilities|join(","))"')"
  pass "1  hello seen by the server: $DEV"
  info "presence: $(echo "$OUT" | jq -c '.devices[0] | {online,idle_seconds,screen_locked,connections}')"
  info "multiplexer ping RTT (median of 5): $(spike ping | jq -r '.rtt_ms_median') ms"
else
  fail "1  no hello within 15 s"
  tail -5 "$WORK/connectd.log" | sed 's/^/      /'
fi

# ---------------------------------------------------------------- 2
say "== 2. ${SIZE_MB} MB through the proxy, both directions"
T0=$(now)
DOWN_SHA="$(curl -sS --noproxy '' -x "$PROXY" -m 300 "$ORIGIN/file" | shasum -a 256 | awk '{print $1}')"
T1=$(now)
if [ "$DOWN_SHA" = "$BLOB_SHA" ]; then
  pass "2a download ${SIZE_MB} MB: sha256 equal, $(echo "$T0 $T1 $SIZE_MB" | awk '{d=$2-$1; printf "%.2f s, %.0f MB/s", d, $3/d}')"
else
  fail "2a download sha256 mismatch (got ${DOWN_SHA:0:16}, want ${BLOB_SHA:0:16})"
fi
T0=$(now)
UP="$(curl -sS --noproxy '' -x "$PROXY" -m 300 -T "$WORK/blob.bin" "$ORIGIN/upload")"
T1=$(now)
if [ "$(echo "$UP" | jq -r '.sha256' 2>/dev/null)" = "$BLOB_SHA" ]; then
  pass "2b upload ${SIZE_MB} MB: sha256 equal, $(echo "$T0 $T1 $SIZE_MB" | awk '{d=$2-$1; printf "%.2f s, %.0f MB/s", d, $3/d}')"
else
  fail "2b upload sha256 mismatch: $UP"
fi

# Proof that the bytes went through the tunnel and not around it.
info "proxy counters: $(spike devices | jq -c '.proxy')"

# Tunnel overhead on a loopback origin: no internet noise in the numbers.
LOOP_DIRECT="$(for _ in 1 2 3 4 5 6 7 8 9; do curl -sS --noproxy '*' -m 5 -o /dev/null -w '%{time_total}\n' "$ORIGIN/hello" 2>/dev/null; done | median)"
LOOP_TUNNEL="$(for _ in 1 2 3 4 5 6 7 8 9; do curl -sS --noproxy '' -x "$PROXY" -m 5 -o /dev/null -w '%{time_total}\n' "$ORIGIN/hello" 2>/dev/null; done | median)"

# ---------------------------------------------------------------- 3
say "== 3. backpressure: reader limited to ${SLOW_MBPS} MB/s"
RSS_LIMIT_MB=64 # allowed growth over the idle baseline, per process
sleep 1
BASE_S="$(rss_kb "$SPIKE_PID")"; BASE_C="$(rss_kb "$CLIENT_PID")"
slow_check() { # $1 label, $2 sha result
  local label="$1" sha="$2" ps pc ds dc
  ps="$(peak_mb server)"; pc="$(peak_mb client)"
  ds="$(echo "$ps $BASE_S" | awk '{printf "%.1f", $1-$2/1024}')"
  dc="$(echo "$pc $BASE_C" | awk '{printf "%.1f", $1-$2/1024}')"
  local msg="$label: peak RSS server ${ps} MB (+${ds}), client ${pc} MB (+${dc}), ${ELAPSED} s"
  if [ "$sha" != "$BLOB_SHA" ]; then
    fail "$msg; sha256 mismatch"
  elif awk -v a="$ds" -v b="$dc" -v l="$RSS_LIMIT_MB" 'BEGIN {exit !(a<l && b<l)}'; then
    pass "$msg; growth under ${RSS_LIMIT_MB} MB for a ${SIZE_MB} MB payload"
  else
    fail "$msg; growth over ${RSS_LIMIT_MB} MB"
  fi
}
info "baseline RSS: server $(echo "$BASE_S" | awk '{printf "%.1f", $1/1024}') MB, client $(echo "$BASE_C" | awk '{printf "%.1f", $1/1024}') MB"
start_sampler
T0=$(date +%s)
SHA="$(curl -sS --noproxy '' -x "$PROXY" -m 600 --limit-rate "${SLOW_MBPS}M" "$ORIGIN/file" | shasum -a 256 | awk '{print $1}')"
ELAPSED=$(( $(date +%s) - T0 ))
stop_sampler
slow_check "3a slow download (origin fast, synth-side reader slow)" "$SHA"
start_sampler
T0=$(date +%s)
UP="$(curl -sS --noproxy '' -x "$PROXY" -m 600 -T "$WORK/blob.bin" "$ORIGIN/upload?rate=$((SLOW_MBPS * 1000000))")"
ELAPSED=$(( $(date +%s) - T0 ))
stop_sampler
slow_check "3b slow upload (synth-side writer fast, target slow)" "$(echo "$UP" | jq -r '.sha256' 2>/dev/null)"

# ---------------------------------------------------------------- 4
say "== 4. kill the client, restart it"
kill_pid "$CLIENT_PID" KILL
if OUT="$(spike wait -offline -timeout 15s)"; then
  pass "4a server saw the disconnect: reason=\"$(echo "$OUT" | jq -r '.devices[0].disconnect_reason')\""
else
  fail "4a server still lists the device as online 15 s after kill -9"
fi
CODE="$(curl -sS --noproxy '' -x "$PROXY" -m 10 -o /dev/null -w '%{http_code}' "$ORIGIN/hello")"
if [ "$CODE" = "503" ]; then
  pass "4b proxy while offline answers 503 (labeled, no hang)"
else
  fail "4b proxy while offline answered $CODE, want 503"
fi
start_client --allow-private
if OUT="$(spike wait -min-connections 2 -timeout 20s)"; then
  pass "4c server saw the new connection: connections=$(echo "$OUT" | jq -r '.devices[0].connections')"
else
  fail "4c no reconnect within 20 s"
fi
BODY="$(curl -sS --noproxy '' -x "$PROXY" -m 10 "$ORIGIN/hello")"
if [ "$BODY" = "hello from testhttp" ]; then
  pass "4d proxy works again after the reconnect"
else
  fail "4d proxy after reconnect returned: $BODY"
fi

# ---------------------------------------------------------------- 5
say "== 5. private ranges without --allow-private"
kill_pid "$CLIENT_PID" TERM
spike wait -offline -timeout 10s >/dev/null
start_client
spike wait -min-connections 3 -timeout 20s >/dev/null || fail "5  client did not come back"
deny_check() { # $1 label, $2 url
  local hdr code why
  hdr="$(curl -sS --noproxy '' -x "$PROXY" -m 15 -o /dev/null -D - "$2" 2>/dev/null)"
  code="$(echo "$hdr" | head -1 | awk '{print $2}')"
  why="$(echo "$hdr" | grep -i '^x-connect-error:' | head -1 | sed 's/^[^:]*: //' | tr -d '\r')"
  case "$why" in
    denied:*) [ "$code" = "403" ] && pass "$1 -> 403 \"$why\"" || fail "$1 -> $code \"$why\"" ;;
    *) fail "$1 -> $code \"$why\" (want 403 with a denied: label)" ;;
  esac
}
deny_check "5a loopback literal 127.0.0.1" "$ORIGIN/hello"
deny_check "5b name resolving to loopback (localhost)" "http://localhost:$PORT_HTTP/hello"
deny_check "5c cloud metadata 169.254.169.254" "http://169.254.169.254/latest/meta-data/"
deny_check "5d RFC1918 10.0.0.1" "http://10.0.0.1/"
deny_check "5e IPv4-mapped IPv6 loopback" "http://[::ffff:127.0.0.1]:$PORT_HTTP/hello"
CONNECT_CODE="$(curl -sS --noproxy '' -x "$PROXY" -m 15 -o /dev/null -w '%{http_connect}' "https://192.168.1.1/" 2>/dev/null)"
if [ "$CONNECT_CODE" = "403" ]; then
  pass "5f CONNECT 192.168.1.1:443 -> 403"
else
  fail "5f CONNECT 192.168.1.1:443 -> $CONNECT_CODE, want 403"
fi

# ---------------------------------------------------------------- 6
say "== 6. MCP against the real Cua Driver daemon (read-only)"
if [ "${P0_SKIP_CUA:-0}" = "1" ]; then
  info "skipped (P0_SKIP_CUA=1)"
else
  if OUT="$(spike mcp-list)"; then
    N="$(echo "$OUT" | jq -r '.tool_count')"
    DESC="$(echo "$OUT" | jq -r '"\(.server_name) \(.server_version), MCP \(.protocol_version), instructions \(.instructions_bytes) bytes, \(.elapsed_ms) ms"')"
    if [ "$N" -ge 40 ]; then
      pass "6a initialize + tools/list: $N tools ($DESC)"
    else
      fail "6a tools/list returned only $N tools ($DESC)"
    fi
  else
    fail "6a initialize + tools/list failed: $OUT"
  fi
  if OUT="$(spike mcp-call -tool list_windows)"; then
    if [ "$(echo "$OUT" | jq -r '.is_error')" = "false" ]; then
      pass "6b tools/call list_windows: $(echo "$OUT" | jq -c '{text_bytes,structured_bytes,elapsed_ms}')"
    else
      fail "6b list_windows returned isError: $OUT"
    fi
  else
    fail "6b tools/call list_windows failed: $OUT"
  fi
  # The client runs with --mcp-tool list_windows, so any other tool must be
  # refused by the app's filter before it reaches the driver. list_apps is
  # read-only, so even a filter bug would be harmless.
  OUT="$(spike mcp-call -tool list_apps)"
  case "$OUT" in
    *"denied: tool"*) pass "6c app-side filter refuses a tool outside the allowlist: $(echo "$OUT" | jq -r '.error' | cut -c1-110)" ;;
    *) fail "6c list_apps was not refused by the app-side filter: $(echo "$OUT" | cut -c1-200)" ;;
  esac
fi

# ---------------------------------------------------------------- 7
say "== 7. files inside the shared folder, escapes refused"
REAL_ROOT="$(cd "$ROOT" && pwd -P)"
if OUT="$(spike files -op list -path "$REAL_ROOT")"; then
  pass "7a list root: $(echo "$OUT" | jq -c '[.entries[] | "\(.name):\(.type)"]')"
else
  fail "7a list root failed: $OUT"
fi
WANT_SHA="$(shasum -a 256 "$ROOT/docs/note.txt" | awk '{print $1}')"
OUT="$(spike files -op read -path "$REAL_ROOT/docs/note.txt")"
if [ "$(echo "$OUT" | jq -r '.sha256' 2>/dev/null)" = "$WANT_SHA" ]; then
  pass "7b read a file in the root: sha256 equal, $(echo "$OUT" | jq -r '.bytes') bytes"
else
  fail "7b read mismatch: $OUT"
fi
files_denied() { # $1 label, $2 op, $3 path
  local out
  out="$(spike files -op "$2" -path "$3")"
  case "$(echo "$out" | jq -r '.error' 2>/dev/null)" in
    denied:*) pass "$1 -> \"$(echo "$out" | jq -r '.error' | sed "s#$WORK#<work>#g")\"" ;;
    *) fail "$1 was not denied: $(echo "$out" | cut -c1-200)" ;;
  esac
}
files_denied "7c read through a symlink that leaves the root" read "$REAL_ROOT/escape/secret.txt"
files_denied "7d list the symlink target" list "$REAL_ROOT/escape"
files_denied "7e dot-dot out of the root" read "$REAL_ROOT/docs/../../outside/secret.txt"
files_denied "7f absolute path outside the root" read "/etc/hosts"

# ---------------------------------------------------------------- 8
say "== 8. real internet through the proxy (client WITHOUT --allow-private)"
IP_TUNNEL="$(curl -sS --noproxy '' -x "$PROXY" -m 20 https://api.ipify.org 2>/dev/null)"
IP_DIRECT="$(curl -sS --noproxy '*' -m 20 https://api.ipify.org 2>/dev/null)"
if echo "$IP_TUNNEL" | grep -Eq '^[0-9a-fA-F:.]+$'; then
  pass "8  curl -x proxy https://api.ipify.org -> $IP_TUNNEL (direct from this machine: ${IP_DIRECT:-unavailable})"
else
  fail "8  no IP through the proxy: ${IP_TUNNEL:-empty}"
fi

say "== latency: small HTTPS request, direct vs through the tunnel (7 runs each, total time incl. TLS)"
D_MED="$(for _ in 1 2 3 4 5 6 7; do curl -sS --noproxy '*' -m 20 -o /dev/null -w '%{time_total}\n' https://api.ipify.org 2>/dev/null; done | median)"
T_MED="$(for _ in 1 2 3 4 5 6 7; do curl -sS --noproxy '' -x "$PROXY" -m 20 -o /dev/null -w '%{time_total}\n' https://api.ipify.org 2>/dev/null; done | median)"
info "direct median ${D_MED} s, tunnel median ${T_MED} s, overhead $(echo "$D_MED $T_MED" | awk '{printf "%+.0f ms", ($2-$1)*1000}')"
info "loopback origin (9 runs each, new connection per request): direct median ${LOOP_DIRECT} s, tunnel median ${LOOP_TUNNEL} s, overhead $(echo "$LOOP_DIRECT $LOOP_TUNNEL" | awk '{printf "%+.1f ms", ($2-$1)*1000}')"
info "multiplexer ping RTT (median of 5): $(spike ping | jq -r '.rtt_ms_median') ms"

# ---------------------------------------------------------------- 9 (optional)
if [ "${P0_HALFOPEN:-0}" = "1" ]; then
  say "== 9. half-open link: client frozen with SIGSTOP"
  kill -STOP "$CLIENT_PID"
  T0=$(date +%s)
  if OUT="$(spike wait -offline -timeout 150s)"; then
    pass "9a server dropped the silent link after $(( $(date +%s) - T0 )) s: reason=\"$(echo "$OUT" | jq -r '.devices[0].disconnect_reason')\""
  else
    fail "9a server kept the silent link for 150 s"
  fi
  kill -CONT "$CLIENT_PID"
  CONNS_BEFORE="$(spike devices | jq -r '.devices[0].connections')"
  if spike wait -min-connections "$((CONNS_BEFORE + 1))" -timeout 60s >/dev/null; then
    pass "9b thawed client noticed the dead link and redialed"
  else
    fail "9b thawed client did not redial within 60 s"
  fi
fi

# ---------------------------------------------------------------- 10 (optional)
if [ "${P0_SOAK_MIN:-0}" != "0" ]; then
  say "== 10. soak: ${P0_SOAK_MIN} min, one request every 30 s"
  CONNS_BEFORE="$(spike devices | jq -r '.devices[0].connections')"
  RSS_S0="$(rss_kb "$SPIKE_PID")"; RSS_C0="$(rss_kb "$CLIENT_PID")"
  ROUNDS=$((P0_SOAK_MIN * 2)); BAD=0
  for _ in $(seq 1 "$ROUNDS"); do
    sleep 30
    [ "$(curl -sS --noproxy '' -x "$PROXY" -m 20 https://api.ipify.org 2>/dev/null)" = "$IP_TUNNEL" ] || BAD=$((BAD + 1))
  done
  OUT="$(spike devices)"
  CONNS_AFTER="$(echo "$OUT" | jq -r '.devices[0].connections')"
  ONLINE="$(echo "$OUT" | jq -r '.devices[0].online')"
  if [ "$ONLINE" = "true" ] && [ "$CONNS_AFTER" = "$CONNS_BEFORE" ] && [ "$BAD" = "0" ]; then
    pass "10 link stayed up ${P0_SOAK_MIN} min, 0 reconnects, $ROUNDS/$ROUNDS requests ok"
    info "RSS before -> after: server $(echo "$RSS_S0 $(rss_kb "$SPIKE_PID")" | awk '{printf "%.1f -> %.1f MB", $1/1024, $2/1024}'), client $(echo "$RSS_C0 $(rss_kb "$CLIENT_PID")" | awk '{printf "%.1f -> %.1f MB", $1/1024, $2/1024}')"
  else
    fail "10 soak: online=$ONLINE reconnects=$((CONNS_AFTER - CONNS_BEFORE)) failed_requests=$BAD/$ROUNDS"
  fi
fi

# ---------------------------------------------------------------- 11 (optional)
if [ "${P0_ABORT:-0}" = "1" ]; then
  say "== 11. synth-side client aborts a download midway"
  CONNS_BEFORE="$(spike devices | jq -r '.devices[0].connections')"
  kill_pid "$CLIENT_PID" TERM
  spike wait -offline -timeout 10s >/dev/null
  start_client --allow-private
  spike wait -min-connections "$((CONNS_BEFORE + 1))" -timeout 20s >/dev/null || fail "11 client did not come back"
  curl -sS --noproxy '' -x "$PROXY" -m 120 --limit-rate 1M -o /dev/null "$ORIGIN/file" 2>/dev/null &
  CURL_PID=$!; PIDS+=("$CURL_PID")
  sleep 2
  STREAMS_DURING="$(spike devices | jq -r '.devices[0].open_streams')"
  kill_pid "$CURL_PID" KILL
  T0=$(date +%s); STREAMS_AFTER=""
  for _ in $(seq 1 90); do
    STREAMS_AFTER="$(spike devices | jq -r '.devices[0].open_streams')"
    [ "$STREAMS_AFTER" = "1" ] && break
    sleep 1
  done
  WAITED=$(( $(date +%s) - T0 ))
  ABORT_LINE="$(grep 'aborted: the synth closed its side' "$WORK/connectd.log" | tail -1 | sed 's/^[^ ]* \[connectd\] //')"
  if [ "$STREAMS_DURING" = "2" ] && [ "$STREAMS_AFTER" = "1" ] && [ -n "$ABORT_LINE" ]; then
    pass "11 aborted download released after ${WAITED} s (open streams 2 -> 1); app log: $ABORT_LINE"
  else
    fail "11 open streams during=$STREAMS_DURING after=$STREAMS_AFTER (waited ${WAITED} s), app log: ${ABORT_LINE:-no abort line}"
  fi
fi

say "== summary: $PASSES passed, $FAILS failed"
if [ "$FAILS" -gt 0 ]; then
  say "--- last lines of connectd.log"; tail -15 "$WORK/connectd.log"
  say "--- last lines of spike.log"; tail -15 "$WORK/spike.log"
  exit 1
fi
exit 0
