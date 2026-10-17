# The Android half of the speech_to_text overlay (../BUILD.bazel.tpl): what
# rules_flutter generates for it, which a top-level overlay otherwise
# replaces with an empty library.

load("@rules_flutter//flutter:android.bzl", "flutter_android_engine")
load("@rules_java//java:java_library.bzl", "java_library")
load("@rules_kotlin//kotlin:android.bzl", "kt_android_library")

# Private engine target — gives the plugin's Kotlin/Java the FlutterPlugin
# SPI plus the embedding's androidx dependencies (via the engine's
# exports) on the compile classpath. Always arm64; Java bytecode is
# ABI-independent. neverlink, so the APK carries only the app's engine, of
# the ABI flutter_android_app(android_abi=...) names.
flutter_android_engine(
    name = "_engine",
    neverlink = True,
    visibility = ["//visibility:private"],
)

# BuildConfig.java stub. Gradle synthesizes this per Android module
# (matching `android.namespace`). rules_android doesn't, so we generate
# a minimal one — `DEBUG = false`, which is correct for release builds
# and harmless for debug (the engine ABI / mode is decided elsewhere).
genrule(
    name = "_build_config_src",
    outs = ["_build_config/com/csdcorp/speech_to_text/BuildConfig.java"],
    cmd = "cat > $@ <<'EOF'\npackage com.csdcorp.speech_to_text;\npublic final class BuildConfig {\n  public static final boolean DEBUG = false;\n  public static final String LIBRARY_PACKAGE_NAME = \"com.csdcorp.speech_to_text\";\n  public static final String BUILD_TYPE = \"release\";\n}\nEOF\n",
)

java_library(
    name = "_build_config",
    srcs = [":_build_config_src"],
    visibility = ["//visibility:private"],
)

kt_android_library(
    name = "lib",
    srcs = glob(["src/main/**/*.kt", "src/main/**/*.java"], allow_empty = True),
    custom_package = "com.csdcorp.speech_to_text",
    manifest = "src/main/AndroidManifest.xml",
    exports_manifest = 1,
    visibility = ["//visibility:public"],
    deps = [
        ":_engine",
        ":_build_config",
        "@{HUB_NAME}__clock//android:lib",
        "@{HUB_NAME}__flutter//android:lib",
        "@{HUB_NAME}__flutter_web_plugins//android:lib",
        "@{HUB_NAME}__json_annotation//android:lib",
        "@{HUB_NAME}__meta//android:lib",
        "@{HUB_NAME}__speech_to_text_platform_interface//android:lib",
        "@{HUB_NAME}__speech_to_text_windows//android:lib",
        "@{HUB_NAME}__web//android:lib",
    ],
)
