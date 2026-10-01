#!/usr/bin/env bash
# Helper functions run through check() and the EXIT trap, which shellcheck
# cannot see.
# shellcheck disable=SC2329
# Checks `clax init` and `clax uninit` against the real `claude`, `codex` and
# `pi` CLIs, entirely inside a scratch root that is deleted on exit:
#
#   scripts/verify-harnesses.sh
#
# Every home the harnesses and Clax read (HOME, CLAX_HOME, CODEX_HOME,
# CLAUDE_CONFIG_DIR, PI_CODING_AGENT_DIR) is set inside the scratch root,
# and the script refuses to run (exit 2) if any of them would fall inside
# your real HOME. No auth file is read or copied, and only non-interactive
# commands that need no login are run. A harness whose CLI is not on PATH
# is skipped. The build under test is $CLAX_BIN when set, else
# target/release/clax, else target/debug/clax; its daemon, if anything
# starts one, listens on a scratch port and is stopped on exit.
#
# The Pi session check needs a model, so it is SKIPPED unless you export
# VERIFY_PI_SESSION=1 and your provider's API key (for example
# ANTHROPIC_API_KEY) yourself.
#
# Prints a PASS/FAIL/SKIPPED table. Exit 0 when nothing failed, 1 when a
# check failed, 2 when it refused to run.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
OLD="arti""fax"   # the previous product name, assembled

refuse() { echo "verify-harnesses: $*" >&2; exit 2; }

[ -n "${HOME:-}" ] || refuse "HOME is not set"
REAL_HOME="$(cd "$HOME" 2>/dev/null && pwd -P)" || refuse "HOME ($HOME) is not a directory"
REAL_HOME_L="$HOME"

# The build under test, as an absolute path.
if [ -n "${CLAX_BIN:-}" ]; then
    [ -x "$CLAX_BIN" ] || refuse "CLAX_BIN ($CLAX_BIN) is not executable"
elif [ -x "$REPO/target/release/clax" ]; then
    CLAX_BIN="$REPO/target/release/clax"
elif [ -x "$REPO/target/debug/clax" ]; then
    CLAX_BIN="$REPO/target/debug/clax"
else
    refuse "no build under test: run \`cargo build -p clax-cli\` first"
fi
CLAX_BIN="$(cd "$(dirname "$CLAX_BIN")" && pwd -P)/$(basename "$CLAX_BIN")"

# Whether $1 is the real HOME or inside it, physically or as written.
inside_real_home() {
    case "$1/" in
        "$REAL_HOME"/* | "$REAL_HOME_L"/*) return 0 ;;
    esac
    return 1
}

ROOT="$(mktemp -d "${TMPDIR:-/tmp}/clax-verify.XXXXXX")"
ROOT="$(cd "$ROOT" && pwd -P)"
S_HOME="$ROOT/home"
S_CLAX="$S_HOME/.clax"
S_CODEX="$S_HOME/.codex"
S_CLAUDE="$S_HOME/.claude"
S_PI="$S_HOME/.pi/agent"
for d in "$ROOT" "$S_HOME" "$S_CLAX" "$S_CODEX" "$S_CLAUDE" "$S_PI"; do
    if inside_real_home "$d"; then
        rm -rf "$ROOT"
        refuse "the scratch directory $d would be inside your real HOME ($REAL_HOME); set TMPDIR to a directory outside it"
    fi
done
mkdir -p "$S_CLAX" "$S_CODEX" "$S_CLAUDE" "$S_PI" "$ROOT/empty" "$ROOT/other"

export HOME="$S_HOME" CLAX_HOME="$S_CLAX" CODEX_HOME="$S_CODEX" \
    CLAUDE_CONFIG_DIR="$S_CLAUDE" PI_CODING_AGENT_DIR="$S_PI" CLAX_BIN
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME
cleanup() {
    "$CLAX_BIN" stop >/dev/null 2>&1 || true
    rm -rf "$ROOT"
}
trap cleanup EXIT
# Harness CLIs read project settings from their working directory too.
cd "$S_HOME"

# A scratch port for any daemon of the scratch home, never 7480 or 7481.
PORT=$((20000 + RANDOM % 30000))
printf '[serve]\nport = %s\n' "$PORT" > "$S_CLAX/config.toml"

CLAX_DIR="$(dirname "$CLAX_BIN")"
export PATH="$CLAX_DIR:$PATH:$REPO/plugins/pi/node_modules/.bin"
MK="$S_CLAX/marketplace"

echo "build under test: $CLAX_BIN ($("$CLAX_BIN" --version 2>/dev/null || echo "no version"))"
echo "scratch root: $ROOT (scratch port $PORT)"

RESULTS=()
FAILS=0
record() { # status, name, detail
    RESULTS+=("$1|$2|${3:-}")
    if [ "$1" = FAIL ]; then FAILS=$((FAILS + 1)); fi
}
check() { # name, detail on failure, command...
    local name="$1" detail="$2"; shift 2
    if "$@"; then record PASS "$name"; else record FAIL "$name" "$detail"; fi
}
have() { command -v "$1" >/dev/null 2>&1; }
HARNESSES=()
for h in claude codex pi; do
    if have "$h"; then
        HARNESSES+=("$h")
        echo "$h: $(command -v "$h") ($("$h" --version </dev/null 2>/dev/null | head -n 1 || true))"
    else
        record SKIPPED "$h" "not on PATH"
    fi
done
uses() { case " ${HARNESSES[*]+"${HARNESSES[*]}"} " in *" $1 "*) return 0 ;; esac; return 1; }

# Each harness's registry files, concatenated.
reg() {
    case "$1" in
        claude) cat "$S_CLAUDE/settings.json" "$S_CLAUDE/plugins/known_marketplaces.json" "$S_CLAUDE/plugins/installed_plugins.json" 2>/dev/null || true ;;
        codex) cat "$S_CODEX/config.toml" 2>/dev/null || true ;;
        pi) cat "$S_PI/settings.json" 2>/dev/null || true ;;
    esac
}
reg_has() { reg "$1" | grep -qF -- "$2"; }
reg_lacks() { ! reg_has "$1" "$2"; }
# Where the registry names the marketplace: by path for Claude Code and Codex,
# and by the package directory (stored relative) for Pi.
mk_mark() { if [ "$1" = pi ]; then echo "marketplace/plugins/pi"; else echo "$MK"; fi; }
pi_count() { reg pi | grep -oF "marketplace/plugins/pi" | wc -l | tr -d ' '; }
# Runs clax with its output in $ROOT/out/$1; true when it exits 0.
run_clax() {
    local name="$1"; shift
    mkdir -p "$ROOT/out"
    "$CLAX_BIN" "$@" >"$ROOT/out/$name" 2>&1 </dev/null
}
out_has() { grep -qF -- "$2" "$ROOT/out/$1"; }
# Runs clax as `run_clax`, recording PASS, or FAIL with its first lines.
check_run() { # check name, output name, clax arguments...
    local name="$1" out="$2"; shift 2
    if run_clax "$out" "$@"; then
        record PASS "$name"
    else
        record FAIL "$name" "$(head -n 3 "$ROOT/out/$out")"
    fi
}
copy_kept() { [ -d "$MK" ] && grep -q "kept" "$ROOT/out/uninit-pi"; }
codex_seed_kept() {
    grep -qx '# keep this comment' "$S_CODEX/config.toml" && grep -qx '\[marketplaces.other\]' "$S_CODEX/config.toml"
}

# Seed for the Codex check: a comment and another marketplace, which every
# run must keep.
printf '# keep this comment\n[marketplaces.other]\nsource_type = "local"\nsource = "%s"\n' "$ROOT/other" > "$S_CODEX/config.toml"

# Seed: a setup under the previous name, like an old checkout's: a
# marketplace with the previous name, registered, then deleted.
OLDDIR="$ROOT/oldcheckout"
PATH="$ROOT/empty" "$CLAX_BIN" init >/dev/null 2>&1 || true # writes the tree only: no harness on that PATH
[ -d "$MK" ] || { echo "verify-harnesses: clax init did not write $MK" >&2; exit 1; }
mv "$MK" "$OLDDIR"
rm -f "$S_CLAX/registrations.json"
for f in .claude-plugin/marketplace.json .agents/plugins/marketplace.json plugins/claude-code/.claude-plugin/plugin.json plugins/clax/.codex-plugin/plugin.json; do
    sed -i.bak "s/\"name\": *\"clax\"/\"name\": \"$OLD\"/" "$OLDDIR/$f" && rm -f "$OLDDIR/$f.bak"
done
sed -i.bak "s#@empathic/clax-pi#@empathic/$OLD-pi#" "$OLDDIR/plugins/pi/package.json" && rm -f "$OLDDIR/plugins/pi/package.json.bak"
seed() {
    case "$1" in
        claude) claude plugin marketplace add "$OLDDIR" && claude plugin install "$OLD@$OLD" ;;
        codex) codex plugin marketplace add "$OLDDIR" && codex plugin add "$OLD@$OLD" ;;
        pi) pi install "$OLDDIR/plugins/pi" ;;
    esac
}
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    # Pi names a package by its directory, the others by the previous name.
    mark="$OLD"; [ "$h" = pi ] && mark="oldcheckout"
    if seed "$h" </dev/null >"$ROOT/seed-$h.log" 2>&1 && reg_has "$h" "$mark"; then
        record PASS "$h: seed registers the previous name"
    else
        record FAIL "$h: seed registers the previous name" "$(tail -n 1 "$ROOT/seed-$h.log")"
    fi
done
rm -rf "$OLDDIR"

# init
check_run "init exits 0" init1 init
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    check "$h: init registers the marketplace" "registry does not name $(mk_mark "$h")" \
        reg_has "$h" "$(mk_mark "$h")"
done
for h in claude codex; do
    uses "$h" || continue
    check "$h: init removes the previous name's registration" "registry still names $OLD" reg_lacks "$h" "$OLD"
done
if uses codex; then
    check "codex: init keeps other config.toml content" "comment or [marketplaces.other] gone" codex_seed_kept
fi
if uses pi; then
    gone="$OLDDIR/plugins/pi"
    check "pi: the deleted previous-name checkout is left and named" "no \`pi remove $gone\` in init's output" \
        out_has init1 "pi remove $gone"
    pi remove "$gone" </dev/null >/dev/null 2>&1 || true
    check "pi: the named \`pi remove\` clears it" "settings.json still names it" reg_lacks pi "oldcheckout"
fi
CACHED=""
if uses claude; then
    p="$(find "$S_CLAUDE/plugins/cache" -type f -name plugin.json -path '*clax*' 2>/dev/null | head -n 1 || true)"
    if [ -n "$p" ]; then
        CACHED="$(dirname "$(dirname "$p")")"
        touch "$CACHED/verify-marker"
        record PASS "claude: plugin cached"
    else
        record FAIL "claude: plugin cached" "no clax plugin.json under $S_CLAUDE/plugins/cache"
    fi
fi

# init again
check_run "init again exits 0" init2 init
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    check "$h: still registered after init again" "registry does not name $(mk_mark "$h")" \
        reg_has "$h" "$(mk_mark "$h")"
done
if uses pi; then
    check "pi: one entry after init again" "$(pi_count) entries" test "$(pi_count)" = 1
fi
if uses codex; then
    check "codex: init again keeps other config.toml content" "comment or [marketplaces.other] gone" codex_seed_kept
fi
if [ -n "$CACHED" ]; then
    check "claude: init again replaces the cached copy" "the marker in $CACHED survived" test ! -e "$CACHED/verify-marker"
fi

# doctor
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    run_clax "doctor-$h" --json doctor --agent "$h" || true
    for c in binary plugin; do
        check "$h: doctor $c passes" "$(tr ',' '\n' <"$ROOT/out/doctor-$h" | grep -A2 "\"name\":\"$c\"" | head -n 1)" \
            out_has "doctor-$h" "\"name\":\"$c\",\"ok\":true"
    done
done

# A Pi session, which needs a model.
if uses pi && [ "${VERIFY_PI_SESSION:-}" = 1 ]; then
    if pi -p "Call the clax_status tool and print what it returns" </dev/null >"$ROOT/out/pi-session" 2>&1 \
        && grep -qi clax "$ROOT/out/pi-session"; then
        record PASS "pi: a session loads the extension"
    else
        record FAIL "pi: a session loads the extension" "$(tail -n 1 "$ROOT/out/pi-session")"
    fi
elif uses pi; then
    record SKIPPED "pi: a session loads the extension" "needs a model: export your provider's API key and VERIFY_PI_SESSION=1, then re-run"
fi

# uninit of Pi alone, while the others still use the copy.
if uses pi; then
    check_run "uninit --agent pi exits 0" uninit-pi uninit --agent pi
    check "pi: uninit --agent pi removes the entry" "settings.json still names the marketplace" \
        reg_lacks pi "marketplace/plugins/pi"
    if uses claude || uses codex; then
        check "uninit --agent pi keeps the copy the others use" "marketplace deleted, or not reported kept" copy_kept
    fi
fi

# uninit
check_run "uninit exits 0" uninit uninit
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    check "$h: uninit leaves no registration" "registry still names $(mk_mark "$h")" \
        reg_lacks "$h" "$(mk_mark "$h")"
done
check "uninit removes the marketplace copy" "$MK still exists" test ! -e "$MK"
if uses codex; then
    check "codex: uninit keeps other config.toml content" "comment or [marketplaces.other] gone" codex_seed_kept
fi

echo
printf '%-8s %-62s %s\n' STATUS CHECK DETAIL
for r in ${RESULTS[@]+"${RESULTS[@]}"}; do
    IFS='|' read -r st name detail <<EOF
$r
EOF
    printf '%-8s %-62s %s\n' "$st" "$name" "$(printf '%s' "$detail" | tr '\n' ' ')"
done
if [ "$FAILS" -gt 0 ]; then
    echo
    echo "$FAILS check(s) failed. The output of each clax run:"
    for f in "$ROOT"/out/*; do
        [ -e "$f" ] || continue
        echo "== $(basename "$f")"
        cat "$f"
    done
    exit 1
fi
exit 0
