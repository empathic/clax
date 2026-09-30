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
# MCP mode first runs `clax mcp <args> --preflight` (the home, config.toml and
# port; no daemon), then execs `clax mcp`, so the harness is its parent. With
# no binary, or when the preflight fails, it answers the MCP client itself
# with a minimal server whose one tool, `status`, states the reason. A clax
# that exits later in the session is not relayed: the client sees the
# connection close. With no binary, hook mode prints one line and exits 0,
# and other modes print the reason and exit 1. A binary whose version is not
# $CLAX_VERSION (this plugin's) runs; MCP and CLI modes warn about it, hooks
# stay silent.
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
BIN="" GOT_VERSION="" WARNING="" REASON="" TRIED="" CHECK_WHY=""
MCP_ARGS=()
# How long `clax --version` and `clax mcp --preflight` may take.
PROBE_SECS=5

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
        kill -KILL "$p"
    ) > /dev/null 2>&1 &
    w=$!
    wait "$p"
    RUN_RC=$?
    if kill -0 "$w" 2>/dev/null; then
        kill "$w" 2>/dev/null
        wait "$w" 2>/dev/null
    else
        RUN_RC=124
    fi
    RUN_OUT="$(cat "$dir/out" 2>/dev/null)"
    RUN_ERR="$(cat "$dir/err" 2>/dev/null)"
    rm -rf "$dir"
}

# The first non-blank line of $1, trimmed, with double quotes as single ones.
first_line() {
    printf '%s\n' "$1" | awk 'NF { sub(/^[ \t]+/, ""); sub(/[ \t\r]+$/, ""); print; exit }' | tr '"' "'"
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
        TRIED="CLAX_BIN=$CLAX_BIN: $CHECK_WHY"
        REASON="CLAX_BIN is set to '$CLAX_BIN', which is not a usable clax binary ($CHECK_WHY). Unset CLAX_BIN, or point it at a clax binary."
        return 1
    fi
    local bad=""
    for dir in ${PATH:-}; do
        [ -n "$dir" ] && [ -e "$dir/clax" ] || continue
        if check_bin "$dir/clax"; then
            BIN="$dir/clax"
            TRIED="${TRIED:+$TRIED; }$BIN: $GOT_VERSION"
            return 0
        fi
        TRIED="${TRIED:+$TRIED; }$dir/clax: $CHECK_WHY"
        bad="${bad:+$bad; }$dir/clax: $CHECK_WHY"
    done
    TRIED="${TRIED:+$TRIED; }PATH has no clax: ${PATH:-(empty)}"
    local install="with \`just install\` in a Clax checkout (it puts clax in ~/.cargo/bin), or with the release installer (~/.local/bin), and start the harness from a shell whose PATH includes that directory."
    if [ -n "$bad" ]; then
        REASON="no usable clax binary is on PATH ($bad). Reinstall it $install"
    else
        REASON="no clax binary is on PATH. Install it $install"
    fi
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

# The status tool's text: the reason, or, once the cause is gone (a clax has
# appeared on PATH, or the preflight now passes), that it is fixed.
status_text() {
    local found
    if [ -z "$BIN" ]; then
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

# A minimal MCP server on stdin/stdout whose one tool, status, states why clax
# cannot run, so the client and the agent see the reason instead of a closed
# pipe. Answers until stdin closes.
serve_unavailable() {
    local text line method id proto
    text="Clax is unavailable: $REASON (Details: ${CLAX_HOME:-~/.clax}/logs/hooks.log.)"
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
            log "clax: $REASON"
            fail_line 0
            exit 0
            ;;
        mcp)
            # No binary, or a failed preflight.
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
