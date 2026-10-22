"""The Clax app's Dart import graph, read at load time: `@clax_app_dart_graph`.

`@clax_app_dart_graph//:graph.bzl` holds `LIBS` and `SUITES`, which
bazel/flutter/dart_graph.py derives from the imports of apps/clax_app's Dart
files (its docstring says how): a library per Dart library under lib/ (a
file, or the files of an import cycle), and the unit tests in groups that
share a compile, each built against the libraries its files import.
apps/clax_app/BUILD.bazel makes the targets. Nothing is checked in, so no
import goes undeclared: the repository is `local` and watches the app's Dart
directories, so an edit there evaluates it again, and an import a commit
adds is in the graph `bazel query` sees (scripts/affected-tests.sh).

The script runs with the host's `python3`, as the scripts under scripts/ do.
"""

_DIRS = ["lib", "test", "tool", "integration_test"]

def _impl(ctx):
    app = ctx.path(Label("//apps/clax_app:pubspec.yaml")).dirname
    script = ctx.path(Label("//bazel/flutter:dart_graph.py"))
    ctx.watch(script)
    for d in _DIRS:
        ctx.watch_tree(app.get_child(d))
    python = ctx.which("python3")
    if not python:
        fail("python3 is not on PATH: bazel/flutter/dart_graph.py derives the Clax app's Dart targets with it")
    res = ctx.execute([python, script, app], quiet = True)
    if res.return_code != 0:
        fail("bazel/flutter/dart_graph.py failed:\n" + res.stderr)
    ctx.file("graph.bzl", res.stdout)
    ctx.file("BUILD.bazel", "exports_files([\"graph.bzl\"])\n")

_dart_graph = repository_rule(
    implementation = _impl,
    local = True,
)

def _ext_impl(_ctx):
    _dart_graph(name = "clax_app_dart_graph")

dart_graph = module_extension(implementation = _ext_impl)
