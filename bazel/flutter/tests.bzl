"""Flutter test files as test targets that share a compile.

rules_flutter's `flutter_test` compiles one file into a kernel `.dill` that
carries the whole app and the Flutter framework: about 64 MB for the Clax
app. A target per test file made 127 of them, 7.6 GB, written again to the
shared disk cache on every edit of the app's code.

`flutter_test_suite` compiles a group of test files into one `.dill` (a
`manual` flutter_test, `<name>_all`) whose generated entry point registers
one file's tests, the file `CLAX_TEST_FILE` names, and makes a test target
per file (`<name>`'s `files`, each named by `names`) that runs that `.dill`
with its file named. Each file runs in a `flutter_tester` process of its
own, as under `flutter test`, so what one file leaves in a global (the test
binding's HTTP override, a static cache) never reaches another.
"""

load("@rules_flutter//flutter:defs.bzl", "flutter_test")

def _suite_main_impl(ctx):
    out = ctx.outputs.main

    # Each test by a path from this file back through the exec root, as
    # rules_flutter's flutter_test imports a test from its bootstrap.
    up = "../" * len(out.dirname.split("/"))
    imports = []
    calls = []
    for i, f in enumerate(ctx.files.tests):
        imports.append("import '%s%s' as t%d;" % (up, f.path, i))
        rel = _package_path(ctx, f)
        calls.append("    '%s' => t%d.main," % (rel, i))
    ctx.actions.write(out, """\
// Generated (bazel/flutter/tests.bzl): runs the test file CLAX_TEST_FILE
// names, one of {name}'s.
import 'dart:io';

{imports}

void main() {{
  final file = Platform.environment['CLAX_TEST_FILE'] ?? '';
  final run = switch (file) {{
{calls}
    _ => throw StateError('CLAX_TEST_FILE names no test of {name}: "$file"'),
  }};
  run();
}}
""".format(name = ctx.label.name, imports = "\n".join(imports), calls = "\n".join(calls)))
    return [DefaultInfo(files = depset([out]))]

def _package_path(ctx, f):
    return f.short_path[len(ctx.label.package) + 1:] if ctx.label.package else f.short_path

_suite_main = rule(
    implementation = _suite_main_impl,
    attrs = {"tests": attr.label_list(allow_files = [".dart"], mandatory = True)},
    outputs = {"main": "%{name}.dart"},
)

def _flutter_file_test_impl(ctx):
    suite = ctx.attr.suite
    exe = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.symlink(
        output = exe,
        target_file = suite[DefaultInfo].files_to_run.executable,
        is_executable = True,
    )
    env = dict(suite[RunEnvironmentInfo].environment)
    env["CLAX_TEST_FILE"] = ctx.attr.file

    # The suite's runner reports results under this name.
    env["FLUTTER_TEST_PATH"] = ctx.attr.file
    return [
        DefaultInfo(
            executable = exe,
            runfiles = ctx.runfiles(files = [exe]).merge(suite[DefaultInfo].default_runfiles),
        ),
        RunEnvironmentInfo(environment = env),
    ]

_flutter_file_test = rule(
    implementation = _flutter_file_test_impl,
    attrs = {
        "file": attr.string(mandatory = True, doc = "The test file, relative to the package."),
        "suite": attr.label(mandatory = True, providers = [RunEnvironmentInfo]),
    },
    test = True,
)

def flutter_test_suite(name, tests, names, srcs = [], env = {}, size = None, timeout = None, tags = [], **kwargs):
    """A test target per file of `tests`, compiled together.

    Args:
        name: The suite: `<name>_all` is the shared compile.
        tests: The test files (`*_test.dart`).
        names: {file: target name} for each of `tests`.
        srcs: The files they import that are not in `deps` (test support).
        env: The tests' environment. UNIT_TEST_ASSETS is set to the suite's
            assets, as `flutter test` sets it.
        size: Each test's size.
        timeout: Each test's timeout.
        tags: Each test's tags.
        **kwargs: Passed to flutter_test (package_name, deps, data, …).
    """
    _suite_main(
        name = "_%s_main" % name,
        tests = tests,
        tags = ["manual"],
    )
    pkg = native.package_name()
    flutter_test(
        name = name + "_all",
        main = ":_%s_main.dart" % name,
        srcs = srcs + [t for t in tests if t not in srcs],
        env = env | {"UNIT_TEST_ASSETS": "%s/%s_all_flutter_assets" % (pkg, name)},
        # Built by the file targets; run on its own it needs CLAX_TEST_FILE.
        tags = ["manual"],
        **kwargs
    )
    for t in tests:
        _flutter_file_test(
            name = names[t],
            file = t,
            size = size,
            suite = name + "_all",
            tags = tags,
            timeout = timeout,
        )
