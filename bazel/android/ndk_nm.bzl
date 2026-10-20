"""The `nm` of the C++ toolchain that builds for an Android platform.

`ndk_nm` resolves the C++ toolchain for `platform` (the NDK's, which
`MODULE.bazel` registers from `@androidndk`) and gives its `nm` executable:
the NDK's `llvm-nm` for the host that runs the build. No BUILD file names
the NDK's per-host directory (`toolchains/llvm/prebuilt/<host>`), so
`bazel query` over `//...` loads on every host.
"""

load("@rules_cc//cc:find_cc_toolchain.bzl", "find_cc_toolchain", "use_cc_toolchain")

def _platform_transition_impl(_settings, attr):
    return {"//command_line_option:platforms": str(attr.platform)}

_platform_transition = transition(
    implementation = _platform_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:platforms"],
)

def _ndk_nm_impl(ctx):
    cc_toolchain = find_cc_toolchain(ctx)
    path = cc_toolchain.nm_executable
    nm = [f for f in cc_toolchain.all_files.to_list() if f.path == path]
    if len(nm) != 1:
        fail("%s: the C++ toolchain for %s has no file %s among its files" % (ctx.label, ctx.attr.platform.label, path))
    return [DefaultInfo(files = depset(nm), runfiles = ctx.runfiles(files = nm))]

ndk_nm = rule(
    implementation = _ndk_nm_impl,
    attrs = {
        "platform": attr.label(mandatory = True, doc = "An Android platform, such as //bazel/android:android_arm64."),
    },
    cfg = _platform_transition,
    toolchains = use_cc_toolchain(),
    fragments = ["cpp"],
)
