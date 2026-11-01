#!/usr/bin/env bash
# Memory-guarded benchmark runner for one laptop (design doc 13.4).
#
#   scripts/bench-local.sh [options] [repo ...]
#
#   --tier standard|restricted   standard: CGR_MEMORY_MB=4096, threads automatic
#                                restricted: CGR_THREADS=2 CGR_MEMORY_MB=2048 (stands in for a 2 core / 6 GB box)
#   --cgr PATH                   binary under test (default: target/release/cgr, then target/fastrel/cgr)
#   --repos-dir DIR              checkouts of the benchmark repositories (default: $CGR_BENCH_REPOS or ./bench/repos)
#   --kinds index,sync,explore   which benchmarks to run (default: all three)
#   --runs N                     index runs per repository, median reported (default 3)
#   --edits N                    random single-file edits for `bench sync` (default 200)
#   --cooldown SECONDS           pause between runs so the laptop is not throttling (default 30)
#   --out DIR                    raw results (default: ./bench/out)
#   --history DIR                saved summaries (default: ./bench/history)
#   --compare                    fail when any timing is >10% slower than the last saved summary of this tier
#   --accept                     save this run as the new baseline even if it regressed
#
# Guard rails, all of which are MUST per the design:
#   1. refuse to start unless available memory >= 6 GB, swap in use < 1 GB and free disk >= 20 GB
#   2. keep the machine awake (`caffeinate -i` on macOS)
#   3. sample the RSS of the whole process tree every 200 ms; kill it when it passes tier budget x 1.5
#      or when swap grows by more than 1 GB while it runs
#   4. only one process under test at any time
#
# Exit codes: 0 ok, 1 regression (--compare), 2 usage, 3 refused to start, 4 killed by the guard.
set -euo pipefail

TIER=standard
CGR=""
REPOS_DIR="${CGR_BENCH_REPOS:-bench/repos}"
KINDS="index,sync,explore"
RUNS=3
EDITS=200
COOLDOWN=30
OUT="bench/out"
HISTORY="bench/history"
COMPARE=0
ACCEPT=0
REPOS=()

# thresholds of rule 1; overridable only through the environment, never silently
MIN_AVAIL_MB="${CGR_BENCH_MIN_AVAIL_MB:-6144}"
MIN_DISK_MB="${CGR_BENCH_MIN_DISK_MB:-20480}"
MAX_SWAP_MB="${CGR_BENCH_MAX_SWAP_MB:-1024}"
MAX_SWAP_GROWTH_MB=1024

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --tier) TIER="$2"; shift 2;;
    --cgr) CGR="$2"; shift 2;;
    --repos-dir) REPOS_DIR="$2"; shift 2;;
    --kinds) KINDS="$2"; shift 2;;
    --runs) RUNS="$2"; shift 2;;
    --edits) EDITS="$2"; shift 2;;
    --cooldown) COOLDOWN="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --history) HISTORY="$2"; shift 2;;
    --compare) COMPARE=1; shift;;
    --accept) ACCEPT=1; shift;;
    -h|--help) usage; exit 0;;
    -*) echo "unknown option: $1" >&2; usage >&2; exit 2;;
    *) REPOS+=("$1"); shift;;
  esac
done

case "$TIER" in
  standard) export CGR_MEMORY_MB=4096; unset CGR_THREADS || true; BUDGET_MB=4096;;
  restricted) export CGR_THREADS=2 CGR_MEMORY_MB=2048; BUDGET_MB=2048;;
  *) echo "unknown tier: $TIER" >&2; exit 2;;
esac
LIMIT_MB="${CGR_BENCH_LIMIT_MB:-$((BUDGET_MB * 3 / 2))}" # CGR_BENCH_LIMIT_MB exists to test the kill path

if [ -z "$CGR" ]; then
  for c in target/release/cgr target/fastrel/cgr; do [ -x "$c" ] && CGR="$c" && break; done
fi
[ -n "$CGR" ] && [ -x "$CGR" ] || { echo "no cgr binary found; build one or pass --cgr" >&2; exit 2; }
CGR="$(cd "$(dirname "$CGR")" && pwd)/$(basename "$CGR")"

OS="$(uname -s)"

# ---------------------------------------------------------------- measurements

avail_mb() {
  if [ "$OS" = Darwin ]; then
    local page; page="$(sysctl -n hw.pagesize 2>/dev/null || echo 4096)"
    vm_stat | awk -v ps="$page" '
      /Pages free/ {f=$3} /Pages inactive/ {i=$3} /Pages speculative/ {s=$3}
      END {gsub(/\./,"",f); gsub(/\./,"",i); gsub(/\./,"",s); printf "%d", (f+i+s)*ps/1048576}'
  else
    awk '/^MemAvailable:/ {printf "%d", $2/1024}' /proc/meminfo
  fi
}

swap_used_mb() {
  if [ "$OS" = Darwin ]; then
    sysctl -n vm.swapusage 2>/dev/null | awk '
      {for (i = 1; i <= NF; i++) if ($i == "used") {v = $(i + 2); u = substr(v, length(v)); sub(/[A-Za-z]$/, "", v);
                                                   if (u == "G") v *= 1024; else if (u == "K") v /= 1024; printf "%d", v}}'
  else
    awk '/^SwapTotal:/ {t=$2} /^SwapFree:/ {f=$2} END {printf "%d", (t-f)/1024}' /proc/meminfo
  fi
}

disk_free_mb() { df -Pk "$1" | awk 'NR==2 {printf "%d", $4/1024}'; }

# resident memory (KB) of a process and everything it started
tree_rss_kb() {
  ps -A -o pid=,ppid=,rss= | awk -v root="$1" '
    {ppid[$1]=$2; rss[$1]=$3}
    END {s = 0
         for (p in rss) { q = p; n = 0
           while (q != "" && q != 0 && n++ < 64) { if (q == root) { s += rss[p]; break } q = ppid[q] } }
         print s}'
}

kill_tree() {
  local p="$1" c
  for c in $(pgrep -P "$p" 2>/dev/null || true); do kill_tree "$c"; done
  kill -9 "$p" 2>/dev/null || true
}

preflight() {
  mkdir -p "$OUT" "$HISTORY"
  local avail swap disk
  avail="$(avail_mb)"; swap="$(swap_used_mb)"; disk="$(disk_free_mb "$OUT")"
  echo "preflight: available memory ${avail} MB (need ${MIN_AVAIL_MB}), swap in use ${swap} MB (max ${MAX_SWAP_MB}), free disk ${disk} MB (need ${MIN_DISK_MB})"
  if [ "$avail" -lt "$MIN_AVAIL_MB" ]; then echo "refusing to start: not enough available memory (close other applications)" >&2; exit 3; fi
  if [ "$swap" -ge "$MAX_SWAP_MB" ]; then echo "refusing to start: swap is already in use (reboot or close applications)" >&2; exit 3; fi
  if [ "$disk" -lt "$MIN_DISK_MB" ]; then echo "refusing to start: not enough free disk space for $OUT" >&2; exit 3; fi
}

# ---------------------------------------------------------------- running

AWAKE=()
if [ "$OS" = Darwin ] && command -v caffeinate >/dev/null 2>&1; then AWAKE=(caffeinate -i); fi

# guarded <name> <command...>: stdout -> $OUT/<name>.json, stderr -> $OUT/<name>.log,
# process-tree peak -> $OUT/<name>.guard.json
guarded() {
  local name="$1"; shift
  local swap0 t0 pid peak=0 killed="" mb kb sw
  swap0="$(swap_used_mb)"; t0="$(date +%s)"
  echo "-> $name"
  "${AWAKE[@]+"${AWAKE[@]}"}" "$@" > "$OUT/$name.json" 2> "$OUT/$name.log" &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    kb="$(tree_rss_kb "$pid")"; mb=$((kb / 1024))
    [ "$mb" -gt "$peak" ] && peak=$mb
    if [ "$mb" -gt "$LIMIT_MB" ]; then killed="memory ${mb} MB is over the limit of ${LIMIT_MB} MB (tier budget ${BUDGET_MB} MB x 1.5)"; break; fi
    sw="$(swap_used_mb)"
    if [ $((sw - swap0)) -gt "$MAX_SWAP_GROWTH_MB" ]; then killed="swap grew by $((sw - swap0)) MB"; break; fi
    sleep 0.2
  done
  if [ -n "$killed" ]; then
    kill_tree "$pid"; wait "$pid" 2>/dev/null || true
    printf '{"name":"%s","killed":"%s","peakRssMb":%d}\n' "$name" "$killed" "$peak" > "$OUT/$name.guard.json"
    echo "KILLED by the guard: $killed" >&2
    rm -f "$OUT/$name.json"
    exit 4
  fi
  local rc=0; wait "$pid" || rc=$?
  printf '{"name":"%s","peakRssMb":%d,"wallSeconds":%d,"exit":%d}\n' "$name" "$peak" $(( $(date +%s) - t0 )) "$rc" > "$OUT/$name.guard.json"
  if [ "$rc" -ne 0 ]; then echo "failed ($rc): see $OUT/$name.log" >&2; tail -5 "$OUT/$name.log" >&2 || true; fi
  echo "   peak process-tree RSS ${peak} MB, $(( $(date +%s) - t0 )) s"
  return "$rc"
}

cool() { [ "$COOLDOWN" -gt 0 ] && { echo "   cooling down ${COOLDOWN}s"; sleep "$COOLDOWN"; } || true; }

if [ ${#REPOS[@]} -eq 0 ]; then
  for r in express excalidraw django flask gin dubbo gson okhttp tokio ripgrep; do
    [ -d "$REPOS_DIR/$r" ] && REPOS+=("$r")
  done
fi
[ ${#REPOS[@]} -gt 0 ] || { echo "no repositories found in $REPOS_DIR" >&2; exit 2; }

preflight
rm -f "$OUT"/*.json "$OUT"/*.log # results of earlier runs would otherwise leak into the summary
echo "tier $TIER (CGR_MEMORY_MB=${CGR_MEMORY_MB}${CGR_THREADS:+, CGR_THREADS=$CGR_THREADS}), kill limit ${LIMIT_MB} MB, binary $CGR"
echo "repositories: ${REPOS[*]}"

first=1
for repo in "${REPOS[@]}"; do
  dir="$REPOS_DIR/$repo"
  [ -d "$dir" ] || { echo "skipping $repo: $dir does not exist" >&2; continue; }
  for kind in ${KINDS//,/ }; do
    [ "$first" -eq 1 ] || cool
    first=0
    case "$kind" in
      index) guarded "$repo.index" "$CGR" bench index "$dir" --runs "$RUNS" --json;;
      sync)
        [ -f "$dir/.cgr/manifest" ] || "$CGR" index "$dir" --quiet
        guarded "$repo.sync" "$CGR" bench sync "$dir" --edits "$EDITS" --seed 1 --json;;
      explore)
        [ -f "$dir/.cgr/manifest" ] || "$CGR" index "$dir" --quiet
        guarded "$repo.explore" "$CGR" bench explore "$dir" --runs "$RUNS" --json;;
      *) echo "unknown benchmark kind: $kind" >&2; exit 2;;
    esac
  done
done

echo
rc=0
"$CGR" bench compare --results "$OUT" --history "$HISTORY" --tier "$TIER" --threshold 10 $([ "$ACCEPT" -eq 1 ] && echo --force-save) || rc=$?
if [ "$COMPARE" -eq 1 ]; then exit "$rc"; fi
exit 0
