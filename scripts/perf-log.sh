#!/usr/bin/env bash
# The perf log: one JSON line per benchmark run, kept on the orphan branch
# `perf-log` (file perf/log.jsonl) by CI's perf-log job, and locally in
# target/perf-log.jsonl by `just perf`. Benchmarks gate nothing; this is
# how their numbers are kept and read over time.
#
#   scripts/perf-log.sh record [--append FILE] METRICS.jsonl...
#       Prints one record built from the benchmarks' measures (the lines
#       they write to CLAX_PERF_JSON): the commit and date, the runner, its
#       CPU count and load average, and each measure with its budget. With
#       --append, also appends it to FILE.
#   scripts/perf-log.sh show [--runs N] [--file FILE]
#       Prints a trend table per measure over the last N runs (default 10):
#       from FILE, or else from the `perf-log` branch fetched from origin.
#   scripts/perf-log.sh publish RECORD-FILE
#       CI only: appends the record to perf/log.jsonl on the `perf-log`
#       branch (made as an orphan if it does not exist) and pushes it,
#       rebasing and retrying when another run pushed first.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

usage() {
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

cmd="${1:-}"
[ "$#" -gt 0 ] && shift

case "$cmd" in
record)
    append=""
    if [ "${1:-}" = "--append" ]; then
        append="${2:?usage: $0 record --append FILE METRICS.jsonl...}"
        shift 2
    fi
    [ "$#" -gt 0 ] || usage
    commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
    if [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then commit="$commit+dirty"; fi
    line="$(PERF_COMMIT="$commit" python3 - "$@" <<'PY'
import datetime, json, os, platform, sys
metrics = {}
for path in sys.argv[1:]:
    if not os.path.exists(path):
        continue
    for raw in open(path, encoding="utf-8"):
        raw = raw.strip()
        if raw:
            m = json.loads(raw)
            metrics[m["name"]] = {"value": m["value"], "budget": m.get("budget"), "unit": m.get("unit", "")}
runner = os.environ.get("RUNNER_NAME") or platform.node()
kind = " ".join(x for x in (os.environ.get("RUNNER_OS"), os.environ.get("RUNNER_ARCH"), os.environ.get("RUNNER_ENVIRONMENT")) if x)
print(json.dumps({
    "commit": os.environ["PERF_COMMIT"],
    "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
    "runner": runner,
    "runner_type": kind or f"{platform.system()} {platform.machine()} (local)",
    "platform": platform.platform(),
    "cpus": os.cpu_count(),
    "load": [round(x, 2) for x in os.getloadavg()],
    "metrics": metrics,
}, sort_keys=True))
PY
)"
    echo "$line"
    if [ -n "$append" ]; then
        mkdir -p "$(dirname "$append")"
        printf '%s\n' "$line" >> "$append"
    fi
    ;;
show)
    runs=10
    file=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --runs) runs="${2:?}"; shift 2 ;;
            --file) file="${2:?}"; shift 2 ;;
            *) usage ;;
        esac
    done
    src="$(mktemp)"
    trap 'rm -f "$src"' EXIT
    if [ -n "$file" ]; then
        cat "$file" > "$src"
    else
        git fetch -q origin +perf-log:refs/remotes/origin/perf-log
        git show origin/perf-log:perf/log.jsonl > "$src"
    fi
    python3 - "$src" "$runs" <<'PY'
import json, sys
records = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()][-int(sys.argv[2]):]
if not records:
    sys.exit("no runs in the log")
print("runs, oldest first:")
for i, r in enumerate(records, 1):
    print(f"  {i:>2}. {r['date']}  {r['commit'][:10]}  {r.get('runner_type', '')}, {r.get('cpus')} CPUs, load {r.get('load', [''])[0]}")
names = sorted({n for r in records for n in r["metrics"]})
width = max(len(n) for n in names)
print()
print(f"{'measure':<{width}}  {'budget':>9}  " + "  ".join(f"{i:>9}" for i in range(1, len(records) + 1)))
for n in names:
    cells, budget, unit = [], "", ""
    for r in records:
        m = r["metrics"].get(n)
        if m is None:
            cells.append(f"{'-':>9}")
            continue
        unit = m.get("unit") or ""
        budget = "" if m.get("budget") is None else f"{m['budget']:g}"
        v = m["value"]
        mark = "*" if m.get("budget") is not None and v is not None and v > m["budget"] else " "
        cells.append(f"{'-' if v is None else format(v, 'g'):>8}{mark}")
    print(f"{n:<{width}}  {budget:>9}  " + "  ".join(cells) + (f"  {unit}" if unit else ""))
print()
print("* over its budget (reported, not gated); budgets as each run read them, the last shown")
PY
    ;;
publish)
    record="${1:?usage: $0 publish RECORD-FILE}"
    [ -s "$record" ] || { echo "error: $record is empty" >&2; exit 1; }
    work="$(mktemp -d)"
    trap 'git worktree remove --force "$work" >/dev/null 2>&1 || rm -rf "$work"' EXIT
    if git fetch -q origin +perf-log:refs/remotes/origin/perf-log 2>/dev/null; then
        git worktree add -q --detach "$work" origin/perf-log
    else
        # The first run: an orphan branch holding only the log.
        git worktree add -q --detach "$work" HEAD
        git -C "$work" checkout -q --orphan perf-log
        git -C "$work" rm -rq --cached .
        find "$work" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
    fi
    for attempt in 1 2 3 4 5; do
        mkdir -p "$work/perf"
        cat "$record" >> "$work/perf/log.jsonl"
        git -C "$work" add perf/log.jsonl
        git -C "$work" -c user.name="github-actions[bot]" \
            -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
            commit -q -m "Log the benchmarks of $(git rev-parse --short HEAD)"
        if git -C "$work" push -q origin HEAD:refs/heads/perf-log; then
            echo "perf log: appended to perf-log (attempt $attempt)"
            exit 0
        fi
        # Another run pushed first: start again from its branch.
        git fetch -q origin +perf-log:refs/remotes/origin/perf-log
        git -C "$work" reset -q --hard origin/perf-log
    done
    echo "error: could not push the perf log after 5 attempts" >&2
    exit 1
    ;;
*) usage ;;
esac
