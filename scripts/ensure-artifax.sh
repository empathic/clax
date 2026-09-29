#!/usr/bin/env bash
# Locate the Artifax `artifax` binary, installing it globally if needed.
#
# Usage:
#   ensure-artifax.sh                 print the absolute binary path on stdout
#   ensure-artifax.sh exec <args...>  resolve, then run `artifax <args...>`
#
# Everything except the resolved path / exec'd command output goes to stderr.
#
# Resolution order:
#   1. $ARTIFAX_BIN, when set and usable
#   2. `artifax` on PATH that identifies as the Artifax CLI
#   3. $ARTIFAX_INSTALL_DIR/artifax (default ~/.local/bin/artifax)
#   4. $ARTIFAX_CONFIG_DIR/bin/artifax (default ~/.artifax/bin/artifax)
#   5. A source checkout's build: the newer of target/release/artifax and
#      target/debug/artifax that identifies as the Artifax CLI (used with a
#      warning when older than MIN_VERSION). The checkout is the first
#      directory whose Cargo.toml names artifax-cli, searched upwards from
#      each of: $ARTIFAX_SOURCE_DIR; the `source` path recorded in the plugin
#      root's .codex-plugin/plugin.json or .claude-plugin/plugin.json (an
#      optional field no build writes); for a Codex plugin cache copy
#      (<codex home>/plugins/cache/<marketplace>/<plugin>/<version>), the
#      `source` of [marketplaces.<marketplace>] in <codex home>/config.toml;
#      and this script's own directory.
#   6. Download the latest GitHub release, verify its checksum, and install
#      to ~/.local/bin — or to ~/.artifax/bin when an unrelated binary
#      named `artifax` would shadow the ~/.local/bin name.
#
# `exec hook ...` never downloads: a lifecycle hook must not install software
# or fail its harness. With no binary found it prints one line to stderr and
# exits 0 with empty stdout. Every failure to resolve a binary appends one
# line to ${ARTIFAX_HOME:-~/.artifax}/logs/hooks.log (rotated to hooks.log.1
# past 1 MiB); a log that cannot be written is ignored.
#
# Environment variables:
#   ARTIFAX_BIN           Absolute path to a specific `artifax` to use, ahead
#                         of every other candidate. Set but unusable is a
#                         warning, not a silent fall-through.
#   ARTIFAX_SOURCE_DIR    A source checkout (or a directory inside one) whose
#                         target/ build to use when steps 1-4 find nothing
#   ARTIFAX_HOME          Where hooks.log lives (default ~/.artifax)
#   ARTIFAX_INSTALL_DIR   Override the global install directory
#   ARTIFAX_CONFIG_DIR    Override ~/.artifax (fallback bin lives under it)
#   ARTIFAX_RELEASE_BASE_URL
#                         Override the release download base URL (default
#                         https://github.com/empathic/artifax/releases/download);
#                         files are fetched from <base>/<version>/
#   ARTIFAX_RELEASE_VERSION
#                         Install this release tag instead of looking up the
#                         latest one

set -euo pipefail

REPO="empathic/artifax"
INSTALL_DIR="${ARTIFAX_INSTALL_DIR:-$HOME/.local/bin}"
FALLBACK_DIR="${ARTIFAX_CONFIG_DIR:-$HOME/.artifax}/bin"
RELEASE_BASE_URL="${ARTIFAX_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
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
REMEDY="install with \`cargo install --path crates/artifax-cli\` from the checkout or set ARTIFAX_BIN; see \`artifax doctor\`"

log() { echo "$@" >&2; }

# Appends one line naming this run and $1 (the reason) to hooks.log, rotating
# the file to hooks.log.1 once it passes HOOKS_LOG_MAX_BYTES. Never fails.
log_failure() {
    {
        local dir file agent="" size prev=""
        dir="${ARTIFAX_HOME:-$HOME/.artifax}/logs"
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

# The Artifax source checkout at or above $1: the first directory whose
# Cargo.toml names artifax-cli.
find_checkout() {
    local dir="$1"
    [ -n "$dir" ] && [ -d "$dir" ] || return 1
    dir="$(cd "$dir" && pwd -P)" || return 1
    while :; do
        if [ -f "$dir/Cargo.toml" ] && grep -q "artifax-cli" "$dir/Cargo.toml" 2>/dev/null; then
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

# The newer of a checkout's release and debug builds that is the Artifax CLI.
checkout_binary() {
    local start checkout best="" candidate
    for start in "${ARTIFAX_SOURCE_DIR:-}" "$(manifest_source)" "$(codex_marketplace_source)" "$SCRIPT_DIR"; do
        checkout="$(find_checkout "$start")" || continue
        for candidate in "$checkout/target/release/artifax" "$checkout/target/debug/artifax"; do
            if [ -x "$candidate" ] && is_artifax "$candidate"; then
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

is_artifax() {
    "$1" --version 2>/dev/null | head -1 | grep -qi "artifax"
}

warn_if_old() {
    local bin="$1" version
    version="$("$bin" --version 2>/dev/null | awk '{print $2}')" || return 0
    [ -n "$version" ] || return 0
    if [ "$(printf '%s\n%s\n' "$MIN_VERSION" "$version" | sort -V | head -1)" != "$MIN_VERSION" ]; then
        log "warning: artifax ${version} is older than ${MIN_VERSION}; some plugin features may not work."
        log "Upgrade: re-run this script after removing the old binary, or see https://github.com/${REPO}/releases"
    fi
}

resolve_existing() {
    local candidate
    # An explicit $ARTIFAX_BIN wins over everything, including an `artifax`
    # already on PATH: it is how you point the plugin at a working tree's
    # `target/release/artifax` without installing anything.
    #
    # If it is set but unusable, warn rather than fall through quietly. A
    # stale override (the usual cause is `cargo clean`) would otherwise be
    # indistinguishable from having no override at all, and the tools
    # would keep working against a *different* binary than the one you
    # believe you are testing — the failure that costs an afternoon.
    if [ -n "${ARTIFAX_BIN:-}" ]; then
        if [ -x "$ARTIFAX_BIN" ] && is_artifax "$ARTIFAX_BIN"; then
            echo "$ARTIFAX_BIN"
            return 0
        fi
        # Hook mode folds this into its single no-binary line (see main).
        if [ "$MODE" != hook ]; then
            log "warning: \$ARTIFAX_BIN is set to '${ARTIFAX_BIN}' but is not a usable Artifax CLI; ignoring it."
        fi
    fi
    if candidate="$(command -v artifax 2>/dev/null)" && is_artifax "$candidate"; then
        echo "$candidate"
        return 0
    fi
    for candidate in "$INSTALL_DIR/artifax" "$FALLBACK_DIR/artifax"; do
        if [ -x "$candidate" ] && is_artifax "$candidate"; then
            echo "$candidate"
            return 0
        fi
    done
    checkout_binary
}

choose_install_dir() {
    # An `artifax` binary that isn't the Artifax CLI already claims the name
    # (resolve_existing ran first, so anything found here is foreign) —
    # install under ~/.artifax/bin instead of shadow-fighting it.
    if command -v artifax >/dev/null 2>&1 || [ -e "$INSTALL_DIR/artifax" ]; then
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
    log "Build Artifax from source instead: https://github.com/${REPO}"
    exit 1
}

fetch_latest_version() {
    if [ -n "${ARTIFAX_RELEASE_VERSION:-}" ]; then
        echo "$ARTIFAX_RELEASE_VERSION"
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
    local tarball="artifax-${target}.tar.gz"

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
            log "Note: ${dir} is not in your PATH. To use \`artifax\` from your own shell, add:"
            log "  export PATH=\"${dir}:\$PATH\""
            ;;
    esac
}

# Sets $RESOLVED_BIN. Deliberately not invoked via command substitution:
# $(...) subshells don't inherit `set -e` (without bash 4.4's inherit_errexit,
# which macOS's bash 3.2 lacks), and this chain must abort on a failed
# download or checksum.
install_artifax() {
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

    log "Installing artifax ${version} (${target}) to ${dest_dir}..."
    TMPDIR_CLEANUP="$(mktemp -d)"

    download_and_verify "$version" "$target" "$TMPDIR_CLEANUP"
    mkdir -p "$dest_dir"
    mv "${TMPDIR_CLEANUP}/artifax" "${dest_dir}/artifax"
    chmod +x "${dest_dir}/artifax"

    DOWNLOADING=""
    log "Installed artifax to ${dest_dir}/artifax"
    path_hint "$dest_dir"
    RESOLVED_BIN="${dest_dir}/artifax"
}

# Removes the download's temp directory; after a failed download, says why in
# plain words and logs it.
on_exit() {
    local rc=$?
    if [ -n "$TMPDIR_CLEANUP" ]; then rm -rf "$TMPDIR_CLEANUP"; fi
    if [ "$rc" != 0 ] && [ -n "$DOWNLOADING" ]; then
        if [ "$DOWNLOADING" = verify ]; then
            log "artifax: no binary found, and the downloaded release failed verification."
        elif [ "$DOWNLOADING" = setup ]; then
            log "artifax: no binary found, and a release cannot be downloaded here (see the error above)."
        else
            log "artifax: no binary found, and the release download failed: no Artifax release has been published yet."
        fi
        log "artifax: ${REMEDY}"
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
        log_failure "artifax exited ${rc}: ${tail}" "$rc"
    fi
    exit 0
}

main() {
    local bin why=""
    if ! bin="$(resolve_existing)"; then
        if [ "${1:-}" = exec ] && [ "${2:-}" = hook ]; then
            if [ -n "${ARTIFAX_BIN:-}" ]; then
                why=" (ARTIFAX_BIN '${ARTIFAX_BIN}' is not a usable Artifax CLI)"
            fi
            log "artifax: no binary found${why}; ${REMEDY}"
            log_failure "no binary found${why}" 0
            exit 0
        fi
        install_artifax
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
            log "usage: ensure-artifax.sh [exec <args...>]"
            exit 2
            ;;
    esac
}

main "$@"
