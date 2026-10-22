#!/usr/bin/env bash
# app-phone.sh [--i-know-what-i-am-doing]: builds the Clax app's release APK
# from this checkout's committed HEAD and installs it on the plugged-in
# Android phone, keeping the app's data, then opens the app and checks that
# its core opens (a `clax-core: ready pid=<the app's PID>` line in logcat,
# lib/core/core_host.dart) and the app keeps that PID. Without the flag it
# prints what it would do and changes nothing.
#
# With Bazel (scripts/lib/build-system.sh's choice; CLAX_BUILD=native for
# the native build) the APK is //apps/clax_app:apk and adb the SDK's that
# Bazel fetches. HEAD is built in place when the checkout is HEAD exactly
# (no changes, no untracked files),
# else from a clean copy of HEAD at target/app-phone/tree, one directory
# reused for every HEAD so its Bazel output base and the shared cache keep
# the work of the last build.
#
# Natively the build runs in a clean copy of HEAD under target/app-phone/<sha>
# with Flutter and Gradle (`just app-apk-native`); a HEAD already built is
# installed without rebuilding, and the newest three copies are kept.
# Cargo's output is shared by every copy (target/app-phone-cargo).
#
# Either way the APK carries the home app's bundle, packed and signed from
# HEAD's apps/clax_app/home by the same build (//apps/clax_app:home_bundle;
# natively scripts/build-home-bundle.sh), at the version this machine's
# home release gives those sources (apps/clax_app/tool/home-version.sh): a
# higher one than any earlier build's when they changed, so the phone's
# home app updates. Nothing is packed or committed by hand.
#
# Either way, uncommitted changes are left out (it says so) and while it
# builds it prints the step it is on, and the time every 30 s. A phone need
# not be attached until the build is done: it then waits for one.
#
# Environment:
#   ANDROID_SERIAL  which device, when more than one is attached
#   APP_PHONE_WAIT  seconds to wait for a phone after the build (default 900)
#   APP_PHONE_READY_WAIT  seconds to wait for the core to say it opened (default 300)
#   JAVA_HOME       natively, a JDK 17 (default: the newest ~/Library/Java/jdk-17*)
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"

act=""
case "${1:-}" in
    --i-know-what-i-am-doing) act=1 ;;
    "") ;;
    *) echo "usage: $0 [--i-know-what-i-am-doing]" >&2; exit 2 ;;
esac

# shellcheck source=scripts/lib/build-system.sh
source scripts/lib/build-system.sh
if [ "${CLAX_BUILD:-auto}" = native ]; then
    mode=native
elif build_system_choose rust >/dev/null 2>&1 && [ "$BUILD_SYSTEM" = bazel ]; then
    mode=bazel
else
    mode=native
fi

sha="$(git rev-parse --short=8 HEAD)"
pkg=dev.empathic.clax
# Tracked changes, and untracked files a glob could take into the build.
dirty=""
if [ -n "$(git status --porcelain)" ]; then
    dirty=1
    echo "app-phone: uncommitted changes are not in this build (HEAD $sha)"
fi

# The adb to use: Bazel's (the pinned platform tools), or the one on PATH.
if [ "$mode" = bazel ]; then
    "$BAZEL" build --lockfile_mode=error @androidsdk//:platform-tools/adb >/dev/null 2>&1 ||
        { echo "app-phone: could not build Bazel's adb (bazelisk build @androidsdk//:platform-tools/adb)" >&2; exit 1; }
    # A file of an external repository: under the execution root.
    adb="$("$BAZEL" info --lockfile_mode=error execution_root 2>/dev/null)/$("$BAZEL" cquery --lockfile_mode=error --output=files @androidsdk//:platform-tools/adb 2>/dev/null | sed -n 1p)"
    for tool in git; do
        command -v "$tool" >/dev/null || { echo "app-phone: $tool is not on PATH" >&2; exit 1; }
    done
else
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
    [ -d web/node_modules ] || { echo "app-phone: web/node_modules is missing; run: cd web && pnpm install --frozen-lockfile" >&2; exit 1; }
    adb="$(command -v adb)"
fi

# The attached phones, and those waiting for USB debugging to be allowed.
attached() { "$adb" devices | awk 'NR > 1 && $2 == "device" { print $1 }'; }
unauthorized() { "$adb" devices | awk 'NR > 1 && $2 == "unauthorized" { print $1 }'; }
# pick: sets ANDROID_SERIAL to the one attached phone; 1 when there is none,
# 2 (after naming them) when there is more than one.
pick() {
    [ -n "${ANDROID_SERIAL:-}" ] && return 0
    local devices; devices="$(attached)"
    case "$(printf '%s\n' "$devices" | grep -c .)" in
        0) return 1 ;;
        1) export ANDROID_SERIAL="$devices" ;;
        *) echo "app-phone: more than one device; set ANDROID_SERIAL to one of:" >&2
           printf '%s\n' "$devices" | sed 's/^/  /' >&2; return 2 ;;
    esac
}
# A phone need not be attached until the build is done (APP_PHONE_WAIT
# seconds, 900 by default, are allowed for it then); two or more stop now.
picked=0; pick || picked=$?
[ "$picked" = 2 ] && exit 1
if [ "$picked" = 1 ]; then
    echo "app-phone: no phone attached yet; it is needed once the build is done"
fi

if [ "$mode" = bazel ]; then
    if [ -n "$dirty" ]; then where="target/app-phone/tree"; else where="."; fi
    log="$root/target/app-phone/$sha.log"
else
    where="target/app-phone/$sha"
    log="$root/target/app-phone/$sha.log"
    apk="$where/apps/clax_app/build/app/outputs/flutter-apk/app-release.apk"
fi

# home_release: the home bundle HEAD's build would carry, from HEAD's home
# sources, without recording anything.
home_release() {
    local t out version
    t="$(mktemp -d)"
    git archive HEAD apps/clax_app/home apps/clax_app/assets/kit apps/clax_app/tool | tar -x -C "$t"
    out="$(apps/clax_app/tool/home-version.sh "$t/apps/clax_app" peek 2>&1)" || { rm -rf "$t"; echo "no home bundle: $out"; return; }
    version="$(sed -n 's/^version=//p' <<<"$out")"
    series="$(sed -n 's/^ *"release": *"\([^"]*\)".*/\1/p' "$t/apps/clax_app/home/manifest.json")"
    rm -rf "$t"
    if [ "$(sed -n 's/^mode=//p' <<<"$out")" = dev ]; then
        echo "v$version (release $series.$version), signed by the development key: no release key on this machine"
    else
        echo "v$version (release $series.$version), signed by the home app's release key"
    fi
}

if [ -z "$act" ]; then
    if [ "$mode" = bazel ]; then
        if [ "$where" = . ]; then echo "would build HEAD $sha here with Bazel (//apps/clax_app:apk; log: target/app-phone/$sha.log)"
        else echo "would build HEAD $sha with Bazel (//apps/clax_app:apk) in a clean copy at $where (log: target/app-phone/$sha.log)"; fi
    elif [ -f "$apk" ]; then echo "would install the APK already built for $sha"
    else echo "would build HEAD $sha in $where with Gradle (log: target/app-phone/$sha.log)"; fi
    [ "$mode" = native ] && [ -f "$apk" ] ||
        echo "that build packs HEAD's home app (apps/clax_app/home) into the APK's home bundle: $(home_release)"
    echo "would install it on ${ANDROID_SERIAL:-the phone attached by then} with $adb, keeping its data, open Clax and check its core opens and the app stays up"
    echo "run again with --i-know-what-i-am-doing (or: just app-phone) to do it"
    exit 0
fi

# watch PID: prints the step the build is on (its "build-app-clax: …" and
# "app-apk: …" lines natively, Bazel's progress lines with Bazel) when it
# changes and otherwise every 30 s, until PID exits; returns its status.
watch() {
    local build="$1" start=$SECONDS shown="" told=0 e step
    trap 'kill "$build" 2>/dev/null' INT TERM
    while kill -0 "$build" 2>/dev/null; do
        sleep 1
        e=$((SECONDS - start))
        if [ "$mode" = bazel ]; then
            step="$(grep -E '^\[[0-9,]+ / [0-9,]+\] ' "$log" 2>/dev/null | tail -n 1 | sed -E 's/;.*//' || true)"
        else
            step="$(grep -E '^(build-app-clax|app-apk): ' "$log" 2>/dev/null | tail -n 1 | sed -E 's/^[a-z-]+: //' || true)"
        fi
        if [ -n "$step" ] && [ "$step" != "$shown" ] && { [ "$mode" != bazel ] || [ $((e - told)) -ge 5 ]; }; then
            shown="$step"; told=$e
            printf 'app-phone: %dm%02ds  %s\n' $((e / 60)) $((e % 60)) "$step"
        elif [ $((e - told)) -ge 30 ]; then
            told=$e
            printf 'app-phone: %dm%02ds  still %s\n' $((e / 60)) $((e % 60)) "${shown:-starting}"
        fi
    done
    trap - INT TERM
    if ! wait "$build"; then
        tail -20 "$log" >&2
        echo "app-phone: build failed; the whole log is $log" >&2
        return 1
    fi
    e=$((SECONDS - start))
    printf 'app-phone: built in %dm%02ds\n' $((e / 60)) $((e % 60))
}

mkdir -p target/app-phone
if [ "$mode" = bazel ]; then
    if [ "$where" != . ]; then
        echo "app-phone: copying HEAD $sha to $where"
        rm -rf "$where"
        mkdir -p "$where"
        git archive HEAD | tar -x -C "$where"
    fi
    echo "app-phone: building $sha with Bazel (log: $log)"
    (cd "$where" && "$BAZEL" build --lockfile_mode=error --curses=no --color=no --show_progress_rate_limit=1 //apps/clax_app:apk) > "$log" 2>&1 &
    watch $! || exit 1
    apk="$where/$(cd "$where" && "$BAZEL" cquery --lockfile_mode=error --output=files //apps/clax_app:apk 2>/dev/null | sed -n 1p)"
elif [ ! -f "$apk" ]; then
    echo "app-phone: building $sha from a clean copy (the first build takes several minutes; later ones reuse its Rust output) (log: $log)"
    rm -rf "$where"
    mkdir -p "$where"
    git archive HEAD | tar -x -C "$where"
    ln -s "$root/web/node_modules" "$where/web/node_modules"
    (cd "$where" && CLAX_BUILD=native CARGO_TARGET_DIR="$root/target/app-phone-cargo" just app-apk-native) > "$log" 2>&1 &
    watch $! || exit 1
    # Keep the newest three copies (and their logs).
    ls -1t target/app-phone | { grep -v '\.log$' | grep -vx tree || true; } | tail -n +4 | while read -r old; do
        rm -rf "target/app-phone/$old" "target/app-phone/$old.log"
    done
fi

# Wait for the phone, if it was not attached when the build began.
picked=0; pick || picked=$?
[ "$picked" = 2 ] && exit 1
if [ "$picked" = 1 ]; then
    wait_s="${APP_PHONE_WAIT:-900}"
    echo "app-phone: built; plug in the phone and allow USB debugging (waiting up to $((wait_s / 60)) min)"
    start=$SECONDS told=0 asked=""
    while :; do
        picked=0; pick || picked=$?
        [ "$picked" = 0 ] && break
        [ "$picked" = 2 ] && exit 1
        e=$((SECONDS - start))
        if [ "$e" -ge "$wait_s" ]; then
            echo "app-phone: no phone after $((wait_s / 60)) min; the APK is kept, so run again to install it" >&2
            exit 1
        fi
        if [ -z "$asked" ] && [ -n "$(unauthorized)" ]; then
            asked=1; echo "app-phone: a phone is attached but USB debugging is not allowed yet: tap Allow on the phone"
        fi
        if [ $((e - told)) -ge 60 ]; then
            told=$e; printf 'app-phone: still waiting for a phone (%dm%02ds)\n' $((e / 60)) $((e % 60))
        fi
        sleep 2
    done
fi

echo "app-phone: installing $sha on $ANDROID_SERIAL"
"$adb" install -r "$apk"
"$adb" shell monkey -p "$pkg" -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# The app should open its core and keep one PID; a crash loop shows as a
# new PID. The core says it opened, or failed, in the platform log, naming
# the app's PID (`clax-core: ready pid=<pid>` or `clax-core: failed
# pid=<pid> code=<code>`, lib/core/core_host.dart), read here with
# `adb logcat --pid`: files/core.json says the same, but a release build is
# not debuggable, so `run-as` cannot read it. The first launch after an
# upgrade may migrate and back up the store before the core answers, so the
# wait for it is long (APP_PHONE_READY_WAIT); once ready, the app is
# watched 10 s more for a restart.
ready_wait="${APP_PHONE_READY_WAIT:-300}"
echo "app-phone: opened Clax; waiting for its core to say it is ready (at most ${ready_wait} s)"
first="" ready="" start=$SECONDS
# same_pid: the app runs with PID $first still (it is set on the first).
same_pid() {
    pid="$("$adb" shell pidof "$pkg" | tr -d '\r')"
    [ -n "$pid" ] || return 0
    if [ -z "$first" ]; then first="$pid"
    elif [ "$pid" != "$first" ]; then
        echo "app-phone: Clax restarted ($first, then $pid); see: adb logcat -d | grep -i clax" >&2
        exit 1
    fi
}
while [ $((SECONDS - start)) -lt "$ready_wait" ]; do
    sleep 2
    same_pid
    [ -n "$first" ] || continue
    said="$("$adb" logcat -d --pid="$first" 2>/dev/null | tr -d '\r' || true)"
    if failed="$(grep -E "clax-core: failed pid=$first( |$)" <<<"$said")"; then
        echo "app-phone: the core did not open: ${failed##*clax-core: }" >&2
        exit 1
    fi
    if grep -qE "clax-core: ready pid=$first( |$)" <<<"$said"; then ready=1; break; fi
done
if [ -z "$first" ]; then
    echo "app-phone: Clax did not start; see: adb logcat -d | grep -i clax" >&2
    exit 1
fi
if [ -z "$ready" ]; then
    echo "app-phone: the core did not say it opened within ${ready_wait} s (no 'clax-core: ready pid=$first' in adb logcat --pid=$first); see: adb logcat -d | grep -i clax" >&2
    exit 1
fi
for _ in 1 2 3 4 5; do
    sleep 2
    same_pid
done
echo "app-phone: $sha installed; Clax is open, its core is ready and it stayed up (PID $first)"
