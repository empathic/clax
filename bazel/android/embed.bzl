"""The embedded core built as a shared library for an Android CPU, as the
Clax app loads it.

`android_embed` builds `lib` (a rust_shared_library) optimized (as `cargo
build --release`) and stripped (`-Cstrip=symbols`, which keeps the dynamic
symbols the app looks up) for `platform` with the NDK's toolchain, and names
it `<abi>/libclax_embed.so`, where the APK packs each ABI's libraries. The
name is a symbolic link; nothing is copied.
"""

_RUSTC_FLAGS = "@rules_rust//rust/settings:extra_rustc_flag"

def _android_transition_impl(settings, attr):
    return {
        "//command_line_option:platforms": str(attr.platform),
        "//command_line_option:compilation_mode": "opt",
        _RUSTC_FLAGS: settings[_RUSTC_FLAGS] + ["-Cstrip=symbols"],
    }

_android_transition = transition(
    implementation = _android_transition_impl,
    inputs = [_RUSTC_FLAGS],
    outputs = [
        "//command_line_option:platforms",
        "//command_line_option:compilation_mode",
        _RUSTC_FLAGS,
    ],
)

def _android_embed_impl(ctx):
    libs = [f for f in ctx.attr.lib[0][DefaultInfo].files.to_list() if f.extension == "so"]
    if len(libs) != 1:
        fail("%s: expected one shared library, got %s" % (ctx.attr.lib[0].label, libs))
    out = ctx.actions.declare_file("%s/%s/libclax_embed.so" % (ctx.label.name, ctx.attr.abi))
    ctx.actions.symlink(output = out, target_file = libs[0])
    return [DefaultInfo(files = depset([out]), runfiles = ctx.runfiles(files = [out, libs[0]]))]

android_embed = rule(
    implementation = _android_embed_impl,
    attrs = {
        "abi": attr.string(mandatory = True, doc = "The app's ABI directory: arm64-v8a or x86_64."),
        "lib": attr.label(mandatory = True, cfg = _android_transition),
        "platform": attr.label(mandatory = True),
    },
)
