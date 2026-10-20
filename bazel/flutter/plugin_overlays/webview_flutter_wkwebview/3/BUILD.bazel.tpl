# Overlay for `package:webview_flutter_wkwebview` 3.x (bazel/flutter/plugin_overlays,
# registered in MODULE.bazel): the generated BUILD file, with the plugin's
# Apple sources patched by configuration-hook.patch beside this file, which
# adds `FWFWebViewFlutterWKWebViewExternalAPI.configurationHook`, called
# with each WKWebViewConfiguration before the plugin makes a WKWebView with
# it. The runner sets it to register its `clax` scheme handler
# (macos/Runner/SchemeHost.swift, ios/Runner/SchemeHost.swift), which
# WebKit takes only before the view exists. scripts/patch-pub.sh applies
# the same patch for the native builds. The rest is what rules_flutter
# generates.

load("@rules_flutter//flutter:defs.bzl", "flutter_plugin")
load("@rules_flutter//flutter:macos.bzl", "flutter_apple_plugin_library")

flutter_plugin(
    name = "webview_flutter_wkwebview",
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
    plugin_platforms_json = "{\"ios\":{\"dartPluginClass\":\"WebKitWebViewPlatform\",\"pluginClass\":\"WebViewFlutterPlugin\",\"sharedDarwinSource\":true},\"macos\":{\"dartPluginClass\":\"WebKitWebViewPlatform\",\"pluginClass\":\"WebViewFlutterPlugin\",\"sharedDarwinSource\":true}}",
    language_version = "{LANGUAGE_VERSION}",
    version = "{VERSION}",
    apple_libs = select({
        "@platforms//os:macos": [
            ":webview_flutter_wkwebview_apple_macos",
        ],
        "@platforms//os:ios": [
            ":webview_flutter_wkwebview_apple_ios",
        ],
        "//conditions:default": [],
    }),
    apple_privacy_files = select({
        "@platforms//os:macos": [
            "darwin/webview_flutter_wkwebview/Sources/webview_flutter_wkwebview/Resources/PrivacyInfo.xcprivacy",
        ],
        "@platforms//os:ios": [
            "darwin/webview_flutter_wkwebview/Sources/webview_flutter_wkwebview/Resources/PrivacyInfo.xcprivacy",
        ],
        "//conditions:default": [],
    }),
    visibility = ["//visibility:public"],
)

_SOURCES = "darwin/webview_flutter_wkwebview/Sources/webview_flutter_wkwebview/"

# The files the patch changes, patched.
_PATCHED = [
    "WebViewFlutterWKWebViewExternalAPI.swift",
    "WebViewProxyAPIDelegate.swift",
]

genrule(
    name = "patched_sources",
    srcs = [_SOURCES + f for f in _PATCHED] + [
        "@@//bazel/flutter/plugin_overlays:webview_flutter_wkwebview/3/configuration-hook.patch",
    ],
    outs = ["patched/" + f for f in _PATCHED],
    cmd = """set -euo pipefail
dir="$$(mktemp -d)"
trap 'rm -rf "$$dir"' EXIT
mkdir -p "$$dir/{sources}"
for f in {files}; do cat "$$(dirname $(location {first}))/$$f" > "$$dir/{sources}$$f"; done
/usr/bin/patch --quiet --forward -p1 -d "$$dir" < $(location @@//bazel/flutter/plugin_overlays:webview_flutter_wkwebview/3/configuration-hook.patch)
for f in {files}; do cat "$$dir/{sources}$$f" > "$(RULEDIR)/patched/$$f"; done
""".format(
        sources = _SOURCES,
        files = " ".join(_PATCHED),
        first = _SOURCES + _PATCHED[0],
    ),
)

[
    flutter_apple_plugin_library(
        name = "webview_flutter_wkwebview_apple_" + platform,
        srcs = glob(
            [
                _SOURCES + "**/*.swift",
                _SOURCES + "**/*.m",
                _SOURCES + "**/*.mm",
                _SOURCES + "**/*.h",
            ],
            exclude = [
                _SOURCES + "test/**",
                _SOURCES + "example/**",
            ] + [_SOURCES + f for f in _PATCHED],
            allow_empty = True,
        ) + ["patched/" + f for f in _PATCHED],
        includes = [],
        module_name = "webview_flutter_wkwebview",
        platform = platform,
        visibility = ["//visibility:public"],
    )
    for platform in ["macos", "ios"]
]
