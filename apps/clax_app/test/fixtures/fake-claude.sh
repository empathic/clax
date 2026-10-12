#!/bin/sh
# A stand-in for `claude -p` in ClaudeCliBackend's tests, run as
# `/bin/sh fake-claude.sh <args>` so no new executable is assessed per run.
#
# Environment:
#   FAKE_CLAUDE_LOG     its arguments, one per line, are written here; stdin
#                       goes to "$FAKE_CLAUDE_LOG.stdin" and the file named
#                       after --system-prompt-file is copied to
#                       "$FAKE_CLAUDE_LOG.system"; the working directory
#                       goes to "$FAKE_CLAUDE_LOG.cwd" and the environment
#                       to "$FAKE_CLAUDE_LOG.env".
#   FAKE_CLAUDE_IGNORE_TERM  when 1, SIGTERM is ignored.
#   FAKE_CLAUDE_READY   a FIFO: after logging (and the trap), write a line to
#                       it, so a test knows the fixture is about to block.
#   FAKE_CLAUDE_BLOCK   a FIFO: after logging, block opening it for reading.
#   FAKE_CLAUDE_STDERR  printed to stderr.
#   FAKE_CLAUDE_OUT     a file printed to stdout.
#   FAKE_CLAUDE_EXIT    the exit status (default 0).
set -u
log="${FAKE_CLAUDE_LOG:-/dev/null}"
: > "$log"
system=""
want_system=0
for a in "$@"; do
  printf '%s\n' "$a" >> "$log"
  if [ "$want_system" = 1 ]; then system="$a"; want_system=0; fi
  if [ "$a" = "--system-prompt-file" ]; then want_system=1; fi
done
if [ "$log" != /dev/null ]; then
  cat > "$log.stdin"
  pwd -P > "$log.cwd"
  env > "$log.env"
  if [ -n "$system" ] && [ -f "$system" ]; then cat "$system" > "$log.system"; fi
else
  cat > /dev/null
fi
if [ "${FAKE_CLAUDE_IGNORE_TERM:-}" = 1 ]; then trap '' TERM; fi
if [ -n "${FAKE_CLAUDE_READY:-}" ]; then printf 'ready\n' > "$FAKE_CLAUDE_READY"; fi
if [ -n "${FAKE_CLAUDE_BLOCK:-}" ]; then
  read -r _ < "$FAKE_CLAUDE_BLOCK"
fi
if [ -n "${FAKE_CLAUDE_STDERR:-}" ]; then printf '%s\n' "$FAKE_CLAUDE_STDERR" >&2; fi
if [ -n "${FAKE_CLAUDE_OUT:-}" ]; then cat "$FAKE_CLAUDE_OUT"; fi
exit "${FAKE_CLAUDE_EXIT:-0}"
