"""The macOS app's embedded clax, named and signed as the app's Xcode build does.

The Runner's "Embed clax" build phase (macos/Runner.xcodeproj) copies the
host clax into Contents/Resources/clax and signs it with
Runner/ClaxHelper.entitlements (app-sandbox and inherit): a sandboxed app
may only start a helper sandboxed the same way, or macOS kills the helper
at launch. `macos_helper` does the same for the Bazel-built app, with an
ad-hoc signature (the app's bundle is ad-hoc signed by rules_apple too),
using the system's `codesign`, which no toolchain provides.
"""

def _macos_helper_impl(ctx):
    src = ctx.attr.binary[DefaultInfo].files_to_run.executable
    # At the package's top: rules_apple places a file by its package path.
    out = ctx.actions.declare_file(ctx.attr.filename)
    # Without entitlements (a debug app, which is not sandboxed), as the
    # Xcode build signs the Debug and Profile helper.
    if ctx.file.entitlements:
        command = 'cat "$1" > "$2" && chmod 755 "$2" && /usr/bin/codesign --force --sign - --entitlements "$3" --options runtime "$2"'
        arguments = [src.path, out.path, ctx.file.entitlements.path]
        inputs = [src, ctx.file.entitlements]
    else:
        command = 'cat "$1" > "$2" && chmod 755 "$2" && /usr/bin/codesign --force --sign - --options runtime "$2"'
        arguments = [src.path, out.path]
        inputs = [src]
    ctx.actions.run_shell(
        command = command,
        arguments = arguments,
        inputs = inputs,
        outputs = [out],
        mnemonic = "MacosHelperSign",
        progress_message = "Signing %s for the sandboxed app" % ctx.attr.filename,
        execution_requirements = {"no-remote": "1"},
    )
    return [DefaultInfo(files = depset([out]))]

macos_helper = rule(
    implementation = _macos_helper_impl,
    attrs = {
        "binary": attr.label(mandatory = True, executable = True, cfg = "target"),
        "entitlements": attr.label(allow_single_file = [".entitlements", ".plist"]),
        "filename": attr.string(default = "clax"),
    },
)

def _dbg_transition_impl(_settings, _attr):
    return {"//command_line_option:compilation_mode": "dbg"}

_dbg_transition = transition(
    implementation = _dbg_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:compilation_mode"],
)

def _debug_app_impl(ctx):
    app = ctx.attr.app[0]
    zips = [f for f in app[DefaultInfo].files.to_list() if f.extension == "zip"]
    if len(zips) != 1:
        fail("%s gives %d zips" % (app.label, len(zips)))
    out = ctx.actions.declare_file(ctx.label.name + ".zip")
    ctx.actions.symlink(output = out, target_file = zips[0])
    return [DefaultInfo(files = depset([out]))]

debug_app = rule(
    implementation = _debug_app_impl,
    attrs = {"app": attr.label(mandatory = True, cfg = _dbg_transition)},
    doc = """A macOS app built as Flutter's debug build (-c dbg: JIT, the
    Runner's `#if DEBUG` code, DebugProfile.entitlements), as `<name>.zip`;
    what `flutter test integration_test/... -d macos` runs.""",
)

def _integration_main_impl(ctx):
    out = ctx.outputs.main

    # The test and the files beside it, linked next to the entry point: a
    # generated `main` is compiled from its output directory
    # (`org-dartlang-app:///`), where a relative import of a source file
    # would not resolve.
    files = [out]
    test = None
    for src in ctx.files.srcs:
        rel = src.short_path[len(ctx.label.package) + 1:] if ctx.label.package else src.short_path
        link = ctx.actions.declare_file("%s/%s" % (ctx.label.name, rel))
        ctx.actions.symlink(output = link, target_file = src)
        files.append(link)
        if src == ctx.file.test:
            test = "%s/%s" % (ctx.label.name, rel)
    if not test:
        fail("%s: srcs do not include %s" % (ctx.label, ctx.file.test.short_path))
    ctx.actions.write(out, """\
// Generated (bazel/flutter/macos.bzl): runs {short} in the app, as
// `flutter test` does on a device, then prints CLAX_INTEGRATION_RESULT=0
// (every test passed) or =1, which the iOS runner reads from the console,
// and exits with that status.
import 'dart:io';

import 'package:integration_test/integration_test.dart';

import '{test}' as t;

Future<void> main() async {{
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  t.main();
  final passed = await binding.allTestsPassed.future;
  for (final f in binding.failureMethodsDetails) {{
    stderr.writeln('FAIL: ${{f.methodName}}\\n${{f.details}}');
  }}
  stdout.writeln(passed ? 'integration_test: all tests passed' : 'integration_test: tests failed');
  stdout.writeln('CLAX_INTEGRATION_RESULT=${{passed ? 0 : 1}}');
  await stdout.flush();
  exit(passed ? 0 : 1);
}}
""".format(test = test, short = ctx.file.test.short_path))
    return [DefaultInfo(files = depset(files))]

integration_main = rule(
    implementation = _integration_main_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = [".dart"], doc = "The test and the files it imports, from integration_test/."),
        "test": attr.label(mandatory = True, allow_single_file = [".dart"]),
    },
    # The entry point by its own label, `:<name>.dart`, for an app's `main`.
    outputs = {"main": "%{name}.dart"},
    doc = "An app entry point that runs an integration test, prints CLAX_INTEGRATION_RESULT=<0|1> and exits with it.",
)
