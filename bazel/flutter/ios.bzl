"""The Clax app for iPhone and iPad, with its daemon inside the app's process.

`clax_ios_app` wires rules_flutter's `flutter_ios_app` to the runner in
`apps/clax_app/ios/Runner` (every `*.swift` there, `Info.plist`, the
storyboards, `PrivacyInfo.xcprivacy`) and to `ClaxDaemon`, a Swift
library over the C functions of
`//crates/clax-ios:clax_ios_c`. There is no Xcode project: Bazel is the
only iOS build. `clax_ios_integration_tests` runs the app's simulator
integration tests.
"""

load("@rules_apple//apple:versioning.bzl", "apple_bundle_version")
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
# daemon's static libraries built for a newer one.
IOS_MINIMUM_OS = "17.0"

def clax_ios_app(name, version_name, version_code):
    """The iOS app and its checks, in apps/clax_app.

    Declares:
      <name>        flutter_ios_app (iPhone and iPad, iOS 17.0) with the
                    in-process daemon (`:ios_daemon`); manual
      ios_daemon    swift_library ClaxDaemon importing ClaxIOS
      ios_plugin_frameworks
                    the system frameworks the plugins' Objective-C needs
                    and rules_flutter's generated libraries do not link
      ios_version   apple_bundle_version from pubspec.yaml's version
      ios_app_sim   the simulator app built -c dbg (flutter_ios_simulator_build)
      ios_app_test  tool/ios-app-check.sh over it (macOS only)

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

    apple_bundle_version(
        name = "ios_version",
        build_version = version_code,
        short_version_string = version_name,
    )

    flutter_ios_app(
        name = name,
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
        runner_deps = [
            ":ios_daemon",
            ":ios_plugin_frameworks",
        ],
        tags = ["manual"],
        version = ":ios_version",
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
            "$(rootpaths //crates/clax-ios:clax_ios_builds)",
        ],
        data = [
            ":assets/home/home.clax",
            ":ios_app_sim",
            "//crates/clax-ios:clax_ios_builds",
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
                ":ios_daemon",
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
