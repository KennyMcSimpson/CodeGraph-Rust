#!/usr/bin/env bash
# Real-repository verification of a built binary (the "实测" of milestone M4):
# per repository, on a scratch copy so the checkout is never touched —
#   1. full index with timings and peak memory
#   2. `status --verify` (every partition checksum)
#   3. determinism: the same content from a second run and from CGR_THREADS=1
#   4. `bench sync` with random edits, ending with "incremental == full rebuild"
#   5. `bench explore` latency
#
#   scripts/verify-repos.sh <cgr binary> <repos-dir> [--edits N] [--work DIR] [repo ...]
#
# Output is plain text, one block per repository, and a final table. Exit status 1 if any check
# failed. Memory-light by design: one process at a time; use scripts/bench-local.sh for the guard.
set -uo pipefail

BIN="${1:?usage: verify-repos.sh <cgr> <repos-dir> [--edits N] [--work DIR] [repo ...]}"
REPOS_DIR="${2:?usage: verify-repos.sh <cgr> <repos-dir> [--edits N] [--work DIR] [repo ...]}"
shift 2
EDITS=100
WORK="${TMPDIR:-/tmp}/cgr-verify"
REPOS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --edits) EDITS="$2"; shift 2;;
    --work) WORK="$2"; shift 2;;
    *) REPOS+=("$1"); shift;;
  esac
done
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
HERE="$(cd "$(dirname "$0")" && pwd)"
[ ${#REPOS[@]} -gt 0 ] || while IFS= read -r d; do REPOS+=("$(basename "$d")"); done < <(find "$REPOS_DIR" -mindepth 1 -maxdepth 1 -type d | sort)
mkdir -p "$WORK"

failed=0
summary=()
json_field() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

for repo in "${REPOS[@]}"; do
  src="$REPOS_DIR/$repo"; dir="$WORK/$repo"
  [ -d "$src" ] || { echo "skip $repo (not found)"; continue; }
  echo "=== $repo"
  rm -rf "$dir"; mkdir -p "$dir"
  (cd "$src" && tar --exclude=.git --exclude=.cgr --exclude=node_modules -cf - .) | (cd "$dir" && tar -xf -)

  ok=1
  idx_json="$("$BIN" bench index "$dir" --runs 1 --json 2>"$WORK/$repo.err")" || { echo "  index FAILED: $(tail -2 "$WORK/$repo.err")"; failed=1; summary+=("$repo|FAILED"); continue; }
  files=$(echo "$idx_json" | json_field "d['files']")
  nodes=$(echo "$idx_json" | json_field "d['nodes']")
  edges=$(echo "$idx_json" | json_field "d['edges']")
  resolved=$(echo "$idx_json" | json_field "'%d/%d' % (d['resolved'], d['refs'])")
  ms=$(echo "$idx_json" | json_field "round(d['totalMs']['p50'])")
  rss=$(echo "$idx_json" | json_field "round(d['peakRssMb']['p50'])")
  seg=$(echo "$idx_json" | json_field "d['segmentMb']")
  echo "  index: $files files, $nodes nodes, $edges edges, refs resolved $resolved, ${ms} ms, peak RSS ${rss} MB, segment ${seg} MB"

  if "$BIN" status --verify --path "$dir" >/dev/null 2>&1 || "$BIN" -p "$dir" status --verify >/dev/null 2>&1; then v="ok"; else v="FAILED"; ok=0; fi
  echo "  status --verify: $v"

  "$BIN" -p "$dir" export --sqlite "$WORK/$repo.a.sqlite" --force >/dev/null 2>&1
  "$BIN" -p "$dir" index --quiet >/dev/null 2>&1
  "$BIN" -p "$dir" export --sqlite "$WORK/$repo.b.sqlite" --force >/dev/null 2>&1
  CGR_THREADS=1 "$BIN" -p "$dir" index --quiet >/dev/null 2>&1
  "$BIN" -p "$dir" export --sqlite "$WORK/$repo.c.sqlite" --force >/dev/null 2>&1
  if python3 "$HERE/db_digest.py" "$WORK/$repo.a.sqlite" "$WORK/$repo.b.sqlite" >/dev/null; then d1="same"; else d1="DIFFERENT"; ok=0; fi
  if python3 "$HERE/db_digest.py" "$WORK/$repo.a.sqlite" "$WORK/$repo.c.sqlite" >/dev/null; then d2="same"; else d2="DIFFERENT"; ok=0; fi
  echo "  determinism: second run $d1, one thread $d2"
  rm -f "$WORK/$repo".{a,b,c}.sqlite

  sync_json="$("$BIN" bench sync "$dir" --edits "$EDITS" --seed 1 --verify --json 2>>"$WORK/$repo.err")"
  if [ -n "$sync_json" ]; then
    p50=$(echo "$sync_json" | json_field "round(d['latencyMs']['p50'],1)")
    p95=$(echo "$sync_json" | json_field "round(d['latencyMs']['p95'],1)")
    delta=$(echo "$sync_json" | json_field "d['deltaSyncs']")
    full=$(echo "$sync_json" | json_field "d['fullRebuilds']")
    eq=$(echo "$sync_json" | json_field "d['incrementalEqualsFull']")
    [ "$eq" = "True" ] || ok=0
    echo "  sync x$EDITS: p50 ${p50} ms, p95 ${p95} ms, $delta delta syncs, $full full rebuilds; incremental == full: $eq"
  else
    echo "  sync FAILED: $(tail -2 "$WORK/$repo.err")"; ok=0; p50="-"; p95="-"; eq="-"
  fi

  exp_json="$("$BIN" bench explore "$dir" --count 100 --runs 2 --json 2>>"$WORK/$repo.err")"
  if [ -n "$exp_json" ]; then
    e50=$(echo "$exp_json" | json_field "round(d['latencyMs']['p50'],1)")
    e95=$(echo "$exp_json" | json_field "round(d['latencyMs']['p95'],1)")
    e99=$(echo "$exp_json" | json_field "round(d['latencyMs']['p99'],1)")
    echo "  explore: p50 ${e50} ms, p95 ${e95} ms, p99 ${e99} ms"
  else
    echo "  explore FAILED: $(tail -2 "$WORK/$repo.err")"; ok=0; e50="-"; e95="-"; e99="-"
  fi

  [ $ok -eq 1 ] || failed=1
  summary+=("$repo|$files|$nodes|$edges|${ms}|${rss}|$([ $ok -eq 1 ] && echo pass || echo FAIL)|$p50|$p95|$e50|$e95")
  rm -rf "$dir"
done

echo
printf '%-12s %7s %9s %9s %9s %8s %6s %9s %9s %9s %9s\n' repo files nodes edges index_ms rss_mb result sync_p50 sync_p95 expl_p50 expl_p95
for line in "${summary[@]}"; do
  IFS='|' read -r a b c d e f g h i j k <<<"$line"
  printf '%-12s %7s %9s %9s %9s %8s %6s %9s %9s %9s %9s\n' "$a" "$b" "$c" "$d" "$e" "$f" "$g" "$h" "$i" "$j" "$k"
done
exit $failed
