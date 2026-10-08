#!/usr/bin/env bash
# Installs a released clax into ~/.local/bin, for people who want the `clax`
# command on PATH without a checkout (with a checkout, run `just install`
# instead). The agent plugins do not need it: they download the release they
# pin into ~/.clax/bin themselves.
#
# Usage: install.sh [version]      (default: the latest release)
#   curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
#
# It downloads clax-<version>-<target>.tar.gz and SHA256SUMS from the release,
# checks the archive's checksum (SHA256SUMS comes from the same place, so this
# protects integrity, not authenticity) and the binary's version, and moves it
# into place in one rename. The download works only while the GitHub
# repository is public; until then GitHub answers 404.
#
# It installs per user and refuses to run as root (as with `| sudo bash`),
# unless CLAX_ALLOW_ROOT=1 and CLAX_INSTALL_DIR are both set.
#
# Environment:
#   CLAX_INSTALL_DIR          where to install (default ~/.local/bin)
#   CLAX_ALLOW_ROOT           1 to install as root, into CLAX_INSTALL_DIR
#   CLAX_RELEASE_BASE_URL     release download base (files come from
#                             <base>/v<version>/); for tests
#   CLAX_RELEASE_LATEST_URL   the URL that redirects to the latest release's
#                             tag; for tests
#   CLAX_DOWNLOAD_TIMEOUT     seconds each download may take (default 300)
set -euo pipefail

REPO="empathic/clax"
BASE="${CLAX_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
LATEST_URL="${CLAX_RELEASE_LATEST_URL:-https://github.com/${REPO}/releases/latest}"
INSTALL_DIR="${CLAX_INSTALL_DIR:-$HOME/.local/bin}"
TIMEOUT="${CLAX_DOWNLOAD_TIMEOUT:-300}"
TMP=""
STAGED=""
cleanup() {
    if [ -n "$STAGED" ]; then rm -f "$STAGED"; fi
    if [ -n "$TMP" ]; then rm -rf "$TMP"; fi
}
trap cleanup EXIT

die() { echo "clax install: $*" >&2; exit 1; }

target() {
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
        Darwin/x86_64)
            # A shell under Rosetta reports x86_64 on Apple silicon.
            if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
                echo aarch64-apple-darwin
            else
                echo x86_64-apple-darwin
            fi
            ;;
        Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-musl ;;
        Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-musl ;;
        *) die "there is no prebuilt clax for $(uname -s)/$(uname -m); build it from a checkout with \`just install\`" ;;
    esac
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

# Downloads $1 to $2, naming the failure.
fetch() {
    local code rc=0
    code="$(curl -sSL --connect-timeout 10 --max-time "$TIMEOUT" -o "$2" -w '%{http_code}' "$1" 2>/dev/null)" || rc=$?
    case "$rc" in
        0) ;;
        28) die "downloading $1 timed out after ${TIMEOUT}s" ;;
        18) die "the download of $1 was cut short" ;;
        6 | 7) die "cannot reach $1" ;;
        *) die "downloading $1 failed (curl exit $rc)" ;;
    esac
    [ "$code" = 200 ] || die "$1 answered HTTP $code (the release may not exist, or the repository may not be public yet)"
}

# Refuses root unless CLAX_ALLOW_ROOT=1 names an explicit CLAX_INSTALL_DIR.
check_user() {
    [ "$(id -u)" = 0 ] || return 0
    if [ "${CLAX_ALLOW_ROOT:-}" = 1 ] && [ -n "${CLAX_INSTALL_DIR:-}" ]; then return 0; fi
    die "clax installs per user and will not run as root (as with \`| sudo bash\`); run it without sudo, or set CLAX_ALLOW_ROOT=1 and CLAX_INSTALL_DIR to install for every user"
}

main() {
    local version="${1:-}" t name expected actual url
    check_user
    for c in curl tar awk; do command -v "$c" >/dev/null 2>&1 || die "$c is required"; done
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || die "sha256sum or shasum is required"
    t="$(target)"
    if [ -z "$version" ]; then
        url="$(curl -sSL -o /dev/null -w '%{url_effective}' --connect-timeout 10 --max-time 30 "$LATEST_URL" 2>/dev/null)" \
            || die "cannot reach $LATEST_URL"
        version="${url##*/}"
        case "$version" in v[0-9]*) ;; *) die "could not find the latest release at $LATEST_URL (the repository may not be public yet)" ;; esac
    fi
    version="${version#v}"
    name="clax-$version-$t"
    TMP="$(mktemp -d)"
    echo "clax install: downloading clax $version ($t)"
    fetch "$BASE/v$version/SHA256SUMS" "$TMP/SHA256SUMS"
    fetch "$BASE/v$version/$name.tar.gz" "$TMP/$name.tar.gz"
    expected="$(awk -v f="$name.tar.gz" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; exit }' "$TMP/SHA256SUMS")"
    [ -n "$expected" ] || die "SHA256SUMS of v$version does not list $name.tar.gz"
    actual="$(sha256 "$TMP/$name.tar.gz")"
    [ "$actual" = "$expected" ] || die "checksum mismatch for $name.tar.gz (SHA256SUMS says $expected, the download is $actual); nothing was installed"
    mkdir "$TMP/x"
    tar -xzf "$TMP/$name.tar.gz" -C "$TMP/x" 2>/dev/null || die "$name.tar.gz could not be unpacked"
    [ -f "$TMP/x/$name/clax" ] || die "$name.tar.gz does not hold $name/clax"
    # Staged and checked in the install directory itself, so a noexec /tmp
    # does not matter and the final step is a same-directory rename.
    mkdir -p "$INSTALL_DIR"
    STAGED="$INSTALL_DIR/.clax.$$"
    cp "$TMP/x/$name/clax" "$STAGED"
    chmod 755 "$STAGED"
    [ "$("$STAGED" --version 2>/dev/null | awk 'NR == 1')" = "clax $version" ] \
        || die "$name.tar.gz does not hold clax $version"
    mv -f "$STAGED" "$INSTALL_DIR/clax"
    STAGED=""
    echo "clax install: installed clax $version at $INSTALL_DIR/clax"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) echo "clax install: $INSTALL_DIR is not on PATH; add it: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
    esac
    echo "clax install: to register this clax's plugins with Claude Code, Codex, Grok and Pi and have them run this binary, run \`clax init\`"
}

main "$@"
