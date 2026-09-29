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
#   5. Download the latest GitHub release, verify its checksum, and install
#      to ~/.local/bin — or to ~/.artifax/bin when an unrelated binary
#      named `artifax` would shadow the ~/.local/bin name.
#
# Environment variables:
#   ARTIFAX_BIN           Absolute path to a specific `artifax` to use, ahead
#                         of every other candidate. Intended for running a
#                         build from a working tree (target/release/artifax)
#                         without installing it. Set but unusable is a
#                         warning, not a silent fall-through.
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

log() { echo "$@" >&2; }

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
        log "warning: \$ARTIFAX_BIN is set to '${ARTIFAX_BIN}' but is not a usable Artifax CLI; ignoring it."
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
    return 1
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
    check_dependencies
    local target version dest_dir
    target="$(resolve_target)"
    version="$(fetch_latest_version)"
    dest_dir="$(choose_install_dir)"

    log "Installing artifax ${version} (${target}) to ${dest_dir}..."
    TMPDIR_CLEANUP="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR_CLEANUP"' EXIT

    download_and_verify "$version" "$target" "$TMPDIR_CLEANUP"
    mkdir -p "$dest_dir"
    mv "${TMPDIR_CLEANUP}/artifax" "${dest_dir}/artifax"
    chmod +x "${dest_dir}/artifax"

    log "Installed artifax to ${dest_dir}/artifax"
    path_hint "$dest_dir"
    RESOLVED_BIN="${dest_dir}/artifax"
}

main() {
    local bin
    if ! bin="$(resolve_existing)"; then
        install_artifax
        bin="$RESOLVED_BIN"
    fi
    warn_if_old "$bin"

    case "${1:-}" in
        exec)
            shift
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
