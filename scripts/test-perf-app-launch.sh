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
    && has "$out" "CFBundleIdentifier to dev.empathic.clax.perf" \
    && has "$out" "codesign --force --sign - --preserve-metadata=entitlements,flags,runtime" \
    && has "$out" "10 times: CLAX_APP_NO_KEY=1 <copy>/Contents/MacOS/Clax" && has "$out" "Library/Containers/dev.empathic.clax.perf" \
    && has "$out" "SIGTERM the app and its daemon by PID" && has "$out" "drop the first launch" \
    && has "$out" "after the first, wait for its home snapshot, Data/Library/Caches/dev.empathic.clax.perf/home/home-snapshot.bin" \
    && ! has "$out" "Containers/dev.empathic.clax/" && ! has "$out" "CLAX_APP_DIR" \
    && has "$out" "--i-know-what-i-am-doing"; then
    ok "--macos dry run: build, a copy re-identified as dev.empathic.clax.perf in its own container, ten launches for nine runs with the saved key withheld, SIGTERM by PID"
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
    && has "$out" "wait for a clax-timing: home-usable line in adb logcat -d --pid=<the app's PID>" && has "$out" "drop the first launch" \
    && has "$out" "after the first, wait for clax-home: snapshot kept in that log" && ! has "$out" "run-as"; then
    ok "--android dry run: force-stop and start -W per launch, the timings read from the app's log (no run-as: a release build is not debuggable), the dropped launch's snapshot awaited"
else
    bad "--android dry run: $out"
fi
out="$(run --android --record)"
if has "$out" "record the device's missing budgets in web/perf/app-budget.json" && ! has "$out" "web/perf/budget.json"; then
    ok "the app's budgets are kept in web/perf/app-budget.json, apart from the web's budget.json that usable.perf.ts reads"
else
    bad "the budgets file: $out"
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
echo '{"darwin-arm64": {"app_home_cover_ms": {"budget": 25}}}' >"$d/budget.json"
want='{"platform": "darwin-arm64", "build": {"commit": "abc123"}, "app_home_usable_ms": {"median": 410, "p90": 520, "runs": 5, "missing": 0}, "app_home_cover_ms": {"median": 23.5, "p90": 30, "runs": 4, "missing": 1}}'
out="$(CLAX_PERF_APP_TIMINGS="$d/timings.jsonl" CLAX_PERF_PLATFORM=darwin-arm64 CLAX_PERF_BUDGETS="$d/budget.json" \
    CLAX_PERF_JSON="$d/metrics.jsonl" run --commit abc123 2>"$d/err")"
if [ "$out" = "$want" ]; then
    ok "five launches: home-usable median 410 and p90 520, home-cover of four 23.5 and 30, the build named by --commit"
else
    bad "the summary: $out (want $want)"
fi
want_log='{"name": "app launch: darwin-arm64 home usable median", "value": 410, "budget": null, "unit": "ms"}
{"name": "app launch: darwin-arm64 home usable p90", "value": 520, "budget": null, "unit": "ms"}
{"name": "app launch: darwin-arm64 home cover median", "value": 23.5, "budget": null, "unit": "ms"}
{"name": "app launch: darwin-arm64 home cover p90", "value": 30, "budget": 25, "unit": "ms"}
{"name": "app launch: darwin-arm64 home usable missing", "value": 0, "budget": null, "unit": "launches"}
{"name": "app launch: darwin-arm64 home cover missing", "value": 1, "budget": null, "unit": "launches"}'
if [ "$(cat "$d/metrics.jsonl")" = "$want_log" ] && grep -qF "app_home_cover_ms p90 30 ms is over its budget of 25 ms (reported, not gated)" "$d/err"; then
    ok "CLAX_PERF_JSON: each median and p90 for perf-log.sh, the p90s beside their budgets, and the launches missing; over budget is reported"
else
    bad "the perf-log lines: $(cat "$d/metrics.jsonl") / $(cat "$d/err")"
fi
# scripts/perf-log.sh names the commit PERF_COMMIT gives, when it is set.
rec="$(PERF_COMMIT=feedface0 bash scripts/perf-log.sh record "$d/metrics.jsonl")"
if [ "$(python3 -I -c 'import json,sys; r=json.loads(sys.argv[1]); print(r["commit"], r["metrics"]["app launch: darwin-arm64 home usable p90"]["value"])' "$rec")" = "feedface0 520" ]; then
    ok "perf-log.sh record names PERF_COMMIT's build"
else
    bad "perf-log.sh record with PERF_COMMIT: $rec"
fi
# A run with a launch missing records nothing.
before="$(cat "$d/budget.json")"
if CLAX_PERF_APP_TIMINGS="$d/timings.jsonl" CLAX_PERF_PLATFORM=darwin-arm64 CLAX_PERF_BUDGETS="$d/budget.json" \
    run --record >/dev/null 2>"$d/err" || [ "$(cat "$d/budget.json")" != "$before" ] \
    || ! grep -qF "records no budget from a run with launches missing (home cover: 1)" "$d/err"; then
    bad "--record with a launch missing: $(cat "$d/budget.json") / $(cat "$d/err")"
else
    ok "--record refuses a run with a launch missing and writes nothing"
fi
# Five launches, each with its cover.
{
    line home-cover 25; line home-usable 400
    line home-cover 22; line home-usable 380
    line home-cover 24; line home-usable 520
    line home-cover 30; line home-usable 410
    line home-cover 21; line home-usable 450
} >"$d/timings.jsonl"
CLAX_PERF_APP_TIMINGS="$d/timings.jsonl" CLAX_PERF_PLATFORM=darwin-arm64 CLAX_PERF_BUDGETS="$d/budget.json" \
    run --record >/dev/null 2>"$d/err"
want_budget='{
  "darwin-arm64": {
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

# The Android launch loop over a fake adb: each `am start` starts a new PID
# whose log (`logcat -d --pid=<PID>`) holds that launch's clax-timing:
# lines and, a while after its home-usable, clax-home: snapshot kept; the
# first launch has no snapshot to cover it. Like a release build, `run-as`
# fails.
mkdir -p "$d/adb"
echo 0 >"$d/adb/n"
cat >"$d/adb/adb" <<EOF
#!/bin/bash
s="$d/adb"
echo "\$*" >>"\$s/calls"
case "\$1 \${2:-}" in
    "get-state "*) echo device ;;
    "shell pm") echo package:/data/app/base.apk ;;
    "shell getprop") echo "Pixel 9" ;;
    "shell dumpsys") echo "    versionCode=12 minSdk=29 targetSdk=36"; echo "    versionName=0.6.0" ;;
    "shell run-as") echo "run-as: package not debuggable: dev.empathic.clax" >&2; exit 1 ;;
    "shell am")
        if [ "\$3" = start ]; then
            n=\$((\$(cat "\$s/n") + 1)); echo "\$n" >"\$s/n"; echo "\$((1000 + n))" >"\$s/pid"
        else
            rm -f "\$s/pid"
        fi ;;
    "shell pidof") cat "\$s/pid" 2>/dev/null || true ;;
    "logcat -d")
        pid="\${3#--pid=}"; n=\$((pid - 1000))
        [ -e "\$s/hang" ] && exit 0
        read -r usable cover < <(sed -n "\${n}p" "$d/adb/values")
        say() { printf '10-20 22:00:00.000  %s  %s I flutter : %s\n' "\$pid" "\$pid" "\$1"; }
        say "clax-core: ready pid=\$pid"
        [ "\$cover" = - ] || say "clax-timing: {\"name\":\"home-cover\",\"elapsed_ms\":\$cover,\"at\":\"2026-10-20T22:00:00.000Z\",\"extra\":{}}"
        say "clax-timing: {\"name\":\"home-usable\",\"elapsed_ms\":\$usable,\"at\":\"2026-10-20T22:00:00.000Z\",\"extra\":{}}"
        # The snapshot is kept a while after the home screen is up.
        if [ ! -e "\$s/nosnap" ] && [ "\$(cat "\$s/polls" 2>/dev/null || echo 0)" -ge 2 ]; then say "clax-home: snapshot kept"; fi
        echo \$((\$(cat "\$s/polls" 2>/dev/null || echo 0) + 1)) >"\$s/polls" ;;
    *) echo "fake adb: unexpected: \$*" >&2; exit 1 ;;
esac
EOF
chmod +x "$d/adb/adb"
printf '%s\n' "900 -" "400 20" "300 25" "500 30" >"$d/adb/values"
droid() { ADB="$d/adb/adb" CLAX_PERF_BUDGETS="$d/none.json" run --android --runs 3 --i-know-what-i-am-doing "$@"; }
out="$(droid --commit feed 2>"$d/err")" || true
want='"app_home_usable_ms": {"median": 400, "p90": 500, "runs": 3, "missing": 0}, "app_home_cover_ms": {"median": 25, "p90": 30, "runs": 3, "missing": 0}}'
if has "$out" "$want" && has "$out" '"platform": "android-Pixel-9"' && has "$out" '"version": "0.6.0 (12)"' \
    && [ "$(grep -c '^shell am start -W' "$d/adb/calls")" = 4 ] && ! grep -q 'run-as' "$d/adb/calls" \
    && grep -q '^logcat -d --pid=1004$' "$d/adb/calls"; then
    ok "--android: four launches for three runs, each read from its own PID's log (never run-as), the first dropped once its snapshot is kept"
else
    bad "--android launch loop: $out / $(cat "$d/err") / $(tr '\n' ';' <"$d/adb/calls")"
fi
echo 0 >"$d/adb/n"; rm -f "$d/adb/polls"; : >"$d/adb/calls"
touch "$d/adb/nosnap"
if CLAX_PERF_CAP=1 droid >"$d/out" 2>"$d/err" || [ "$(grep -c '^shell am start -W' "$d/adb/calls")" != 1 ] \
    || ! grep -qF "no clax-home: snapshot kept in its log" "$d/err"; then
    bad "--android: a dropped launch that keeps no snapshot stops the run: $(cat "$d/out") / $(cat "$d/err")"
else
    ok "--android: a dropped launch that keeps no snapshot within the cap stops the run"
fi
rm "$d/adb/nosnap"
echo 0 >"$d/adb/n"; : >"$d/adb/calls"
touch "$d/adb/hang"
if CLAX_PERF_CAP=1 droid >"$d/out" 2>"$d/err" || [ "$(grep -c '^shell am start -W' "$d/adb/calls")" != 1 ] \
    || ! grep -qF "the dropped launch gave no home-usable within 1 s" "$d/err"; then
    bad "--android: a dropped launch without home-usable stops the run: $(cat "$d/out") / $(cat "$d/err")"
else
    ok "--android: a dropped launch without home-usable in its log stops the run"
fi
rm "$d/adb/hang"

# The launch loop, on a Mac: a fake app that writes the next launch's lines
# to the timings.jsonl in its container's support directory and then waits
# for SIGTERM. Its copy is re-identified (plutil, and codesign stubbed on
# PATH) and runs in a container of its own under $d.
if [ "$(uname -s)" = Darwin ]; then
    app="$d/Fake.app"
    perf=dev.empathic.clax.perf
    box="$d/Containers/$perf"
    support="$box/Data/Library/Application Support/$perf"
    mkdir -p "$app/Contents/MacOS" "$d/bin" "$box/Data"
    : >"$box/Data/stale"
    cat >"$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Clax</string>
<key>CFBundleIdentifier</key><string>dev.empathic.clax</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
<key>CFBundleVersion</key><string>7</string>
</dict></plist>
EOF
    printf '#!/bin/sh\necho "$*" >>"%s/codesign.log"\n' "$d" >"$d/bin/codesign"
    # ioreg: the screen is locked while $d/locked exists.
    printf '#!/bin/sh\nif [ -e "%s/locked" ]; then l=Yes; else l=No; fi\necho "  \\"IOConsoleUsers\\" = ({\\"CGSSessionScreenIsLocked\\"=$l})"\n' "$d" >"$d/bin/ioreg"
    chmod +x "$d/bin/codesign" "$d/bin/ioreg"
    # Each line: the launch's home-usable and the home-cover it writes when
    # a snapshot is kept. Like the app, the fake writes home-cover only
    # when a snapshot exists, and takes the snapshot a while after the home
    # screen is up (the app's settle, apps/clax_app/lib/home/home_snapshot.dart),
    # never on SIGTERM; with $d/nosnap it never takes one.
    printf '%s\n' "900 19" "400 20" "300 25" "500 30" >"$d/values"
    cat >"$app/Contents/MacOS/Clax" <<EOF
#!/bin/bash
set -eu
[ ! -e "$box/Data/stale" ] || echo stale >>"$d/stale"
echo "\$0 \${CLAX_APP_DIR-unset} \${CLAX_APP_NO_KEY-unset}" >>"$d/launched"
echo "\$\$" >>"$d/pids"
t="$support/timings.jsonl"
snap="$box/Data/Library/Caches/$perf/home/home-snapshot.bin"
mkdir -p "$support"
[ ! -e "$d/hang" ] || exec tail -f /dev/null
n=\$(grep -c home-usable "\$t" 2>/dev/null || true)
read -r usable cover < <(sed -n "\$((\${n:-0} + 1))p" "$d/values")
[ ! -e "\$snap" ] || printf '{"name":"home-cover","elapsed_ms":%s}\n' "\$cover" >>"\$t"
printf '{"name":"home-usable","elapsed_ms":%s}\n' "\$usable" >>"\$t"
sleep 1 &
wait
if [ ! -e "$d/nosnap" ]; then mkdir -p "\$(dirname "\$snap")"; : >"\$snap"; fi
exec tail -f /dev/null
EOF
    chmod +x "$app/Contents/MacOS/Clax"
    loop() {
        PATH="$d/bin:$PATH" CLAX_PERF_CONTAINER="$box" CLAX_PERF_WORK="$d/work" CLAX_PERF_BUDGETS="$d/none.json" \
            CLAX_APP_DIR=/nowhere CLAX_APP_NO_KEY=0 run --macos --app "$app" --runs 3 --i-know-what-i-am-doing "$@"
    }
    out="$(loop --commit feed 2>"$d/err")"
    want='"app_home_usable_ms": {"median": 400, "p90": 500, "runs": 3, "missing": 0}, "app_home_cover_ms": {"median": 25, "p90": 30, "runs": 3, "missing": 0}}'
    alive=""
    while read -r p; do kill -0 "$p" 2>/dev/null && alive="$alive $p"; done <"$d/pids"
    if has "$out" "$want" && [ "$(wc -l <"$d/pids" | tr -d ' ')" = 4 ] && [ -z "$alive" ]; then
        ok "--macos: four launches for three runs, the first dropped once its snapshot is kept, so each measured launch has its cover; each app stopped"
    else
        bad "--macos launch loop: $out / alive:$alive / $(cat "$d/err")"
    fi
    digest="$(cd "$app" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1)"
    if [ "$(python3 -I -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1])["build"], sort_keys=True))' "$out")" \
        = "{\"app\": \"$app\", \"commit\": \"feed\", \"sha256\": \"$digest\", \"version\": \"1.2.3 (7)\"}" ]; then
        ok "--macos: the JSON names the build measured: --commit, the app, its digest and its version"
    else
        bad "--macos build: $out (digest $digest)"
    fi
    copy="$(ls -d "$d"/work/clax-perf-*/Fake.app 2>/dev/null | sed -n 1p)"
    if [ -n "$copy" ] && [ "$(plutil -extract CFBundleIdentifier raw -o - "$copy/Contents/Info.plist")" = "$perf" ] \
        && [ "$(plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")" = dev.empathic.clax ] \
        && grep -qxF -- "--force --sign - --preserve-metadata=entitlements,flags,runtime ${copy%/Fake.app}.new/Fake.app" "$d/codesign.log" \
        && [ "$(sort -u "$d/launched")" = "$copy/Contents/MacOS/Clax unset 1" ]; then
        ok "--macos: the copy, never the app given, is re-identified as $perf and signed again keeping its entitlements; every launch runs without CLAX_APP_DIR and with CLAX_APP_NO_KEY=1"
    else
        bad "--macos copy: ${copy:-none} / $(cat "$d/codesign.log" 2>/dev/null) / $(sort -u "$d/launched")"
    fi
    if [ ! -e "$d/stale" ] && [ ! -e "$box" ]; then
        ok "--macos: the copy's container is removed before the first launch and at the end"
    else
        bad "--macos container: stale:$(cat "$d/stale" 2>/dev/null) / $(ls -A "$box" 2>/dev/null)"
    fi
    : >"$d/pids"
    touch "$d/nosnap"
    if CLAX_PERF_CAP=1 loop >"$d/out" 2>"$d/err" || [ "$(wc -l <"$d/pids" | tr -d ' ')" != 1 ] \
        || ! grep -qF "no home snapshot within 1 s" "$d/err" || [ -e "$box" ]; then
        bad "a dropped launch that keeps no snapshot stops the run: $(cat "$d/out") / $(cat "$d/err")"
    else
        ok "a dropped launch that keeps no snapshot within the cap stops the run before any measured launch"
    fi
    rm "$d/nosnap"
    : >"$d/pids"
    touch "$d/hang"
    if CLAX_PERF_CAP=1 loop >"$d/out" 2>"$d/err" || [ "$(wc -l <"$d/pids" | tr -d ' ')" != 1 ] \
        || ! grep -qF "the dropped launch gave no home-usable within 1 s" "$d/err" || [ -e "$box" ]; then
        bad "a dropped launch without home-usable stops the run: $(cat "$d/out") / $(cat "$d/err")"
    else
        ok "a dropped launch without home-usable stops the run after one launch, its app stopped"
    fi
    rm "$d/hang"
    : >"$d/pids"
    touch "$d/locked"
    if loop >"$d/out" 2>"$d/err" || [ -s "$d/pids" ] || ! grep -qF "the screen is locked" "$d/err"; then
        bad "a locked screen stops the run before any launch: $(cat "$d/out") / $(cat "$d/err")"
    else
        ok "a locked screen (CGSSessionScreenIsLocked) stops the run before any launch"
    fi
    rm "$d/locked"
    : >"$d/launched"
    if CLAX_PERF_CONTAINER="$d/Containers/dev.empathic.clax" CLAX_PERF_WORK="$d/work" run --macos --app "$app" --i-know-what-i-am-doing >/dev/null 2>"$d/err" \
        || [ -s "$d/launched" ] || ! grep -qF "dev.empathic.clax.perf" "$d/err"; then
        bad "a container not the copy's is refused: $(cat "$d/err")"
    else
        ok "a container not named dev.empathic.clax.perf is refused before any launch"
    fi
else
    echo "SKIP: the --macos launch loop runs on a Mac"
fi

exit "$fails"
