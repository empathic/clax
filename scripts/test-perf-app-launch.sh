#!/usr/bin/env bash
# Checks scripts/perf-app-launch.sh without the app: the steps its dry runs
# print for --macos and --android, the median and p90 it gives a
# timings.jsonl of five launches, the perf-log lines and the budgets it
# records, and (on a Mac) its launch loop over a fake app that writes the
# lines a launch writes and waits for SIGTERM.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
has() { grep -qF -- "$2" <<<"$1"; }
run() { bash scripts/perf-app-launch.sh "$@"; }

# Dry runs print the steps and change nothing.
out="$(run --macos)"
if has "$out" "would run" && has "$out" "./scripts/run.sh app-macos" \
    && has "$out" "10 times: CLAX_APP_DIR=" && has "$out" "Contents/MacOS/Clax" \
    && has "$out" "SIGTERM the app and its daemon by PID" && has "$out" "drop the first launch" \
    && has "$out" "--i-know-what-i-am-doing"; then
    ok "--macos dry run: build, scratch CLAX_APP_DIR, ten launches for nine runs, SIGTERM by PID"
else
    bad "--macos dry run: $out"
fi
out="$(CLAX_PERF_QUICK=1 run --macos --app /x/Clax.app)"
if has "$out" "take the app /x/Clax.app" && has "$out" "6 times:" && ! has "$out" "run.sh app-macos"; then
    ok "--app and CLAX_PERF_QUICK=1: no build, six launches for five runs"
else
    bad "--app with CLAX_PERF_QUICK=1: $out"
fi
out="$(run --android --runs 3)"
if has "$out" "4 times: adb shell am force-stop dev.empathic.clax; adb shell am start -W -n dev.empathic.clax/dev.empathic.clax_app.MainActivity" \
    && has "$out" "run-as dev.empathic.clax cat files/timings.jsonl" && has "$out" "drop the first launch"; then
    ok "--android dry run: force-stop and start -W per launch, the timings read with run-as"
else
    bad "--android dry run: $out"
fi
if run --runs 0 >/dev/null 2>&1; then bad "--runs 0 is refused"; else ok "--runs 0 is refused"; fi

# Five launches, the third without a snapshot (no home-cover line).
line() { printf '{"name":"%s","elapsed_ms":%s,"at":"2026-10-20T00:00:00.000Z","extra":{}}\n' "$1" "$2"; }
{
    line home-cover 25; line home-usable 400
    line home-cover 22; line home-usable 380
    echo 'not json'; line home-usable 520
    line home-cover 30; line home-usable 410
    line say-usable 999; line home-cover 21; line home-usable 450
} >"$d/timings.jsonl"
echo '{"darwin-arm64": {"baseline": {}, "app_home_cover_ms": {"budget": 25}}}' >"$d/budget.json"
want='{"platform": "darwin-arm64", "app_home_usable_ms": {"median": 410, "p90": 520, "runs": 5, "missing": 0}, "app_home_cover_ms": {"median": 23.5, "p90": 30, "runs": 4, "missing": 1}}'
out="$(CLAX_PERF_APP_TIMINGS="$d/timings.jsonl" CLAX_PERF_PLATFORM=darwin-arm64 CLAX_PERF_BUDGETS="$d/budget.json" \
    CLAX_PERF_JSON="$d/metrics.jsonl" run 2>"$d/err")"
if [ "$out" = "$want" ]; then
    ok "five launches: home-usable median 410 and p90 520, home-cover of four 23.5 and 30"
else
    bad "the summary: $out (want $want)"
fi
want_log='{"name": "app launch: darwin-arm64 home usable median", "value": 410, "budget": null, "unit": "ms"}
{"name": "app launch: darwin-arm64 home usable p90", "value": 520, "budget": null, "unit": "ms"}
{"name": "app launch: darwin-arm64 home cover median", "value": 23.5, "budget": null, "unit": "ms"}
{"name": "app launch: darwin-arm64 home cover p90", "value": 30, "budget": 25, "unit": "ms"}'
if [ "$(cat "$d/metrics.jsonl")" = "$want_log" ] && grep -qF "app_home_cover_ms p90 30 ms is over its budget of 25 ms (reported, not gated)" "$d/err"; then
    ok "CLAX_PERF_JSON: each median and p90 for perf-log.sh, the p90s beside their budgets; over budget is reported"
else
    bad "the perf-log lines: $(cat "$d/metrics.jsonl") / $(cat "$d/err")"
fi
CLAX_PERF_APP_TIMINGS="$d/timings.jsonl" CLAX_PERF_PLATFORM=darwin-arm64 CLAX_PERF_BUDGETS="$d/budget.json" \
    run --record >/dev/null 2>"$d/err"
want_budget='{
  "darwin-arm64": {
    "baseline": {},
    "app_home_cover_ms": {
      "budget": 25
    },
    "app_home_usable_ms": {
      "budget": 550
    }
  }
}'
if [ "$(cat "$d/budget.json")" = "$want_budget" ]; then
    ok "--record adds the missing budget, the p90 rounded up to 50 ms, and keeps the rest"
else
    bad "--record: $(cat "$d/budget.json") / $(cat "$d/err")"
fi
CLAX_PERF_APP_TIMINGS="$d/timings.jsonl" CLAX_PERF_PLATFORM=darwin-arm64 CLAX_PERF_BUDGETS="$d/budget.json" \
    run --record >/dev/null 2>"$d/err"
if [ "$(cat "$d/budget.json")" = "$want_budget" ] && grep -qF "already has darwin-arm64's budgets" "$d/err"; then
    ok "--record changes no budget a platform has"
else
    bad "--record twice: $(cat "$d/budget.json") / $(cat "$d/err")"
fi

# The launch loop, on a Mac: a fake app that writes the next launch's lines
# to its CLAX_APP_DIR's timings.jsonl and then waits for SIGTERM.
if [ "$(uname -s)" = Darwin ]; then
    app="$d/Fake.app"
    mkdir -p "$app/Contents/MacOS" "$d/base"
    printf '%s\n' "900 -" "400 20" "300 -" "500 30" >"$d/values"
    cat >"$app/Contents/MacOS/Clax" <<EOF
#!/bin/bash
set -eu
t="\$CLAX_APP_DIR/support/timings.jsonl"
mkdir -p "\$CLAX_APP_DIR/support"
n=\$(grep -c home-usable "\$t" 2>/dev/null || true)
read -r usable cover < <(sed -n "\$((\${n:-0} + 1))p" "$d/values")
[ "\$cover" = - ] || printf '{"name":"home-cover","elapsed_ms":%s}\n' "\$cover" >>"\$t"
printf '{"name":"home-usable","elapsed_ms":%s}\n' "\$usable" >>"\$t"
echo "\$\$" >>"$d/pids"
exec tail -f /dev/null
EOF
    chmod +x "$app/Contents/MacOS/Clax"
    out="$(CLAX_PERF_APP_BASE="$d/base" CLAX_PERF_BUDGETS="$d/none.json" run --macos --app "$app" --runs 3 --i-know-what-i-am-doing 2>"$d/err")"
    want='"app_home_usable_ms": {"median": 400, "p90": 500, "runs": 3, "missing": 0}, "app_home_cover_ms": {"median": 25.0, "p90": 30, "runs": 2, "missing": 1}}'
    alive=""
    while read -r p; do kill -0 "$p" 2>/dev/null && alive="$alive $p"; done <"$d/pids"
    if has "$out" "$want" && [ "$(wc -l <"$d/pids" | tr -d ' ')" = 4 ] && [ -z "$alive" ] \
        && [ -z "$(ls -A "$d/base")" ]; then
        ok "--macos: four launches for three runs, the first dropped, each app stopped, the scratch directory removed"
    else
        bad "--macos launch loop: $out / alive:$alive / $(ls -A "$d/base") / $(cat "$d/err")"
    fi
else
    echo "SKIP: the --macos launch loop runs on a Mac"
fi

exit "$fails"
