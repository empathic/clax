"""The third-party notices a Flutter app's licences page shows (NOTICES).

`flutter build` gathers every pub package's LICENSE and the engine's
licences into the bundle's NOTICES.Z, which `LicenseRegistry` reads and
`showLicensePage` lists. rules_flutter writes NOTICES.Z only from
`flutter_application`'s `license_files`, so `flutter_notices` makes that
one file, in the form `LicenseRegistry` parses: entries split by a line
of 80 dashes, each the names of the packages it covers, one per line, a
blank line, and the licence text. Packages with the same licence text
share an entry, as `flutter build` does.

It holds:
- each pub package in `pub_licenses`, by its LICENSE (the hub's
  `<package>__license` alias, which bazel/flutter/rules_flutter.patch
  adds; `pub_licenses()` makes the attribute). `pub_licenses_test`
  checks that they are exactly the packages the app's Dart code reaches
  (its `deps`, through the `deps` of each library), naming any missing or
  extra one, so the list stays in step with pubspec.lock. The check is a
  target of its own so that NOTICES, whose bytes no Dart source changes,
  does not depend on the app's Dart in the build graph (`bazel query
  rdeps`, scripts/affected-tests.sh);
- the engine's licences (`engine_license`, sky_engine's LICENSE, already
  in this form);
- the third-party Rust crates linked into each binary of `rust_binaries`
  (the embedded clax), by the licence files at each crate's root
  (LICENSE*, LICENCE*, COPYING*, NOTICE*, UNLICENSE*), which
  crate_universe's `compile_data` carries, or, for a crate published
  without one, the licence its Cargo.toml names. Build scripts and proc macros,
  which run at build time only, are left out.
"""

_LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE")

PubPackagesInfo = provider(
    doc = "The pub packages (spoke repository names) a Dart target reaches.",
    fields = {"repos": "depset of canonical repository names"},
)

RustLicensesInfo = provider(
    doc = "The licence files of the third-party crates a Rust target links.",
    fields = {"files": "depset of Files"},
)

def _children(ctx, attrs, info):
    out = []
    for name in attrs:
        v = getattr(ctx.rule.attr, name, None)
        if v == None:
            continue
        for t in (v if type(v) == "list" else [v]):
            if type(t) == "Target" and info in t:
                out.append(t[info])
    return out

def _pub_aspect_impl(target, ctx):
    repo = target.label.repo_name
    direct = [repo] if repo else []
    return [PubPackagesInfo(repos = depset(
        direct,
        transitive = [c.repos for c in _children(ctx, ["deps"], PubPackagesInfo)],
    ))]

_pub_aspect = aspect(
    implementation = _pub_aspect_impl,
    attr_aspects = ["deps"],
)

def _rust_aspect_impl(target, ctx):
    if ctx.rule.kind in ("cargo_build_script", "rust_proc_macro"):
        return [RustLicensesInfo(files = depset())]
    direct = []
    if ctx.rule.kind == "rust_library" and "crates__" in target.label.repo_name:
        root = [
            f
            for f in getattr(ctx.rule.files, "compile_data", [])
            if f.owner.package == "" and "/" not in f.short_path.split("/", 2)[-1]
        ]
        direct = [f for f in root if f.basename.upper().startswith(_LICENSE_PREFIXES)]

        # A crate published without its licence text: its Cargo.toml, whose
        # `license` names it.
        if not direct:
            direct = [f for f in root if f.basename == "Cargo.toml"]
    return [RustLicensesInfo(files = depset(
        direct,
        transitive = [c.files for c in _children(ctx, ["binary", "deps"], RustLicensesInfo)],
    ))]

_rust_aspect = aspect(
    implementation = _rust_aspect_impl,
    attr_aspects = ["binary", "deps"],
)

def _crate_of(f):
    # crate_universe's repositories are named `<prefix>crates__<name>-<version>`.
    return "%s\t%s" % (f.owner.repo_name.rpartition("crates__")[2], f.path)

def _flutter_notices_impl(ctx):
    rust = depset(transitive = [b[RustLicensesInfo].files for b in ctx.attr.rust_binaries])
    out = ctx.actions.declare_file(ctx.label.name + "/NOTICES")
    args = ctx.actions.args()
    args.add("--out", out)
    args.add("--engine", ctx.file.engine_license)
    pub_files = []
    for target, names in ctx.attr.pub_licenses.items():
        f = target.files.to_list()[0]
        pub_files.append(f)
        for name in names.split(" "):
            args.add("--pub", "%s\t%s" % (name, f.path))
    args.add_all(rust, before_each = "--crate", map_each = _crate_of)
    ctx.actions.run(
        executable = ctx.executable._tool,
        arguments = [args],
        inputs = depset(pub_files + [ctx.file.engine_license], transitive = [rust]),
        outputs = [out],
        mnemonic = "FlutterNotices",
        progress_message = "Writing the third-party notices of %{label}",
    )
    return [DefaultInfo(files = depset([out]))]

flutter_notices = rule(
    implementation = _flutter_notices_impl,
    doc = "Writes the NOTICES file of a Flutter app (see the module's doc).",
    attrs = {
        "pub_licenses": attr.label_keyed_string_dict(
            allow_files = True,
            mandatory = True,
            doc = "Each LICENSE, and the pub packages it covers, space-separated (`pub_licenses`).",
        ),
        "engine_license": attr.label(allow_single_file = True, mandatory = True),
        "rust_binaries": attr.label_list(aspects = [_rust_aspect], doc = "The Rust binaries the app embeds."),
        "_tool": attr.label(
            default = "//bazel/flutter:notices_tool",
            executable = True,
            cfg = "exec",
        ),
    },
)

def _pub_licenses_test_impl(ctx):
    prefix = ctx.attr.hub + "__"
    reached = {}
    for d in ctx.attr.deps:
        for repo in d[PubPackagesInfo].repos.to_list():
            name = repo.rpartition(prefix)[2] if prefix in repo else ""
            if name:
                reached[name] = True
    listed = {}
    for names in ctx.attr.pub_licenses.values():
        for p in names.split(" "):
            listed[p] = True
    missing = sorted([p for p in reached if p not in listed])
    extra = sorted([p for p in listed if p not in reached])
    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    if missing or extra:
        body = "cat >&2 <<'EOF'\n%s: pub_licenses must name exactly the pub packages the app's Dart code reaches.\n  add: %s\n  remove: %s\nEOF\nexit 1\n" % (ctx.label, missing, extra)
    else:
        body = "exit 0\n"
    ctx.actions.write(script, "#!/bin/sh\n" + body, is_executable = True)
    return [DefaultInfo(executable = script)]

pub_licenses_test = rule(
    implementation = _pub_licenses_test_impl,
    doc = "Passes when `pub_licenses` names exactly the pub packages `deps` reach (see the module's doc).",
    attrs = {
        "deps": attr.label_list(aspects = [_pub_aspect], doc = "The app's Dart libraries."),
        "hub": attr.string(mandatory = True, doc = "The pub hub's name, as MODULE.bazel's flutter.pub gives it."),
        "pub_licenses": attr.label_keyed_string_dict(
            allow_files = True,
            mandatory = True,
            doc = "`flutter_notices`'s `pub_licenses`.",
        ),
    },
    test = True,
)

def pub_licenses(hub, packages, license_of = {}):
    """The `pub_licenses` of `flutter_notices` for `packages`.

    Args:
        hub: The pub hub's name.
        packages: The pub packages the app ships.
        license_of: The package whose LICENSE covers a package with none of
            its own (an SDK package covered by `flutter`'s).

    Returns:
        A dict of each `@<hub>//:<package>__license` to the packages it covers.
    """
    out = {}
    for p in packages:
        label = "@%s//:%s__license" % (hub, license_of.get(p, p))
        out[label] = (out[label] + " " + p) if label in out else p
    return out
