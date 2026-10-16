"""Picks one directory out of a target that writes several."""

def _pick_dir_impl(ctx):
    picked = [f for f in ctx.attr.src[DefaultInfo].files.to_list() if f.is_directory and f.short_path.endswith("/" + ctx.attr.dir)]
    if len(picked) != 1:
        fail("%s writes %d directories ending in /%s" % (ctx.attr.src.label, len(picked), ctx.attr.dir))
    return [DefaultInfo(files = depset(picked), runfiles = ctx.runfiles(files = picked))]

pick_dir = rule(
    implementation = _pick_dir_impl,
    attrs = {
        "src": attr.label(mandatory = True),
        "dir": attr.string(mandatory = True),
    },
    doc = "The one directory of `src` whose path ends in `/dir`, so `$(execpath)` names it.",
)
