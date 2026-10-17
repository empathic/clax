#!/usr/bin/env bash
# app-phone.sh [--i-know-what-i-am-doing]: builds the Clax app's release APK
# from this checkout's committed HEAD and installs it on the plugged-in
# Android phone, keeping the app's data, then opens the app and checks that
# its daemon (libclax.so) stays up. Without the flag it prints what it would
# do and changes nothing.
#
# The build runs in a clean copy of HEAD under target/app-phone/<sha>, so
# uncommitted changes are left out (it says so) and builds in apps/clax_app
# are not disturbed. A HEAD already built is installed without rebuilding;
# the newest three copies are kept. Cargo's output is shared by every copy
# (target/app-phone-cargo), so a new HEAD recompiles only what changed.
# While it builds it prints the step it is on, and the time every 30 s.
#
# Environment:
#   ANDROID_SERIAL  which device, when more than one is attached
#   JAVA_HOME       a JDK 17 (default: the newest ~/Library/Java/jdk-17*)
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"

act=""
case "${1:-}" in
    --i-know-what-i-am-doing) act=1 ;;
    "") ;;
    *) echo "usage: $0 [--i-know-what-i-am-doing]" >&2; exit 2 ;;
esac

# Tools: rustup's cargo stays ahead of Homebrew's; flutter comes from Homebrew.
export PATH="$PATH:$HOME/.local/homebrew/bin:$HOME/Library/Android/sdk/platform-tools"
if [ -z "${JAVA_HOME:-}" ]; then
    jdk="$(ls -d "$HOME"/Library/Java/jdk-17* 2>/dev/null | sort | tail -1 || true)"
    [ -n "$jdk" ] && export JAVA_HOME="${jdk%/}"
fi
for tool in git just flutter cargo adb; do
    command -v "$tool" >/dev/null || { echo "app-phone: $tool is not on PATH" >&2; exit 1; }
done
[ -n "${JAVA_HOME:-}" ] || { echo "app-phone: no JDK 17; set JAVA_HOME" >&2; exit 1; }
[ -d web/node_modules ] || { echo "app-phone: web/node_modules is missing; run: cd web && npm ci" >&2; exit 1; }

sha="$(git rev-parse --short=8 HEAD)"
copy="target/app-phone/$sha"
apk="$copy/apps/clax_app/build/app/outputs/flutter-apk/app-release.apk"
pkg=dev.empathic.clax

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "app-phone: uncommitted changes are not in this build (HEAD $sha)"
fi

devices="$(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }')"
if [ -z "${ANDROID_SERIAL:-}" ]; then
    case "$(printf '%s\n' "$devices" | grep -c .)" in
        0) echo "app-phone: no phone attached (adb devices lists none); plug it in and allow USB debugging" >&2
           [ -n "$act" ] && exit 1 ;;
        1) export ANDROID_SERIAL="$devices" ;;
        *) echo "app-phone: more than one device; set ANDROID_SERIAL to one of:" >&2
           printf '%s\n' "$devices" | sed 's/^/  /' >&2; exit 1 ;;
    esac
fi

if [ -z "$act" ]; then
    if [ -f "$apk" ]; then echo "would install the APK already built for $sha"
    else echo "would build HEAD $sha in $copy (log: target/app-phone/$sha.log)"; fi
    echo "would install it on ${ANDROID_SERIAL:-the attached phone}, keeping its data, open Clax and check its daemon stays up"
    echo "run again with --i-know-what-i-am-doing (or: just app-phone) to do it"
    exit 0
fi

if [ ! -f "$apk" ]; then
    log="$root/target/app-phone/$sha.log"
    echo "app-phone: building $sha from a clean copy (the first build takes several minutes; later ones reuse its Rust output) (log: $log)"
    rm -rf "$copy"
    mkdir -p "$copy"
    git archive HEAD | tar -x -C "$copy"
    ln -s "$root/web/node_modules" "$copy/web/node_modules"
    (cd "$copy" && CARGO_TARGET_DIR="$root/target/app-phone-cargo" just app-apk) > "$log" 2>&1 &
    build=$!
    trap 'kill "$build" 2>/dev/null' INT TERM
    # The step the build is on (its "build-app-clax: …" and "app-apk: …"
    # lines), printed when it changes and otherwise every 30 s.
    start=$SECONDS
    shown=""
    told=0
    while kill -0 "$build" 2>/dev/null; do
        sleep 1
        e=$((SECONDS - start))
        step="$(grep -E '^(build-app-clax|app-apk): ' "$log" 2>/dev/null | tail -n 1 | sed -E 's/^[a-z-]+: //' || true)"
        if [ -n "$step" ] && [ "$step" != "$shown" ]; then
            shown="$step"; told=$e
            printf 'app-phone: %dm%02ds  %s\n' $((e / 60)) $((e % 60)) "$step"
        elif [ $((e - told)) -ge 30 ]; then
            told=$e
            printf 'app-phone: %dm%02ds  still %s\n' $((e / 60)) $((e % 60)) "${shown:-starting}"
        fi
    done
    if ! wait "$build"; then
        trap - INT TERM
        tail -20 "$log" >&2
        echo "app-phone: build failed; the whole log is $log" >&2
        exit 1
    fi
    trap - INT TERM
    e=$((SECONDS - start))
    printf 'app-phone: built in %dm%02ds\n' $((e / 60)) $((e % 60))
    # Keep the newest three copies (and their logs).
    ls -1t target/app-phone | { grep -v '\.log$' || true; } | tail -n +4 | while read -r old; do
        rm -rf "target/app-phone/$old" "target/app-phone/$old.log"
    done
fi

echo "app-phone: installing $sha on $ANDROID_SERIAL"
adb install -r "$apk"
adb shell monkey -p "$pkg" -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# The daemon should come up and keep one PID; a crash loop shows as a new PID.
echo "app-phone: opened Clax; watching its daemon for 20 s"
first=""
for _ in $(seq 1 10); do
    sleep 2
    pid="$(adb shell ps -A -o PID,NAME | awk '$2 == "libclax.so" { print $1 }' | tr -d '\r')"
    if [ -z "$pid" ]; then continue; fi
    if [ -z "$first" ]; then first="$pid"
    elif [ "$pid" != "$first" ]; then
        echo "app-phone: the daemon restarted ($first, then $pid); see: adb logcat -d | grep -i clax" >&2
        exit 1
    fi
done
if [ -z "$first" ]; then
    echo "app-phone: the daemon did not start; see: adb logcat -d | grep -i clax" >&2
    exit 1
fi
echo "app-phone: $sha installed; Clax is open and its daemon (PID $first) stayed up"
