# Sourced by the script gates (bash). `fake_exe PATH` puts the script read
# from stdin, which starts with a #! line naming a shell or Python, at PATH
# as an executable stand-in (a fake `clax`, `codex`, `cargo`, ...).
#
# macOS assesses every new executable file the first time it runs
# (syspolicyd, XProtect); on a loaded machine that first run can take
# seconds, longer than the limit the code under test puts on a program it
# starts, while later runs of the same file take milliseconds. The
# assessment belongs to the file: a new file with the same text is assessed
# again, running an assessed file through a symbolic link to it is not a
# first run. So each text is stored once, read-only, in $TMPDIR/clax-fake-exe
# under a hash of the text, run once with CLAX_FAKE_EXE_WARMUP set (a line
# added after the #! line makes that run exit at once), and PATH is a
# symbolic link to it. A gate's own runs of the fake are then never a first
# run. Per-case state stays out of the text: a fake finds its case's files
# beside itself ("$(dirname "$0")", the link's directory), in its arguments
# or in its environment. The stored copy is read-only; to change a fake,
# install another text at the same path. A path resolved through symbolic
# links (realpath) names the stored copy, shared by every fake with the same
# text; where the code under test identifies a program by its file,
# fake_exe_own puts a file of its own at PATH instead, run once there (a
# first run per call, paid before the gate relies on it).
# crates/clax-fake-exe does the same for the Rust tests.

# The tools, found now: a gate may later narrow PATH or put fakes on it.
FAKE_EXE_CAT="$(command -v cat)"
FAKE_EXE_MKDIR="$(command -v mkdir)"
FAKE_EXE_CHMOD="$(command -v chmod)"
FAKE_EXE_MV="$(command -v mv)"
FAKE_EXE_LN="$(command -v ln)"
FAKE_EXE_RM="$(command -v rm)"
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

# Sets FAKE_EXE_TEXT to the script read from stdin with a line after its #!
# line that ends a warm-up run; $1 names the fake in an error.
fake_exe_text() {
    local text first rest guard
    text="$("$FAKE_EXE_CAT"; printf x)"
    text="${text%x}"
    first="${text%%$'\n'*}"
    rest=""
    case "$text" in *$'\n'*) rest="${text#*$'\n'}" ;; esac
    case "$first" in
        '#!'*python*) guard='import os as _o, sys as _s'$'\n''if _o.environ.get("CLAX_FAKE_EXE_WARMUP"): _s.exit(0)' ;;
        '#!'*) guard='[ -z "${CLAX_FAKE_EXE_WARMUP:-}" ] || exit 0' ;;
        *) echo "fake_exe: $1: the script has no #! line" >&2; return 1 ;;
    esac
    FAKE_EXE_TEXT="$first"$'\n'"$guard"$'\n'"$rest"
}

# Writes FAKE_EXE_TEXT to $1, makes it read-only, runs it once and renames
# it to $2, so whoever finds a file at $2 finds a whole, warmed one. A
# rename keeps the file, so its first run stays behind it.
fake_exe_store() {
    printf '%s' "$FAKE_EXE_TEXT" > "$1" && "$FAKE_EXE_CHMOD" 555 "$1" \
        && fake_exe_warm "$1" && "$FAKE_EXE_MV" -f "$1" "$2"
}

# Makes $1's directory and removes whatever is at $1.
fake_exe_clear() {
    case "$1" in */*) "$FAKE_EXE_MKDIR" -p "${1%/*}" || return 1 ;; esac
    "$FAKE_EXE_RM" -f "$1"
}

fake_exe() {
    local at="$1" name shared
    fake_exe_text "$at" || return 1
    name="sha256-$(printf '%s' "$FAKE_EXE_TEXT" | if [ "${FAKE_EXE_SUM##*/}" = shasum ]; then "$FAKE_EXE_SUM" -a 256; else "$FAKE_EXE_SUM"; fi)"
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
                # Stored under a name of its own, then renamed into place,
                # so a concurrent gate storing the same text never sees a
                # partial or unassessed copy.
                fake_exe_store "$FAKE_EXE_DIR/.$name.$$.$RANDOM" "$shared" || return 1
            fi
            FAKE_EXE_WARMED="$FAKE_EXE_WARMED$name "
            ;;
    esac
    fake_exe_clear "$at" || return 1
    "$FAKE_EXE_LN" -s "$shared" "$at"
}

# `fake_exe_own PATH`: like fake_exe, but PATH is a read-only file of its
# own, already run once, for a program the code under test identifies by its
# file (its resolved path, as `just install` does an installed clax).
fake_exe_own() {
    local at="$1"
    fake_exe_text "$at" && fake_exe_clear "$at" || return 1
    case "$at" in */*) ;; *) at="./$at" ;; esac
    fake_exe_store "${at%/*}/.${at##*/}.$$.$RANDOM" "$at"
}
