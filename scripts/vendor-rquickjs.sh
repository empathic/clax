#!/usr/bin/env bash
# vendor-rquickjs.sh [--check]: writes the patched QuickJS bindings that
# clax-bot-js builds on, as two crates of the workspace with names of their
# own (docs/bazel.md, "Patched third-party crates"):
#
#   third_party/clax-rquickjs-sys    rquickjs-sys, with
#                                    third_party/rquickjs-sys.patch applied,
#                                    cut to what a native build of the
#                                    platforms the release builds reads
#                                    (.github/workflows/release.yml)
#   third_party/clax-rquickjs-core   rquickjs-core, unchanged but for its
#                                    manifest: it depends on
#                                    clax-rquickjs-sys, and has no optional
#                                    dependencies
#
# Their library names stay `rquickjs_sys` and `rquickjs`, so their sources
# and clax-bot-js's need no change, and nothing is patched over crates.io's
# crates: a package published from this workspace builds what it tests.
#
# The patch makes QuickJS poll its interrupt handler every 100 back-edges
# and calls instead of every 10,000; makes running out of memory end the
# script (the error cannot be caught and every later poll interrupts); and
# makes the Function constructors compile their source without running it
# and refuse parameters or a body that close the function, as a separate
# parse of each would.
#
# The crates are downloaded from crates.io and checked against the SHA-256s
# below. With --check nothing is written: the command fails if either
# directory differs from what it would write. To move to a new version,
# change VERSION and the SHA-256s, rerun, fix the patch if it no longer
# applies, and run `just bazel-sync`.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=0.14.0
SYS_SHA256=cee271d0eeba64f0915b846cb7ae02e16faf3dfdffdca91731101d9d30fe3423
CORE_SHA256=6657979a24d543e5fc66f32cef2f47ca82ce8eb55cec5f7d59a12b7b570bbf24
SYS=third_party/clax-rquickjs-sys
CORE=third_party/clax-rquickjs-core
PATCH=third_party/rquickjs-sys.patch

check=0
if [ "${1:-}" = "--check" ]; then
  check=1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# fetch NAME SHA256: downloads NAME $VERSION into $work and unpacks it.
fetch() {
  local crate="$work/$1-$VERSION.crate" got
  curl -fsSL -o "$crate" "https://static.crates.io/crates/$1/$1-$VERSION.crate"
  got="$(shasum -a 256 "$crate" | cut -d' ' -f1)"
  if [ "$got" != "$2" ]; then
    echo "vendor-rquickjs: $1 $VERSION has SHA-256 $got, expected $2" >&2
    exit 1
  fi
  tar -xzf "$crate" -C "$work"
}
fetch rquickjs-sys "$SYS_SHA256"
fetch rquickjs-core "$CORE_SHA256"

# --- clax-rquickjs-sys
src="$work/rquickjs-sys-$VERSION"
out="$work/out/sys"
mkdir -p "$out/src/bindings" "$out/src/inlines" "$out/quickjs"
cp "$src/build.rs" "$src/quickjs.bind.h" "$src/README.md" "$out/"
# The published manifest, renamed, without its optional build dependencies
# (bindgen, pretty_env_logger): as a workspace member the crate's optional
# dependencies would be resolved into Cargo.lock and fetched by Bazel. Their
# features stay, empty, so rquickjs-core's references to them resolve; the
# build script cannot run with `bindgen` or `logging` on.
cat > "$out/Cargo.toml" <<EOF
# rquickjs-sys $VERSION from crates.io, renamed, with
# third_party/rquickjs-sys.patch applied: written by scripts/vendor-rquickjs.sh,
# do not edit.
[package]
edition = "2021"
name = "clax-rquickjs-sys"
version = "$VERSION"
authors = ["Mees Delzenne <mees.delzenne@gmail.com>"]
build = "build.rs"
autolib = false
autobins = false
autoexamples = false
autotests = false
autobenches = false
description = "QuickJS bindings for rquickjs, with Clax's patch (interrupt every 100 steps, uncatchable out-of-memory, Function constructors that compile before they run)"
readme = "README.md"
license = "MIT"
repository = "https://github.com/empathic/clax"
publish = false

[features]
bindgen = []
disable-assertions = []
dump-atoms = []
dump-bytecode = []
dump-free = []
dump-gc = []
dump-gc-free = []
dump-leaks = []
dump-mem = []
dump-module-resolve = []
dump-objects = []
dump-promise = []
dump-read-object = []
dump-shapes = []
logging = []
update-bindings = ["bindgen"]

[lib]
name = "rquickjs_sys"
path = "src/lib.rs"
# Its own tests need its dev-dependencies, which are not kept.
test = false
doctest = false

[build-dependencies]
cc = "1"

# Third-party code: the workspace's lints and clippy do not apply.
[lints.rust]
warnings = "allow"

[lints.clippy]
all = "allow"
EOF
# Its Bazel targets, by hand rather than cargo_package() (bazel/cargo.bzl),
# which gives a crate the workspace's lints.
cat > "$out/BUILD.bazel" <<EOF
# clax-rquickjs-sys (rquickjs-sys $VERSION, patched): scripts/vendor-rquickjs.sh
# writes this file; do not edit.
load("@crates//:defs.bzl", "all_crate_deps")
load("@rules_rust//cargo:defs.bzl", "cargo_build_script")
load("@rules_rust//rust:defs.bzl", "rust_library")

cargo_build_script(
    name = "build_script",
    srcs = ["build.rs"],
    data = glob(["quickjs/**", "src/**"]) + ["quickjs.bind.h"],
    edition = "2021",
    version = "$VERSION",
    deps = all_crate_deps(build = True),
)

rust_library(
    name = "rquickjs_sys",
    srcs = glob(["src/**/*.rs"]),
    crate_root = "src/lib.rs",
    edition = "2021",
    version = "$VERSION",
    visibility = ["//visibility:public"],
    deps = [":build_script"],
)

# The manifest, for the layering check (//scripts:crate_layers) and the
# release script test.
filegroup(
    name = "manifest",
    srcs = ["Cargo.toml"],
    visibility = ["//visibility:public"],
)

exports_files(["Cargo.toml"])
EOF
cp "$src/src/lib.rs" "$out/src/"
cp "$src"/src/inlines/*.rs "$out/src/inlines/"
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu \
  aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  cp "$src/src/bindings/$target.rs" "$out/src/bindings/"
done
for f in libregexp.c libunicode.c quickjs.c dtoa.c \
  builtin-array-fromasync.h builtin-iterator-zip-keyed.h builtin-iterator-zip.h cutils.h dtoa.h \
  libregexp-opcode.h libregexp.h libunicode-table.h libunicode.h list.h quickjs-atom.h \
  quickjs-opcode.h quickjs-c-atomics.h quickjs.h LICENSE; do
  cp "$src/quickjs/$f" "$out/quickjs/"
done
(cd "$out" && patch -s -p1 < "$OLDPWD/$PATCH")
find "$out" -name '*.orig' -delete

# --- clax-rquickjs-core
src="$work/rquickjs-core-$VERSION"
out="$work/out/core"
mkdir -p "$out"
cp -R "$src/src" "$src/README.md" "$out/"
# The published manifest, renamed, its library still `rquickjs`, depending
# on clax-rquickjs-sys, and with only the features that need no optional
# dependency (std is what clax-bot-js uses).
cat > "$out/Cargo.toml" <<EOF
# rquickjs-core $VERSION from crates.io, renamed, over clax-rquickjs-sys:
# written by scripts/vendor-rquickjs.sh, do not edit.
[package]
edition = "2021"
name = "clax-rquickjs-core"
version = "$VERSION"
authors = [
    "Mees Delzenne <mees.delzenne@gmail.com>",
    "K. <kayo@illumium.org>",
]
build = false
autolib = false
autobins = false
autoexamples = false
autotests = false
autobenches = false
description = "High level bindings to the QuickJS JavaScript engine (rquickjs-core), over Clax's patched QuickJS"
readme = "README.md"
license = "MIT"
repository = "https://github.com/empathic/clax"
publish = false

[features]
default = ["std"]
std = []
allocator = []
array-buffer = []
classes = []
compile-tests = []
doc-cfg = []
multi-ctx = []
properties = []
rust-alloc = []
disable-assertions = ["clax-rquickjs-sys/disable-assertions"]

[lib]
name = "rquickjs"
path = "src/lib.rs"
# Its own tests need its dev-dependencies, which are not kept.
test = false
doctest = false

[dependencies]
clax-rquickjs-sys = { path = "../clax-rquickjs-sys" }
hashbrown = "0.17"

# Third-party code: the workspace's lints and clippy do not apply.
[lints.rust]
warnings = "allow"

[lints.clippy]
all = "allow"
EOF
cat > "$out/BUILD.bazel" <<EOF
# clax-rquickjs-core (rquickjs-core $VERSION, over clax-rquickjs-sys):
# scripts/vendor-rquickjs.sh writes this file; do not edit.
load("@crates//:defs.bzl", "all_crate_deps")
load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
    name = "rquickjs",
    srcs = glob(["src/**/*.rs"]),
    crate_features = ["std"],
    crate_root = "src/lib.rs",
    edition = "2021",
    version = "$VERSION",
    visibility = ["//visibility:public"],
    deps = ["//third_party/clax-rquickjs-sys:rquickjs_sys"] + all_crate_deps(normal = True),
)

# The manifest, for the layering check (//scripts:crate_layers) and the
# release script test.
filegroup(
    name = "manifest",
    srcs = ["Cargo.toml"],
    visibility = ["//visibility:public"],
)

exports_files(["Cargo.toml"])
EOF

if [ "$check" = 1 ]; then
  for pair in "sys:$SYS" "core:$CORE"; do
    if ! diff -r "$work/out/${pair%%:*}" "${pair#*:}" >/dev/null; then
      echo "vendor-rquickjs: ${pair#*:} is not what scripts/vendor-rquickjs.sh writes; run it" >&2
      exit 1
    fi
  done
  echo "vendor-rquickjs: $SYS and $CORE are rquickjs $VERSION with $PATCH applied"
  exit 0
fi
rm -rf "$SYS" "$CORE"
mkdir -p third_party
cp -R "$work/out/sys" "$SYS"
cp -R "$work/out/core" "$CORE"
echo "vendor-rquickjs: wrote $SYS and $CORE"
