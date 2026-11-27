#!/bin/sh
# Gate for `clax hook --agent <harness> tool` (PostToolUse, spec §13) and
# `clax hook --agent antigravity prompt` (Antigravity's PreInvocation).
#
# tool mode (the default): working records lapse 120 s after their last
# renewal, so renewing once a minute is enough: this starts clax only when
# this session's stamp file is at least 60 s old, and otherwise exits after
# a cat and two date calls. It prints nothing.
#
# prompt mode: PreInvocation fires before every model call, and
# `invocationNum` is 0 for the first of a turn, so this starts clax only
# when `invocationNum` is 0 or absent. It keeps no stamp, and passes clax's
# stdout through: that is what Antigravity adds to the turn.
#
# It always exits 0, so it can never fail the harness; clax logs its own
# failures to hooks.log.
#
# Usage: tool-hook.sh <claude|codex|antigravity> [prompt]
#        (the hook's JSON on stdin; the session ID is its "session_id",
#        or Antigravity's "conversationId")
# Stamp: ${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>
# Portable: `date +%s` and `date -r FILE +%s` (a file's mtime) behave the
# same in BSD date (macOS), GNU coreutils and BusyBox; `stat` and
# `find -mmin` do not.
agent="$1"
case "$agent" in claude|codex|antigravity) ;; *) exit 0 ;; esac
input="$(cat)"
if [ "${2:-}" = prompt ]; then
  num=""
  pat='"invocationNum":'
  rest="${input#*"$pat"}"
  if [ "$rest" != "$input" ]; then
    rest="${rest# }"
    num="${rest%%[!0-9]*}"
  fi
  case "$num" in ''|0) ;; *) exit 0 ;; esac
  printf '%s' "$input" | bash "${0%/*}/ensure-clax.sh" exec hook --agent "$agent" prompt 2>/dev/null
  exit 0
fi
key=session_id
[ "$agent" = antigravity ] && key=conversationId
sid=""
for pat in "\"$key\":\"" "\"$key\": \""; do
  rest="${input#*"$pat"}"
  if [ "$rest" != "$input" ]; then
    sid="${rest%%\"*}"
    break
  fi
done
case "$sid" in ''|*[!A-Za-z0-9._-]*) sid="" ;; esac
if [ -n "$sid" ]; then
  dir="${CLAX_HOME:-$HOME/.clax}/run/tool-hook"
  stamp="$dir/$agent-$sid"
  now="$(date +%s)"
  last="$(date -r "$stamp" +%s 2>/dev/null)" || last=0
  age=$((now - last))
  if [ "$age" -ge 0 ] && [ "$age" -lt 60 ]; then
    exit 0
  fi
  # touch, not `: >`: a failed redirection on a special builtin exits sh.
  { mkdir -p "$dir" && touch "$stamp"; } 2>/dev/null
fi
printf '%s' "$input" | bash "${0%/*}/ensure-clax.sh" exec hook --agent "$agent" tool >/dev/null 2>&1
exit 0
