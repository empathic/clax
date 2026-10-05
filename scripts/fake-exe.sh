# Sourced by the script gates (bash). `fake_exe PATH` puts the script read
# from stdin, which starts with a #! line naming a shell or Python, at PATH
# as an executable stand-in (a fake `clax`, `codex`, `cargo`, ...).
#
# macOS assesses every new executable file the first time it runs
# (syspolicyd, XProtect); on a loaded machine that first run can take
# seconds, longer than the limit the code under test puts on a program it
# starts, while later runs of the same file take milliseconds. The
# assessment belongs to the file: a new file with the same text is assessed
# again, a hard link to an assessed file is not. So each text is stored once,
# read-only, in $TMPDIR/clax-fake-exe under a hash of the text, run once
# with CLAX_FAKE_EXE_WARMUP set (a line added after the #! line makes that
# run exit at once), and PATH is a hard link to it. A gate's own runs of the
# fake are then never a first run. Per-case state stays out of the text: a
# fake finds its case's files beside itself ("$(dirname "$0")"), in its
# arguments or in its environment. PATH is read-only; to change a fake,
# install another text at the same path. crates/clax-fake-exe does the same
# for the Rust tests.

# The tools, found now: a gate may later narrow PATH or put fakes on it.
FAKE_EXE_CAT="$(command -v cat)"
FAKE_EXE_MKDIR="$(command -v mkdir)"
FAKE_EXE_CHMOD="$(command -v chmod)"
FAKE_EXE_MV="$(command -v mv)"
FAKE_EXE_LN="$(command -v ln)"
FAKE_EXE_RM="$(command -v rm)"
FAKE_EXE_CP="$(command -v cp)"
FAKE_EXE_SUM="$(command -v shasum || command -v sha256sum)"
FAKE_EXE_DIR="${TMPDIR:-/tmp}"
FAKE_EXE_DIR="${FAKE_EXE_DIR%/}/clax-fake-exe"
# The stored copies this gate has run once already.
FAKE_EXE_WARMED=" "

# Runs the fake at $1 once as a warm-up, however long that takes.
fake_exe_warm() {
    CLAX_FAKE_EXE_WARMUP=1 "$1" < /dev/null > /dev/null 2>&1 || {
        echo "fake_exe: the warm-up run of $1 failed" >&2
        return 1
    }
}

fake_exe() {
    local at="$1" text first rest guard name shared tmp
    text="$("$FAKE_EXE_CAT"; printf x)"
    text="${text%x}"
    first="${text%%$'\n'*}"
    rest=""
    case "$text" in *$'\n'*) rest="${text#*$'\n'}" ;; esac
    case "$first" in
        '#!'*python*) guard='import os as _o, sys as _s'$'\n''if _o.environ.get("CLAX_FAKE_EXE_WARMUP"): _s.exit(0)' ;;
        '#!'*) guard='[ -z "${CLAX_FAKE_EXE_WARMUP:-}" ] || exit 0' ;;
        *) echo "fake_exe: $at: the script has no #! line" >&2; return 1 ;;
    esac
    text="$first"$'\n'"$guard"$'\n'"$rest"
    name="sha256-$(printf '%s' "$text" | if [ "${FAKE_EXE_SUM##*/}" = shasum ]; then "$FAKE_EXE_SUM" -a 256; else "$FAKE_EXE_SUM"; fi)"
    name="${name%% *}"
    shared="$FAKE_EXE_DIR/$name"
    case "$FAKE_EXE_WARMED" in
        *" $name "*) ;;
        *)
            "$FAKE_EXE_MKDIR" -p "$FAKE_EXE_DIR" || return 1
            if [ -e "$shared" ]; then
                # Assessed when it was stored; run again in case that was
                # before a restart.
                fake_exe_warm "$shared" || return 1
            else
                # Written, made read-only and run under a name of its own,
                # then renamed into place, so a concurrent gate storing the
                # same text never sees a partial or unassessed copy.
                tmp="$FAKE_EXE_DIR/.$name.$$.$RANDOM"
                printf '%s' "$text" > "$tmp" && "$FAKE_EXE_CHMOD" 555 "$tmp" \
                    && fake_exe_warm "$tmp" && "$FAKE_EXE_MV" -f "$tmp" "$shared" || return 1
            fi
            FAKE_EXE_WARMED="$FAKE_EXE_WARMED$name "
            ;;
    esac
    case "$at" in */*) "$FAKE_EXE_MKDIR" -p "${at%/*}" || return 1 ;; esac
    "$FAKE_EXE_RM" -f "$at" || return 1
    if ! "$FAKE_EXE_LN" "$shared" "$at" 2> /dev/null; then
        # Another file system: a copy of its own, warmed here.
        "$FAKE_EXE_CP" "$shared" "$at" && fake_exe_warm "$at" || return 1
    fi
}
