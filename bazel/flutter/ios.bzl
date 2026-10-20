"""The Clax app for iPhone and iPad, with its core inside the app's process.

`clax_ios_app` wires rules_flutter's `flutter_ios_app` to the runner in
`apps/clax_app/ios/Runner` (every `*.swift` there, `Info.plist`, the
storyboards, `PrivacyInfo.xcprivacy`) and links the embedded core,
`//crates/clax-embed:clax_embed_ios_c`, into the runner with its four C
functions exported, which Dart calls by FFI through
`DynamicLibrary.process()`; no Swift module calls them. There is no Xcode
project: Bazel is the only iOS build. `clax_ios_integration_tests` runs the app's simulator
integration tests.
"""

load("@rules_apple//apple:providers.bzl", "apple_provider")
load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("@rules_flutter//flutter:defs.bzl", "flutter_application")
load("@rules_flutter//flutter:ios.bzl", "flutter_ios_app", "flutter_ios_simulator_build")
load("@rules_shell//shell:sh_test.bzl", "sh_test")
load("@rules_swift//swift:swift_library.bzl", "swift_library")
load(":macos.bzl", "integration_main")

# The app's bundle ID, pending the owner's confirmation with the Apple
# Developer account (iOS spec, open question 2).
IOS_BUNDLE_ID = "dev.empathic.clax"

# The oldest iOS the app runs on, as .bazelrc's --ios_minimum_os gives it
# to every iOS configuration: :ios_app_test fails on an object of the
# core's static libraries built for a newer one.
IOS_MINIMUM_OS = "17.0"

def _bundle_version_impl(ctx):
    build = ctx.var.get("clax_build_number", ctx.attr.build_version)
    if not build.isdigit():
        fail("--define=clax_build_number=%s: a build number is a whole number" % build)
    out = ctx.actions.declare_file(ctx.label.name + ".bundle_version")
    ctx.actions.write(out, json.encode({
        "build_version": build,
        "short_version_string": ctx.attr.short_version_string,
    }))
    return [
        apple_provider.make_apple_bundle_version_info(version_file = out),
        DefaultInfo(files = depset([out])),
    ]

# rules_apple's apple_bundle_version takes the build number as a fixed
# string; this one lets `--define=clax_build_number=<n>` replace it, so a
# TestFlight build carries the number scripts/app-iphone.sh takes from its
# ledger while every other build keeps pubspec.yaml's.
_bundle_version = rule(
    implementation = _bundle_version_impl,
    attrs = {
        "build_version": attr.string(mandatory = True),
        "short_version_string": attr.string(mandatory = True),
    },
    doc = "CFBundleVersion and CFBundleShortVersionString, as AppleBundleVersionInfo.",
)

def clax_ios_app(name, version_name, version_code):
    """The iOS app and its checks, in apps/clax_app.

    Declares:
      <name>        flutter_ios_app (iPhone and iPad, iOS 17.0) with the
                    core linked in (clax_embed_ios_c); manual
      ios_daemon    swift_library ClaxDaemon importing ClaxIOS, the earlier
                    in-process daemon's; no app links it
      ios_intents   swift_library ClaxIntents: the App Intent "Comment in
                    Clax" and the inbox the runner's IntentsChannel takes
                    it from; linked into the runner and its metadata
                    extracted into the bundle's Metadata.appintents
      ios_plugin_frameworks
                    the system frameworks the plugins' Objective-C needs
                    and rules_flutter's generated libraries do not link
      ios_version   the bundle version: pubspec.yaml's version name, and its
                    build number unless `--define=clax_build_number=<n>`
                    gives another (scripts/app-iphone.sh --testflight)
      ios_app_sim   the simulator app built -c dbg (flutter_ios_simulator_build)
      ios_app_test  tool/ios-app-check.sh over it (macOS only)
      ipa_dev, ipa_appstore
                    the app signed for a device with the team's development
                    profile, and for TestFlight with its App Store profile
                    (`@clax_apple_signing`, bazel/apple/signing.bzl); manual,
                    never cached, and built with `--ios_multi_cpus=arm64`

    Args:
        name: The app's target name, a bare Swift identifier (the runner's
            module is `<name>Runner`).
        version_name: pubspec.yaml's version name (CFBundleShortVersionString).
        version_code: pubspec.yaml's build number (CFBundleVersion).
    """
    swift_library(
        name = "ios_daemon",
        srcs = ["ios/Daemon/ClaxDaemon.swift"],
        module_name = "ClaxDaemon",
        tags = ["manual"],
        target_compatible_with = ["@platforms//os:ios"],
        deps = ["//crates/clax-ios:clax_ios_c"],
    )

    swift_library(
        name = "ios_intents",
        srcs = ["ios/Intents/CommentInClax.swift"],
        # rules_apple's app_intents reads the linked SDK frameworks to find
        # the intents library: it must name AppIntents.
        linkopts = [
            "-framework",
            "AppIntents",
        ],
        module_name = "ClaxIntents",
        tags = ["manual"],
        target_compatible_with = ["@platforms//os:ios"],
    )

    # share_plus's podspec links LinkPresentation and image_picker_ios's
    # kUTType constants are MobileCoreServices', but the libraries
    # rules_flutter generates for them link neither: the app fails to link
    # on their symbols without these.
    cc_library(
        name = "ios_plugin_frameworks",
        linkopts = [
            "-framework",
            "LinkPresentation",
            "-framework",
            "MobileCoreServices",
        ],
        tags = ["manual"],
        target_compatible_with = ["@platforms//os:ios"],
    )

    _bundle_version(
        name = "ios_version",
        build_version = version_code,
        short_version_string = version_name,
    )

    app = dict(
        app_intents = [":ios_intents"],
        app_name = "Clax",
        application = ":app",
        bundle_id = IOS_BUNDLE_ID,
        families = [
            "iphone",
            "ipad",
        ],
        minimum_os_version = IOS_MINIMUM_OS,
        # The app's own privacy manifest, at the bundle's root beside the
        # plugins' (rules_flutter adds those).
        resources = ["ios/Runner/PrivacyInfo.xcprivacy"],
        # ios_intents is linked here, and its metadata extracted through
        # app_intents (ios_application's, through flutter_ios_app's
        # **kwargs): app_intents does not link.
        runner_deps = [
            "//crates/clax-embed:clax_embed_ios_c",
            ":ios_intents",
            ":ios_plugin_frameworks",
        ],
        version = ":ios_version",
    )

    flutter_ios_app(
        name = name,
        tags = ["manual"],
        **app
    )

    # The same app signed with the team's profiles. A device build needs the
    # files of ~/.config/clax/apple (docs/usage.md, "Signing for a device and
    # TestFlight"); without them @clax_apple_signing's targets fail, naming
    # each missing file. Nothing else depends on these two, so the simulator
    # app builds without the directory. Every action of theirs runs here and
    # is kept out of every cache: the profiles and the signed app never
    # leave this machine.
    for kind, profile in [("dev", "development"), ("appstore", "appstore")]:
        flutter_ios_app(
            name = "ipa_" + kind,
            entitlements = "@clax_apple_signing//:entitlements_" + profile,
            provisioning_profile = "@clax_apple_signing//:" + profile,
            tags = [
                "manual",
                "no-cache",
                "no-remote",
            ],
            **app
        )

    # The simulator engine runs only debug builds.
    flutter_ios_simulator_build(
        name = "ios_app_sim",
        tags = ["manual"],
        targets = [":" + name],
    )

    sh_test(
        name = "ios_app_test",
        srcs = ["tool/ios-app-check.sh"],
        args = [
            "$(rootpath :ios_app_sim)",
            "$(rootpath :assets/home/home.clax)",
            "$(rootpaths //crates/clax-embed:clax_embed_ios_builds)",
        ],
        data = [
            ":assets/home/home.clax",
            ":ios_app_sim",
            "//crates/clax-embed:clax_embed_ios_builds",
        ],
        env = {
            "IOS_MINIMUM_OS": IOS_MINIMUM_OS,
            "VERSION_CODE": version_code,
            "VERSION_NAME": version_name,
        },
        tags = ["no-remote"],
        target_compatible_with = ["@platforms//os:macos"],
    )

def clax_ios_integration_tests(assets, deps):
    """The simulator integration tests, a target per
    `integration_test/ios_<name>_test.dart` in apps/clax_app.

    Each test is the entry point (`integration_main`) of a debug app of its
    own, built for the simulator as `:ios_app` is, and run on "Clax iPhone"
    by `tool/ios-integration-run.sh`, which installs it fresh, reads its
    console and acts on what the test asks of it. They boot a simulator and
    take minutes, so they are `manual`: `just app-integration --ios` runs
    them. Declares, per test:
      _integration_ios_<name>_main  the entry point
      _integration_ios_<name>_app   its flutter_application
      _integration_ios_<name>       flutter_ios_app; manual
      _integration_ios_<name>_sim   its simulator build (-c dbg)
      integration_ios_<name>        the sh_test; manual, macOS only

    Args:
        assets: The app's assets (`:app`'s).
        deps: The application's Dart dependencies.
    """
    for test in native.glob(["integration_test/ios_*_test.dart"]):
        n = "ios_" + test[len("integration_test/ios_"):-len("_test.dart")]
        integration_main(
            name = "_integration_%s_main" % n,
            srcs = native.glob(["integration_test/*.dart"]),
            test = test,
            tags = ["manual"],
        )
        flutter_application(
            name = "_integration_%s_app" % n,
            package_name = "clax_app",
            srcs = native.glob(["lib/**/*.dart"]) + [":_integration_%s_main" % n],
            assets = assets,
            main = ":_integration_%s_main.dart" % n,
            tags = ["manual"],
            deps = deps,
        )
        flutter_ios_app(
            name = "_integration_%s" % n,
            app_name = "Clax",
            application = ":_integration_%s_app" % n,
            bundle_id = IOS_BUNDLE_ID,
            families = [
                "iphone",
                "ipad",
            ],
            minimum_os_version = IOS_MINIMUM_OS,
            runner_deps = [
                "//crates/clax-embed:clax_embed_ios_c",
                ":ios_intents",
                ":ios_plugin_frameworks",
            ],
            tags = ["manual"],
            version = ":ios_version",
        )
        flutter_ios_simulator_build(
            name = "_integration_%s_sim" % n,
            tags = ["manual"],
            targets = [":_integration_%s" % n],
        )
        sh_test(
            name = "integration_%s" % n,
            timeout = "long",
            srcs = ["tool/ios-integration-run.sh"],
            args = [
                "$(rootpath :_integration_%s_sim)" % n,
                "$(rootpath //scripts:app-ios.sh)",
            ],
            data = [
                ":_integration_%s_sim" % n,
                "//scripts:app-ios.sh",
            ],
            tags = [
                "exclusive",
                "manual",
                "no-remote",
                "no-sandbox",
            ],
            target_compatible_with = ["@platforms//os:macos"],
        )
