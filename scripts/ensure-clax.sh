#!/usr/bin/env bash
# Locate the Clax `clax` binary, installing it globally if needed.
#
# Usage:
#   ensure-clax.sh                 print the absolute binary path on stdout
#   ensure-clax.sh exec <args...>  resolve, then run `clax <args...>`
#
# Everything except the resolved path / exec'd command output goes to stderr.
#
# Resolution order:
#   1. $CLAX_BIN, when set and usable
#   2. `clax` on PATH that identifies as the Clax CLI
#   3. $CLAX_INSTALL_DIR/clax (default ~/.local/bin/clax)
#   4. $CLAX_CONFIG_DIR/bin/clax (default ~/.clax/bin/clax)
#   5. A source checkout's build: the newer of target/release/clax and
#      target/debug/clax that identifies as the Clax CLI (used with a
#      warning when older than MIN_VERSION). The checkout is the first
#      directory whose Cargo.toml names clax-cli, searched upwards from
#      each of: $CLAX_SOURCE_DIR; the `source` path recorded in the plugin
#      root's .codex-plugin/plugin.json or .claude-plugin/plugin.json (an
#      optional field no build writes); for a Codex plugin cache copy
#      (<codex home>/plugins/cache/<marketplace>/<plugin>/<version>), the
#      `source` of [marketplaces.<marketplace>] in <codex home>/config.toml;
#      and this script's own directory.
#   6. Download the latest GitHub release, verify its checksum, and install
#      to ~/.local/bin — or to ~/.clax/bin when an unrelated binary
#      named `clax` would shadow the ~/.local/bin name.
#
# `exec hook ...` never downloads: a lifecycle hook must not install software
# or fail its harness. With no binary found it prints one line to stderr and
# exits 0 with empty stdout. Every failure to resolve a binary appends one
# line to ${CLAX_HOME:-~/.clax}/logs/hooks.log (rotated to hooks.log.1
# past 1 MiB); a log that cannot be written is ignored.
#
# Environment variables:
#   CLAX_BIN              Absolute path to a specific `clax` to use, ahead
#                         of every other candidate. Set but unusable is a
#                         warning, not a silent fall-through.
#   CLAX_SOURCE_DIR       A source checkout (or a directory inside one) whose
#                         target/ build to use when steps 1-4 find nothing
#   CLAX_HOME             Where hooks.log lives (default ~/.clax)
#   CLAX_INSTALL_DIR      Override the global install directory
#   CLAX_CONFIG_DIR       Override ~/.clax (fallback bin lives under it)
#   CLAX_RELEASE_BASE_URL
#                         Override the release download base URL (default
#                         https://github.com/empathic/clax/releases/download);
#                         files are fetched from <base>/<version>/
#   CLAX_RELEASE_VERSION
#                         Install this release tag instead of looking up the
#                         latest one

set -euo pipefail

REPO="empathic/clax"
INSTALL_DIR="${CLAX_INSTALL_DIR:-$HOME/.local/bin}"
FALLBACK_DIR="${CLAX_CONFIG_DIR:-$HOME/.clax}/bin"
RELEASE_BASE_URL="${CLAX_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
# Oldest release whose CLI surface the plugins are written against.
MIN_VERSION="0.2.0"
TMPDIR_CLEANUP=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
PLUGIN_ROOT_DIR="$(dirname "$SCRIPT_DIR")"
HOOKS_LOG_MAX_BYTES=1048576
ARGV="$*"
MODE="${1:-}"
if [ "$MODE" = exec ]; then MODE="${2:-exec}"; fi
if [ -z "$MODE" ]; then MODE="print"; fi
DOWNLOADING=""
REMEDY="install with \`cargo install --path crates/clax-cli\` from the checkout or set CLAX_BIN; see \`clax doctor\`"

log() { echo "$@" >&2; }

# Appends one line naming this run and $1 (the reason) to hooks.log, rotating
# the file to hooks.log.1 once it passes HOOKS_LOG_MAX_BYTES. Never fails.
log_failure() {
    {
        local dir file agent="" size prev=""
        dir="${CLAX_HOME:-$HOME/.clax}/logs"
        file="$dir/hooks.log"
        for a in $ARGV; do
            if [ "$prev" = "--agent" ]; then agent="$a"; fi
            prev="$a"
        done
        mkdir -p "$dir" || return 0
        if [ -f "$file" ]; then
            size="$(wc -c < "$file" | tr -d ' ')"
            if [ "${size:-0}" -gt "$HOOKS_LOG_MAX_BYTES" ]; then mv -f "$file" "$file.1"; fi
        fi
        printf '%s launcher mode=%s agent=%s exit=%s reason="%s" argv="%s"\n' \
            "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$MODE" "${agent:--}" "$2" "$1" "$ARGV" >> "$file"
    } 2>/dev/null || true
}

# The Clax source checkout at or above $1: the first directory whose
# Cargo.toml names clax-cli.
find_checkout() {
    local dir="$1"
    [ -n "$dir" ] && [ -d "$dir" ] || return 1
    dir="$(cd "$dir" && pwd -P)" || return 1
    while :; do
        if [ -f "$dir/Cargo.toml" ] && grep -q "clax-cli" "$dir/Cargo.toml" 2>/dev/null; then
            echo "$dir"
            return 0
        fi
        [ "$dir" = "/" ] && return 1
        dir="$(dirname "$dir")"
    done
}

# The `source` path recorded in the plugin root's manifest, if any.
manifest_source() {
    local m
    for m in "$PLUGIN_ROOT_DIR/.codex-plugin/plugin.json" "$PLUGIN_ROOT_DIR/.claude-plugin/plugin.json"; do
        [ -f "$m" ] || continue
        sed -n 's/.*"source"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$m" | head -1
    done
}

# For a Codex plugin cache copy at <home>/plugins/cache/<marketplace>/<plugin>/<version>,
# the directory <home>/config.toml records as the marketplace's source.
codex_marketplace_source() {
    local version_dir plugin_dir market_dir cache_dir plugins_dir home market
    version_dir="$PLUGIN_ROOT_DIR"
    plugin_dir="$(dirname "$version_dir")"
    market_dir="$(dirname "$plugin_dir")"
    cache_dir="$(dirname "$market_dir")"
    plugins_dir="$(dirname "$cache_dir")"
    [ "${cache_dir##*/}" = cache ] && [ "${plugins_dir##*/}" = plugins ] || return 0
    home="$(dirname "$plugins_dir")"
    market="${market_dir##*/}"
    [ -f "$home/config.toml" ] || return 0
    awk -v sec="[marketplaces.${market}]" '
        $0 == sec { on = 1; next }
        /^[[:space:]]*\[/ { on = 0 }
        on && /^[[:space:]]*source[[:space:]]*=/ {
            sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; exit
        }
    ' "$home/config.toml"
}

# The newer of a checkout's release and debug builds that is the Clax CLI.
checkout_binary() {
    local start checkout best="" candidate
    for start in "${CLAX_SOURCE_DIR:-}" "$(manifest_source)" "$(codex_marketplace_source)" "$SCRIPT_DIR"; do
        checkout="$(find_checkout "$start")" || continue
        for candidate in "$checkout/target/release/clax" "$checkout/target/debug/clax"; do
            if [ -x "$candidate" ] && is_clax "$candidate"; then
                if [ -z "$best" ] || [ "$candidate" -nt "$best" ]; then best="$candidate"; fi
            fi
        done
        if [ -n "$best" ]; then
            echo "$best"
            return 0
        fi
    done
    return 1
}

is_clax() {
    "$1" --version 2>/dev/null | head -1 | grep -qi "clax"
}

warn_if_old() {
    local bin="$1" version
    version="$("$bin" --version 2>/dev/null | awk '{print $2}')" || return 0
    [ -n "$version" ] || return 0
    if [ "$(printf '%s\n%s\n' "$MIN_VERSION" "$version" | sort -V | head -1)" != "$MIN_VERSION" ]; then
        log "warning: clax ${version} is older than ${MIN_VERSION}; some plugin features may not work."
        log "Upgrade: re-run this script after removing the old binary, or see https://github.com/${REPO}/releases"
    fi
}

resolve_existing() {
    local candidate
    # An explicit $CLAX_BIN wins over everything, including an `clax`
    # already on PATH: it is how you point the plugin at a working tree's
    # `target/release/clax` without installing anything.
    #
    # If it is set but unusable, warn rather than fall through quietly. A
    # stale override (the usual cause is `cargo clean`) would otherwise be
    # indistinguishable from having no override at all, and the tools
    # would keep working against a *different* binary than the one you
    # believe you are testing — the failure that costs an afternoon.
    if [ -n "${CLAX_BIN:-}" ]; then
        if [ -x "$CLAX_BIN" ] && is_clax "$CLAX_BIN"; then
            echo "$CLAX_BIN"
            return 0
        fi
        # Hook mode folds this into its single no-binary line (see main).
        if [ "$MODE" != hook ]; then
            log "warning: \$CLAX_BIN is set to '${CLAX_BIN}' but is not a usable Clax CLI; ignoring it."
        fi
    fi
    if candidate="$(command -v clax 2>/dev/null)" && is_clax "$candidate"; then
        echo "$candidate"
        return 0
    fi
    for candidate in "$INSTALL_DIR/clax" "$FALLBACK_DIR/clax"; do
        if [ -x "$candidate" ] && is_clax "$candidate"; then
            echo "$candidate"
            return 0
        fi
    done
    checkout_binary
}

choose_install_dir() {
    # An `clax` binary that isn't the Clax CLI already claims the name
    # (resolve_existing ran first, so anything found here is foreign) —
    # install under ~/.clax/bin instead of shadow-fighting it.
    if command -v clax >/dev/null 2>&1 || [ -e "$INSTALL_DIR/clax" ]; then
        echo "$FALLBACK_DIR"
    else
        echo "$INSTALL_DIR"
    fi
}

check_dependencies() {
    local missing=()
    for cmd in curl tar; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if ! command -v sha256sum &>/dev/null && ! command -v shasum &>/dev/null; then
        missing+=("sha256sum or shasum")
    fi
    if [ ${#missing[@]} -gt 0 ]; then
        log "Error: required commands not found: ${missing[*]}"
        exit 1
    fi
}

resolve_target() {
    local os arch
    case "$(uname -s)" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        *)      cargo_fallback "unsupported OS '$(uname -s)'" ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)             cargo_fallback "unsupported architecture '$(uname -m)'" ;;
    esac
    case "${os}-${arch}" in
        macos-aarch64) echo "aarch64-apple-darwin" ;;
        linux-x86_64)  echo "x86_64-unknown-linux-musl" ;;
        linux-aarch64) echo "aarch64-unknown-linux-musl" ;;
        *)             cargo_fallback "no prebuilt binary for ${os}-${arch}" ;;
    esac
}

cargo_fallback() {
    log "Error: $1."
    log "Build Clax from source instead: https://github.com/${REPO}"
    exit 1
}

fetch_latest_version() {
    if [ -n "${CLAX_RELEASE_VERSION:-}" ]; then
        echo "$CLAX_RELEASE_VERSION"
        return 0
    fi
    local url version
    url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")"
    version="${url##*/}"
    if [ -z "$version" ] || [ "$version" = "latest" ]; then
        log "Error: could not determine the latest release."
        log "Check https://github.com/${REPO}/releases"
        exit 1
    fi
    echo "$version"
}

download_and_verify() {
    local version="$1" target="$2" tmpdir="$3"
    local base_url="${RELEASE_BASE_URL}/${version}"
    local tarball="clax-${target}.tar.gz"

    log "Downloading ${tarball}..."
    curl -fsSL "${base_url}/${tarball}" -o "${tmpdir}/${tarball}"
    curl -fsSL "${base_url}/${tarball}.sha256" -o "${tmpdir}/${tarball}.sha256"

    DOWNLOADING="verify"
    (
        cd "$tmpdir"
        log "Verifying checksum..."
        if command -v sha256sum &>/dev/null; then
            sha256sum -c "${tarball}.sha256" >&2
        else
            shasum -a 256 -c "${tarball}.sha256" >&2
        fi
        tar xzf "$tarball"
    )
}

path_hint() {
    local dir="$1"
    case ":$PATH:" in
        *":${dir}:"*) ;;
        *)
            log ""
            log "Note: ${dir} is not in your PATH. To use \`clax\` from your own shell, add:"
            log "  export PATH=\"${dir}:\$PATH\""
            ;;
    esac
}

# Sets $RESOLVED_BIN. Deliberately not invoked via command substitution:
# $(...) subshells don't inherit `set -e` (without bash 4.4's inherit_errexit,
# which macOS's bash 3.2 lacks), and this chain must abort on a failed
# download or checksum.
install_clax() {
    local target version dest_dir
    # Armed first, so a missing tool or an unsupported platform is logged and
    # names the remedies like a failed download.
    DOWNLOADING="setup"
    trap on_exit EXIT
    check_dependencies
    target="$(resolve_target)"
    DOWNLOADING=1
    version="$(fetch_latest_version)"
    dest_dir="$(choose_install_dir)"

    log "Installing clax ${version} (${target}) to ${dest_dir}..."
    TMPDIR_CLEANUP="$(mktemp -d)"

    download_and_verify "$version" "$target" "$TMPDIR_CLEANUP"
    mkdir -p "$dest_dir"
    mv "${TMPDIR_CLEANUP}/clax" "${dest_dir}/clax"
    chmod +x "${dest_dir}/clax"

    DOWNLOADING=""
    log "Installed clax to ${dest_dir}/clax"
    path_hint "$dest_dir"
    RESOLVED_BIN="${dest_dir}/clax"
}

# Removes the download's temp directory; after a failed download, says why in
# plain words and logs it.
on_exit() {
    local rc=$?
    if [ -n "$TMPDIR_CLEANUP" ]; then rm -rf "$TMPDIR_CLEANUP"; fi
    if [ "$rc" != 0 ] && [ -n "$DOWNLOADING" ]; then
        if [ "$DOWNLOADING" = verify ]; then
            log "clax: no binary found, and the downloaded release failed verification."
        elif [ "$DOWNLOADING" = setup ]; then
            log "clax: no binary found, and a release cannot be downloaded here (see the error above)."
        else
            log "clax: no binary found, and the release download failed: no Clax release has been published yet."
        fi
        log "clax: ${REMEDY}"
        log_failure "no binary found; release download failed" 1
        exit 1
    fi
    exit "$rc"
}

# Runs a hook with the found binary and always exits 0: a hook must never
# fail its harness. Its stderr passes through; its stdout only when it exited
# 0. A non-zero exit is logged with the end of its stderr.
run_hook() {
    local bin="$1" out rc errfile="" tail=""
    shift
    errfile="$(mktemp 2>/dev/null)" || errfile=""
    if [ -n "$errfile" ]; then
        if out="$("$bin" "$@" 2>"$errfile")"; then rc=0; else rc=$?; fi
        cat "$errfile" >&2 2>/dev/null || true
        tail="$(tr '\n"' " '" < "$errfile" 2>/dev/null)" || tail=""
        rm -f "$errfile"
    else
        if out="$("$bin" "$@")"; then rc=0; else rc=$?; fi
    fi
    if [ "$rc" = 0 ]; then
        if [ -n "$out" ]; then printf '%s\n' "$out"; fi
    else
        tail="${tail% }"
        if [ "${#tail}" -gt 200 ]; then tail="${tail: -200}"; fi
        log_failure "clax exited ${rc}: ${tail}" "$rc"
    fi
    exit 0
}

main() {
    local bin why=""
    if ! bin="$(resolve_existing)"; then
        if [ "${1:-}" = exec ] && [ "${2:-}" = hook ]; then
            if [ -n "${CLAX_BIN:-}" ]; then
                why=" (CLAX_BIN '${CLAX_BIN}' is not a usable Clax CLI)"
            fi
            log "clax: no binary found${why}; ${REMEDY}"
            log_failure "no binary found${why}" 0
            exit 0
        fi
        install_clax
        bin="$RESOLVED_BIN"
    fi
    warn_if_old "$bin"

    case "${1:-}" in
        exec)
            shift
            if [ "${1:-}" = hook ]; then run_hook "$bin" "$@"; fi
            exec "$bin" "$@"
            ;;
        "")
            echo "$bin"
            ;;
        *)
            log "usage: ensure-clax.sh [exec <args...>]"
            exit 2
            ;;
    esac
}

main "$@"
