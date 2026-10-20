"""A binary built optimized, as `cargo build --release` builds it.

`opt_binary` builds `binary` with compilation_mode `opt` (Cargo's release
profile with no `[profile.release]`: opt-level 3, no debug information,
16 codegen units from .bazelrc) and names it by a symbolic link, so nothing
is copied and nothing is hard linked.
"""

def _opt_transition_impl(_settings, _attr):
    return {"//command_line_option:compilation_mode": "opt"}

_opt_transition = transition(
    implementation = _opt_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:compilation_mode"],
)

def _opt_binary_impl(ctx):
    src = ctx.attr.binary[0][DefaultInfo].files_to_run.executable
    out = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.symlink(output = out, target_file = src, is_executable = True)
    return [DefaultInfo(
        executable = out,
        files = depset([out]),
        runfiles = ctx.runfiles(files = [out, src]),
    )]

opt_binary = rule(
    implementation = _opt_binary_impl,
    attrs = {
        "binary": attr.label(mandatory = True, executable = True, cfg = _opt_transition),
    },
    executable = True,
)

def _opt_library_impl(ctx):
    return [ctx.attr.lib[0][DefaultInfo]]

opt_library = rule(
    implementation = _opt_library_impl,
    doc = "`lib` (a library, its files as DefaultInfo) built with compilation_mode `opt`.",
    attrs = {
        "lib": attr.label(mandatory = True, cfg = _opt_transition),
    },
)
