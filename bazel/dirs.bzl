"""Picks one directory out of a target that writes several."""

_PLATFORMS = "//command_line_option:platforms"

def _without_node(platform):
    """Whether rules_nodejs has no runtime for `platform`: Android, iOS."""
    return "android" in platform or "//platforms:ios_" in platform

def _built_on_host_impl(settings, _attr):
    # The web builds are the same files for any target, and need a Node
    # runtime for the platform they are built for: a binary built for
    # Android (bazel/android/clax.bzl) or a library built for iOS
    # (bazel/apple/ios_rust.bzl) takes them as built for the host. On the
    # host the configuration is left as it is, so nothing is built twice.
    if any([_without_node(str(p)) for p in settings[_PLATFORMS]]):
        return {_PLATFORMS: ["@bazel_tools//tools:host_platform"]}
    return {_PLATFORMS: settings[_PLATFORMS]}

_built_on_host = transition(
    implementation = _built_on_host_impl,
    inputs = [_PLATFORMS],
    outputs = [_PLATFORMS],
)

def _pick_dir_impl(ctx):
    src = ctx.attr.src[0] if type(ctx.attr.src) == "list" else ctx.attr.src
    picked = [f for f in src[DefaultInfo].files.to_list() if f.is_directory and f.short_path.endswith("/" + ctx.attr.dir)]
    if len(picked) != 1:
        fail("%s writes %d directories ending in /%s" % (src.label, len(picked), ctx.attr.dir))
    return [DefaultInfo(files = depset(picked), runfiles = ctx.runfiles(files = picked))]

pick_dir = rule(
    implementation = _pick_dir_impl,
    attrs = {
        "src": attr.label(mandatory = True, cfg = _built_on_host),
        "dir": attr.string(mandatory = True),
    },
    doc = "The one directory of `src` whose path ends in `/dir`, so `$(execpath)` names it.",
)
