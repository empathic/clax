#!/usr/bin/env bash
# Tests scripts/run.sh's choice of build system: which one runs for what is
# on PATH and for each CLAX_BUILD, what it tells the person and how
# often, and the exit status it hands back.
#
# PATH holds stub tools only: a directory of links to the few system
# programs the scripts need, and a directory of stub bazelisk, bazel, cargo,
# node and pnpm that each case fills. No build runs. A stub records how it
# was called in $tmp/calls and exits with STUB_EXIT (default 0).

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp="$(mktemp -d)"
trap 'chmod -R u+w "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT

fails=0
_ok() { echo "ok: $1"; }
_fail() {
    echo "FAIL: $1" >&2
    sed 's/^/    | /' "$tmp/out" >&2
    fails=$((fails + 1))
}

mkdir -p "$tmp/sys"
for tool in bash env sh sed awk grep head tr cat rm mktemp dirname uname find cp chmod mkdir stat; do
    ln -s "$(command -v "$tool")" "$tmp/sys/$tool"
done

want_bazel="$(tr -d '[:space:]' < .bazelversion)"

# A scratch checkout holding what run.sh reads, so no case touches this one.
repo="$tmp/repo"
mkdir -p "$repo/scripts/lib" "$repo/web/dist" "$repo/web/dist-extension"
cp scripts/run.sh scripts/native-test.sh "$repo/scripts/"
cp scripts/lib/build-system.sh "$repo/scripts/lib/"
cp .bazelversion "$repo/"
cp web/package.json "$repo/web/"
touch "$repo/web/dist/.gitkeep" "$repo/web/dist-extension/.gitkeep"

# _stubs <name[=version]>...: the tools installed for the next case.
_stubs() {
    local spec name version
    rm -rf "${tmp:?}/bin"
    mkdir -p "$tmp/bin"
    for spec in "$@"; do
        name="${spec%%=*}"
        version=""
        case "$spec" in *=*) version="${spec#*=}" ;; esac
        if [ -z "$version" ]; then
            case "$name" in
                node) version="v26.8.2" ;;
                pnpm) version="12.3.4" ;;
                bazel) version="bazel $want_bazel" ;;
            esac
        fi
        cat > "$tmp/bin/$name" <<STUB
#!$tmp/sys/bash
if [ "\${1:-}" = "--version" ]; then echo "$version"; exit 0; fi
# No cargo-nextest: scripts/native-test.sh runs cargo test.
if [ "$name \${1:-}" = "cargo nextest" ]; then exit 1; fi
echo "$name \$*" >> "$tmp/calls"
exit "\${STUB_EXIT:-0}"
STUB
        chmod +x "$tmp/bin/$name"
    done
}

# _try [VAR=value]... -- <run.sh arguments>: runs it on the stub PATH;
# output (both streams) in $tmp/out, the status in $status.
_try() {
    local vars=()
    while [ "$1" != "--" ]; do vars+=("$1"); shift; done
    shift
    : > "$tmp/calls"
    status=0
    env -u CLAX_BUILD "${vars[@]+"${vars[@]}"}" PATH="$tmp/bin:$tmp/sys" \
        "$tmp/sys/bash" "$repo/scripts/run.sh" "$@" > "$tmp/out" 2>&1 || status=$?
}

_count() { grep -c -- "$1" "$tmp/out" || true; }
# _expect <what> <status> [<count>x<pattern>]...
_expect() {
    local what="$1" want="$2" spec n pattern
    shift 2
    if [ "$status" != "$want" ]; then _fail "$what: exit $status, want $want"; return; fi
    for spec in "$@"; do
        n="${spec%%x*}"
        pattern="${spec#*x}"
        if [ "$(_count "$pattern")" != "$n" ]; then
            _fail "$what: want $n line(s) matching '$pattern', got $(_count "$pattern")"
            return
        fi
    done
    _ok "$what"
}
_called() { grep -q -- "$1" "$tmp/calls"; }

native=(cargo node pnpm)
notice="running \`.*\` with the native tools"
summary="with the native tools, not Bazel. CI gates on Bazel"

# 1. Bazelisk on PATH: Bazel runs, with nothing said about the choice, and
#    its status is the command's.
_stubs bazelisk "${native[@]}"
_try -- web-test --test_output=errors
_expect "bazelisk: Bazel runs web-test" 0 "0x$notice" "0x$summary" "0xInstall"
_called "^bazelisk test --lockfile_mode=error //web:lint //web:typecheck //web:unit //web:bundle_size --test_output=errors$" \
    || _fail "bazelisk: called as expected"
if _called "^pnpm\|^cargo"; then _fail "bazelisk: no native tool runs"; fi
_try STUB_EXIT=3 -- web-test
_expect "bazelisk: a failing run's status is handed back" 3 "0x$summary"
_try -- --dry-run ci
_expect "bazelisk: ci is the script CI runs" 0 "1xwould run: ./scripts/bazel-test.sh --ci$"
_try -- which
_expect "bazelisk: which says bazel" 0 "1x^bazel: Bazelisk found"

# 1b. Bazelisk and the native tools, and the worktree has the native build
#     state a task reuses: speed wins, and the native tools run that task,
#     saying why; a task without it, or CLAX_BUILD=bazel, runs Bazel.
mkdir -p "$repo/target/debug" "$repo/web/node_modules"
_try -- test
_expect "native build state: test runs natively, saying why" 0 \
    "1xthis worktree has a native build (target/debug)" "1x$summary" "0xInstall"
_called "^cargo test --workspace$" || _fail "native build state: cargo test called"
if _called "^bazelisk"; then _fail "native build state: Bazel does not run"; fi
_try -- --dry-run web-test
_expect "native build state: web-test runs natively" 0 "1xweb/node_modules" "1xwould run: bash -c cd web"
_try -- --dry-run ci
_expect "native build state: ci needs Pi's packages too, else Bazel" 0 "1xwould run: ./scripts/bazel-test.sh --ci$"
_try CLAX_BUILD=bazel -- --dry-run test
_expect "native build state: CLAX_BUILD=bazel still runs Bazel" 0 "1xwould run: ./scripts/bazel-test.sh --rust$" "0xnative build"
_try -- --dry-run build
_expect "native build state: build is not one of them" 0 "1xwould run: .*bazelisk build"
rm -rf "$repo/target" "$repo/web/node_modules"

# 2. Bazelisk alone, no native tools: fine, and nothing is said.
_stubs bazelisk
_try -- web-test
_expect "bazelisk alone: runs, silently" 0 "0xnative" "0xInstall" "0xnote"
_try -- --dry-run check
_expect "bazelisk alone: check runs unformatted, and says so" 0 \
    "1xnot auto-formatting" "0xcargo fmt" "1xwould run: ./scripts/bazel-test.sh --ci$"

# 3. No Bazel: the native tools run, the notice comes once before and the
#    summary once after, and the status is the command's.
_stubs "${native[@]}"
_try -- test
_expect "no Bazel: cargo runs, said once before and once after" 0 \
    "1x── Bazel not found: $notice (Cargo and pnpm) ──" \
    "1xneeds no target/ or node_modules" \
    "1x^   Install it: " \
    "1x\`test\` passed $summary, and the two can differ: this does not say what CI will. Install Bazelisk for the same checks: "
[ "$(wc -l < "$tmp/out" | tr -d ' ')" = 5 ] || _fail "no Bazel: five lines in all around the run"
_called "^cargo test --workspace$" || _fail "no Bazel: cargo test called"
[ "$(sed -n 1p "$tmp/out" | grep -c "Bazel not found")" = 1 ] || _fail "no Bazel: the notice is the first line"
[ "$(tail -1 "$tmp/out" | grep -c "$summary")" = 1 ] || _fail "no Bazel: the summary is the last line"
_try STUB_EXIT=7 -- test
_expect "no Bazel: a failing run's status is handed back" 7 \
    "1x\`test\` failed (exit 7) $summary" "1x$notice"
_try -- --dry-run ci
_expect "no Bazel: ci is scripts/quality_gates.sh" 0 "1xwould run: ./scripts/quality_gates.sh$" "1x$notice" "1x$summary"
_try -- which
_expect "no Bazel: which says native, and why" 0 "1x^native: Bazel not found"

# 4. A stray bazel of another version, no Bazelisk: not used; the notice
#    names both versions.
_stubs "bazel=bazel 6.4.0" "${native[@]}"
_try -- test
_expect "old bazel: falls back and says why" 0 \
    "1xbazel 6.4.0 is on PATH, but .bazelversion names $want_bazel and Bazelisk is not installed: $notice" \
    "1x$summary"
if _called "^bazel "; then _fail "old bazel: never run"; fi
_try CLAX_BUILD=bazel -- web-test
_expect "old bazel: CLAX_BUILD=bazel refuses it" 2 "1xerror: CLAX_BUILD=bazel, and bazel 6.4.0 is on PATH" "1xInstall Bazelisk: "

# 5. A bazel at the pinned version, no Bazelisk: used.
_stubs bazel "${native[@]}"
_try -- web-test
_expect "pinned bazel: used" 0 "0x$notice" "0x$summary"
_called "^bazel test " || _fail "pinned bazel: called"

# 6. CLAX_BUILD=bazel with no Bazel: an error with the install hint,
#    and nothing runs.
_stubs "${native[@]}"
_try CLAX_BUILD=bazel -- test
_expect "CLAX_BUILD=bazel without Bazel: an error, no fallback" 2 \
    "1xerror: CLAX_BUILD=bazel, and Bazel not found" "1xInstall Bazelisk: " "0x$summary"
[ ! -s "$tmp/calls" ] || _fail "CLAX_BUILD=bazel without Bazel: nothing runs"

# 7. CLAX_BUILD=native with Bazelisk installed: native, Bazel never
#    looked at, no install notice, and the summary still names the tools.
_stubs bazelisk "${native[@]}"
_try CLAX_BUILD=native -- test
_expect "CLAX_BUILD=native: native, no notice, summary once" 0 \
    "0x$notice" "0xInstall" "1x\`test\` passed $summary"
:
if _called "^bazelisk"; then _fail "CLAX_BUILD=native: Bazel never runs"; fi
_try CLAX_BUILD=auto -- web-test
_expect "CLAX_BUILD=auto: as unset" 0 "0x$summary"
_called "^bazelisk test" || _fail "CLAX_BUILD=auto: Bazel called"
_try CLAX_BUILD=cargo -- test
_expect "CLAX_BUILD of another value is refused" 2 "1xmust be auto, bazel or native (got 'cargo')"

# 8. CLAX_BUILD=native without the tools: an error naming them.
_stubs bazelisk cargo
_try CLAX_BUILD=native -- web-test
_expect "CLAX_BUILD=native without pnpm: an error" 2 "1xnative tools are not all installed: node, pnpm\\."
_try CLAX_BUILD=native -- test
_expect "CLAX_BUILD=native: a Rust task needs only cargo" 0 "1x$summary"

# 9. Neither: one message with both ways out, and nothing runs.
_stubs
_try -- ci
_expect "neither toolchain: one message, both options" 2 \
    "1xerror: nothing to build with" "1xBazelisk, which fetches everything else: " \
    "1xnative tools, missing here: cargo (Rust: https://rustup.rs), node, pnpm" "0x$summary"
_try -- which
_expect "neither toolchain: which says none" 1 "1x^none: nothing to build with"

# 10. A Node or pnpm the repository does not pin for: a note, once, and
#     the run goes on.
_stubs cargo "node=v18.0.0" "pnpm=8.1.0"
_try -- --dry-run web-test
_expect "an unpinned pnpm: one note" 0 \
    "1xnote: pnpm 8.1.0 is installed; web/package.json pins pnpm" "1x$summary"

# 11. `web` with Bazel: the built files land in web/dist and
#     web/dist-extension as writable regular files with one link each, never
#     links to Bazel's read-only outputs; .gitkeep stays.
_stubs bazelisk
cat > "$tmp/bin/bazelisk" <<STUB
#!$tmp/sys/bash
if [ "\${1:-}" = "info" ]; then echo "$tmp/bazel-bin"; exit 0; fi
echo "bazelisk \$*" >> "$tmp/calls"
exit 0
STUB
chmod +x "$tmp/bin/bazelisk"
mkdir -p "$tmp/bazel-bin/web/dist/_clax" "$tmp/bazel-bin/web/dist-extension"
echo built > "$tmp/bazel-bin/web/dist/_clax/bridge.js"
echo built > "$tmp/bazel-bin/web/dist-extension/manifest.json"
chmod -R a-w "$tmp/bazel-bin"
_try -- web
ok=1
for f in "$repo/web/dist/_clax/bridge.js" "$repo/web/dist-extension/manifest.json"; do
    links="$(stat -f %l "$f" 2>/dev/null || stat -c %h "$f")"
    { [ -f "$f" ] && [ ! -L "$f" ] && [ -w "$f" ] && [ "$links" = 1 ]; } || ok=0
done
[ -f "$repo/web/dist/.gitkeep" ] || ok=0
chmod -R u+w "$tmp/bazel-bin"
if [ "$status" = 0 ] && [ "$ok" = 1 ]; then _ok "web: copies, writable, one link each"; else _fail "web: copies, writable, one link each"; fi

if [ "$fails" -gt 0 ]; then
    echo "$fails check(s) failed" >&2
    exit 1
fi
echo "all run.sh checks passed"
