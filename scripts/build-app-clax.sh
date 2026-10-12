#!/usr/bin/env bash
# build-app-clax.sh [android|macos|all] [--debug]: builds clax for the Clax
# app and places it where the app's build picks it up:
#   macos    apps/clax_app/build-native/macos/clax (the host target)
#   android  apps/clax_app/android/app/src/main/jniLibs/<abi>/libclax.so for
#            arm64-v8a and x86_64, linked with the NDK's API 29 clang and
#            stripped
# The default is all, a release build. A release build embeds web/dist, so
# the web UI is built first. Each binary is written beside its destination
# and renamed into place, never copied over a binary that may be running;
# missing destination directories are made.
#
# Environment:
#   ANDROID_NDK_HOME     the NDK (default ~/Library/Android/sdk/ndk/28.2.13676358)
#   CLAX_APP_DRY_RUN=1   print the commands ("+ …") instead of running them
#   CLAX_APP_PLACE_ONLY=1  with arguments SRC DEST: only place SRC at DEST
set -euo pipefail
cd "$(dirname "$0")/.."
run() { if [ "${CLAX_APP_DRY_RUN:-}" = 1 ]; then echo "+ $*"; else "$@"; fi; }
place() { # place SRC DEST
    local dir; dir="$(dirname "$2")"; run mkdir -p "$dir"
    if [ "${CLAX_APP_DRY_RUN:-}" = 1 ]; then echo "+ place $1 -> $2"; return; fi
    local tmp; tmp="$(mktemp "$dir/.clax.XXXXXX")"
    if ! { cat "$1" > "$tmp" && chmod 755 "$tmp" && mv -f "$tmp" "$2"; }; then
        rm -f "$tmp"
        return 1
    fi
}
if [ "${CLAX_APP_PLACE_ONLY:-}" = 1 ]; then
    [ "$#" = 2 ] || { echo "usage: CLAX_APP_PLACE_ONLY=1 $0 SRC DEST" >&2; exit 2; }
    place "$1" "$2"
    exit
fi
usage() { echo "usage: $0 [android|macos|all] [--debug]" >&2; exit 2; }
what="${1:-all}"
case "$what" in android|macos|all) ;; *) usage ;; esac
profile=release
case "${2:-}" in "") ;; --debug) profile=debug ;; *) usage ;; esac
[ "$#" -le 2 ] || usage
# Bash 3.2 (macOS) treats an empty array as unset under `set -u`, so the
# flag is expanded only when present.
flag=(); [ "$profile" = release ] && flag=(--release)
run ./scripts/build-web.sh
if [ "$what" = macos ] || [ "$what" = all ]; then
    run cargo build -q ${flag[@]+"${flag[@]}"} -p clax-cli
    place "target/$profile/clax" apps/clax_app/build-native/macos/clax
fi
if [ "$what" = android ] || [ "$what" = all ]; then
    ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
    bin="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
    for t in aarch64-linux-android:arm64-v8a x86_64-linux-android:x86_64; do
        triple="${t%%:*}"; abi="${t##*:}"
        var="CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER"
        run rustup target add "$triple"
        run env "$var=$bin/${triple}29-clang" cargo build -q ${flag[@]+"${flag[@]}"} -p clax-cli --target "$triple"
        run "$bin/llvm-strip" -o "target/$triple/$profile/clax.stripped" "target/$triple/$profile/clax"
        place "target/$triple/$profile/clax.stripped" "apps/clax_app/android/app/src/main/jniLibs/$abi/libclax.so"
    done
fi
