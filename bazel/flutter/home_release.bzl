"""The home app's release on this machine: its bundle's version and signing key.

`@clax_home_release//:version` holds the version the home bundle
(//apps/clax_app:home_bundle) carries and `:mode` whether it is a release
or a development build; `:key` is the seed that signs it. All three come
from apps/clax_app/tool/home-version.sh, which this repository rule runs
with `record`: the version is the last one this machine gave while the
home sources (apps/clax_app/home, apps/clax_app/assets/kit) are unchanged,
and the next one, recorded in the ledger beside the key, when they changed.

The key is this machine's, like the Android debug key
(bazel/android/debug_keystore.bzl): `~/.config/clax/home-bundle/key`, or
the file CLAX_HOME_BUNDLE_KEY names. Without it, and without a ledger (CI),
the bundle is signed by the repository's development key on the
development lineage. The repository is `local` and watches the sources, the
key and the ledger, so it is evaluated again when any of them changes (a
build in another checkout appends to the ledger, and this one then takes a
version above it); the bundle that names it is built locally (`no-remote`).
"""

def _impl(ctx):
    app = ctx.path(Label("//apps/clax_app:home/manifest.json")).dirname.dirname
    home = ctx.getenv("HOME", "")
    xdg = ctx.getenv("XDG_CONFIG_HOME", "") or "%s/.config" % home
    key = ctx.getenv("CLAX_HOME_BUNDLE_KEY", "") or "%s/clax/home-bundle/key" % xdg
    ctx.watch_tree(app.get_child("home"))
    ctx.watch_tree(app.get_child("assets").get_child("kit"))
    ctx.watch(app.get_child("tool").get_child("home-version.sh"))
    ctx.watch(key)
    script = app.get_child("tool").get_child("home-version.sh")
    res = ctx.execute(["bash", str(script), str(app), "record"])
    if res.return_code != 0:
        fail("apps/clax_app/tool/home-version.sh failed:\n" + res.stderr)
    out = {}
    for line in res.stdout.splitlines():
        k, _, v = line.partition("=")
        out[k] = v

    # The ledger as this run left it: a later build elsewhere that appends
    # to it brings this repository up to date.
    ledger = ctx.path("%s/releases" % key.rpartition("/")[0])
    ctx.watch(ledger)
    if out["mode"] == "dev":
        # buildifier: disable=print
        print("The home bundle is signed by the development key (apps/clax_app/tool/home-dev.key): there is no release key at %s" % key)
    ctx.symlink(out["key"], "key")
    ctx.file("version", out["version"] + "\n")
    ctx.file("mode", out["mode"] + "\n")
    ctx.file("BUILD.bazel", """\
exports_files(
    ["key", "mode", "version"],
    visibility = ["//visibility:public"],
)
""")

_home_release = repository_rule(
    implementation = _impl,
    environ = ["CLAX_HOME_BUNDLE_KEY", "HOME", "XDG_CONFIG_HOME"],
    local = True,
)

def _ext_impl(_ctx):
    _home_release(name = "clax_home_release")

home_release = module_extension(implementation = _ext_impl)
