# Overlay for `package:speech_to_text` 7.x (bazel/flutter/plugin_overlays,
# registered in MODULE.bazel): the generated BUILD file, with the Swift
# package its Package.swift depends on, CwlCatchException, as a dependency
# of its Apple code (the root module's @cwl_catch_exception, named by its
# canonical name, which a pub package's repository can see;
# bazel/flutter/third_party). The rest is what rules_flutter generates.

load("@rules_flutter//flutter:defs.bzl", "flutter_plugin")
load("@rules_flutter//flutter:macos.bzl", "flutter_apple_plugin_library")
flutter_plugin(
    name = "speech_to_text",
    srcs = glob(["lib/**/*.dart"], allow_empty = True),
    resources = glob(
        ["lib/**"],
        exclude = ["lib/**/*.dart"],
        allow_empty = True,
    ),
    deps = [
        {DEPS}
    ],
    package_name = "{PKG}",
    plugin_platforms_json = "{\"android\":{\"package\":\"com.csdcorp.speech_to_text\",\"pluginClass\":\"SpeechToTextPlugin\"},\"ios\":{\"pluginClass\":\"SpeechToTextPlugin\",\"sharedDarwinSource\":true},\"macos\":{\"pluginClass\":\"SpeechToTextPlugin\",\"sharedDarwinSource\":true},\"web\":{\"fileName\":\"speech_to_text_web.dart\",\"pluginClass\":\"SpeechToTextPlugin\"},\"windows\":{\"default_package\":\"speech_to_text_windows\"}}",
    language_version = "{LANGUAGE_VERSION}",
    apple_libs = select({
        "@platforms//os:macos": [
            ":speech_to_text_apple_macos",
        ],
        "@platforms//os:ios": [
            ":speech_to_text_apple_ios",
        ],
        "//conditions:default": [],
    }),
    visibility = ["//visibility:public"],
)

flutter_apple_plugin_library(
    name = "speech_to_text_apple_macos",
    srcs = glob(
        [
        "darwin/speech_to_text/Sources/speech_to_text/**/*.swift",
        "darwin/speech_to_text/Sources/speech_to_text/**/*.m",
        "darwin/speech_to_text/Sources/speech_to_text/**/*.mm",
        "darwin/speech_to_text/Sources/speech_to_text/**/*.h",
        ],
        exclude = [
        "darwin/speech_to_text/Sources/speech_to_text/test/**",
        "darwin/speech_to_text/Sources/speech_to_text/example/**",
        ],
        allow_empty = True,
    ),
    includes = [],
    module_name = "speech_to_text",
    platform = "macos",
    deps = ["@@+http_archive+cwl_catch_exception//:CwlCatchException"],
    visibility = ["//visibility:public"],
)

flutter_apple_plugin_library(
    name = "speech_to_text_apple_ios",
    srcs = glob(
        [
        "darwin/speech_to_text/Sources/speech_to_text/**/*.swift",
        "darwin/speech_to_text/Sources/speech_to_text/**/*.m",
        "darwin/speech_to_text/Sources/speech_to_text/**/*.mm",
        "darwin/speech_to_text/Sources/speech_to_text/**/*.h",
        ],
        exclude = [
        "darwin/speech_to_text/Sources/speech_to_text/test/**",
        "darwin/speech_to_text/Sources/speech_to_text/example/**",
        ],
        allow_empty = True,
    ),
    includes = [],
    module_name = "speech_to_text",
    platform = "ios",
    deps = ["@@+http_archive+cwl_catch_exception//:CwlCatchException"],
    visibility = ["//visibility:public"],
)
