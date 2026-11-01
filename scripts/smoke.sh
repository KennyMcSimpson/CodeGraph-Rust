#!/usr/bin/env bash
# End-to-end smoke test of a built binary on a tiny three-language project:
# init -> status -> explore -> edit + sync -> query -> MCP handshake over stdio -> prompt hook.
# It needs nothing but the binary (no Node, no network), so the same script runs on a clean CI
# runner, in a `scratch` container, or on a fresh VM.
#
#   scripts/smoke.sh path/to/cgr
set -euo pipefail

BIN="${1:?usage: smoke.sh <cgr binary>}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
DIR="$(mktemp -d 2>/dev/null || mktemp -d -t cgr-smoke)"
trap 'rm -rf "$DIR"' EXIT
cd "$DIR"

fail() { echo "SMOKE FAILED: $*" >&2; exit 1; }
expect() { # expect <haystack> <needle> <what>
  case "$1" in *"$2"*) ;; *) echo "--- output ---" >&2; echo "$1" | head -40 >&2; fail "$3 (missing: $2)";; esac
}

# run a command with a time limit, without relying on GNU `timeout` (absent on macOS runners)
run_limited() { # run_limited <seconds> <cmd...>
  local limit="$1"; shift
  "$@" &
  local pid=$!
  ( sleep "$limit"; kill -9 "$pid" 2>/dev/null ) &
  local killer=$!
  local rc=0
  wait "$pid" || rc=$?
  kill "$killer" 2>/dev/null || true
  wait "$killer" 2>/dev/null || true
  return $rc
}

mkdir -p web/src svc/pkg lib
cat > web/src/greet.ts <<'EOF'
export function greet(name: string): string {
  return format(name);
}
export function format(name: string): string {
  return `hello ${name}`;
}
EOF
cat > web/src/main.ts <<'EOF'
import { greet } from './greet';
export function main(): string {
  return greet('world');
}
EOF
cat > svc/pkg/__init__.py <<'EOF'
EOF
cat > svc/pkg/util.py <<'EOF'
def normalise(value):
    return value.strip().lower()
EOF
cat > svc/pkg/app.py <<'EOF'
from .util import normalise

def handle(request):
    return normalise(request)
EOF
cat > lib/go.mod <<'EOF'
module example.com/lib

go 1.21
EOF
cat > lib/lib.go <<'EOF'
package lib

func Add(a, b int) int { return a + b }

func Total(xs []int) int {
	t := 0
	for _, x := range xs {
		t = Add(t, x)
	}
	return t
}
EOF

echo "== version"
"$BIN" version --verbose | head -3

echo "== init"
out="$("$BIN" init)"; echo "$out"
expect "$out" "indexed" "init should index the project"

echo "== status"
out="$("$BIN" status --json)"
expect "$out" '"files": 6' "status should count six files"
"$BIN" status --verify >/dev/null

echo "== explore"
out="$("$BIN" explore "greet format")"
expect "$out" "greet" "explore should mention greet"
expect "$out" "format" "explore should mention format"

echo "== callers"
out="$("$BIN" callers Add)"
expect "$out" "Total" "Total calls Add"

echo "== edit + sync"
cat >> web/src/greet.ts <<'EOF'
export function shout(name: string): string {
  return greet(name).toUpperCase();
}
EOF
out="$("$BIN" sync)"; echo "$out"
out="$("$BIN" query shout)"
expect "$out" "shout" "sync should have picked up the new function"
"$BIN" debug diff >/dev/null || fail "incremental index differs from a full rebuild"

echo "== MCP over stdio"
req='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codegraph-rust_explore","arguments":{"query":"normalise handle"}}}'
out="$(printf '%s\n' "$req" | run_limited 60 "$BIN" serve --mcp --no-watch --path "$DIR")"
expect "$out" '"id":1' "initialize reply"
expect "$out" 'codegraph-rust_explore' "tools/list should offer the explore tool"
expect "$out" 'normalise' "explore over MCP should find normalise"

echo "== prompt hook"
out="$(printf '{"prompt":"how does greet call format?","cwd":"%s"}' "$DIR" | "$BIN" prompt-hook)" || fail "prompt-hook must exit 0"
echo "(hook output: ${#out} bytes)"
# broken input and a directory without an index must not fail either
printf 'not json' | "$BIN" prompt-hook >/dev/null || fail "prompt-hook must exit 0 on garbage"
( cd / && printf '{"prompt":"x","cwd":"/"}' | "$BIN" prompt-hook >/dev/null ) || fail "prompt-hook must exit 0 without an index"

echo "== export"
if "$BIN" export --sqlite "$DIR/graph.sqlite" >/dev/null 2>&1; then
  [ -s "$DIR/graph.sqlite" ] || fail "export wrote nothing"
fi

echo "SMOKE OK"
