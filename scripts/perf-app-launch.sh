#!/usr/bin/env bash
# perf-app-launch.sh [--macos|--android] [--runs N] [--app <Clax.app|zip>]
#                    [--commit <label>] [--record] [--i-know-what-i-am-doing]
#
# Cold-starts the Clax app N times (default 9; CLAX_PERF_QUICK=1: 5) and
# reports the median and p90 of its two launch timings, `home-usable`
# (launch to the home app's first `shown`) and `home-cover` (launch to the
# kept snapshot's first frame), from the lines each launch adds to
# timings.jsonl (docs/usage.md, "Measuring the home screen's start"). It
# gates nothing. Without --i-know-what-i-am-doing it prints each step and
# changes nothing.
#
# --macos (the default): takes --app (an app or the zip `just app-macos`
#   makes), or builds //apps/clax_app:macos_app (scripts/run.sh app-macos),
#   and makes, once per build, a copy of it under target/perf/clax-perf-<sha>
#   (CLAX_PERF_WORK names another parent) whose CFBundleIdentifier is
#   dev.empathic.clax.perf, signed again ad hoc with its entitlements kept.
#   The copy runs in a sandbox container of its own,
#   ~/Library/Containers/dev.empathic.clax.perf (CLAX_PERF_CONTAINER names
#   another; it must end in dev.empathic.clax.perf), which the script
#   removes before the first launch and at the end; the installed app's
#   container, dev.empathic.clax's, is never read or written. Each launch
#   runs the copy's Contents/MacOS/Clax and waits for a new home-usable
#   line in its support directory's timings.jsonl, at most 60 s (a launch
#   without one counts as missing), then sends the app SIGTERM by its PID,
#   and the daemon it started too (by the PID in its daemon.json), waiting
#   for each to exit.
# --android: on the plugged-in phone or emulator (ANDROID_SERIAL picks
#   one), with the app already installed (`just app-phone`): `am
#   force-stop` and `am start -W` N times, each waiting for a new
#   home-usable line in files/timings.jsonl (read with `run-as`).
#
# One launch more than N runs first and is dropped: it may have no snapshot
# (on macOS the copy's container is new; on Android the first launch after
# an install has none) and pays macOS's assessment of new executables. It
# stays up until its home screen has kept its snapshot
# (cache/home/home-snapshot.bin, a few seconds after the home screen is up;
# the app keeps none when it is stopped), at most the same 60 s, so every
# launch after it starts from that snapshot; without one the run stops.
#
# Output: a JSON object on stdout, {"platform", "build", "app_home_usable_ms":
# {"median", "p90", "runs", "missing"}, "app_home_cover_ms": {…}}, where
# "build" names what was measured: "commit" (--commit's label; without it,
# the checkout's HEAD when the script built the app, else null), and for a
# launch loop the app taken ("app"), its "sha256" (of the zip, or of every
# file of the bundle by path) and its "version" (CFBundleShortVersionString
# and CFBundleVersion on macOS, versionName and versionCode on Android); and
# `missing` counts the launches that gave no figure (no home-usable within
# the cap; no snapshot for home-cover). The median of an even count is the
# mean of the middle two; the p90 is the nearest rank, the ceil(0.9 n)-th
# smallest. With CLAX_PERF_JSON set, one
# line per figure is appended to that file for scripts/perf-log.sh
# ({"name", "value", "budget", "unit"}), each p90 beside its budget in
# web/perf/app-budget.json (`<platform>.app_home_usable_ms.budget`, and
# app_home_cover_ms; CLAX_PERF_BUDGETS names another file), and one line
# per timing with its `missing` count (unit "launches"). --record writes a
# platform's missing budgets there, the p90 rounded up to the next 50 ms,
# and refuses (writing nothing, exiting 1) when any launch is missing.
#
# On macOS the run stops before the first launch while the screen is
# locked (ioreg's CGSSessionScreenIsLocked), and on either platform after
# a dropped launch that gives no home-usable within the cap: a lock or a
# keychain prompt does not clear by itself. A launch after it without one
# is counted as missing.
#
# CLAX_PERF_APP_TIMINGS=<timings.jsonl> skips the launches and summarizes
# that file's lines instead, dropping none (for the test, and for a file
# copied off a device by hand).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

usage() {
    sed -n '2,3p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

target=macos
runs=""
app=""
record=""
act=""
commit=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --macos) target=macos; shift ;;
        --android) target=android; shift ;;
        --runs) runs="${2:?--runs needs a count}"; shift 2 ;;
        --app) app="${2:?--app needs a path}"; shift 2 ;;
        --commit) commit="${2:?--commit needs a label}"; shift 2 ;;
        --record) record=1; shift ;;
        --i-know-what-i-am-doing) act=1; shift ;;
        *) usage ;;
    esac
done
if [ -z "$runs" ]; then
    if [ "${CLAX_PERF_QUICK:-}" = 1 ]; then runs=5; else runs=9; fi
fi
case "$runs" in '' | *[!0-9]* | 0) echo "perf-app-launch: --runs takes a count of at least 1" >&2; exit 2 ;; esac

pkg=dev.empathic.clax
# The wait for each launch's home-usable, and for the dropped launch's
# snapshot (CLAX_PERF_CAP: another, in seconds, for the test).
cap="${CLAX_PERF_CAP:-60}"
# The app's launch budgets, kept apart from web/perf/budget.json: the web
# benchmark (web/perf/usable.perf.ts) takes any entry of its file under its
# platform's key as a whole web budget.
budgets="${CLAX_PERF_BUDGETS:-web/perf/app-budget.json}"

# summarize PLATFORM FILE WANT [BUILD]: the JSON object for FILE's
# home-usable and home-cover lines (WANT launches were asked for; 0: as
# many as there are; BUILD: a JSON object naming the build, which gains
# "commit"), then the perf-log lines and, with --record, the budgets.
summarize() {
    python3 -I - "$1" "$2" "$3" "$budgets" "${CLAX_PERF_JSON:-}" "$record" "${4:-}" "$commit" <<'PY'
import json, math, sys
platform, path, want, budgets, sink, record, build, commit = sys.argv[1:9]
want = int(want)
build = {"commit": commit or None, **json.loads(build or "{}")}
lines = []
for raw in open(path, encoding="utf-8"):
    try:
        lines.append(json.loads(raw))
    except ValueError:
        pass
# Each launch adds its home-cover (when it had a snapshot), then its
# home-usable: the launches are split at each home-usable line.
launches, cover = [], None
for r in lines:
    if r.get("name") == "home-cover":
        cover = r["elapsed_ms"]
    elif r.get("name") == "home-usable":
        launches.append((r["elapsed_ms"], cover))
        cover = None
def stats(xs):
    xs = sorted(xs)
    n = len(xs)
    if not n:
        return {"median": None, "p90": None, "runs": 0}
    mid = xs[n // 2] if n % 2 else (xs[n // 2 - 1] + xs[n // 2]) / 2
    return {"median": mid, "p90": xs[math.ceil(0.9 * n) - 1], "runs": n}
measured = {"app_home_usable_ms": [u for u, _ in launches], "app_home_cover_ms": [c for _, c in launches if c is not None]}
out = {"platform": platform, "build": build}
for k, xs in measured.items():
    out[k] = stats(xs)
# Launches that gave no figure: no home-usable within the cap, or a launch
# with no snapshot to cover it.
out["app_home_usable_ms"]["missing"] = max(0, want - len(launches)) if want else 0
out["app_home_cover_ms"]["missing"] = len(launches) - len(measured["app_home_cover_ms"])
try:
    all_budgets = json.load(open(budgets, encoding="utf-8"))
except (OSError, ValueError):
    all_budgets = {}
mine = all_budgets.get(platform, {})
if sink:
    with open(sink, "a", encoding="utf-8") as f:
        for k in measured:
            label = "home usable" if k == "app_home_usable_ms" else "home cover"
            for fig in ("median", "p90"):
                if out[k][fig] is None:
                    continue
                cap = mine.get(k, {}).get("budget") if fig == "p90" else None
                f.write(json.dumps({"name": f"app launch: {platform} {label} {fig}", "value": out[k][fig], "budget": cap, "unit": "ms"}) + "\n")
        for k in measured:
            label = "home usable" if k == "app_home_usable_ms" else "home cover"
            f.write(json.dumps({"name": f"app launch: {platform} {label} missing", "value": out[k]["missing"], "budget": None, "unit": "launches"}) + "\n")
for k in measured:
    cap = mine.get(k, {}).get("budget")
    p90 = out[k]["p90"]
    if cap is not None and p90 is not None and p90 > cap:
        print(f"perf-app-launch: {k} p90 {p90} ms is over its budget of {cap} ms (reported, not gated)", file=sys.stderr)
gaps = [f"{'home usable' if k == 'app_home_usable_ms' else 'home cover'}: {out[k]['missing']}" for k in measured if out[k]["missing"]]
if record and gaps:
    print(f"perf-app-launch: --record records no budget from a run with launches missing ({', '.join(gaps)}); {budgets} is unchanged", file=sys.stderr)
    print(json.dumps(out))
    sys.exit(1)
if record:
    added = []
    for k in measured:
        if k not in mine and out[k]["p90"] is not None:
            mine[k] = {"budget": int(math.ceil(out[k]["p90"] / 50.0) * 50)}
            added.append(k)
    if added:
        all_budgets[platform] = mine
        with open(budgets, "w", encoding="utf-8") as f:
            f.write(json.dumps(all_budgets, indent=2) + "\n")
        print(f"perf-app-launch: recorded {', '.join(added)} for {platform} in {budgets}", file=sys.stderr)
    else:
        print(f"perf-app-launch: {budgets} already has {platform}'s budgets; nothing recorded", file=sys.stderr)
print(json.dumps(out))
PY
}

host_platform() {
    local arch
    arch="$(uname -m)"
    [ "$arch" = x86_64 ] && arch=x64
    echo "$(uname -s | tr '[:upper:]' '[:lower:]')-$arch"
}

if [ -n "${CLAX_PERF_APP_TIMINGS:-}" ]; then
    summarize "${CLAX_PERF_PLATFORM:-$(host_platform)}" "$CLAX_PERF_APP_TIMINGS" 0
    exit 0
fi

# count FILE: the home-usable lines in FILE (0 when it does not exist).
count() {
    local n
    n="$(grep -c '"name":"home-usable"' "$1" 2>/dev/null || true)"
    echo "${n:-0}"
}

# stop_pid PID WHAT: SIGTERM to PID, then wait for it to exit (30 s, then
# SIGKILL).
stop_pid() {
    kill -TERM "$1" 2>/dev/null || return 0
    local _
    for _ in $(seq 60); do
        kill -0 "$1" 2>/dev/null || return 0
        sleep 0.5
    done
    echo "perf-app-launch: $2 $1 did not exit within 30 s of SIGTERM; killing it" >&2
    kill -KILL "$1" 2>/dev/null || true
}

if [ "$target" = macos ]; then
    perf_id="$pkg.perf"
    work="${CLAX_PERF_WORK:-target/perf}"
    container="${CLAX_PERF_CONTAINER:-$HOME/Library/Containers/$perf_id}"
    if [ -z "$act" ]; then
        echo "perf-app-launch: would run, in $(pwd):"
        if [ -n "$app" ]; then
            echo "  take the app $app"
        else
            echo "  ./scripts/run.sh app-macos  (builds //apps/clax_app:macos_app)"
        fi
        echo "  copy it once per build into $work/clax-perf-<sha> (a zip unpacked, an app copied with ditto), set the copy's CFBundleIdentifier to $perf_id (plutil) and sign it again ad hoc: codesign --force --sign - --preserve-metadata=entitlements,flags,runtime <copy>"
        echo "  rm -r $container  (the copy's own container, if an earlier run left it)"
        echo "  $((runs + 1)) times: <copy>/Contents/MacOS/Clax, wait for a new home-usable line in $container/Data/Library/Application Support/$perf_id/timings.jsonl (at most ${cap} s); after the first, wait for its home snapshot, Data/Library/Caches/$perf_id/home/home-snapshot.bin (at most ${cap} s); SIGTERM the app and its daemon by PID and wait for them to exit"
        echo "  drop the first launch; print the median and p90 of home-usable and home-cover${CLAX_PERF_JSON:+ and append them to $CLAX_PERF_JSON}"
        [ -n "$record" ] && echo "  record $(host_platform)'s missing budgets in $budgets"
        echo "  rm -r $container"
        echo "run again with --i-know-what-i-am-doing to do it"
        exit 0
    fi
    [ "$(uname -s)" = Darwin ] || { echo "perf-app-launch: --macos runs on a Mac" >&2; exit 1; }
    # No launch reaches its home screen while the screen is locked.
    if ioreg -n Root -d1 | grep -qF '"CGSSessionScreenIsLocked"=Yes'; then
        echo "perf-app-launch: the screen is locked (CGSSessionScreenIsLocked), and no launch reaches its home screen while it is; unlock it and run again" >&2
        exit 1
    fi
    # Only the copy's own container is ever removed.
    [ "$(basename "$container")" = "$perf_id" ] ||
        { echo "perf-app-launch: CLAX_PERF_CONTAINER must be the copy's container, named $perf_id: $container" >&2; exit 1; }
    case "$work" in /*) ;; *) work="$PWD/$work" ;; esac
    if [ -z "$app" ]; then
        app="$(./scripts/run.sh app-macos | tail -1)"
        if [ -z "$commit" ]; then
            commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
            if [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then commit="$commit+dirty"; fi
        fi
    fi
    [ -n "$commit" ] || echo "perf-app-launch: no --commit: the JSON names the app and its digest, not its commit" >&2
    case "$app" in
        *.zip)
            [ -f "$app" ] || { echo "perf-app-launch: no app at $app; pass --app" >&2; exit 1; }
            sha="$(shasum -a 256 "$app" | cut -d' ' -f1)"
            ;;
        *)
            [ -f "$app/Contents/Info.plist" ] || { echo "perf-app-launch: no Contents/Info.plist in $app; pass --app" >&2; exit 1; }
            # A digest of every file in the bundle, by path.
            sha="$(cd "$app" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1)"
            ;;
    esac
    copy="$work/clax-perf-$(printf %s "$sha" | cut -c1-12)"
    if [ ! -d "$copy" ]; then
        # Made aside and moved into place whole: a copy under its name is
        # always re-identified, and no binary is ever written over.
        rm -rf "$copy.new"
        mkdir -p "$copy.new"
        case "$app" in
            *.zip) unzip -q "$app" -d "$copy.new" ;;
            *) ditto "$app" "$copy.new/$(basename "$app")" ;;
        esac
        b="$(ls -d "$copy.new"/*.app | sed -n 1p)"
        plutil -replace CFBundleIdentifier -string "$perf_id" "$b/Contents/Info.plist"
        codesign --force --sign - --preserve-metadata=entitlements,flags,runtime "$b"
        codesign --verify --strict "$b"
        mv "$copy.new" "$copy"
    fi
    bundle="$(ls -d "$copy"/*.app | sed -n 1p)"
    [ "$(plutil -extract CFBundleIdentifier raw -o - "$bundle/Contents/Info.plist" 2>/dev/null)" = "$perf_id" ] ||
        { echo "perf-app-launch: $bundle is not identified as $perf_id; remove $copy and run again" >&2; exit 1; }
    exe="$bundle/Contents/MacOS/Clax"
    [ -x "$exe" ] || { echo "perf-app-launch: no Contents/MacOS/Clax in $bundle" >&2; exit 1; }
    support="$container/Data/Library/Application Support/$perf_id"
    timings="$support/timings.jsonl"
    snapshot="$container/Data/Library/Caches/$perf_id/home/home-snapshot.bin"
    logs="$copy.logs"
    mkdir -p "$logs"
    if [ -e "$container" ]; then
        echo "perf-app-launch: removing $container, left by an earlier run" >&2
        rm -rf "$container"
    fi
    pid=""
    cleanup() {
        [ -z "$pid" ] || stop_pid "$pid" "the app"
        rm -rf "$container"
    }
    trap cleanup EXIT
    for i in $(seq 0 "$runs"); do
        before="$(count "$timings")"
        env -u CLAX_APP_DIR "$exe" >"$logs/launch-$i.log" 2>&1 &
        pid=$!
        got=""
        for _ in $(seq $((cap * 4))); do
            if [ "$(count "$timings")" -gt "$before" ]; then got=1; break; fi
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.25
        done
        if [ -z "$got" ] && [ "$i" = 0 ]; then
            # A keychain prompt or a lock does not clear by itself: the
            # launches after it would each wait out the cap too.
            echo "perf-app-launch: the dropped launch gave no home-usable within ${cap} s (a keychain prompt waiting? the screen locked? see $logs/launch-0.log); nothing is measured" >&2
            exit 1
        elif [ -z "$got" ]; then
            echo "perf-app-launch: launch $i: no home-usable within ${cap} s (a keychain prompt waiting? the screen locked?); counted as missing" >&2
        elif [ "$i" = 0 ]; then
            # The home screen keeps its snapshot a few seconds after it is
            # up, never on SIGTERM: without one no launch has a cover.
            kept=""
            for _ in $(seq $((cap * 4))); do
                if [ -e "$snapshot" ]; then kept=1; break; fi
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.25
            done
            if [ -z "$kept" ]; then
                echo "perf-app-launch: the dropped launch kept no home snapshot within ${cap} s ($snapshot); no launch would have its cover, so nothing is measured" >&2
                exit 1
            fi
        fi
        stop_pid "$pid" "the app"
        wait "$pid" 2>/dev/null || true
        pid=""
        # The daemon the app started, if it outlived the app.
        info="$support/clax-home/daemon.json"
        if [ -f "$info" ]; then
            dpid="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("pid", ""))' "$info" 2>/dev/null || true)"
            case "$(ps -p "${dpid:-0}" -o comm= 2>/dev/null)" in
                */Contents/Resources/clax) stop_pid "$dpid" "the daemon" ;;
            esac
        fi
        if [ "$i" = 0 ]; then
            # The dropped launch's lines are cut from what is summarized.
            first_lines=0
            [ -f "$timings" ] && first_lines="$(wc -l <"$timings" | tr -d ' ')"
        fi
    done
    measured="$logs/measured.jsonl"
    tail -n "+$((${first_lines:-0} + 1))" "$timings" >"$measured" 2>/dev/null || : >"$measured"
    plist="$bundle/Contents/Info.plist"
    version="$(plutil -extract CFBundleShortVersionString raw -o - "$plist" 2>/dev/null || echo "?") ($(plutil -extract CFBundleVersion raw -o - "$plist" 2>/dev/null || echo "?"))"
    build="$(python3 -I -c 'import json,sys; print(json.dumps({"app": sys.argv[1], "sha256": sys.argv[2], "version": sys.argv[3]}))' "$app" "$sha" "$version")"
    summarize "$(host_platform)" "$measured" "$runs" "$build"
    exit 0
fi

# --android
adb="${ADB:-}"
if [ -z "$adb" ]; then adb="$(command -v adb || true)"; fi
if [ -z "$act" ]; then
    echo "perf-app-launch: would run, on the plugged-in device${ANDROID_SERIAL:+ $ANDROID_SERIAL}:"
    echo "  adb shell run-as $pkg ls files/timings.jsonl  (where the app keeps its timings)"
    echo "  $((runs + 1)) times: adb shell am force-stop $pkg; adb shell am start -W -n $pkg/dev.empathic.clax_app.MainActivity; wait for a new home-usable line in adb exec-out run-as $pkg cat files/timings.jsonl (at most ${cap} s); after the first, wait for adb shell run-as $pkg ls cache/home/home-snapshot.bin (at most ${cap} s)"
    echo "  adb shell am force-stop $pkg"
    echo "  drop the first launch; print the median and p90 of home-usable and home-cover${CLAX_PERF_JSON:+ and append them to $CLAX_PERF_JSON}"
    [ -n "$record" ] && echo "  record the device's missing budgets in $budgets"
    echo "run again with --i-know-what-i-am-doing to do it"
    exit 0
fi
if [ -z "$adb" ]; then
    # Bazel's pinned platform tools (as scripts/app-phone.sh takes them).
    bazelisk build --lockfile_mode=error @androidsdk//:platform-tools/adb >/dev/null 2>&1 ||
        { echo "perf-app-launch: no adb on PATH, and Bazel's would not build" >&2; exit 1; }
    adb="$(bazelisk info --lockfile_mode=error execution_root 2>/dev/null)/$(bazelisk cquery --lockfile_mode=error --output=files @androidsdk//:platform-tools/adb 2>/dev/null | sed -n 1p)"
fi
"$adb" get-state >/dev/null 2>&1 || { echo "perf-app-launch: no device attached (ANDROID_SERIAL picks one of several)" >&2; exit 1; }
"$adb" shell pm path "$pkg" >/dev/null 2>&1 || { echo "perf-app-launch: $pkg is not installed (just app-phone)" >&2; exit 1; }
model="$("$adb" shell getprop ro.product.model | tr -d '\r' | tr -c 'A-Za-z0-9\n' '-')"
# The installed build: its versionName and versionCode.
dump="$("$adb" shell dumpsys package "$pkg" | tr -d '\r')"
version="$(sed -n 's/.*versionName=\([^ ]*\).*/\1/p' <<<"$dump" | sed -n 1p) ($(sed -n 's/.*versionCode=\([0-9]*\).*/\1/p' <<<"$dump" | sed -n 1p))"
[ -n "$commit" ] || echo "perf-app-launch: no --commit: the JSON names the installed version, not its commit" >&2
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pull() { "$adb" exec-out run-as "$pkg" cat files/timings.jsonl >"$work/timings.jsonl" 2>/dev/null || : >"$work/timings.jsonl"; }
pull
start_lines="$(wc -l <"$work/timings.jsonl" | tr -d ' ')"
for i in $(seq 0 "$runs"); do
    pull
    before="$(count "$work/timings.jsonl")"
    "$adb" shell am force-stop "$pkg"
    "$adb" shell am start -W -n "$pkg/dev.empathic.clax_app.MainActivity" >/dev/null
    got=""
    for _ in $(seq $((cap * 2))); do
        pull
        if [ "$(count "$work/timings.jsonl")" -gt "$before" ]; then got=1; break; fi
        sleep 0.5
    done
    if [ -z "$got" ] && [ "$i" = 0 ]; then
        echo "perf-app-launch: the dropped launch gave no home-usable within ${cap} s (is the phone locked?); nothing is measured" >&2
        "$adb" shell am force-stop "$pkg"
        exit 1
    fi
    [ -n "$got" ] || echo "perf-app-launch: launch $i: no home-usable within ${cap} s; counted as missing" >&2
    if [ -n "$got" ] && [ "$i" = 0 ]; then
        # As on macOS: the dropped launch keeps the snapshot the others
        # are covered by, a few seconds after its home screen is up.
        kept=""
        for _ in $(seq $((cap * 2))); do
            if "$adb" shell run-as "$pkg" ls cache/home/home-snapshot.bin >/dev/null 2>&1; then kept=1; break; fi
            sleep 0.5
        done
        if [ -z "$kept" ]; then
            echo "perf-app-launch: the dropped launch kept no home snapshot within ${cap} s (cache/home/home-snapshot.bin); no launch would have its cover, so nothing is measured" >&2
            "$adb" shell am force-stop "$pkg"
            exit 1
        fi
    fi
    if [ "$i" = 0 ]; then
        pull
        start_lines="$(wc -l <"$work/timings.jsonl" | tr -d ' ')"
    fi
done
"$adb" shell am force-stop "$pkg"
pull
tail -n "+$((start_lines + 1))" "$work/timings.jsonl" >"$work/measured.jsonl"
summarize "android-$model" "$work/measured.jsonl" "$runs" "$(python3 -I -c 'import json,sys; print(json.dumps({"version": sys.argv[1]}))' "$version")"
