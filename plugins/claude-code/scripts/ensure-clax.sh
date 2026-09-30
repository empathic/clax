#!/usr/bin/env bash
# Runs the `clax` on PATH for the Clax plugins, and says why when it cannot.
#
# Usage:
#   ensure-clax.sh                   print the path of the clax that would run
#   ensure-clax.sh exec mcp <args>   run the MCP server
#   ensure-clax.sh exec hook <args>  run a hook (always exits 0)
#   ensure-clax.sh exec <args>       run any other clax command
#
# The binary is $CLAX_BIN when set (it must then be a usable clax), else the
# first `clax` on PATH whose --version names clax. This script never
# downloads, builds, or looks anywhere else.
#
# With no binary, or when the MCP server exits with an error (as it does at
# startup over a malformed config.toml), MCP mode answers the MCP client
# itself with a minimal server whose one tool, `status`, states the reason.
# With no binary, hook mode prints one line and exits 0, and other modes
# print the reason and exit 1. A binary whose version is not $CLAX_VERSION
# (this plugin's) runs, with a warning.
#
# Every failure, and every MCP start, appends one line to
# ${CLAX_HOME:-~/.clax}/logs/hooks.log (rotated to hooks.log.1 past 1 MiB).

set -uo pipefail

# This plugin's Clax version; a clax of another version runs with a warning.
CLAX_VERSION="0.2.0"
LOG_MAX_BYTES=1048576
ARGV="$*"

case "${1:-}" in
    "") MODE=print ;;
    exec)
        case "${2:-}" in
            mcp) MODE=mcp ;;
            hook) MODE=hook ;;
            *) MODE=cli ;;
        esac
        ;;
    *) echo "usage: ensure-clax.sh [exec <clax arguments...>]" >&2; exit 2 ;;
esac
AGENT=-
prev=""
for a in "$@"; do
    if [ "$prev" = --agent ]; then AGENT="$a"; fi
    prev="$a"
done
BIN="" GOT_VERSION="" WARNING="" REASON="" TRIED=""

log() { echo "$@" >&2; }
oneline() { printf '%s' "$1" | tr '\n"' " '"; }

# Appends "<time> $1" to hooks.log, rotating it past LOG_MAX_BYTES. Never fails.
hooks_log() {
    {
        local dir size
        if [ -n "${CLAX_HOME:-}" ]; then dir="$CLAX_HOME/logs"
        elif [ -n "${HOME:-}" ]; then dir="$HOME/.clax/logs"
        else return 0; fi
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

# True when $1 is an executable file whose --version names clax; sets
# GOT_VERSION to that line.
check_bin() {
    GOT_VERSION=""
    [ -f "$1" ] && [ -x "$1" ] || return 1
    GOT_VERSION="$("$1" --version 2>/dev/null < /dev/null | head -1)"
    case "$GOT_VERSION" in "clax "*) return 0 ;; *) return 1 ;; esac
}

# Sets BIN (and GOT_VERSION, TRIED); on failure sets REASON and returns 1.
resolve() {
    local dir IFS=:
    TRIED=""
    if [ -n "${CLAX_BIN:-}" ]; then
        if check_bin "$CLAX_BIN"; then
            BIN="$CLAX_BIN"
            TRIED="CLAX_BIN=$CLAX_BIN: $GOT_VERSION"
            return 0
        fi
        TRIED="CLAX_BIN=$CLAX_BIN: not a usable clax"
        REASON="CLAX_BIN is set to '$CLAX_BIN', which is not a usable clax binary. Unset CLAX_BIN, or point it at a clax binary."
        return 1
    fi
    for dir in ${PATH:-}; do
        [ -n "$dir" ] || continue
        if check_bin "$dir/clax"; then
            BIN="$dir/clax"
            TRIED="${TRIED:+$TRIED; }$BIN: $GOT_VERSION"
            return 0
        fi
        if [ -e "$dir/clax" ]; then TRIED="${TRIED:+$TRIED; }$dir/clax: not clax"; fi
    done
    TRIED="${TRIED:+$TRIED; }PATH has no clax: ${PATH:-(empty)}"
    REASON="no clax binary is on PATH. Install it with \`just install\` in a Clax checkout (it puts clax in ~/.cargo/bin), or with the release installer (~/.local/bin), and start the harness from a shell whose PATH includes that directory."
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
json_field() { printf '%s' "$2" | sed -nE "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"([^\"]*)\".*/\\1/p" | head -1; }
reply() { printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"; }

# The status tool's text: the reason, or, when no clax was found and one has
# appeared since, that it is there now.
status_text() {
    local found
    if [ -z "$BIN" ] && found="$(resolve > /dev/null 2>&1 && echo "$BIN")" && [ -n "$found" ]; then
        echo "clax is now available at $found. Reconnect the clax MCP server (/mcp in Claude Code), or start a new session, to use it."
    else
        echo "$1"
    fi
}

# A minimal MCP server on stdin/stdout whose one tool, status, states why clax
# cannot run, so the client and the agent see the reason instead of a closed
# pipe. Answers until stdin closes.
serve_unavailable() {
    local text line method id proto
    text="Clax is unavailable: $REASON (Details: ${CLAX_HOME:-~/.clax}/logs/hooks.log.)"
    while IFS= read -r line || [ -n "$line" ]; do
        method="$(json_field method "$line")"
        id="$(printf '%s' "$line" | sed -nE 's/.*"id"[[:space:]]*:[[:space:]]*("([^"\\]|\\.)*"|-?[0-9]+).*/\1/p' | head -1)"
        [ -n "$method" ] && [ -n "$id" ] || continue
        case "$method" in
            initialize)
                proto="$(json_field protocolVersion "$line")"
                reply "$id" "{\"protocolVersion\":\"${proto:-2025-06-18}\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"clax\",\"version\":\"$CLAX_VERSION\"},\"instructions\":$(json_string "$text")}"
                ;;
            tools/list)
                reply "$id" "{\"tools\":[{\"name\":\"status\",\"description\":$(json_string "Clax could not start. Call this tool for the reason and the fix."),\"inputSchema\":{\"type\":\"object\",\"properties\":{}}}]}"
                ;;
            tools/call)
                reply "$id" "{\"content\":[{\"type\":\"text\",\"text\":$(json_string "$(status_text "$text")")}],\"isError\":true}"
                ;;
            ping) reply "$id" "{}" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":%s}}\n' "$id" "$(json_string "$text")" ;;
        esac
    done
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

# The error clax printed on stderr file $1: from its last line that starts
# with "error:" to the end (else the whole file), on one line, at most 600
# characters.
error_text() {
    local text
    text="$(awk '/^error:/ { buf = "" } { buf = buf $0 " " } END { print buf }' "$1" 2>/dev/null | tr '"' "'")" || text=""
    text="$(printf '%s' "$text" | tr -s ' ')"
    text="${text% }"
    if [ "${#text}" -gt 600 ]; then text="${text:0:600}..."; fi
    printf '%s' "$text"
}

# Runs the MCP server on the client's stdin and stdout. Its stderr passes
# through and is also kept, so that when it exits non-zero (as it does at
# startup over a malformed config.toml) the client gets the reason from the
# fallback server instead of a closed pipe. TERM, INT and HUP are forwarded
# to it, and the wrapper then exits with its status.
run_mcp() {
    local bin="$1" dir pid tee_pid rc i stopping=0
    shift
    dir="$(mktemp -d 2>/dev/null)" || dir=""
    if [ -z "$dir" ] || ! mkfifo "$dir/stderr" 2>/dev/null; then
        if [ -n "$dir" ]; then rm -rf "$dir"; fi
        exec "$bin" "$@"
    fi
    tee "$dir/stderr.log" < "$dir/stderr" >&2 &
    tee_pid=$!
    # An explicit stdin redirection: a background command's stdin would
    # otherwise be /dev/null.
    "$bin" "$@" 0<&0 2>"$dir/stderr" &
    pid=$!
    trap 'stopping=1; kill -TERM "$pid" 2>/dev/null' TERM INT HUP
    while :; do
        wait "$pid"
        rc=$?
        if kill -0 "$pid" 2>/dev/null; then continue; fi
        break
    done
    trap - TERM INT HUP
    # The tee ends when the server's stderr closes; bound the wait in case a
    # process it started still holds it.
    for (( i = 0; i < 40; i++ )); do
        kill -0 "$tee_pid" 2>/dev/null || break
        sleep 0.05
    done
    kill "$tee_pid" 2>/dev/null
    wait "$tee_pid" 2>/dev/null
    if [ "$rc" = 0 ] || [ "$stopping" = 1 ]; then
        rm -rf "$dir"
        exit "$rc"
    fi
    REASON="clax exited ${rc}: $(error_text "$dir/stderr.log")"
    rm -rf "$dir"
    fail_line fallback
    serve_unavailable
    exit 0
}

main() {
    if resolve; then
        if [ "$GOT_VERSION" != "clax $CLAX_VERSION" ]; then
            WARNING="$BIN is $GOT_VERSION, but this plugin is clax $CLAX_VERSION; run \`just install\` (or \`clax init\`) so the plugin and the binary match"
        fi
        case "$MODE" in
            print) echo "$BIN"; exit 0 ;;
            hook) shift; run_hook "$BIN" "$@" ;;
            mcp)
                if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
                hooks_log "launch mode=mcp agent=$AGENT bin=\"$(oneline "$BIN")\" version=\"$GOT_VERSION\" warning=\"$(oneline "$WARNING")\""
                shift
                run_mcp "$BIN" "$@"
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
            log "clax: $REASON"
            fail_line 0
            exit 0
            ;;
        mcp)
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
