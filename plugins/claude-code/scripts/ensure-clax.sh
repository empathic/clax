#!/usr/bin/env bash
# Runs the clax binary for the Clax plugins, installing the Clax release this
# plugin pins when that is the one to run, and says why when it cannot.
#
# Usage:
#   ensure-clax.sh                   print the path of the clax that would run
#   ensure-clax.sh exec mcp <args>   run the MCP server
#   ensure-clax.sh exec hook <args>  run a hook (always exits 0)
#   ensure-clax.sh exec <args>       run any other clax command
#   ensure-clax.sh pinned-version    print PINNED_VERSION (empty when none)
#
# The binary, in this order (PATH is never consulted):
#   1. $CLAX_BIN, when set. It must then be a usable clax (an executable
#      file whose --version names clax); otherwise that is the error, with
#      no fall-through.
#   2. The `bin` setting in $CLAX_HOME/config.toml ($CLAX_HOME defaults to
#      ~/.clax), which `clax bin set` (and `clax init`) writes as one line,
#      `bin = "<absolute path>"`, before the file's first [table]. Only that
#      exact form is read; any other `bin` line there, or a binary that is
#      not a usable clax, is the error.
#   3. The managed install of the pinned release:
#      $CLAX_HOME/bin/<PINNED_VERSION>/clax, used when its sha256 matches
#      the one recorded beside it (clax.sha256) and its --version is then
#      exactly `clax <PINNED_VERSION>`. The hash is checked before the
#      binary runs. Otherwise the release's
#      clax-<version>-<target>.tar.gz is downloaded, checked against the
#      SHA256 embedded below for this platform, unpacked into a staging
#      directory inside $CLAX_HOME/bin, its binary's sha256 recorded there,
#      and renamed to $CLAX_HOME/bin/<PINNED_VERSION> (concurrent installs
#      settle on one). After a successful install, the N.N.N directories
#      older than the pin are removed, except the newest of them: a daemon
#      started by the previous plugin may still run from it, and a failed
#      upgrade restarts that daemon's executable (crates/clax-cli/src/
#      client.rs). A newer version is never removed. A failed download or
#      checksum installs and removes nothing. With no pinned release, this
#      step is the error, and it says how to name a binary.
#
# Hooks never download: with the managed install missing they read their
# input, log one `install-pending` line and exit 0 silently. The MCP server
# downloads in a background process of its own process group, which
# finishes even if the harness gives up, and waits up to
# MCP_INSTALL_WAIT_SECS for it; a download still running then is reported
# by the fallback server below, whose status tool says once it has
# finished. Other modes download in the foreground, with progress on
# stderr.
#
# MCP mode first runs `clax mcp <args> --preflight` (the home, config.toml and
# port; no daemon), then execs `clax mcp`, so the harness is its parent. With
# no binary, or when the preflight fails, it answers the MCP client itself
# with a minimal server whose one tool, `status`, states the reason. A clax
# that exits later in the session is not relayed: the client sees the
# connection close. With no binary, hook mode prints one line and exits 0,
# and other modes print the reason and exit 1. A binary named by CLAX_BIN or
# the `bin` setting whose version is not $CLAX_VERSION (this plugin's) runs;
# MCP and CLI modes warn about it, hooks stay silent.
#
# In a Grok Build session, Clax acts only through --agent grok (the
# clax-grok plugin); Grok also loads this Claude Code plugin. A run with
# --agent claude that Grok started stands down before any clax is looked
# for: a hook (GROK_HOOK_EVENT set) reads its input and exits 0 silently;
# the MCP server (GROK_SESSION_ID set, and CLAUDE_PID not this script's
# parent) answers with a minimal server whose one tool, status, says so.
#
# Every failure, every install, and every MCP start, appends one line to
# $CLAX_HOME/logs/hooks.log (rotated to hooks.log.1 past 1 MiB).
#
# Environment, besides CLAX_BIN and CLAX_HOME:
#   CLAX_RELEASE_BASE_URL   release download base; files come from
#                           <base>/v<version>/ (default: the GitHub releases
#                           of empathic/clax). The embedded checksums apply
#                           whatever the base.
#   CLAX_DOWNLOAD_TIMEOUT   seconds the download may take (default 300)

set -uo pipefail

# This plugin's Clax version.
CLAX_VERSION="0.3.1"
# The Clax release the plugins run when neither CLAX_BIN nor the `bin`
# setting names a binary, and the sha256 of its archive for each target.
# scripts/pin-release.sh writes them, from the release's SHA256SUMS, into
# every copy of this script. They are embedded rather than fetched so that a
# tampered release is caught at download time. Empty: no release is pinned.
PINNED_VERSION="0.3.1"
SHA256_AARCH64_APPLE_DARWIN="8276760af413ba2163fca7fb0b2e698616a420c3df1c59f52090ec4d662b63ab"
SHA256_X86_64_APPLE_DARWIN="70fdd8490924af344557416082b3d39160ce56bb86d27a956901d6ac502199f7"
SHA256_X86_64_UNKNOWN_LINUX_MUSL="709ba8ba8339972dca51cb86c295ae280fa1ab1d60f440578189e408a7618927"
SHA256_AARCH64_UNKNOWN_LINUX_MUSL="c8ea1256f4870d6fbe006c1732290f990feea7debcbbe5ee0c82af2aa07d72b3"
REPO="empathic/clax"
# What the Claude Code copy's MCP server says when Grok Build runs it; the
# same text as GROK_STANDDOWN in crates/clax-mcp/src/standdown.rs.
GROK_STANDDOWN="This is the Clax plugin for Claude Code, which Grok Build also loads. In Grok, Clax runs from the clax-grok plugin, whose tools are named \`clax_grok__<tool>\` (for example \`clax_grok__publish\`); this server does nothing. If no \`clax_grok\` tools are listed, run \`clax init --agent grok\`. To remove this server from Grok, run \`grok plugin disable clax\`."
LOG_MAX_BYTES=1048576
# How long the MCP server waits for a first-run download before it answers
# with the fallback server (Codex gives an MCP server 10 s to start).
MCP_INSTALL_WAIT_SECS=8
ARGV="$*"

case "${1:-}" in
    "") MODE=print ;;
    pinned-version) echo "$PINNED_VERSION"; exit 0 ;;
    exec)
        case "${2:-}" in
            mcp) MODE=mcp ;;
            hook) MODE=hook ;;
            *) MODE=cli ;;
        esac
        ;;
    *) echo "usage: ensure-clax.sh [exec <clax arguments...> | pinned-version]" >&2; exit 2 ;;
esac
AGENT=-
prev=""
for a in "$@"; do
    if [ "$prev" = --agent ]; then AGENT="$a"; fi
    prev="$a"
done
BIN="" SOURCE="" GOT_VERSION="" WARNING="" REASON="" TRIED="" CHECK_WHY=""
PENDING="" INSTALL_STATE=""
MCP_ARGS=()
# How long `clax --version` and `clax mcp --preflight` may take.
PROBE_SECS=5

# The Clax home, as clax resolves it (a relative CLAX_HOME against the
# working directory); empty when neither CLAX_HOME nor HOME is set.
CLAX_HOME_DIR=""
if [ -n "${CLAX_HOME:-}" ]; then
    case "$CLAX_HOME" in /*) CLAX_HOME_DIR="$CLAX_HOME" ;; *) CLAX_HOME_DIR="$PWD/$CLAX_HOME" ;; esac
elif [ -n "${HOME:-}" ]; then
    CLAX_HOME_DIR="$HOME/.clax"
fi
CONFIG="${CLAX_HOME_DIR:+$CLAX_HOME_DIR/config.toml}"
BIN_ROOT="${CLAX_HOME_DIR:+$CLAX_HOME_DIR/bin}"
SHOWN_HOME="${CLAX_HOME:-~/.clax}"

log() { echo "$@" >&2; }
oneline() { printf '%s' "$1" | tr '\n"' " '"; }

# Appends "<time> $1" to hooks.log, rotating it past LOG_MAX_BYTES. Never fails.
hooks_log() {
    {
        local dir size
        [ -n "$CLAX_HOME_DIR" ] || return 0
        dir="$CLAX_HOME_DIR/logs"
        mkdir -p "$dir" || return 0
        if [ -f "$dir/hooks.log" ]; then
            size="$(wc -c < "$dir/hooks.log" | tr -d ' ')"
            if [ "${size:-0}" -gt "$LOG_MAX_BYTES" ]; then mv -f "$dir/hooks.log" "$dir/hooks.log.1"; fi
        fi
        printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >> "$dir/hooks.log"
    } 2>/dev/null || true
}

# Logs this run's failure, with $1 as its exit status.
fail_line() {
    hooks_log "launcher mode=$MODE agent=$AGENT exit=$1 reason=\"$(oneline "$REASON")\" tried=\"$(oneline "$TRIED")\" argv=\"$(oneline "$ARGV")\""
}

# Runs "$@" with stdin from /dev/null, killing it after $1 seconds. Sets
# RUN_RC (124 when it was killed), RUN_OUT and RUN_ERR (its stdout and
# stderr).
bounded() {
    local limit="$1" dir p w
    shift
    RUN_OUT="" RUN_ERR="" RUN_RC=0
    if ! dir="$(mktemp -d 2>/dev/null)"; then
        RUN_OUT="$("$@" < /dev/null 2>/dev/null)"
        RUN_RC=$?
        return 0
    fi
    "$@" < /dev/null > "$dir/out" 2> "$dir/err" &
    p=$!
    (
        trap 'kill "$s" 2>/dev/null; exit 0' TERM
        sleep "$limit" &
        s=$!
        wait "$s"
        : > "$dir/timed-out"
        kill -KILL "$p"
    ) > /dev/null 2>&1 &
    w=$!
    wait "$p"
    RUN_RC=$?
    # The watcher marks the cut-off before it kills: once it has killed, it
    # may still be running when this wait returns.
    if [ -e "$dir/timed-out" ]; then
        RUN_RC=124
    else
        kill "$w" 2>/dev/null
        wait "$w" 2>/dev/null
    fi
    RUN_OUT="$(cat "$dir/out" 2>/dev/null)"
    RUN_ERR="$(cat "$dir/err" 2>/dev/null)"
    rm -rf "$dir"
}

# The first non-blank line of $1, trimmed, with double quotes as single ones.
first_line() {
    printf '%s\n' "$1" | awk 'NF && !seen { sub(/^[ \t]+/, ""); sub(/[ \t\r]+$/, ""); print; seen = 1 }' | tr '"' "'"
}

# True when $1 is an executable file whose --version names clax; sets
# GOT_VERSION to that line. Otherwise sets CHECK_WHY to what went wrong.
check_bin() {
    GOT_VERSION="" CHECK_WHY=""
    if [ ! -f "$1" ] || [ ! -x "$1" ]; then CHECK_WHY="not an executable file"; return 1; fi
    bounded "$PROBE_SECS" "$1" --version
    GOT_VERSION="$(first_line "$RUN_OUT")"
    if [ "$RUN_RC" = 124 ]; then CHECK_WHY="\`--version\` did not finish within ${PROBE_SECS} s"; return 1; fi
    if [ "$RUN_RC" != 0 ]; then CHECK_WHY="\`--version\` exited $RUN_RC: $(first_line "$RUN_ERR")"; return 1; fi
    case "$GOT_VERSION" in "clax "*) return 0 ;; esac
    CHECK_WHY="\`--version\` printed '$GOT_VERSION', not clax"
    return 1
}

# --- the `bin` setting ---------------------------------------------------------

# Reads the `bin` setting: the lines of config.toml before its first [table]
# that assign the key bin. Returns 1 when there is none. Otherwise sets
# CFG_BIN to the path when there is exactly one such line and it is
# `bin = "<absolute path>"` (no quote, backslash or control character in
# the path), else CFG_BAD to the offending line.
read_config_bin() {
    local lines n line re
    CFG_BIN="" CFG_BAD=""
    [ -n "$CONFIG" ] && [ -f "$CONFIG" ] || return 1
    lines="$(awk '
        { l = $0; sub(/^[ \t]+/, "", l) }
        substr(l, 1, 1) == "[" { exit }
        l ~ /^("bin"|'"'"'bin'"'"'|bin)[ \t]*=/ { print }
    ' "$CONFIG" 2>/dev/null)"
    [ -n "$lines" ] || return 1
    n="$(printf '%s\n' "$lines" | wc -l | tr -d ' ')"
    line="$(printf '%s\n' "$lines" | sed -n 1p)"
    re='^bin = "(/[^"\\]*)"$'
    if [ "$n" = 1 ] && [[ $line =~ $re ]]; then CFG_BIN="${BASH_REMATCH[1]:-}"; fi
    if [ -z "$CFG_BIN" ] || [[ $CFG_BIN =~ [[:cntrl:]] ]]; then
        CFG_BIN=""
        CFG_BAD="$(oneline "$line")"
        if [ "$n" != 1 ]; then CFG_BAD="$CFG_BAD (and $((n - 1)) more)"; fi
    fi
    return 0
}

# --- the managed install ---------------------------------------------------------

sha256_of() {
    if command -v sha256sum > /dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

# This platform's release target; fails when there is none.
release_target() {
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
        *) return 1 ;;
    esac
}

expected_sha256() {
    case "$1" in
        aarch64-apple-darwin) echo "$SHA256_AARCH64_APPLE_DARWIN" ;;
        x86_64-apple-darwin) echo "$SHA256_X86_64_APPLE_DARWIN" ;;
        x86_64-unknown-linux-musl) echo "$SHA256_X86_64_UNKNOWN_LINUX_MUSL" ;;
        aarch64-unknown-linux-musl) echo "$SHA256_AARCH64_UNKNOWN_LINUX_MUSL" ;;
    esac
}

# True when directory $1 holds a valid managed install: a clax whose sha256
# matches the record beside it, then reporting exactly the pinned version.
# The hash is checked before the binary runs. Sets CHECK_WHY otherwise.
valid_install() {
    local bin="$1/clax" recorded actual
    CHECK_WHY=""
    if [ ! -f "$bin" ] || [ ! -x "$bin" ]; then CHECK_WHY="not installed"; return 1; fi
    recorded="$(cat "$1/clax.sha256" 2>/dev/null)" || recorded=""
    case "$recorded" in *[!0-9a-f]* | "") CHECK_WHY="its sha256 record is missing or malformed"; return 1 ;; esac
    if [ "${#recorded}" != 64 ]; then CHECK_WHY="its sha256 record is malformed"; return 1; fi
    actual="$(sha256_of "$bin" 2>/dev/null)" || actual=""
    if [ "$actual" != "$recorded" ]; then CHECK_WHY="its sha256 does not match the one recorded at install"; return 1; fi
    check_bin "$bin" || return 1
    if [ "$GOT_VERSION" != "clax $PINNED_VERSION" ]; then CHECK_WHY="\`--version\` printed '$GOT_VERSION', not 'clax $PINNED_VERSION'"; return 1; fi
}

# Succeeds when $1 and $2 are both N.N.N and $1 is strictly older.
version_older() {
    local a b i
    [[ $1 =~ ^[0-9]{1,9}\.[0-9]{1,9}\.[0-9]{1,9}$ ]] && [[ $2 =~ ^[0-9]{1,9}\.[0-9]{1,9}\.[0-9]{1,9}$ ]] || return 1
    IFS=. read -r -a a <<< "$1"
    IFS=. read -r -a b <<< "$2"
    for i in 0 1 2; do
        if [ $((10#${a[i]})) -lt $((10#${b[i]})) ]; then return 0; fi
        if [ $((10#${a[i]})) -gt $((10#${b[i]})) ]; then return 1; fi
    done
    return 1
}

# Removes the staging and stale directories an interrupted install left
# more than an hour ago (a younger one may be a concurrent install's).
remove_leftovers() {
    [ -n "$BIN_ROOT" ] && [ -d "$BIN_ROOT" ] || return 0
    find "$BIN_ROOT" -mindepth 1 -maxdepth 1 -type d \( -name '.staging.*' -o -name '.stale.*' \) -mmin +60 \
        -exec rm -rf {} + > /dev/null 2>&1 || true
}

# Removes the N.N.N directories older than the pin except the newest of them,
# then leftovers. Nothing else under $BIN_ROOT is touched. Best-effort.
cleanup_old_versions() {
    local dir name keep=""
    for dir in "$BIN_ROOT"/*.*.*; do
        [ -d "$dir" ] || continue
        name="${dir##*/}"
        version_older "$name" "$PINNED_VERSION" || continue
        if [ -z "$keep" ] || version_older "$keep" "$name"; then keep="$name"; fi
    done
    for dir in "$BIN_ROOT"/*.*.*; do
        [ -d "$dir" ] || continue
        name="${dir##*/}"
        if version_older "$name" "$PINNED_VERSION" && [ "$name" != "$keep" ]; then
            rm -rf "$dir" > /dev/null 2>&1 || true
        fi
    done
    remove_leftovers
}

# Downloads, checks and installs the pinned release as the managed install.
# Progress goes to stderr. On failure sets REASON and returns 1, leaving
# nothing installed and nothing removed.
install_managed() {
    local target expected actual base url name staging dest="$BIN_ROOT/$PINNED_VERSION" stale c rc=0 code
    if ! target="$(release_target)"; then
        REASON="there is no prebuilt clax for $(uname -s)/$(uname -m). Build one from a Clax checkout with \`just install\`, or set CLAX_BIN to a clax binary."
        return 1
    fi
    expected="$(expected_sha256 "$target")"
    if [ -z "$expected" ]; then
        REASON="the pinned clax $PINNED_VERSION has no checksum for $target in this plugin. Set CLAX_BIN to a clax binary, or run \`clax bin set <path>\`."
        return 1
    fi
    for c in curl tar; do
        command -v "$c" > /dev/null 2>&1 || { REASON="installing clax $PINNED_VERSION needs $c, which is not on PATH."; return 1; }
    done
    if ! command -v sha256sum > /dev/null 2>&1 && ! command -v shasum > /dev/null 2>&1; then
        REASON="installing clax $PINNED_VERSION needs sha256sum or shasum, and neither is on PATH."
        return 1
    fi
    name="clax-$PINNED_VERSION-$target"
    base="${CLAX_RELEASE_BASE_URL:-https://github.com/$REPO/releases/download}"
    url="$base/v$PINNED_VERSION/$name.tar.gz"
    # Staging lives inside $BIN_ROOT, so the final rename stays on one
    # filesystem; its dot-prefixed name never looks like a version.
    if ! mkdir -p "$BIN_ROOT" 2>/dev/null || ! staging="$(mktemp -d "$BIN_ROOT/.staging.XXXXXX" 2>/dev/null)"; then
        REASON="cannot create a directory in $BIN_ROOT to install clax $PINNED_VERSION into."
        return 1
    fi
    log "clax: downloading clax $PINNED_VERSION ($target) from $url"
    code="$(curl -sSL --connect-timeout 10 --max-time "${CLAX_DOWNLOAD_TIMEOUT:-300}" -o "$staging/$name.tar.gz" -w '%{http_code}' "$url" 2>/dev/null)" || rc=$?
    if [ "$rc" != 0 ] || [ "$code" != 200 ]; then
        case "$rc" in
            0) REASON="downloading $url answered HTTP $code." ;;
            28) REASON="downloading $url timed out after ${CLAX_DOWNLOAD_TIMEOUT:-300} s." ;;
            6 | 7) REASON="cannot reach $url." ;;
            *) REASON="downloading $url failed (curl exit $rc)." ;;
        esac
        REASON="$REASON Check the network and try again, or set CLAX_BIN to a clax binary."
        rm -rf "$staging"
        return 1
    fi
    actual="$(sha256_of "$staging/$name.tar.gz")"
    if [ "$actual" != "$expected" ]; then
        REASON="the download of $name.tar.gz does not match the checksum this plugin pins (expected $expected, got $actual); nothing was installed."
        rm -rf "$staging"
        return 1
    fi
    if ! tar -xzf "$staging/$name.tar.gz" -C "$staging" 2>/dev/null || [ ! -f "$staging/$name/clax" ]; then
        REASON="$name.tar.gz does not hold $name/clax; nothing was installed."
        rm -rf "$staging"
        return 1
    fi
    mv "$staging/$name/clax" "$staging/clax" && rm -rf "${staging:?}/$name" "$staging/$name.tar.gz"
    chmod 755 "$staging/clax"
    sha256_of "$staging/clax" > "$staging/clax.sha256"
    if ! valid_install "$staging"; then
        REASON="the downloaded clax for $target does not run as clax $PINNED_VERSION ($CHECK_WHY); nothing was installed."
        rm -rf "$staging"
        return 1
    fi
    if [ -e "$dest" ] && ! valid_install "$dest"; then
        # A damaged or partial install: set it aside so the rename below can
        # put this one in its place. A concurrent install may have replaced
        # it since, so it is checked again just before the move.
        if stale="$(mktemp -d "$BIN_ROOT/.stale.XXXXXX" 2>/dev/null)"; then
            if ! valid_install "$dest"; then mv "$dest" "$stale/" > /dev/null 2>&1 || true; fi
            rm -rf "$stale" > /dev/null 2>&1 || true
        fi
    fi
    if [ ! -e "$dest" ]; then mv "$staging" "$dest" 2>/dev/null || true; fi
    # When a concurrent install renamed its copy into place first, mv nested
    # this one inside it (or left it where it was): discard it either way.
    rm -rf "${dest:?}/${staging##*/}" "$staging"
    if ! valid_install "$dest"; then
        # A concurrent install's rename may land just after ours failed.
        sleep 1
        if ! valid_install "$dest"; then
            REASON="could not install clax $PINNED_VERSION to $dest ($CHECK_WHY)."
            return 1
        fi
    fi
    cleanup_old_versions
    log "clax: installed clax $PINNED_VERSION at $dest/clax"
    return 0
}

# Starts install_managed in a background process of its own process group,
# its stdio detached, so it finishes even when the harness stops waiting for
# this script. Its exit status and reason land in $INSTALL_STATE/rc and
# $INSTALL_STATE/reason.
start_background_install() {
    INSTALL_STATE="$(mktemp -d 2>/dev/null)" || { INSTALL_STATE=""; return 1; }
    set -m
    (
        set +m
        install_managed > /dev/null 2>&1
        rc=$?
        printf '%s' "$REASON" > "$INSTALL_STATE/reason"
        hooks_log "install mode=$MODE agent=$AGENT version=$PINNED_VERSION exit=$rc reason=\"$(oneline "$REASON")\""
        printf '%s' "$rc" > "$INSTALL_STATE/rc.tmp" && mv "$INSTALL_STATE/rc.tmp" "$INSTALL_STATE/rc"
    ) < /dev/null > /dev/null 2>&1 &
    set +m
}

# Waits up to $1 seconds for the background install to finish. True when it
# finished; then the install succeeded when $INSTALL_STATE/rc is 0.
wait_background_install() {
    local i=0 limit=$(( $1 * 10 ))
    while [ ! -e "$INSTALL_STATE/rc" ]; do
        [ "$i" -lt "$limit" ] || return 1
        sleep 0.1
        i=$((i + 1))
    done
}

# --- resolution --------------------------------------------------------------------

# Sets BIN, SOURCE (env, config or managed), GOT_VERSION and TRIED. With $1
# = install, a missing or invalid managed install is downloaded (in the
# background in MCP mode, see above); otherwise it sets PENDING. On failure
# sets REASON and returns 1.
resolve() {
    local may="${1:-}" dest why
    TRIED="" PENDING=""
    if [ -n "${CLAX_BIN:-}" ]; then
        if check_bin "$CLAX_BIN"; then
            BIN="$CLAX_BIN" SOURCE=env
            TRIED="CLAX_BIN=$CLAX_BIN: $GOT_VERSION"
            return 0
        fi
        TRIED="CLAX_BIN=$CLAX_BIN: $CHECK_WHY"
        REASON="CLAX_BIN is set to '$CLAX_BIN', which is not a usable clax binary ($CHECK_WHY). Unset CLAX_BIN, or point it at a clax binary."
        return 1
    fi
    if [ -z "$CLAX_HOME_DIR" ]; then
        TRIED="no CLAX_HOME or HOME"
        REASON="neither CLAX_HOME nor HOME is set, so there is no Clax home to read the bin setting from or install clax into. Set HOME, or set CLAX_BIN to a clax binary."
        return 1
    fi
    if read_config_bin; then
        if [ -n "$CFG_BAD" ]; then
            TRIED="$CONFIG: bin line not as clax writes it: $CFG_BAD"
            REASON="$CONFIG sets bin in a form the plugins do not read: $CFG_BAD. The plugins read exactly one line, bin = \"<absolute path>\", before the first [table], with no quote or backslash in the path; \`clax bin set <path>\` writes it. Fix or remove that line."
            return 1
        fi
        if check_bin "$CFG_BIN"; then
            BIN="$CFG_BIN" SOURCE=config
            TRIED="$CONFIG bin=$CFG_BIN: $GOT_VERSION"
            return 0
        fi
        TRIED="$CONFIG bin=$CFG_BIN: $CHECK_WHY"
        REASON="$CONFIG sets bin = \"$CFG_BIN\", which is not a usable clax binary ($CHECK_WHY). Run \`clax bin set <path>\` with a clax binary, or remove that line to use the release this plugin pins."
        return 1
    fi
    if [ -z "$PINNED_VERSION" ]; then
        TRIED="no CLAX_BIN; no bin setting in $CONFIG; no pinned release"
        REASON="this plugin pins no Clax release yet, and neither CLAX_BIN nor the bin setting in $SHOWN_HOME/config.toml names a clax binary. Build and register one from a Clax checkout with \`just install\`, or run \`clax bin set <absolute path of a clax>\`, or set CLAX_BIN to one."
        return 1
    fi
    dest="$BIN_ROOT/$PINNED_VERSION"
    if valid_install "$dest"; then
        BIN="$dest/clax" SOURCE=managed
        TRIED="managed $dest/clax: $GOT_VERSION"
        remove_leftovers
        return 0
    fi
    why="$CHECK_WHY"
    TRIED="managed $dest/clax: $why"
    if [ "$may" != install ]; then
        PENDING="$why"
        REASON="clax $PINNED_VERSION, which this plugin runs, is not installed in $dest yet ($why); the clax MCP server installs it when it starts."
        return 1
    fi
    if [ "$MODE" = mcp ]; then
        start_background_install || { REASON="cannot create a temporary directory to install clax $PINNED_VERSION."; return 1; }
        if ! wait_background_install "$MCP_INSTALL_WAIT_SECS"; then
            PENDING=downloading
            REASON="clax $PINNED_VERSION is still downloading (the first start installs it in $dest). Call status again in a moment."
            return 1
        fi
        if [ "$(cat "$INSTALL_STATE/rc" 2>/dev/null)" != 0 ]; then
            REASON="$(cat "$INSTALL_STATE/reason" 2>/dev/null)"
            REASON="could not install clax $PINNED_VERSION: ${REASON:-unknown error}"
            return 1
        fi
    else
        if ! install_managed; then
            hooks_log "install mode=$MODE agent=$AGENT version=$PINNED_VERSION exit=1 reason=\"$(oneline "$REASON")\""
            REASON="could not install clax $PINNED_VERSION: $REASON"
            return 1
        fi
        hooks_log "install mode=$MODE agent=$AGENT version=$PINNED_VERSION exit=0 reason=\"\""
    fi
    if valid_install "$dest"; then
        BIN="$dest/clax" SOURCE=managed
        TRIED="managed $dest/clax (installed now): $GOT_VERSION"
        return 0
    fi
    REASON="clax $PINNED_VERSION was installed in $dest but does not check out ($CHECK_WHY)."
    return 1
}

json_string() {
    local s="$1" out="" c i
    for (( i = 0; i < ${#s}; i++ )); do
        c="${s:i:1}"
        case "$c" in
            '"') out="$out\\\"" ;;
            '\') out="$out\\\\" ;;
            $'\n') out="$out\\n" ;;
            $'\t') out="$out\\t" ;;
            $'\r') out="$out\\r" ;;
            [[:cntrl:]]) ;;
            *) out="$out$c" ;;
        esac
    done
    printf '"%s"' "$out"
}
# Prints a JSON-RPC message's top-level "method" and "id" and its
# params.protocolVersion, as raw JSON separated by \037 (empty when absent).
# Keys nested deeper, such as a tool call's arguments, are skipped.
parse_request() {
    printf '%s\n' "$1" | awk '
    function emit(k, raw) {
        if (depth == 1 && k == "id") id = raw
        else if (depth == 1 && k == "method") method = raw
        else if (depth == 2 && parent[2] == "params" && k == "protocolVersion") proto = raw
    }
    {
        n = length($0); depth = 0; instr = 0; esc = 0; tok = ""; scal = ""
        wantkey = 0; expect = 0; key = ""; id = ""; method = ""; proto = ""
        for (i = 1; i <= n; i++) {
            c = substr($0, i, 1)
            if (instr) {
                tok = tok c
                if (esc) esc = 0
                else if (c == "\\") esc = 1
                else if (c == "\"") {
                    instr = 0
                    if (wantkey) { key = substr(tok, 2, length(tok) - 2); wantkey = 0 }
                    else if (expect) { emit(key, tok); expect = 0 }
                }
                continue
            }
            if (scal != "" && (c == "," || c == "}" || c == "]" || c == " " || c == "\t" || c == "\r")) {
                emit(key, scal); scal = ""; expect = 0
            }
            if (c == "\"") { instr = 1; tok = c }
            else if (c == "{" || c == "[") {
                depth++; type[depth] = c; parent[depth] = expect ? key : ""
                wantkey = (c == "{"); expect = 0
            }
            else if (c == "}" || c == "]") { depth--; wantkey = 0; expect = 0 }
            else if (c == ":") expect = 1
            else if (c == ",") { wantkey = (type[depth] == "{"); expect = 0 }
            else if (expect && c != " " && c != "\t" && c != "\r") scal = scal c
        }
        printf "%s\037%s\037%s\n", method, id, proto
    }'
}
reply() { printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"; }

# The status tool's text: the reason, or, once the cause is gone (a clax is
# now available, the download has finished, or the preflight now passes),
# that it is fixed.
status_text() {
    local found rc
    if [ -z "$BIN" ]; then
        if [ -n "$INSTALL_STATE" ] && [ -e "$INSTALL_STATE/rc" ] && [ "$(cat "$INSTALL_STATE/rc" 2>/dev/null)" != 0 ]; then
            rc="$(cat "$INSTALL_STATE/reason" 2>/dev/null)"
            echo "Clax is unavailable: could not install clax $PINNED_VERSION: ${rc:-unknown error} Reconnect the clax MCP server (/mcp in Claude Code), or start a new session, to try again."
            return 0
        fi
        if found="$(resolve > /dev/null 2>&1 && echo "$BIN")" && [ -n "$found" ]; then
            echo "clax is now available at $found. Reconnect the clax MCP server (/mcp in Claude Code), or start a new session, to use it."
            return 0
        fi
    elif preflight; then
        echo "clax can start now. Reconnect the clax MCP server (/mcp in Claude Code), or start a new session, to use it."
        return 0
    fi
    echo "$1"
}

# True when Grok Build started this Claude Code copy's hook or MCP server.
# Claude Code sets CLAUDE_PID to its own PID and starts MCP servers
# directly, so under Claude Code CLAUDE_PID is this script's parent, even
# when Claude Code was started from a Grok shell that passed on
# GROK_SESSION_ID. Grok sets GROK_HOOK_EVENT on every hook it runs.
grok_runs_claude_copy() {
    [ "$AGENT" = claude ] || return 1
    case "$MODE" in
        hook) [ -n "${GROK_HOOK_EVENT:-}" ] ;;
        mcp) [ -n "${GROK_SESSION_ID:-}" ] && [ "${CLAUDE_PID:-}" != "$PPID" ] ;;
        *) return 1 ;;
    esac
}

# A minimal MCP server on stdin/stdout whose one tool, status, is described
# by $2. Its instructions are $1. With $3 = true, a status call is an error
# that states $1, or that the cause is gone (status_text); with $3 = false
# it returns $1. Answers until stdin closes.
serve_status() {
    local text="$1" desc="$2" is_error="$3" line method id proto out
    while IFS= read -r line || [ -n "$line" ]; do
        IFS=$'\037' read -r method id proto <<EOF
$(parse_request "$line")
EOF
        # Requests only: a string method and a string or number ID.
        case "$method" in \"*\") method="${method#\"}"; method="${method%\"}" ;; *) continue ;; esac
        case "$id" in \"*\" | -[0-9]* | [0-9]*) ;; *) continue ;; esac
        case "$proto" in \"*\") ;; *) proto='"2025-06-18"' ;; esac
        case "$method" in
            initialize)
                reply "$id" "{\"protocolVersion\":$proto,\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"clax\",\"version\":\"$CLAX_VERSION\"},\"instructions\":$(json_string "$text")}"
                ;;
            tools/list)
                reply "$id" "{\"tools\":[{\"name\":\"status\",\"description\":$(json_string "$desc"),\"inputSchema\":{\"type\":\"object\",\"properties\":{}}}]}"
                ;;
            tools/call)
                if [ "$is_error" = true ]; then out="$(status_text "$text")"; else out="$text"; fi
                reply "$id" "{\"content\":[{\"type\":\"text\",\"text\":$(json_string "$out")}],\"isError\":$is_error}"
                ;;
            ping) reply "$id" "{}" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":%s}}\n' "$id" "$(json_string "$text")" ;;
        esac
    done
}

# Clax cannot run: the reason, and the fix.
serve_unavailable() {
    serve_status "Clax is unavailable: $REASON (Details: $SHOWN_HOME/logs/hooks.log.)" \
        "Clax could not start. Call this tool for the reason and the fix." true
}

# This Claude Code copy in a Grok Build session: clax-grok acts instead.
serve_standdown() {
    serve_status "$GROK_STANDDOWN" "Says which Clax plugin serves this Grok session; this server does nothing else." false
}

# Runs a hook with the binary and always exits 0: a hook must never fail its
# harness. Its stderr passes through; its stdout only when it exited 0. A
# non-zero exit is logged with the end of its stderr.
run_hook() {
    local bin="$1" out rc errfile tail=""
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
        REASON="clax exited ${rc}: ${tail}"
        fail_line "$rc"
    fi
    exit 0
}

# Runs `clax <MCP_ARGS> --preflight`. True when it passes, or when the binary
# predates --preflight; otherwise sets REASON.
preflight() {
    bounded "$PROBE_SECS" "$BIN" ${MCP_ARGS[@]+"${MCP_ARGS[@]}"} --preflight
    [ "$RUN_RC" = 0 ] && return 0
    case "$RUN_ERR" in *"'--preflight'"*) return 0 ;; esac
    if [ "$RUN_RC" = 124 ]; then
        REASON="\`clax mcp --preflight\` did not finish within ${PROBE_SECS} s."
    else
        local why
        why="$(first_line "$RUN_ERR")"
        why="${why#error: }"
        REASON="clax cannot start its MCP server: ${why:-\`clax mcp --preflight\` exited $RUN_RC}. Fix that, then reconnect the clax MCP server (/mcp in Claude Code) or start a new session."
    fi
    return 1
}

main() {
    if grok_runs_claude_copy; then
        hooks_log "standdown mode=$MODE agent=claude host=grok"
        case "$MODE" in
            # Read the hook's input, so Grok's write to stdin never fails.
            hook) cat > /dev/null 2>&1 || true ;;
            *) serve_standdown ;;
        esac
        exit 0
    fi
    local may=install
    if [ "$MODE" = hook ]; then may=""; fi
    if resolve "$may"; then
        if [ "$SOURCE" != managed ] && [ "$GOT_VERSION" != "clax $CLAX_VERSION" ]; then
            WARNING="$BIN is $GOT_VERSION, but this plugin is clax $CLAX_VERSION; build and install the matching clax (\`just install\` in a Clax checkout), or clear the override so the plugin runs the release it pins"
        fi
        case "$MODE" in
            print) echo "$BIN"; exit 0 ;;
            hook) shift; run_hook "$BIN" "$@" ;;
            mcp)
                if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
                hooks_log "launch mode=mcp agent=$AGENT bin=\"$(oneline "$BIN")\" version=\"$GOT_VERSION\" warning=\"$(oneline "$WARNING")\""
                shift
                MCP_ARGS=("$@")
                if preflight; then exec "$BIN" "$@"; fi
                ;;
            *)
                if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
                shift
                exec "$BIN" "$@"
                ;;
        esac
    fi
    case "$MODE" in
        hook)
            # Not installed yet: the MCP server installs it. Silent, so a
            # first session's hooks say nothing while it downloads.
            if [ -n "$PENDING" ]; then
                hooks_log "install-pending mode=hook agent=$AGENT version=$PINNED_VERSION reason=\"$(oneline "$PENDING")\""
                exit 0
            fi
            log "clax: $REASON"
            fail_line 0
            exit 0
            ;;
        mcp)
            # No binary, a download still running, or a failed preflight.
            fail_line fallback
            log "clax: $REASON"
            serve_unavailable
            exit 0
            ;;
        *)
            log "clax: $REASON"
            fail_line 1
            exit 1
            ;;
    esac
}

main "$@"
