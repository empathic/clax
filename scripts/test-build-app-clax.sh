#!/usr/bin/env bash
# Checks scripts/build-app-clax.sh without building: the commands it prints
# under CLAX_APP_DRY_RUN=1 for each platform and profile, and its place step
# (CLAX_APP_PLACE_ONLY=1), which must rename a new file into place so a
# process holding the old binary open keeps reading the old one. The script
# runs through bash, so a load on macOS executable assessment does not stall it.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
# has OUT LINE / lacks OUT LINE: whether OUT has LINE as a whole line.
has() { grep -qxF -- "$2" <<<"$1"; }
lacks() { ! grep -qF -- "$2" <<<"$1"; }
dry() { CLAX_APP_DRY_RUN=1 ANDROID_NDK_HOME=/ndk bash scripts/build-app-clax.sh "$@"; }

bin=/ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin
mac_place="+ place target/release/libclax_embed.dylib -> apps/clax_app/build-native/macos/libclax_embed.dylib"
arm_place="+ place target/aarch64-linux-android/release/libclax_embed.stripped.so -> apps/clax_app/android/app/src/main/jniLibs/arm64-v8a/libclax_embed.so"
x86_place="+ place target/x86_64-linux-android/release/libclax_embed.stripped.so -> apps/clax_app/android/app/src/main/jniLibs/x86_64/libclax_embed.so"
pages="-Clink-arg=-Wl,-z,max-page-size=16384 -Clink-arg=-Wl,-z,common-page-size=16384"
arm_build="+ env CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$bin/aarch64-linux-android29-clang CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS=$pages CC_aarch64_linux_android=$bin/aarch64-linux-android29-clang AR_aarch64_linux_android=$bin/llvm-ar cargo build -q --release -p clax-embed --target aarch64-linux-android"
x86_build="+ env CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=$bin/x86_64-linux-android29-clang CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS=$pages CC_x86_64_linux_android=$bin/x86_64-linux-android29-clang AR_x86_64_linux_android=$bin/llvm-ar cargo build -q --release -p clax-embed --target x86_64-linux-android"

out="$(dry macos)"
if has "$out" "+ ./scripts/build-web.sh" && has "$out" "+ cargo build -q --release -p clax-embed" \
    && has "$out" "$mac_place" && lacks "$out" "linux-android"; then
    ok "macos: web UI, release host build, placed under build-native"
else
    bad "macos: $out"
fi

out="$(dry android)"
if has "$out" "+ ./scripts/build-web.sh" && has "$out" "$arm_build" && has "$out" "$x86_build" \
    && has "$out" "+ $bin/llvm-strip -o target/aarch64-linux-android/release/libclax_embed.stripped.so target/aarch64-linux-android/release/libclax_embed.so" \
    && has "$out" "+ $bin/llvm-strip -o target/x86_64-linux-android/release/libclax_embed.stripped.so target/x86_64-linux-android/release/libclax_embed.so" \
    && has "$out" "$arm_place" && has "$out" "$x86_place" && lacks "$out" "build-native"; then
    ok "android: a linker, 16 KB pages, C compiler and archiver per triple, stripped, placed per ABI"
else
    bad "android: $out"
fi

out="$(dry)"
if has "$out" "$mac_place" && has "$out" "$arm_place" && has "$out" "$x86_place" \
    && [ "$(grep -c -xF "+ ./scripts/build-web.sh" <<<"$out")" = 1 ] && [ "$out" = "$(dry all)" ]; then
    ok "all (the default): every platform, the web UI built once"
else
    bad "all: $out"
fi

out="$(dry all --debug)"
if lacks "$out" "--release" && has "$out" "+ cargo build -q -p clax-embed" \
    && has "$out" "+ place target/debug/libclax_embed.dylib -> apps/clax_app/build-native/macos/libclax_embed.dylib" \
    && has "$out" "+ place target/aarch64-linux-android/debug/libclax_embed.stripped.so -> apps/clax_app/android/app/src/main/jniLibs/arm64-v8a/libclax_embed.so"; then
    ok "--debug: no --release, debug output paths"
else
    bad "--debug: $out"
fi

out="$(CLAX_APP_DRY_RUN=1 env -u ANDROID_NDK_HOME bash scripts/build-app-clax.sh android)"
if has "$out" "+ env CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android29-clang CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS=$pages CC_aarch64_linux_android=$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android29-clang AR_aarch64_linux_android=$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-ar cargo build -q --release -p clax-embed --target aarch64-linux-android"; then
    ok "the NDK defaults to 28.2.13676358 under ~/Library/Android/sdk"
else
    bad "default NDK: $out"
fi

out="$(dry all; dry all --debug)"
if lacks "$out" "-p clax-cli" && lacks "$out" "libclax.so" && ! grep -qE 'build-native/macos/clax$' <<<"$out"; then
    ok "no clax executable is built or placed"
else
    bad "the clax executable is still built or placed: $out"
fi
if lacks "$out" "+ cp " && ! grep -qE '(^|[^[:alnum:]_-])cp ' scripts/build-app-clax.sh; then
    ok "libraries are placed, never copied with cp"
else
    bad "cp used"
fi

rc=0
out="$(dry ios 2>&1)" || rc=$?
if [ "$rc" = 2 ] && lacks "$out" "+ "; then
    ok "an unknown platform is refused before anything runs"
else
    bad "unknown platform: rc=$rc $out"
fi
rc=0
out="$(dry macos --fast 2>&1)" || rc=$?
if [ "$rc" = 2 ] && lacks "$out" "+ "; then
    ok "an unknown option is refused before anything runs"
else
    bad "unknown option: rc=$rc $out"
fi

# place: the destination's directory is made when missing, and the new file
# arrives executable, with nothing left beside it.
printf 'old\n' > "$d/old"
place() { CLAX_APP_PLACE_ONLY=1 bash scripts/build-app-clax.sh "$@"; }
if place "$d/old" "$d/a/b/clax" && [ "$(cat "$d/a/b/clax")" = old ] && [ -x "$d/a/b/clax" ] \
    && [ "$(ls -A "$d/a/b")" = clax ]; then
    ok "place makes the destination directory and an executable file"
else
    bad "place into a new directory: $(ls -lA "$d/a/b" 2>&1)"
fi

# place over a file held open: the holder still reads the old content (a
# rename gives the path a new inode; writing over it in place would not).
printf 'new\n' > "$d/new"
exec 3< "$d/a/b/clax"
before="$(ls -i "$d/a/b/clax")"
place "$d/new" "$d/a/b/clax"
held="$(cat <&3)"
exec 3<&-
if [ "$held" = old ] && [ "$(cat "$d/a/b/clax")" = new ] && [ "$before" != "$(ls -i "$d/a/b/clax")" ] \
    && [ "$(ls -A "$d/a/b")" = clax ]; then
    ok "place renames into place: an open holder keeps the old file"
else
    bad "place over an open file: held=$held now=$(cat "$d/a/b/clax") ls=$(ls -A "$d/a/b" | tr '\n' ' ')"
fi

rc=0
place "$d/missing" "$d/a/b/clax" 2>/dev/null || rc=$?
if [ "$rc" != 0 ] && [ "$(cat "$d/a/b/clax")" = new ] && [ "$(ls -A "$d/a/b")" = clax ]; then
    ok "a missing source fails and leaves the destination and no temporary file"
else
    bad "missing source: rc=$rc ls=$(ls -A "$d/a/b" | tr '\n' ' ')"
fi

exit "$fails"
