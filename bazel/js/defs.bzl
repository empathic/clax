"""Runs a pnpm project's package.json scripts as Bazel tests and actions.

The web UI (web/) and the Pi extension (plugins/pi/) keep their checks and
builds as scripts in package.json (`pnpm test`, `pnpm run typecheck`,
`pnpm run build`). These rules run those same scripts, with Node and the
packages Bazel fetched from the lockfile, over the whole project at once: a
change anywhere in it reruns them, and no change means every result is
cached. Nothing about a check is restated here, so it cannot drift from
what `pnpm run` does.
"""

load("@aspect_rules_js//js:defs.bzl", "js_binary", "js_run_binary", "js_test")

_RUNNER = "//bazel/js:runner"
_RUNNER_LIB = "//bazel/js:runner_lib"

# The runner copies exactly an action's declared files into a private
# directory and runs the script there (npm_script.mjs says why). So
# rules_js's own guard against following links, which patches Node's file
# system calls and cannot reach Vite's native code, is off: there are no
# links to follow.

def npm_script_test(name, script, data, args = [], **kwargs):
    """`pnpm run <script> [args]` in this package, as a test.

    Args:
      name: the test's name.
      script: the script in this package's package.json.
      data: everything it reads: sources, config, `:node_modules`.
      args: arguments appended to the script's command.
      **kwargs: passed to js_test (size, timeout, tags, env).
    """
    js_test(
        name = name,
        entry_point = _RUNNER,
        args = [native.package_name(), script] + args,
        data = data + [_RUNNER_LIB],
        patch_node_fs = False,
        # A cold runner starts Node, copies the project and type-checks or
        # tests all of it; Bazel's default for a test is five minutes.
        timeout = kwargs.pop("timeout", "long"),
        **kwargs
    )

def npm_script_build(name, script, srcs, out_dirs, args = [], env = {}, **kwargs):
    """`pnpm run <script> [args]` in this package, as a build step.

    Args:
      name: the target; its outputs are the directories in `out_dirs`.
      script: the script in this package's package.json.
      srcs: everything it reads.
      out_dirs: the directories it writes, relative to this package.
      args: arguments appended to the script's command.
      env: environment variables for it.
      **kwargs: passed to js_run_binary (visibility, tags).
    """
    js_binary(
        name = name + "_runner",
        entry_point = _RUNNER,
        data = srcs + [_RUNNER_LIB],
        patch_node_fs = False,
    )
    js_run_binary(
        name = name,
        tool = ":" + name + "_runner",
        args = [arg for out in out_dirs for arg in ["--out", out]] + [native.package_name(), script] + args,
        srcs = srcs,
        out_dirs = out_dirs,
        env = env,
        mnemonic = "NpmScript",
        **kwargs
    )
