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

def npm_script_test(name, script, data, args = [], skips = [], **kwargs):
    """`pnpm run <script> [args]` in this package, as a test.

    Args:
      name: the test's name.
      script: the script in this package's package.json.
      data: everything it reads: sources, config, `:node_modules`.
      args: arguments appended to the script's command.
      skips: paths (from the workspace root) of declared inputs left out of
        the runner's private copy; the script reads them from the runfiles.
      **kwargs: passed to js_test (size, timeout, tags, env).
    """
    js_test(
        name = name,
        entry_point = _RUNNER,
        args = [a for skip in skips for a in ["--skip", skip]] + [native.package_name(), script] + args,
        data = data + [_RUNNER_LIB],
        patch_node_fs = False,
        # A cold runner starts Node, copies the project and type-checks or
        # tests all of it; Bazel's default for a test is five minutes.
        timeout = kwargs.pop("timeout", "long"),
        **kwargs
    )

def npm_script_build(name, script, srcs, out_dirs, args = [], env = {}, project = None, skips = [], in_place = [], **kwargs):
    """`pnpm run <script> [args]` in a project, as a build step.

    Args:
      name: the target; its outputs are the directories in `out_dirs`.
      script: the script in the project's package.json.
      srcs: everything it reads.
      out_dirs: the directories it writes, relative to this package.
      args: arguments appended to the script's command.
      env: environment variables for it.
      project: the package whose package.json holds the script, this one or
        one above it (`web` for a target in `web/reel`); this package by
        default.
      skips: paths (from the workspace root) of declared inputs left out of
        the runner's private copy; the script reads them from the runfiles.
      in_place: inputs the script reads where Bazel put them, never copied
        into the project: built for the target platform as a test's data is
        (a debug binary the script runs, a web build, the pinned browsers),
        where `srcs` are the runner's own and built for the platform running
        the action. Name them in `env` by `$(execpath ...)`, relative to the
        execution root (JS_BINARY__EXECROOT in the script's environment).
      **kwargs: passed to js_run_binary (visibility, tags).
    """
    package = native.package_name()
    project = package if project == None else project
    if package != project and not package.startswith(project + "/"):
        fail("npm_script_build: %s is not inside the project %s" % (package, project))

    # The runner's `--out` names a directory relative to the project.
    within = package[len(project) + 1:] + "/" if package != project else ""
    js_binary(
        name = name + "_runner",
        entry_point = _RUNNER,
        data = srcs + [_RUNNER_LIB],
        patch_node_fs = False,
    )
    js_run_binary(
        name = name,
        tool = ":" + name + "_runner",
        args = [a for skip in skips for a in ["--skip", skip]] +
               [arg for out in out_dirs for arg in ["--out", within + out]] +
               [project, script] + args,
        srcs = srcs + in_place,
        # An external repository's file cannot be copied to the output tree;
        # what `in_place` names is read where it is.
        copy_srcs_to_bin = not in_place,
        out_dirs = out_dirs,
        env = env,
        mnemonic = "NpmScript",
        **kwargs
    )
