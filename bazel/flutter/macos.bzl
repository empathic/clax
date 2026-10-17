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
    ctx.actions.run_shell(
        command = 'cat "$1" > "$3" && chmod 755 "$3" && /usr/bin/codesign --force --sign - --entitlements "$2" --options runtime "$3"',
        arguments = [src.path, ctx.file.entitlements.path, out.path],
        inputs = [src, ctx.file.entitlements],
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
        "entitlements": attr.label(mandatory = True, allow_single_file = [".entitlements", ".plist"]),
        "filename": attr.string(default = "clax"),
    },
)
