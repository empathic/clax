#!/bin/sh
# PostToolUse gate for `clax hook --agent <harness> tool` (spec §13).
# Working records lapse 120 s after their last renewal, so renewing once a
# minute is enough: this starts clax only when this session's stamp file is
# at least 60 s old, and otherwise exits after a cat and two date calls.
# It always exits 0 and prints nothing, so it can never fail or steer the
# harness; clax logs its own failures to hooks.log.
#
# Usage: tool-hook.sh <claude|codex>     (the hook's JSON on stdin)
# Stamp: ${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>
# Portable: `date +%s` and `date -r FILE +%s` (a file's mtime) behave the
# same in BSD date (macOS), GNU coreutils and BusyBox; `stat` and
# `find -mmin` do not.
agent="$1"
case "$agent" in claude|codex) ;; *) exit 0 ;; esac
input="$(cat)"
sid=""
for pat in '"session_id":"' '"session_id": "'; do
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
