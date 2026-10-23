#!/usr/bin/env bash
# Checks scripts/check-crate-layers.py on copies of the workspace's
# manifests, under crates/ and third_party/ (with an empty source file at
# each target path and build script): the workspace
# passes; an upward or sideways edge fails, naming it, however the manifest
# writes it (a [dependencies] line, a [dependencies.<name>] table, a
# [target.'<cfg>'.dependencies] table or one of its <name> tables, a renamed
# dependency, a build-dependency); a crate the layout does not know fails.
# Dev-dependencies pass. Cargo is $CARGO when set (the Bazel test sets it),
# else cargo on PATH.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
# cargo metadata reads no registry with --no-deps --offline; its home is
# scratch so the test reads no developer configuration.
export CARGO_HOME="$d/cargo-home"
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }
_copy() {
    rm -rf "$d/w" && mkdir -p "$d/w"
    cp Cargo.toml "$d/w/"
    for m in crates/*/Cargo.toml third_party/*/Cargo.toml; do
        c="$d/w/$(dirname "$m")"
        mkdir -p "$c/src" && cp "$m" "$c/Cargo.toml" && : > "$c/src/lib.rs"
        # The target paths the manifest names (src/main.rs, tests/*.rs).
        sed -n -e 's/^path = "\([^.][^"]*\.rs\)"$/\1/p' -e 's/^build = "\([^"]*\.rs\)"$/\1/p' "$m" | while read -r p; do
            mkdir -p "$c/$(dirname "$p")" && : > "$c/$p"
        done
    done
}
# _expect pass|fail NAME [TEXT]: runs the check on the copy; TEXT must be in
# what a failure prints.
_expect() {
    local out st
    out="$(python3 scripts/check-crate-layers.py "$d/w" 2>&1)"
    st=$?
    case "$1" in
        pass) [ "$st" = 0 ] || _fail "$2: $out" ;;
        fail) { [ "$st" != 0 ] && grep -qF "$3" <<<"$out"; } || _fail "$2: $out" ;;
    esac
}
# _after FILE HEADER LINE: puts LINE right after the line HEADER in FILE.
_after() {
    python3 - "$@" <<'EOF'
import sys
f, header, line = sys.argv[1:]
t = open(f).read()
assert header + "\n" in t, header
open(f, "w").write(t.replace(header + "\n", header + "\n" + line + "\n", 1))
EOF
}

_copy
_expect pass "the workspace"

_copy
_after "$d/w/crates/clax-core/Cargo.toml" "[dependencies]" 'clax-store = { path = "../clax-store" }'
_expect fail "a [dependencies] line" "clax-core may not depend on clax-store (normal dependency)"

_copy
printf '\n[dependencies.clax-store]\npath = "../clax-store"\n' >> "$d/w/crates/clax-mcp/Cargo.toml"
_expect fail "a [dependencies.<name>] table" "clax-mcp may not depend on clax-store (normal dependency)"

_copy
printf "\n[target.'cfg(unix)'.dependencies]\nclax-store = { path = \"../clax-store\" }\n" >> "$d/w/crates/clax-mcp/Cargo.toml"
_expect fail "a target table" "clax-mcp may not depend on clax-store (normal dependency for cfg(unix))"

_copy
printf "\n[target.'cfg(unix)'.dependencies.clax-store]\npath = \"../clax-store\"\n" >> "$d/w/crates/clax-hooks/Cargo.toml"
_expect fail "a target table of one dependency" "clax-hooks may not depend on clax-store (normal dependency for cfg(unix))"

_copy
_after "$d/w/crates/clax-sample/Cargo.toml" "[dependencies]" 'store = { package = "clax-store", path = "../clax-store" }'
_expect fail "a renamed dependency" "clax-sample may not depend on clax-store (as store) (normal dependency)"

_copy
printf '\n[build-dependencies]\nclax-http = { path = "../clax-http" }\n' >> "$d/w/crates/clax-store/Cargo.toml"
_expect fail "a build-dependency" "clax-store may not depend on clax-http (build dependency)"

_copy
_after "$d/w/crates/clax-hooks/Cargo.toml" "[dev-dependencies]" 'clax-testkit = { path = "../clax-testkit" }'
_expect pass "a dev-dependency on the testkit"

_copy
mkdir -p "$d/w/crates/clax-extra/src"
printf '[package]\nname = "clax-extra"\nversion = "0.0.0"\nedition = "2024"\n' > "$d/w/crates/clax-extra/Cargo.toml"
: > "$d/w/crates/clax-extra/src/lib.rs"
_after "$d/w/Cargo.toml" '    "crates/clax-store",' '    "crates/clax-extra",'
_expect fail "a crate the layout does not know" "clax-extra has no place in the layout"

# The --publishable mode (scripts/crate-package.py runs it): the workspace
# passes; a [patch] section, or clax-cli reaching a third_party/ crate, fails.
_publishable() {
    local out st
    out="$(python3 scripts/check-crate-layers.py --publishable "$d/w" 2>&1)"
    st=$?
    case "$1" in
        pass) [ "$st" = 0 ] || _fail "$2: $out" ;;
        fail) { [ "$st" != 0 ] && grep -qF "$3" <<<"$out"; } || _fail "$2: $out" ;;
    esac
}
_copy
_publishable pass "the workspace is publishable"

_copy
printf '\n[patch.crates-io]\nserde = { path = "x" }\n' >> "$d/w/Cargo.toml"
_publishable fail "a [patch] section" "Cargo.toml has a [patch] section"

_copy
mkdir -p "$d/w/.cargo"
printf '[patch.crates-io]\nserde = { path = "x" }\n' > "$d/w/.cargo/config.toml"
_publishable fail "a [patch] in .cargo/config.toml" ".cargo/config.toml has a [patch] section"

_copy
_after "$d/w/crates/clax-cli/Cargo.toml" "[dependencies]" 'clax-bot-js = { path = "../clax-bot-js" }'
_publishable fail "clax-cli reaching the patched QuickJS" "clax-cli reaches clax-rquickjs-core"

if [ "$fails" = 0 ]; then echo "crate layers tests: all passed"; else echo "crate layers tests: $fails failed"; exit 1; fi
