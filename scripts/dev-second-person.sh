#!/usr/bin/env bash
# `scripts/dev-second-person.sh <invite> [person-name]`: plays a second
# person on this machine against a dev daemon, to try People (`/people`)
# and the other multi-person surfaces by hand.
#
# Make the invite first: run the dev daemon on the LAN (`just watch --bind
# 0.0.0.0`, or `clax serve --bind 0.0.0.0` on a scratch CLAX_HOME), open
# People as the owner (http://localhost:<port>/people), choose Invite a
# person, and pass the command's URL (http://<LAN IP>:<port>/i/<code>) here.
#
# It makes a scratch Clax home (CLAX_SECOND_HOME, else a new directory under
# $TMPDIR), connects it with the invite as the machine `<name>-laptop`
# (answering yes for the person to the owner's content reaching their
# agents), prints the browser link that makes a browser that person (open
# it at the LAN address, which is a different site from localhost, so the
# owner's browser stays the owner's), and registers one agent of the person
# from a scratch project `<name>-notes`, as Claude Code's SessionStart hook
# does. It never reads or writes ~/.clax or the dev home: the home it uses
# must be neither.
#
# `scripts/dev-second-person.sh --off <home>` disconnects that home (its
# grant ends on the daemon, and its agent with it) and removes it.
#
# CLAX_DEV_BIN=<binary> uses that binary; else target/debug/clax, built
# when missing.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { echo "usage: scripts/dev-second-person.sh <invite URL> [person name] | --off <home>" >&2; exit 2; }
[ $# -ge 1 ] || usage

bin="${CLAX_DEV_BIN:-}"
if [ -z "$bin" ]; then
    [ -x target/debug/clax ] || cargo build -q -p clax-cli --bin clax
    bin="$PWD/target/debug/clax"
fi

# refuse_home <home>: exits unless <home> is a scratch home: not ~/.clax
# (the agents' home), not ~/.clax-dev or $CLAX_HOME (the dev daemon's).
refuse_home() {
    local h dir
    h="$(cd "$1" && pwd -P)"
    for dir in "$HOME/.clax" "$HOME/.clax-dev" "${CLAX_HOME:-}"; do
        [ -n "$dir" ] && [ -d "$dir" ] || continue
        if [ "$h" = "$(cd "$dir" && pwd -P)" ]; then
            echo "dev-second-person: $1 is $dir, not a scratch home; refusing" >&2
            exit 1
        fi
    done
}

if [ "$1" = --off ]; then
    [ $# -eq 2 ] && [ -d "$2" ] || usage
    refuse_home "$2"
    [ -f "$2/remote.json" ] || { echo "dev-second-person: $2 has no connection (remote.json)" >&2; exit 1; }
    CLAX_HOME="$2" "$bin" connect --off
    rm -rf "$2"
    echo "dev-second-person: disconnected and removed $2"
    exit 0
fi

invite="$1"
name="${2:-Sam}"
slug="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9' '-' | sed 's/-*$//')"
[ -n "$slug" ] || slug=person
case "$invite" in
    http://*/i/*) ;;
    "clax connect "*) invite="${invite#clax connect }" ;;
    *) echo "dev-second-person: '$invite' is not an invite (http://<IP>:<port>/i/<code>)" >&2; exit 2 ;;
esac

home="${CLAX_SECOND_HOME:-}"
if [ -z "$home" ]; then
    home="$(mktemp -d "${TMPDIR:-/tmp}/clax-second.XXXXXX")"
else
    mkdir -p "$home"
fi
chmod 700 "$home"
refuse_home "$home"
[ ! -e "$home/remote.json" ] || { echo "dev-second-person: $home is connected already; use another home or --off it" >&2; exit 1; }

# Not under an agent: `clax connect` is the person's to run, never an agent's.
unset CLAX_CHANNEL CLAUDECODE CODEX_SESSION_ID
export CLAX_HOME="$home"
unset CLAX_PORT

echo "dev-second-person: connecting $home as $name's machine $slug-laptop"
out="$("$bin" connect "$invite" --name "$slug-laptop" --yes --no-browser 2>&1)" || { printf '%s\n' "$out" >&2; exit 1; }
printf '%s\n' "$out"

# One agent of the person, registered by its SessionStart hook from a
# scratch project. The hook's join sends the project's repository and
# branch, so the agent's derived name comes from that repository.
project="$home/work/$slug-notes"
mkdir -p "$project/.git"
printf 'ref: refs/heads/main\n' > "$project/.git/HEAD"
sid="dev-second-$(date +%s)"
printf '{"session_id":"%s","cwd":"%s","hook_event_name":"SessionStart","source":"startup"}' "$sid" "$project" |
    (cd "$project" && "$bin" hook --agent claude session-start >/dev/null)
echo "dev-second-person: registered an agent of $name from $project"
"$bin" agents || true

cat <<MSG

Open the browser link above at the LAN address to be $name in a browser,
then People: $name's machine and agent are listed for the owner too.
When done: scripts/dev-second-person.sh --off $home
MSG
