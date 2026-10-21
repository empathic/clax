#!/usr/bin/env bash
# build-app-clax.sh [android|macos|all] [--debug]: builds the embedded core
# (crates/clax-embed, a shared library) for the Clax app and places it where
# the app's build picks it up:
#   macos    apps/clax_app/build-native/macos/libclax_embed.dylib, which the
#            Runner's "Embed core" phase copies into Contents/Frameworks: a
#            release build is universal (arm64 and x86_64, joined with
#            lipo), as a Release app Xcode builds is; --debug builds the
#            host's CPU only, as a Debug app is
#   android  apps/clax_app/android/app/src/main/jniLibs/<abi>/libclax_embed.so
#            for arm64-v8a and x86_64, compiled and linked with the NDK's
#            API 29 clang, its segments aligned to 16 KB (a phone with 16 KB
#            pages loads no library aligned less), and stripped
# The default is all, a release build. A release build embeds web/dist, so
# the web UI is built first. Each library is written beside its destination
# and renamed into place, never copied over a file that may be in use;
# missing destination directories are made.
#
# Environment:
#   ANDROID_NDK_HOME     the NDK (default ~/Library/Android/sdk/ndk/28.2.13676358)
#   CLAX_APP_DRY_RUN=1   print the commands ("+ …") instead of running them
#   CLAX_APP_PLACE_ONLY=1  with arguments SRC DEST: only place SRC at DEST
#   CARGO_TARGET_DIR     cargo's output directory (default target)
#
# Each step starts with a "build-app-clax: …" line, so a caller logging the
# build can show which step it is on.
set -euo pipefail
cd "$(dirname "$0")/.."
say() { echo "build-app-clax: $*"; }
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
out="${CARGO_TARGET_DIR:-target}"
say "building the web UI"
run ./scripts/build-web.sh
if [ "$what" = macos ] || [ "$what" = all ]; then
    if [ "$profile" = release ]; then
        for triple in aarch64-apple-darwin x86_64-apple-darwin; do
            say "building the core for macOS ($triple)"
            run rustup target add "$triple"
            run cargo build -q --release -p clax-embed --target "$triple"
        done
        say "joining the core's two CPUs into one library"
        run mkdir -p "$out/universal-apple-darwin/release"
        run lipo -create -output "$out/universal-apple-darwin/release/libclax_embed.dylib" \
            "$out/aarch64-apple-darwin/release/libclax_embed.dylib" "$out/x86_64-apple-darwin/release/libclax_embed.dylib"
        place "$out/universal-apple-darwin/release/libclax_embed.dylib" apps/clax_app/build-native/macos/libclax_embed.dylib
    else
        say "building the core for macOS"
        run cargo build -q -p clax-embed
        place "$out/debug/libclax_embed.dylib" apps/clax_app/build-native/macos/libclax_embed.dylib
    fi
fi
if [ "$what" = android ] || [ "$what" = all ]; then
    ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
    bin="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
    # The NDK's lld aligns arm64 segments to 4 KB unless told otherwise.
    page_flags="-Clink-arg=-Wl,-z,max-page-size=16384 -Clink-arg=-Wl,-z,common-page-size=16384"
    for t in aarch64-linux-android:arm64-v8a x86_64-linux-android:x86_64; do
        triple="${t%%:*}"; abi="${t##*:}"
        var="CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER"
        flags_var="CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"
        # The C sources of rusqlite (bundled SQLite) and ring are compiled by
        # cc-rs, which reads the target's compiler and archiver from these.
        cc_var="CC_$(echo "$triple" | tr '-' '_')"; ar_var="AR_$(echo "$triple" | tr '-' '_')"
        say "building the core for Android $abi ($triple)"
        run rustup target add "$triple"
        run env "$var=$bin/${triple}29-clang" "$flags_var=$page_flags" "$cc_var=$bin/${triple}29-clang" "$ar_var=$bin/llvm-ar" cargo build -q ${flag[@]+"${flag[@]}"} -p clax-embed --target "$triple"
        run "$bin/llvm-strip" -o "$out/$triple/$profile/libclax_embed.stripped.so" "$out/$triple/$profile/libclax_embed.so"
        place "$out/$triple/$profile/libclax_embed.stripped.so" "apps/clax_app/android/app/src/main/jniLibs/$abi/libclax_embed.so"
    done
fi
