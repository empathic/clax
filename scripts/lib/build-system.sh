# shellcheck shell=bash
# Sourced, not run: which build system a local command uses.
#
#   source scripts/lib/build-system.sh
#   build_system_choose rust web      # the toolchains the task needs natively
#   echo "$BUILD_SYSTEM"              # bazel | native
#
# The choice is made once, by what is installed, and CLAX_BUILD
# overrides it:
#
#   auto (default)  Bazel when Bazelisk, or a `bazel` at the version
#                   .bazelversion names, is on PATH. Otherwise the native
#                   tools: Cargo for Rust, Node and pnpm for the web UI and Pi.
#   bazel           Bazel, or an error saying how to install it. Never a
#                   silent fallback.
#   native          The native tools. Bazel is not looked for.
#
# build_system_choose sets:
#   BUILD_SYSTEM          bazel | native
#   BUILD_SYSTEM_WHY      one line: what was found
#   BUILD_SYSTEM_FALLBACK 1 when `auto` wanted Bazel and did not find it
#   BAZEL                 the command to run Bazel with, on the Bazel path
# and returns 2, after saying what is missing, when nothing usable is
# installed or the override cannot be met. It prints nothing otherwise:
# what to tell the person, and when, is the caller's (scripts/run.sh).
#
# Paths are relative to the repository root, which the caller is in.

# The variables above are this file's result, read by whoever sources it.
# shellcheck disable=SC2034

# How to install Bazelisk here, in one line.
build_system_install_hint() {
    case "$(uname -s)" in
        Darwin) echo "brew install bazelisk" ;;
        *) echo "see https://github.com/bazelbuild/bazelisk#installation (for example: npm install -g @bazel/bazelisk)" ;;
    esac
}

# _bazel_usable: sets BAZEL and _bazel_why, or returns 1 with _bazel_why
# saying what is wrong. Bazelisk reads .bazelversion and fetches that
# Bazel; a plain `bazel` is used only when it is that version already, for
# another version fails far from the cause.
_bazel_usable() {
    local want have
    want="$(tr -d '[:space:]' < .bazelversion)"
    if command -v bazelisk >/dev/null 2>&1; then
        BAZEL="$(command -v bazelisk)"
        _bazel_why="Bazelisk found ($BAZEL)"
        return 0
    fi
    if command -v bazel >/dev/null 2>&1; then
        have="$(bazel --version 2>/dev/null | awk '{ print $2; exit }')"
        if [ "$have" = "$want" ]; then
            BAZEL="$(command -v bazel)"
            _bazel_why="bazel $have found, the version .bazelversion names"
            return 0
        fi
        _bazel_why="bazel ${have:-of an unknown version} is on PATH, but .bazelversion names $want and Bazelisk is not installed"
        return 1
    fi
    _bazel_why="Bazel not found"
    return 1
}

# _native_usable <rust|web>...: returns 1 with _native_missing listing what
# is not installed. A version the repository does not pin for is not
# missing: it goes in _native_warnings, since such a run usually works and,
# when it does not, fails obscurely.
_native_usable() {
    local need pinned have
    _native_missing=()
    _native_warnings=()
    for need in "$@"; do
        case "$need" in
            rust)
                command -v cargo >/dev/null 2>&1 || _native_missing+=("cargo (Rust: https://rustup.rs)")
                ;;
            web)
                if command -v node >/dev/null 2>&1; then
                    :
                else
                    _native_missing+=("node")
                fi
                if command -v pnpm >/dev/null 2>&1; then
                    pinned="$(sed -n 's/.*"packageManager": *"pnpm@\([0-9]*\)\..*/\1/p' web/package.json | head -1)"
                    have="$(pnpm --version 2>/dev/null | sed 's/^\([0-9]*\).*/\1/')"
                    if [ -n "$pinned" ] && [ "$have" != "$pinned" ]; then
                        _native_warnings+=("pnpm $(pnpm --version 2>/dev/null) is installed; web/package.json pins pnpm $pinned")
                    fi
                else
                    _native_missing+=("pnpm")
                fi
                ;;
        esac
    done
    [ "${#_native_missing[@]}" -eq 0 ]
}

build_system_choose() {
    local mode="${CLAX_BUILD:-auto}" missing
    BUILD_SYSTEM=""
    BUILD_SYSTEM_WHY=""
    BUILD_SYSTEM_FALLBACK=0
    BUILD_SYSTEM_WARNINGS=()
    BAZEL=""
    case "$mode" in
        auto|bazel|native) ;;
        *)
            echo "error: CLAX_BUILD must be auto, bazel or native (got '$mode')" >&2
            return 2
            ;;
    esac

    if [ "$mode" != native ] && _bazel_usable; then
        BUILD_SYSTEM=bazel
        BUILD_SYSTEM_WHY="$_bazel_why"
        return 0
    fi
    if [ "$mode" = bazel ]; then
        echo "error: CLAX_BUILD=bazel, and $_bazel_why." >&2
        echo "       Install Bazelisk: $(build_system_install_hint)" >&2
        return 2
    fi

    if _native_usable "$@"; then
        BUILD_SYSTEM=native
        if [ "$mode" = native ]; then
            BUILD_SYSTEM_WHY="CLAX_BUILD=native"
        else
            BUILD_SYSTEM_WHY="$_bazel_why"
            BUILD_SYSTEM_FALLBACK=1
        fi
        BUILD_SYSTEM_WARNINGS=("${_native_warnings[@]+"${_native_warnings[@]}"}")
        return 0
    fi

    missing="$(printf '%s, ' "${_native_missing[@]}")"
    if [ "$mode" = native ]; then
        echo "error: CLAX_BUILD=native, and the native tools are not all installed: ${missing%, }." >&2
        echo "       Install them, or unset CLAX_BUILD to use Bazel." >&2
    else
        echo "error: nothing to build with. Install either:" >&2
        echo "       - Bazelisk, which fetches everything else: $(build_system_install_hint)" >&2
        echo "         ($_bazel_why.)" >&2
        echo "       - or the native tools, missing here: ${missing%, }." >&2
    fi
    return 2
}
