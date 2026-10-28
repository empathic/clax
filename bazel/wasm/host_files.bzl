"""Files built for the machine the build runs on, whatever the target.

A wasm32 build of the Worker embeds the web UI (//web:dist), which builds
with Node and so only for a host: `host_files` builds its `srcs` in the
exec configuration and passes their files on.
"""

def _host_files_impl(ctx):
    return [DefaultInfo(files = depset(transitive = [s[DefaultInfo].files for s in ctx.attr.srcs]))]

host_files = rule(
    implementation = _host_files_impl,
    attrs = {"srcs": attr.label_list(cfg = "exec", mandatory = True)},
)
