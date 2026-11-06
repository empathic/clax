"""The C compiler and archiver a build script uses for wasm32.

Two crates the Worker builds compile C for wasm32-unknown-unknown: ring and
sqlite-wasm-rs (SQLite). cc-rs reads `CC_wasm32_unknown_unknown` and
`AR_wasm32_unknown_unknown` before `CC` and `AR`, and the Android NDK's
clang has a WebAssembly backend, so the NDK Bazel already fetches for the
app compiles them (MODULE.bazel's annotations of ring and sqlite-wasm-rs in
@crates_wasm, which only wasm32 builds use).

`wasm_c_tools` gives a build script, as one of its `toolchains`, the Make
variables WASM32_CC and WASM32_AR: the NDK's clang and llvm-ar.
"""

def _wasm_c_tools_impl(ctx):
    return [
        DefaultInfo(files = depset([ctx.file.clang, ctx.file.ar], transitive = [f.files for f in ctx.attr.files])),
        # Absolute, as the build script's runner expands ${pwd}: cc-rs runs
        # the compiler from the crate's directory.
        platform_common.TemplateVariableInfo({
            "WASM32_AR": "$${pwd}/" + ctx.file.ar.path,
            "WASM32_CC": "$${pwd}/" + ctx.file.clang.path,
        }),
    ]

wasm_c_tools = rule(
    implementation = _wasm_c_tools_impl,
    attrs = {
        "ar": attr.label(allow_single_file = True, cfg = "exec", mandatory = True),
        "clang": attr.label(allow_single_file = True, cfg = "exec", mandatory = True),
        # What clang reads beside itself: its resource directory's headers.
        "files": attr.label_list(cfg = "exec"),
    },
)
