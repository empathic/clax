# shellcheck shell=bash
# Sourced, not run: the flags every `bazel test` of the checks takes
# (scripts/bazel-test.sh, scripts/affected-tests.sh). A test's environment
# is part of its cache key, so the same flags let a test one of them passed
# be a cache hit for the other.
#
#   source scripts/lib/bazel-test-flags.sh
#   bazel test "${bazel_flags[@]}" "${bazel_test_flags[@]}" …

# The variables are this file's result, read by whoever sources it.
# shellcheck disable=SC2034
bazel_flags=(--lockfile_mode=error)

# Fake executables keep one store across tests and runs (CLAX_FAKE_EXE_DIR,
# crates/clax-fake-exe), outside each test's temporary directory, so each
# text is assessed by macOS once and not once per test.
_bazel_fakes="${TMPDIR:-/tmp}"
_bazel_fakes="${_bazel_fakes%/}/clax-fake-exe"
mkdir -p "$_bazel_fakes"
# shellcheck disable=SC2034
bazel_test_flags=(--test_env=CLAX_FAKE_EXE_DIR="$_bazel_fakes" --sandbox_writable_path="$_bazel_fakes")
unset _bazel_fakes
