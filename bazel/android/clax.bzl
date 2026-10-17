"""The clax binary built for an Android CPU, as the Clax app packs it.

`android_clax` builds `binary` optimized (as `cargo build --release`) and
stripped (`-Cstrip=symbols`, which the NDK's linker applies) for `platform`
with the NDK's toolchain, and names it `<abi>/libclax.so`: the app's
packaging extracts native libraries to nativeLibraryDir, the one directory
Android 10 and later lets an app execute from, and only files named lib*.so
are extracted. The name is a symbolic link; nothing is copied.
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

def _android_clax_impl(ctx):
    src = ctx.attr.binary[0][DefaultInfo].files_to_run.executable
    out = ctx.actions.declare_file("%s/%s/libclax.so" % (ctx.label.name, ctx.attr.abi))
    ctx.actions.symlink(output = out, target_file = src, is_executable = True)
    return [DefaultInfo(files = depset([out]), runfiles = ctx.runfiles(files = [out, src]))]

android_clax = rule(
    implementation = _android_clax_impl,
    attrs = {
        "abi": attr.string(mandatory = True, doc = "The app's ABI directory: arm64-v8a or x86_64."),
        "binary": attr.label(mandatory = True, executable = True, cfg = _android_transition),
        "platform": attr.label(mandatory = True),
    },
)
