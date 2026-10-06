#!/usr/bin/env bash
# Helper functions run through check() and the traps, which shellcheck
# cannot see.
# shellcheck disable=SC2329
# Checks `clax init` and `clax uninit` against the real `claude`, `codex` and
# `pi` CLIs, entirely inside a scratch root that is deleted on exit:
#
#   scripts/verify-harnesses.sh
#
# The build under test is $CLAX_BIN when set. Otherwise the script first runs
# `cargo build -p clax-cli --bin clax` (honouring CARGO_TARGET_DIR) and tests
# the binary that build names; it never picks up an older build.
#
# Every harness and Clax command then runs under `env -i` with only these
# variables: HOME, CLAX_HOME, CODEX_HOME, CLAUDE_CONFIG_DIR and
# PI_CODING_AGENT_DIR (all inside the scratch root), CLAX_BIN, CLAX_NO_OPEN,
# PATH (the build under test first), a scratch TMPDIR, TERM and LANG, and
# DISABLE_AUTOUPDATER, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC and
# PI_OFFLINE. Each runs in the scratch HOME and is killed, with everything
# it started, after VERIFY_TIMEOUT seconds (default 120), which reports FAIL;
# the setup step that writes the seed tree gets at least 60 s.
# The script refuses to run (exit 2) if HOME is / or the scratch root would
# fall inside your real HOME. No auth file is read or copied, and only
# non-interactive commands that need no login are run. A harness whose CLI
# is not on PATH is skipped. Clax's daemon, if anything starts one, listens
# on a scratch port (never 7480 or 7481) and is stopped on exit.
#
# The Pi session check needs a model, so it is SKIPPED unless you export
# VERIFY_PI_SESSION=1 and your provider's key (a *_API_KEY variable,
# ANTHROPIC_OAUTH_TOKEN or AZURE_OPENAI_*) yourself; only that step is given
# the key. Pi runs it without its built-in tools, sessions, context files,
# skills, prompt templates or themes, and with only the clax_status tool.
#
# Prints a PASS/FAIL/SKIPPED table. Exit 0 when nothing failed, 1 when a
# check failed, 2 when it refused to run.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
OLD="arti""fax"   # the previous product name, assembled

refuse() { echo "verify-harnesses: $*" >&2; exit 2; }

T="${VERIFY_TIMEOUT:-120}"
T_SESSION="${VERIFY_PI_TIMEOUT:-300}"
case "$T$T_SESSION" in *[!0-9]*) refuse "VERIFY_TIMEOUT and VERIFY_PI_TIMEOUT must be whole seconds" ;; esac
[ "$T" -gt 0 ] && [ "$T_SESSION" -gt 0 ] || refuse "VERIFY_TIMEOUT and VERIFY_PI_TIMEOUT must be positive"

[ -n "${HOME:-}" ] || refuse "HOME is not set"
REAL_HOME="$(cd "$HOME" 2>/dev/null && pwd -P)" || refuse "HOME ($HOME) is not a directory"
[ "$REAL_HOME" != / ] || refuse "HOME is /, so every scratch directory would be inside it"
REAL_HOME_L="$HOME"

# Paths compare without case on macOS, whose file systems usually ignore it.
if [ "$(uname -s)" = Darwin ]; then
    fold() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }
else
    fold() { printf '%s' "$1"; }
fi
# Whether the absolute path $1 is the real HOME or inside it: as written,
# physically, or (for each existing ancestor) as the same directory.
inside_real_home() {
    local p="$1"
    case "$(fold "$p")/" in
        "$(fold "$REAL_HOME")"/* | "$(fold "$REAL_HOME_L")"/*) return 0 ;;
    esac
    while :; do
        if [ -e "$p" ] && [ "$p" -ef "$REAL_HOME" ]; then return 0; fi
        case "$p" in / | . | "") return 1 ;; esac
        p="$(dirname "$p")"
    done
}

# Where the scratch root goes, checked before anything is created there.
BASE="${TMPDIR:-/tmp}"
BASE="$(cd "$BASE" 2>/dev/null && pwd -P)" || refuse "TMPDIR ($TMPDIR) is not a directory"
inside_real_home "$BASE" \
    && refuse "the scratch directory would be inside your real HOME ($REAL_HOME); set TMPDIR to a directory outside it"

# The build under test, built now, while HOME is still the real one, so cargo
# uses your own CARGO_HOME.
GIT_DESC="$(git -C "$REPO" describe --always --dirty 2>/dev/null || echo "unknown")"
# The executable of the `clax` binary in cargo's JSON messages on stdin.
artifact_exe() {
    perl -MJSON::PP -ne '
        my $m = eval { JSON::PP::decode_json($_) } or next;
        next unless ($m->{reason} // "") eq "compiler-artifact";
        next unless ($m->{target}{name} // "") eq "clax" && grep { $_ eq "bin" } @{ $m->{target}{kind} || [] };
        print "$m->{executable}\n" if defined $m->{executable};
    ' | tail -n 1
}
if [ -n "${CLAX_BIN:-}" ]; then
    [ -x "$CLAX_BIN" ] || refuse "CLAX_BIN ($CLAX_BIN) is not executable"
    BUILT="from CLAX_BIN"
else
    command -v cargo >/dev/null 2>&1 || refuse "cargo is not on PATH: build Clax and set CLAX_BIN to the binary"
    echo "building: cargo build -p clax-cli --bin clax" >&2
    CLAX_BIN="$(cd "$REPO" && cargo build -p clax-cli --bin clax --message-format=json-render-diagnostics </dev/null | artifact_exe)" \
        || refuse "cargo build failed"
    [ -n "$CLAX_BIN" ] && [ -x "$CLAX_BIN" ] || refuse "cargo build named no clax executable"
    BUILT="built from $GIT_DESC"
fi
CLAX_BIN="$(cd "$(dirname "$CLAX_BIN")" && pwd -P)/$(basename "$CLAX_BIN")"
CLAX_DIR="$(dirname "$CLAX_BIN")"
SAFE_PATH="$CLAX_DIR:$PATH:$REPO/plugins/pi/node_modules/.bin"

# Runs a command with the scratch environment, killing it and everything it
# started after $1 seconds (exit 124). Ctrl-C and other signals stop it too.
# shellcheck disable=SC2016 # Perl code, expanded by perl
WATCHDOG='
    my $t = shift;
    my $pid = 0;
    my $stop = sub {
        return unless $pid;
        kill "TERM", -$pid;
        for (1 .. 20) { last if waitpid($pid, WNOHANG) != 0; select(undef, undef, undef, 0.1); }
        kill "KILL", -$pid;
        waitpid($pid, 0);
    };
    for my $s (qw(INT TERM HUP)) {
        $SIG{$s} = sub { $stop->(); $SIG{$s} = "DEFAULT"; kill $s, $$; exit 1 };
    }
    $SIG{ALRM} = sub { $stop->(); exit 124 };
    $pid = fork() // die "fork: $!\n";
    if ($pid == 0) { setpgrp(0, 0); exec { $ARGV[0] } @ARGV or die "exec $ARGV[0]: $!\n"; }
    alarm $t;
    waitpid($pid, 0);
    my $st = $?;
    alarm 0;
    kill "KILL", -$pid;
    exit($st & 127 ? 128 + ($st & 127) : $st >> 8);
'
SCRATCH_ENV=()
sx() { # seconds, command...
    local t="$1"; shift
    perl -MPOSIX=WNOHANG -e "$WATCHDOG" "$t" env -i "${SCRATCH_ENV[@]}" "$@"
}

# The scratch root, removed on every exit. Nothing is created before the
# trap is set.
ROOT=""
READY=""
cleanup() {
    if [ -n "$READY" ]; then
        sx 30 "$CLAX_BIN" stop >/dev/null 2>&1 </dev/null || true
    fi
    if [ -n "$ROOT" ]; then rm -rf "$ROOT"; fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
ROOT="$(mktemp -d "$BASE/clax-verify.XXXXXX")"
ROOT="$(cd "$ROOT" && pwd -P)"
S_HOME="$ROOT/home"
S_CLAX="$S_HOME/.clax"
S_CODEX="$S_HOME/.codex"
S_CLAUDE="$S_HOME/.claude"
S_PI="$S_HOME/.pi/agent"
for d in "$ROOT" "$S_HOME" "$S_CLAX" "$S_CODEX" "$S_CLAUDE" "$S_PI"; do
    inside_real_home "$d" \
        && refuse "the scratch directory $d would be inside your real HOME ($REAL_HOME); set TMPDIR to a directory outside it"
done
mkdir -p "$S_CLAX" "$S_CODEX" "$S_CLAUDE" "$S_PI" "$ROOT/tmp" "$ROOT/empty" "$ROOT/other" "$ROOT/out"

# A scratch port for any daemon of the scratch home, never 7480 or 7481.
PORT=$((20000 + RANDOM % 30000))
printf '[serve]\nport = %s\n' "$PORT" > "$S_CLAX/config.toml"

SCRATCH_ENV=(
    HOME="$S_HOME" CLAX_HOME="$S_CLAX" CODEX_HOME="$S_CODEX"
    CLAUDE_CONFIG_DIR="$S_CLAUDE" PI_CODING_AGENT_DIR="$S_PI"
    CLAX_BIN="$CLAX_BIN" CLAX_NO_OPEN=1 PATH="$SAFE_PATH" TMPDIR="$ROOT/tmp"
    DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 PI_OFFLINE=1
)
if [ -n "${TERM:-}" ]; then SCRATCH_ENV+=(TERM="$TERM"); fi
if [ -n "${LANG:-}" ]; then SCRATCH_ENV+=(LANG="$LANG"); fi
# The provider keys, given only to the opt-in Pi session.
PI_KEYS=()
for v in $(compgen -e); do
    case "$v" in
        *_API_KEY | ANTHROPIC_OAUTH_TOKEN | AZURE_OPENAI_*) PI_KEYS+=("$v=${!v}") ;;
    esac
done
# The script's own commands see the scratch homes too.
export HOME="$S_HOME" CLAX_HOME="$S_CLAX" CODEX_HOME="$S_CODEX" \
    CLAUDE_CONFIG_DIR="$S_CLAUDE" PI_CODING_AGENT_DIR="$S_PI" CLAX_BIN \
    PATH="$SAFE_PATH" TMPDIR="$ROOT/tmp"
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME
# clax init registers the Chrome extension's native host; keep it in scratch.
export CLAX_NATIVE_HOST_DIRS="chrome=$ROOT/browsers/chrome"
READY=1
# Harness CLIs read project settings from their working directory too.
cd "$S_HOME"
MK="$S_CLAX/marketplace"

VERSION="$(sx 30 "$CLAX_BIN" --version </dev/null 2>/dev/null | head -n 1 || true)"
HEADER="build under test: $CLAX_BIN (${VERSION:-no version}; $BUILT)"
echo "$HEADER"
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
timed_out() { echo "(timeout after ${1}s)"; }
have() { command -v "$1" >/dev/null 2>&1; }
HARNESSES=()
for h in claude codex pi; do
    if have "$h"; then
        HARNESSES+=("$h")
        v="$(sx 30 "$h" --version </dev/null 2>/dev/null | head -n 1 || true)"
        echo "$h: $(command -v "$h") (${v:-no version})"
    else
        record SKIPPED "$h" "not on PATH"
    fi
done
uses() { case " ${HARNESSES[*]+"${HARNESSES[*]}"} " in *" $1 "*) return 0 ;; esac; return 1; }

# The registration names in Claude Code's and Codex's registries, one per
# line: marketplace names and plugin IDs (plugin@marketplace). A registry
# that cannot be parsed prints "?unparseable <file>".
reg_names() {
    case "$1" in
        claude) perl -MJSON::PP -e '
            for my $f (@ARGV) {
                open(my $fh, "<", $f) or next;
                my $j = eval { local $/; JSON::PP::decode_json(<$fh>) };
                if (ref $j ne "HASH") { print "?unparseable $f\n"; next }
                my @maps = $f =~ /known_marketplaces\.json$/ ? ($j)
                    : $f =~ /installed_plugins\.json$/ ? ($j->{plugins})
                    : ($j->{enabledPlugins}, $j->{extraKnownMarketplaces});
                for my $m (@maps) { print "$_\n" for ref $m eq "HASH" ? sort keys %$m : () }
            }' "$S_CLAUDE/settings.json" "$S_CLAUDE/plugins/known_marketplaces.json" "$S_CLAUDE/plugins/installed_plugins.json" ;;
        codex) [ -e "$S_CODEX/config.toml" ] || return 0
            sed -n -e 's/^[[:space:]]*\[marketplaces\.\(.*\)\][[:space:]]*$/\1/p' \
                -e 's/^[[:space:]]*\[plugins\.\(.*\)\][[:space:]]*$/\1/p' "$S_CODEX/config.toml" | tr -d '"'"'" ;;
    esac
}
# Whether a registration names the previous name, as a marketplace or plugin.
names_old() { reg_names "$1" | grep -qxE -- "\\?unparseable .*|$OLD|$OLD@.*|.*@$OLD"; }
lacks_old() { ! names_old "$1"; }
# Pi's package sources, one per line, as settings.json stores them.
pi_packages() {
    [ -e "$S_PI/settings.json" ] || return 0
    perl -MJSON::PP -e '
        open(my $fh, "<", $ARGV[0]) or exit;
        my $j = eval { local $/; JSON::PP::decode_json(<$fh>) };
        if (ref $j ne "HASH") { print "?unparseable $ARGV[0]\n"; exit }
        for my $p (@{ ref $j->{packages} eq "ARRAY" ? $j->{packages} : [] }) {
            print ref $p eq "HASH" ? ($p->{source} // "") : $p, "\n";
        }' "$S_PI/settings.json"
}
# Each harness's registry files, concatenated.
reg() {
    case "$1" in
        claude) cat "$S_CLAUDE/settings.json" "$S_CLAUDE/plugins/known_marketplaces.json" "$S_CLAUDE/plugins/installed_plugins.json" 2>/dev/null || true ;;
        codex) cat "$S_CODEX/config.toml" 2>/dev/null || true ;;
        pi) pi_packages ;;
    esac
}
# Whether the harness registers the marketplace: by path for Claude Code and
# Codex, and for Pi by a package entry for its package directory (which Pi
# may store relative to its own directory).
reg_has_mk() {
    if [ "$1" = pi ]; then pi_packages | grep -qE '(^|/)marketplace/plugins/pi/?$'; else reg "$1" | grep -qF -- "$MK"; fi
}
reg_lacks_mk() { ! reg_has_mk "$1"; }
mk_mark() { if [ "$1" = pi ]; then echo "marketplace/plugins/pi"; else echo "$MK"; fi; }
pi_count() { pi_packages | grep -cE '(^|/)marketplace/plugins/pi/?$' || true; }
pi_has_old() { pi_packages | grep -qE '(^|/)oldcheckout/plugins/pi/?$'; }
pi_lacks_old() { ! pi_has_old; }
# Runs clax with its output in $ROOT/out/$1; its exit status.
run_clax() {
    local name="$1"; shift
    sx "$T" "$CLAX_BIN" "$@" >"$ROOT/out/$name" 2>&1 </dev/null
}
out_has() { grep -qF -- "$2" "$ROOT/out/$1"; }
# Runs clax as `run_clax`, recording PASS, or FAIL with its first lines.
check_run() { # check name, output name, clax arguments...
    local name="$1" out="$2" rc=0; shift 2
    run_clax "$out" "$@" || rc=$?
    if [ "$rc" = 0 ]; then
        record PASS "$name"
    elif [ "$rc" = 124 ]; then
        record FAIL "$name" "$(timed_out "$T")"
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
# Writes the tree only: no harness on that PATH.
# A setup step, not a check: VERIFY_TIMEOUT does not shorten it.
sx "$(( T > 60 ? T : 60 ))" env PATH="$ROOT/empty" "$CLAX_BIN" init >/dev/null 2>&1 </dev/null || true
[ -d "$MK" ] || { echo "verify-harnesses: clax init did not write $MK" >&2; exit 1; }
mv "$MK" "$OLDDIR"
rm -f "$S_CLAX/registrations.json"
for f in .claude-plugin/marketplace.json .agents/plugins/marketplace.json plugins/claude-code/.claude-plugin/plugin.json plugins/clax/.codex-plugin/plugin.json; do
    sed -i.bak "s/\"name\": *\"clax\"/\"name\": \"$OLD\"/" "$OLDDIR/$f" && rm -f "$OLDDIR/$f.bak"
done
sed -i.bak "s#@empathic/clax-pi#@empathic/$OLD-pi#" "$OLDDIR/plugins/pi/package.json" && rm -f "$OLDDIR/plugins/pi/package.json.bak"
seed() {
    case "$1" in
        claude) sx "$T" claude plugin marketplace add "$OLDDIR" && sx "$T" claude plugin install "$OLD@$OLD" ;;
        codex) sx "$T" codex plugin marketplace add "$OLDDIR" && sx "$T" codex plugin add "$OLD@$OLD" ;;
        pi) sx "$T" pi install "$OLDDIR/plugins/pi" ;;
    esac
}
seeded() { if [ "$1" = pi ]; then pi_has_old; else names_old "$1"; fi; }
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    rc=0
    seed "$h" </dev/null >"$ROOT/seed-$h.log" 2>&1 || rc=$?
    if [ "$rc" = 0 ] && seeded "$h"; then
        record PASS "$h: seed registers the previous name"
    elif [ "$rc" = 124 ]; then
        record FAIL "$h: seed registers the previous name" "$(timed_out "$T")"
    else
        record FAIL "$h: seed registers the previous name" "$(tail -n 1 "$ROOT/seed-$h.log")"
    fi
done
rm -rf "$OLDDIR"

# init
check_run "init exits 0" init1 init
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    check "$h: init registers the marketplace" "registry does not name $(mk_mark "$h")" reg_has_mk "$h"
done
for h in claude codex; do
    uses "$h" || continue
    check "$h: init removes the previous name's registration" \
        "registry still registers $(reg_names "$h" | grep -xE -- "\\?unparseable .*|$OLD|$OLD@.*|.*@$OLD" | tr '\n' ' ')" \
        lacks_old "$h"
done
if uses codex; then
    check "codex: init keeps other config.toml content" "comment or [marketplaces.other] gone" codex_seed_kept
fi
if uses pi; then
    gone="$OLDDIR/plugins/pi"
    check "pi: the deleted previous-name checkout is left and named" "no \`pi remove $gone\` in init's output" \
        out_has init1 "pi remove $gone"
    rc=0
    sx "$T" pi remove "$gone" </dev/null >/dev/null 2>&1 || rc=$?
    if [ "$rc" = 124 ]; then
        record FAIL "pi: the named \`pi remove\` clears it" "$(timed_out "$T")"
    else
        check "pi: the named \`pi remove\` clears it" "settings.json still names it" pi_lacks_old
    fi
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
    check "$h: still registered after init again" "registry does not name $(mk_mark "$h")" reg_has_mk "$h"
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
    rc=0
    run_clax "doctor-$h" --json doctor --agent "$h" || rc=$?
    for c in binary plugin; do
        if [ "$rc" = 124 ]; then
            record FAIL "$h: doctor $c passes" "$(timed_out "$T")"
        else
            check "$h: doctor $c passes" "$(tr ',' '\n' <"$ROOT/out/doctor-$h" | grep -A2 "\"name\":\"$c\"" | head -n 1)" \
                out_has "doctor-$h" "\"name\":\"$c\",\"ok\":true"
        fi
    done
done

# A Pi session, which needs a model. Pi gets no built-in tools (bash, edit,
# write and the rest), only the extension's clax_status, and keeps no session.
PI_SESSION=(pi --no-builtin-tools --tools clax_status --no-session --offline
    --no-context-files --no-skills --no-prompt-templates --no-themes
    -p "Call the clax_status tool and print what it returns")
if uses pi && [ "${VERIFY_PI_SESSION:-}" = 1 ]; then
    rc=0
    sx "$T_SESSION" env ${PI_KEYS[@]+"${PI_KEYS[@]}"} "${PI_SESSION[@]}" </dev/null >"$ROOT/out/pi-session" 2>&1 || rc=$?
    if [ "$rc" = 0 ] && grep -qi clax "$ROOT/out/pi-session"; then
        record PASS "pi: a session loads the extension"
    elif [ "$rc" = 124 ]; then
        record FAIL "pi: a session loads the extension" "$(timed_out "$T_SESSION")"
    else
        record FAIL "pi: a session loads the extension" "$(tail -n 1 "$ROOT/out/pi-session")"
    fi
elif uses pi; then
    record SKIPPED "pi: a session loads the extension" "needs a model: export your provider's API key and VERIFY_PI_SESSION=1, then re-run"
fi

# uninit of Pi alone, while the others still use the copy.
if uses pi; then
    check_run "uninit --agent pi exits 0" uninit-pi uninit --agent pi
    check "pi: uninit --agent pi removes the entry" "settings.json still names the marketplace" reg_lacks_mk pi
    if uses claude || uses codex; then
        check "uninit --agent pi keeps the copy the others use" "marketplace deleted, or not reported kept" copy_kept
    fi
fi

# uninit
check_run "uninit exits 0" uninit uninit
for h in ${HARNESSES[@]+"${HARNESSES[@]}"}; do
    check "$h: uninit leaves no registration" "registry still names $(mk_mark "$h")" reg_lacks_mk "$h"
done
check "uninit removes the marketplace copy" "$MK still exists" test ! -e "$MK"
if uses codex; then
    check "codex: uninit keeps other config.toml content" "comment or [marketplaces.other] gone" codex_seed_kept
fi

echo
echo "$HEADER"
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
