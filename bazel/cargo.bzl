"""Builds a Cargo package's targets the way Cargo does.

A package's BUILD.bazel calls cargo_package(). The package's facts (version,
targets, default features, path dependencies, its build script, its own
lints) come from bazel/workspace.bzl, which
scripts/bazel-sync.sh derives from `cargo metadata`; its third-party
dependencies come from its Cargo.toml through crate_universe; and its sources
are globbed. So a BUILD file changes only for what Cargo knows by convention
and Bazel must be told: which files outside `src/` a target reads.
"""

load("@crates//:defs.bzl", "aliases", "all_crate_deps")
load("@rules_rust//cargo:defs.bzl", "cargo_build_script")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_library", "rust_shared_library", "rust_static_library", "rust_test")
load(":workspace.bzl", "CRATES", "WORKSPACE_RUSTC_FLAGS")

_CLAX = "//crates/clax-cli:clax"
_CLAX_WARM = "//crates/clax-cli:clax_warm"
_TESTKIT = "//crates/clax-testkit:clax_testkit"

def cargo_package(
        compile_data = [],
        test_compile_data = [],
        test_data = [],
        tests_use_clax = [],
        test_env = {},
        shard_counts = {},
        split_tests = {},
        build_script_env = {},
        build_script_data = [],
        staticlib = False,
        cdylib = False,
        disable_pipelining = False,
        visibility = ["//visibility:public"]):
    """Declares the library, binaries, build script and tests of the Cargo
    package here.

    Targets, with <lib> the library's crate name:
      <lib>                 the library (tests link it too: no crate turns on
                            another's features for tests)
      <lib>_lib_test        its unit tests
      <bin>                 each binary, under Cargo's name for it
      <bin>_bin_test        each binary's unit tests
      <test>                each [[test]] target, under its name
      build_script          the build script, when there is one
      <lib>_static          with `staticlib`: the library as a C static
                            library (lib<lib>.a: the crate and every
                            dependency in one archive)
      <lib>_shared          with `cdylib`: the library as a C shared
                            library (lib<lib>.so or .dylib)

    Args:
      compile_data: files outside `src/` that the library and binaries read
        while they compile (include_str!, rust-embed).
      test_compile_data: the same, for the tests only.
      test_data: files the tests read while they run. Paths built from
        CARGO_MANIFEST_DIR resolve as they do under Cargo.
      tests_use_clax: the test targets (by name, including `<lib>_lib_test`
        and `<bin>_bin_test`) that run the `clax` binary: they get it, its
        warm-up stamp and CLAX_TEST_BIN.
      test_env: environment variables for every test.
      shard_counts: {test name: shards}.
      split_tests: {[[test]] name: {"groups": N}}: the modules of tests/
        (every one a module of that test's root, which checks it),
        each gated in the root by
        `#[cfg(any(not(clax_split), clax_split = "<module>"))]`, are dealt
        in name order into N test targets, `<test>__1` … `<test>__N`, with
        the root's own tests in the first; `common` is in all. Each target
        compiles and links its modules alone, so they build and run in
        parallel, and an edit to one test file rebuilds one target. N trades
        that against what every test binary repeats: codegen of the shared
        dependencies' generics, and a link.
      build_script_env: environment for the build script ($(execpath) works).
      build_script_data: files the build script reads.
      staticlib: also declare `<lib>_static`, a rust_static_library of the
        library's sources and dependencies, for a host app to link
        (crates/clax-embed, on iOS).
      cdylib: also declare `<lib>_shared`, a rust_shared_library of the
        library's sources and dependencies, for a host app to load
        (crates/clax-embed).
      disable_pipelining: build the library's metadata and its rlib in one
        action, not two. For a crate whose compile reads paths under the
        action's own sandbox (crates/clax-embedded: rust-embed's folders
        come from its build script's absolute paths, which differ in every
        sandbox): its two actions would give it two crate hashes, and a
        library compiled against the metadata could not be linked with the
        rlib ("can't find crate").
      visibility: the library's and binaries' visibility.
    """
    package = native.package_name()
    crate = CRATES[package]

    # Every Rust source of the crate, for source scans (//scripts:no_unsafe).
    native.filegroup(
        name = "rust_sources",
        srcs = native.glob(["**/*.rs"], allow_empty = True),
        visibility = ["//visibility:public"],
    )

    # The crate's manifest, for the layering check (//scripts:crate_layers).
    native.filegroup(
        name = "manifest",
        srcs = ["Cargo.toml"],
        visibility = ["//visibility:public"],
    )

    # The manifest itself, for the release script test (bazel/release_files.bzl):
    # scripts/bump-version.sh reads which crates take the workspace version.
    native.exports_files(["Cargo.toml"], visibility = ["//visibility:public"])
    features = crate["default_features"]
    lib = crate["lib"]

    rustc_env = {
        # rules_rust's default is the sandbox's absolute path at compile
        # time: gone when a test runs, and different in every build, so it
        # would poison the shared cache. The package's path from the
        # workspace root is stable, and tests run from the workspace root.
        "CARGO_MANIFEST_DIR": package,
        "CARGO_PKG_NAME": crate["name"],
        "CARGO_PKG_VERSION": crate["version"],
        # No cargo runs under Bazel; a test's fallback that would run it
        # fails loudly (spec §5.3).
        "CARGO": "/nonexistent/cargo-under-bazel",
        # Every Bazel build names no commit (spec B13).
        "CLAX_BUILD_COMMIT": "unknown",
    }
    # The cfg names and features Cargo declares to rustc's check-cfg.
    check_cfg = [
        "--check-cfg=cfg(docsrs,test)",
        "--check-cfg=cfg(feature,values(%s))" % ",".join(['"%s"' % f for f in crate["features"]]),
    ]
    # The workspace's lints, or the crate's own [lints.rust] when it has
    # one (only crates/clax-embed: scripts/check-no-unsafe.sh).
    lint_flags = crate.get("lint_flags", WORKSPACE_RUSTC_FLAGS)
    rustc_flags = lint_flags + check_cfg + ["--cfg", "clax_embedded"] + select({
        # Cargo's dev profile keeps line tables, which panics and
        # backtraces use (docs/bazel.md, "Differences from Cargo").
        "//bazel:fastbuild": ["-Cdebuginfo=line-tables-only"],
        "//conditions:default": [],
    })
    common = dict(
        aliases = aliases(),
        edition = crate["edition"],
        version = crate["version"],
        rustc_env = rustc_env,
        rustc_flags = rustc_flags,
    )

    build_deps = []
    if crate["has_build_script"]:
        cargo_build_script(
            name = "build_script",
            srcs = ["build.rs"],
            build_script_env = {"CLAX_BUILD_COMMIT": "unknown"} | build_script_env,
            data = build_script_data,
            edition = crate["edition"],
            deps = all_crate_deps(build = True),
            proc_macro_deps = all_crate_deps(build_proc_macro = True),
            rustc_flags = lint_flags + check_cfg,
            version = crate["version"],
        )
        build_deps = [":build_script"]

    normal_deps = crate["deps"] + all_crate_deps(normal = True) + build_deps
    dev_deps = crate["dev_deps"] + all_crate_deps(normal_dev = True)

    # The testkit serves integration tests only: it depends on the crates it
    # tests, so a unit test that linked it would wait for its crate's own
    # library to compile first.
    unit_dev_deps = [d for d in dev_deps if d != _TESTKIT]
    macros = all_crate_deps(proc_macro = True)
    dev_macros = all_crate_deps(proc_macro_dev = True)
    srcs = native.glob(["src/**/*.rs"])
    src_data = native.glob(
        ["src/**"],
        exclude = ["src/**/*.rs", "**/.qual"],
        allow_empty = True,
    ) + compile_data

    # An integration test compiles no generated file, so its sources stay
    # in the source tree, where its includes of other crates' files resolve.
    own_data = native.glob(
        ["src/**"],
        exclude = ["src/**/*.rs", "**/.qual"],
        allow_empty = True,
    )

    # Every test may read its crate's own files, as tests under Cargo do.
    own_files = native.glob(
        ["src/**", "tests/**"],
        exclude = ["**/.qual"],
        allow_empty = True,
    ) + ["Cargo.toml"]

    def test_kwargs(name, integration = False, uses_clax = None):
        data = own_files + test_data
        env = dict(test_env)
        if name in tests_use_clax or uses_clax in tests_use_clax:
            data += [_CLAX, _CLAX_WARM]
            env["CLAX_TEST_BIN"] = "$(rootpath %s)" % _CLAX
        kwargs = dict(
            common,
            # A test may include the crate's own sources (include_str!).
            compile_data = _uniq((own_data if integration else src_data) + srcs + test_compile_data),
            data = data,
            env = env,
        )
        if name in shard_counts:
            kwargs["shard_count"] = shard_counts[name]
        return kwargs

    if lib:
        rust_library(
            name = lib,
            srcs = srcs,
            compile_data = src_data,
            crate_features = features,
            crate_name = lib,
            crate_root = crate["lib_root"],
            disable_pipelining = disable_pipelining,
            proc_macro_deps = macros,
            visibility = visibility,
            deps = normal_deps,
            **common
        )
        if staticlib:
            rust_static_library(
                name = lib + "_static",
                srcs = srcs,
                compile_data = src_data,
                crate_features = features,
                crate_name = lib,
                crate_root = crate["lib_root"],
                proc_macro_deps = macros,
                visibility = visibility,
                deps = normal_deps,
                **common
            )
        if cdylib:
            rust_shared_library(
                name = lib + "_shared",
                srcs = srcs,
                compile_data = src_data,
                crate_features = features,
                crate_name = lib,
                crate_root = crate["lib_root"],
                proc_macro_deps = macros,
                visibility = visibility,
                deps = normal_deps,
                **common
            )
        _test(
            name = lib + "_lib_test",
            crate = ":" + lib,
            proc_macro_deps = dev_macros,
            deps = unit_dev_deps,
            **test_kwargs(lib + "_lib_test")
        )

    for bin, root in crate["bins"].items():
        rust_binary(
            name = bin,
            srcs = srcs,
            compile_data = src_data,
            crate_features = features,
            crate_root = root,
            proc_macro_deps = macros,
            visibility = visibility,
            deps = ([":" + lib] if lib else []) + normal_deps,
            **common
        )
        _test(
            name = bin + "_bin_test",
            srcs = srcs,
            crate_features = features,
            crate_name = bin.replace("-", "_"),
            crate_root = root,
            proc_macro_deps = macros + dev_macros,
            deps = ([":" + lib] if lib else []) + normal_deps + unit_dev_deps,
            **test_kwargs(bin + "_bin_test")
        )

    test_srcs = native.glob(["tests/**/*.rs"], allow_empty = True)
    roots = [r.split("/")[-1][:-len(".rs")] for r in crate["tests"].values()]
    for name, root in crate["tests"].items():
        if name in split_tests:
            stems = [f.split("/")[-1][:-len(".rs")] for f in native.glob(["tests/*.rs"], allow_empty = True)]
            stems += [f.split("/")[-2] for f in native.glob(["tests/*/mod.rs"], allow_empty = True)]
            spec = split_tests[name]
            modules = ["root"] + sorted([m for m in stems if m not in roots and m != "common"])
            groups = spec["groups"]
            for g in range(groups):
                cfgs = ["--cfg", "clax_split"]
                for module in modules[g::groups]:
                    cfgs += ["--cfg", 'clax_split="%s"' % module]
                kwargs = test_kwargs(name, integration = True, uses_clax = name)
                kwargs["rustc_flags"] = rustc_flags + cfgs
                _test(
                    name = "%s__%d" % (name, g + 1),
                    srcs = test_srcs,
                    crate_features = features,
                    crate_name = name.replace("-", "_"),
                    crate_root = root,
                    proc_macro_deps = macros + dev_macros,
                    deps = ([":" + lib] if lib else []) + normal_deps + dev_deps,
                    **kwargs
                )
            continue
        _test(
            name = name,
            srcs = test_srcs,
            crate_features = features,
            crate_name = name.replace("-", "_"),
            crate_root = root,
            proc_macro_deps = macros + dev_macros,
            deps = ([":" + lib] if lib else []) + normal_deps + dev_deps,
            **test_kwargs(name, integration = True)
        )

def _uniq(labels):
    """`labels` without repeats, in order: a label may be both a crate's
    compile data and its tests'."""
    return [x for i, x in enumerate(labels) if x not in labels[:i]]

def _test(name, **kwargs):
    """A rust_test with this workspace's defaults."""
    rust_test(
        name = name,
        # Bazel's default is five minutes; a loaded developer machine needs
        # more for the largest suites. CI sets its own (.bazelrc, ci).
        timeout = "long",
        **kwargs
    )
