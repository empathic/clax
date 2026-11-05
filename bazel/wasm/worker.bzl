"""The hosted Worker's deployable bundle, as worker-build makes it.

`worker_bundle` builds `wasm` (a cdylib's `<lib>_wasm_shared`) for wasm32
with the root Cargo.toml's `worker-release` profile (opt-level "z", fat LTO,
one codegen unit), then runs bazel/wasm/worker_bundle.mjs over it with the
pinned wasm-bindgen, wasm-opt and esbuild (bazel/wasm/tools.MODULE.bazel).
Its output is one directory, the one wrangler.jsonc's `main` names through
crates/clax-worker/build.sh: index.js, index_bg.wasm, package.json and
worker/shim.mjs.
"""

_RELEASE_FLAGS = ["-Copt-level=z", "-Ccodegen-units=1"]

def _wasm_release_impl(settings, _attr):
    return {
        "//command_line_option:compilation_mode": "opt",
        "//command_line_option:platforms": [Label("//bazel/wasm:wasm32")],
        "@rules_rust//rust/settings:extra_rustc_flag": settings["@rules_rust//rust/settings:extra_rustc_flag"] + _RELEASE_FLAGS,
        "@rules_rust//rust/settings:lto": "fat",
    }

_wasm_release = transition(
    implementation = _wasm_release_impl,
    inputs = ["@rules_rust//rust/settings:extra_rustc_flag"],
    outputs = [
        "//command_line_option:compilation_mode",
        "//command_line_option:platforms",
        "@rules_rust//rust/settings:extra_rustc_flag",
        "@rules_rust//rust/settings:lto",
    ],
)

def _worker_bundle_impl(ctx):
    wasm = [f for f in ctx.attr.wasm[0][DefaultInfo].files.to_list() if f.extension == "wasm"]
    if len(wasm) != 1:
        fail("%s gives %d wasm modules, not one" % (ctx.attr.wasm[0].label, len(wasm)))
    out = ctx.actions.declare_directory(ctx.label.name)
    node = ctx.toolchains["@rules_nodejs//nodejs:toolchain_type"].nodeinfo
    package_json = json.encode({
        "description": ctx.attr.description,
        "files": ["index_bg.wasm", "index.js", "index.d.ts"],
        "license": ctx.attr.license,
        "main": "index.js",
        "name": ctx.attr.package_name,
        "sideEffects": ["./index.js"],
        "type": "module",
        "version": ctx.attr.version,
    })
    args = ctx.actions.args()
    args.add(ctx.file._script)
    args.add("--wasm", wasm[0])
    args.add("--out", out.path)
    args.add("--wasm-bindgen", ctx.executable._wasm_bindgen)
    args.add("--wasm-opt", ctx.executable._wasm_opt)
    args.add("--esbuild", ctx.executable._esbuild)
    args.add("--shim", ctx.file._shim)
    args.add("--sdk-version", ctx.attr.sdk_version)
    args.add("--package-json", package_json)
    ctx.actions.run(
        executable = node.node,
        arguments = [args],
        inputs = depset([wasm[0], ctx.file._script, ctx.file._shim]),
        tools = [ctx.executable._wasm_bindgen, ctx.executable._wasm_opt, ctx.executable._esbuild] + ctx.files._wasm_opt_lib,
        outputs = [out],
        mnemonic = "WorkerBundle",
        progress_message = "Bundling the Worker %{label}",
    )
    return [DefaultInfo(files = depset([out]))]

worker_bundle = rule(
    implementation = _worker_bundle_impl,
    attrs = {
        "description": attr.string(mandatory = True),
        "license": attr.string(mandatory = True),
        "package_name": attr.string(mandatory = True),
        # worker-build's own version, which the producers section names as
        # the workers-rs SDK's.
        "sdk_version": attr.string(mandatory = True),
        "version": attr.string(mandatory = True),
        "wasm": attr.label(mandatory = True, cfg = _wasm_release),
        "_esbuild": attr.label(default = "//bazel/wasm:esbuild", executable = True, allow_single_file = True, cfg = "exec"),
        "_script": attr.label(default = "//bazel/wasm:worker_bundle.mjs", allow_single_file = True),
        "_shim": attr.label(default = "@worker_build_js//:src/js/shim.js", allow_single_file = True),
        "_wasm_bindgen": attr.label(default = "//bazel/wasm:wasm_bindgen", executable = True, allow_single_file = True, cfg = "exec"),
        "_wasm_opt": attr.label(default = "//bazel/wasm:wasm_opt", executable = True, allow_single_file = True, cfg = "exec"),
        "_wasm_opt_lib": attr.label(default = "//bazel/wasm:wasm_opt_lib", cfg = "exec"),
    },
    toolchains = ["@rules_nodejs//nodejs:toolchain_type"],
)

def _wasm_check_transition_impl(settings, _attr):
    return {
        "//command_line_option:platforms": [Label("//bazel/wasm:wasm32")],
        # A warning fails the check, as `-D warnings` does in the native
        # gates (scripts/check-wasm.sh); third-party crates cap their lints.
        "@rules_rust//rust/settings:extra_rustc_flag": settings["@rules_rust//rust/settings:extra_rustc_flag"] + ["-Dwarnings"],
    }

_wasm_check_transition = transition(
    implementation = _wasm_check_transition_impl,
    inputs = ["@rules_rust//rust/settings:extra_rustc_flag"],
    outputs = [
        "//command_line_option:platforms",
        "@rules_rust//rust/settings:extra_rustc_flag",
    ],
)

def _wasm_check_test_impl(ctx):
    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(script, "#!/usr/bin/env bash\n# The libraries built for wasm32 with warnings denied.\nexit 0\n", is_executable = True)
    return [DefaultInfo(
        executable = script,
        runfiles = ctx.runfiles(transitive_files = depset(transitive = [d[DefaultInfo].files for d in ctx.attr.libs])),
    )]

wasm_check_test = rule(
    implementation = _wasm_check_test_impl,
    doc = "A test that passes when every one of `libs` builds for wasm32, warnings denied: scripts/check-wasm.sh's check, under Bazel.",
    attrs = {"libs": attr.label_list(mandatory = True, cfg = _wasm_check_transition)},
    test = True,
)
