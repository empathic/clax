"""A Rust static library for an iOS app, and a check that it builds for iOS.

`ios_opt_library` hands an app the library built optimized (as
`cargo build --release`) in the app's own configuration: rules_apple's
split transition has already chosen the platform (device or simulator)
and `--ios_minimum_os`, so the library and the C its dependencies compile
(ring's, the bundled SQLite's) target what the app links for.
`-Cstrip=symbols` would do nothing here: a static library has no link
step. The device app's binary is stripped where it is linked.

The C toolchain (apple_support) compiles for `--ios_minimum_os`; rustc
reads its deployment target from IPHONEOS_DEPLOYMENT_TARGET instead, so
both transitions here set that from `--ios_minimum_os` for every Rust
crate the library compiles.

`ios_builds` builds the library for each of a list of platforms and checks
each archive's objects: every Mach-O member must carry one
LC_BUILD_VERSION naming that platform (iOS or its simulator) and an iOS no
newer than the configured `--ios_minimum_os`. An object built for a newer
iOS than the app's minimum only makes `ld` warn, and the app could then
fail on the oldest iOS it claims; here it fails the build
(`check-build-version.sh`, a validation action). An older one is allowed:
the Rust standard library comes prebuilt for its target's own minimum
(14.0 for the iOS triples in Rust 1.94).
"""

load("@apple_support//lib:apple_support.bzl", "apple_support")
load("@apple_support//xcode:providers.bzl", "XcodeVersionInfo")
load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")

_MINIMUM_OS = "//command_line_option:ios_minimum_os"
_RUSTC_ENV = "@rules_rust//rust/settings:extra_rustc_env"

def _deployment_target(settings):
    """`extra_rustc_env` with IPHONEOS_DEPLOYMENT_TARGET set to
    `--ios_minimum_os`, which rustc reads for the iOS triples."""
    minimum = settings[_MINIMUM_OS]
    if not minimum:
        fail("--ios_minimum_os is unset: .bazelrc gives the iOS the app runs on")
    env = [e for e in settings[_RUSTC_ENV] if not e.startswith("IPHONEOS_DEPLOYMENT_TARGET=")]
    return env + ["IPHONEOS_DEPLOYMENT_TARGET=" + str(minimum)]

def _opt_transition_impl(settings, _attr):
    return {
        "//command_line_option:compilation_mode": "opt",
        _RUSTC_ENV: _deployment_target(settings),
    }

_opt_transition = transition(
    implementation = _opt_transition_impl,
    inputs = [_MINIMUM_OS, _RUSTC_ENV],
    outputs = ["//command_line_option:compilation_mode", _RUSTC_ENV],
)

def _ios_opt_library_impl(ctx):
    lib = ctx.attr.lib[0]
    return [lib[CcInfo], lib[DefaultInfo]]

ios_opt_library = rule(
    implementation = _ios_opt_library_impl,
    doc = "`lib` (a rust_static_library) built -c opt in the configuration it is depended on from, forwarding its CcInfo and DefaultInfo.",
    attrs = {
        "lib": attr.label(mandatory = True, providers = [CcInfo], cfg = _opt_transition),
    },
)

_IosPlatformInfo = provider(
    doc = "What a configuration targets: its Apple environment and iOS minimum.",
    fields = {
        "environment": "device or simulator",
        "minimum_os": "--ios_minimum_os, dotted (17.0)",
    },
)

def _ios_platform_probe_impl(ctx):
    xcode_config = ctx.attr._xcode_config[XcodeVersionInfo]
    return [_IosPlatformInfo(
        environment = apple_support.target_environment_from_rule_ctx(ctx),
        minimum_os = str(xcode_config.minimum_os_for_platform_type(apple_common.platform_type.ios)),
    )]

ios_platform_probe = rule(
    implementation = _ios_platform_probe_impl,
    doc = "The Apple environment and iOS minimum of the configuration it is built in (bazel/apple:ios_platform_probe).",
    attrs = apple_support.action_required_attrs() | apple_support.platform_constraint_attrs(),
)

def _platforms_split_impl(settings, attr):
    env = _deployment_target(settings)
    return {
        str(p): {"//command_line_option:platforms": str(p), _RUSTC_ENV: env}
        for p in attr.platforms
    }

_platforms_split = transition(
    implementation = _platforms_split_impl,
    inputs = [_MINIMUM_OS, _RUSTC_ENV],
    outputs = ["//command_line_option:platforms", _RUSTC_ENV],
)

# LC_BUILD_VERSION's platform numbers (<mach-o/loader.h>).
_MACHO_PLATFORM = {"device": "2", "simulator": "7"}

def _ios_builds_impl(ctx):
    label = ctx.attr.lib[0].label
    files = []
    checks = []
    for key, lib in ctx.split_attr.lib.items():
        probe = ctx.split_attr._probe[key][_IosPlatformInfo]
        archives = [f for f in lib[DefaultInfo].files.to_list() if f.extension == "a"]
        if len(archives) != 1:
            fail("%s for %s: expected one static library, got %s" % (label, key, archives))
        archive = archives[0]
        # Under a name of its platform's: each build's archive has the same
        # basename and package, so in a test's runfiles one would hide the
        # other.
        named = ctx.actions.declare_file("%s/%s/%s" % (ctx.label.name, Label(key).name, archive.basename))
        ctx.actions.symlink(output = named, target_file = archive)
        files.append(named)
        stamp = ctx.actions.declare_file("%s/%s.build_version" % (ctx.label.name, Label(key).name))
        apple_support.run(
            actions = ctx.actions,
            xcode_config = ctx.attr._xcode_config[XcodeVersionInfo],
            apple_fragment = ctx.fragments.apple,
            executable = ctx.executable._check,
            arguments = [archive.path, _MACHO_PLATFORM[probe.environment], probe.minimum_os, stamp.path],
            inputs = [archive],
            outputs = [stamp],
            mnemonic = "IosBuildVersion",
            progress_message = "Checking %s's build versions for %s" % (label, key),
        )
        checks.append(stamp)
    return [
        DefaultInfo(files = depset(files)),
        OutputGroupInfo(_validation = depset(checks)),
    ]

ios_builds = rule(
    implementation = _ios_builds_impl,
    doc = "`lib` built for each of `platforms`, its archives the rule's outputs (`<name>/<platform>/<archive>`), each checked for its platform and an iOS no newer than --ios_minimum_os.",
    fragments = ["apple"],
    attrs = apple_support.action_required_attrs() | {
        "lib": attr.label(mandatory = True, cfg = _platforms_split),
        "platforms": attr.label_list(mandatory = True, doc = "Apple iOS platforms (@apple_support//platforms:…)."),
        "_check": attr.label(
            default = "//bazel/apple:check_build_version",
            executable = True,
            cfg = "exec",
        ),
        "_probe": attr.label(
            default = "//bazel/apple:ios_platform_probe",
            cfg = _platforms_split,
        ),
    },
)
