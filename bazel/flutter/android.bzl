"""The Clax app's APK: one APK for both Android CPUs it ships, as Gradle builds it.

rules_flutter's `flutter_android_app` builds an APK for one ABI and compiles
the runner's Kotlin without the app's resources. `flutter_fat_apk` composes
rules_flutter's lower-level rules instead:

- for each ABI, the AOT-compiled app (`libapp.so`) from
  `flutter_android_bundle`, and the engine's `libflutter.so`; the engine's
  Java classes once, from the first ABI;
- each ABI's extra executables (`native_files`), packed at `lib/<abi>/`;
- the runner's Kotlin, compiled with the app's resources under its own
  namespace and against its own libraries (`runner_deps`), and the
  generated plugin registrant;
- every pub plugin's Android code (`@<pub>//android:all_android_plugin_libs`);
- the manifest with the permissions the plugins declare lifted into it, as
  Gradle merges them.

The native libraries reach the APK as Java resources of jars (`lib/<abi>/*.so`),
the way rules_flutter packs `libapp.so`; `android_binary` packs a jar's
`lib/**` as it finds it.
"""

load("@bazel_skylib//rules:expand_template.bzl", "expand_template")
load("@rules_android//android:rules.bzl", "android_binary")
load("@rules_flutter//flutter:android.bzl", "flutter_android_engine", "flutter_android_manifest_merge")
load("@rules_flutter//flutter/private:flutter_android_application.bzl", "flutter_android_bundle")
load("@rules_flutter//flutter/private:flutter_android_registrant.bzl", "flutter_android_registrant")
load("@rules_java//java:java_import.bzl", "java_import")
load("@rules_kotlin//kotlin:android.bzl", "kt_android_library")

# rules_flutter's short ABI names, their Android ABI directories, and the
# engine repositories rules_flutter fetches for each.
_ABIS = {
    "arm64": struct(dir = "arm64-v8a", engine = "@flutter_android_engine_arm64//:stripped"),
    "x64": struct(dir = "x86_64", engine = "@flutter_android_engine_x64//:stripped"),
}

def _lib_jar_impl(ctx):
    """A jar of `lib/<abi>/<name>` entries: given files, and a jar's own lib/ files."""
    out = ctx.actions.declare_file(ctx.label.name + ".jar")
    entries = []
    inputs = []
    for target, path in ctx.attr.files.items():
        files = target[DefaultInfo].files.to_list()
        if len(files) != 1:
            fail("%s: %s gives %d files, not one" % (ctx.label, target.label, len(files)))
        entries.append("%s=%s" % (path, files[0].path))
        inputs.append(files[0])
    zipper = ctx.executable._zipper.path
    if ctx.file.lib_from:
        # Only the native libraries of `lib_from` (an engine jar for another
        # ABI): its classes are the first ABI's engine's, already in the APK.
        command = """set -euo pipefail
top="$PWD"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
(cd "$dir" && "$top/{zipper}" x "$top/{jar}")
libs=()
while IFS= read -r f; do libs+=("${{f#$dir/}}=$f"); done < <(find "$dir/lib" -type f | LC_ALL=C sort)
[ "${{#libs[@]}}" -gt 0 ] || {{ echo "no lib/ entries in {jar}" >&2; exit 1; }}
exec "{zipper}" cC "{out}" "${{libs[@]}}" ${{1+"$@"}}
""".format(zipper = zipper, jar = ctx.file.lib_from.path, out = out.path)
        inputs.append(ctx.file.lib_from)
    else:
        command = 'exec "{zipper}" cC "{out}" "$@"'.format(zipper = zipper, out = out.path)
    ctx.actions.run_shell(
        command = command,
        arguments = entries,
        inputs = inputs,
        tools = [ctx.executable._zipper],
        outputs = [out],
        mnemonic = "FlutterLibJar",
        progress_message = "Packing native libraries into %{label}",
    )
    return [DefaultInfo(files = depset([out]))]

lib_jar = rule(
    implementation = _lib_jar_impl,
    attrs = {
        "files": attr.label_keyed_string_dict(
            allow_files = True,
            doc = "Each file's path in the jar, such as lib/arm64-v8a/libclax.so.",
        ),
        "lib_from": attr.label(allow_single_file = [".jar"], doc = "A jar whose lib/ entries are added."),
        "_zipper": attr.label(default = "@bazel_tools//tools/zip:zipper", executable = True, cfg = "exec"),
    },
)

def flutter_fat_apk(
        name,
        application,
        package_name,
        namespace,
        manifest,
        resources,
        kotlin_srcs,
        native_files = {},
        runner_deps = [],
        pub_hub_name = "deps",
        abis = ["arm64", "x64"],
        manifest_values = {},
        **kwargs):
    """An APK of `application` for every ABI in `abis`.

    Args:
        name: The android_binary.
        application: A flutter_application.
        package_name: The application ID.
        namespace: The Java package of the runner's code and its R class
            (Gradle's `namespace`).
        manifest: The runner's AndroidManifest.xml, with `${applicationName}`
            where Flutter's Gradle plugin puts the Application class.
        resources: The runner's res/ files.
        kotlin_srcs: The runner's Kotlin sources.
        native_files: {label: "<abi>/<name>"} for each further native file,
            packed at lib/<abi>/<name>; each label gives one file.
        runner_deps: Further libraries the runner's Kotlin and resources
            use, such as Maven artifacts (@rules_android_maven//:…), which
            Gradle's `dependencies` block names for the native build.
        pub_hub_name: The flutter.pub hub of the app's packages.
        abis: rules_flutter's ABI names; the first one's engine classes ship.
        manifest_values: Passed to android_binary, with applicationId added.
        **kwargs: Passed to android_binary.
    """
    tags = kwargs.pop("tags", ["manual"])
    plugins = "@%s//android:all_android_plugin_libs" % pub_hub_name

    native_libs = []
    for i, abi in enumerate(abis):
        flutter_android_bundle(
            name = "_%s_bundle_%s" % (name, abi),
            application = application,
            android_abi = _ABIS[abi].dir,
            tags = tags,
        )
        native.filegroup(
            name = "_%s_app_libs_%s" % (name, abi),
            srcs = ["_%s_bundle_%s" % (name, abi)],
            output_group = "native_libs_jar",
            tags = tags,
        )
        native_libs.append("_%s_app_libs_%s" % (name, abi))
        if i > 0:
            lib_jar(
                name = "_%s_engine_libs_%s" % (name, abi),
                lib_from = _ABIS[abi].engine,
                tags = tags,
            )
            native_libs.append("_%s_engine_libs_%s" % (name, abi))

    if native_files:
        lib_jar(
            name = "_%s_native_files" % name,
            files = {label: "lib/" + path for label, path in native_files.items()},
            tags = tags,
        )
        native_libs.append("_%s_native_files" % name)

    java_import(
        name = "_%s_native_libs" % name,
        jars = native_libs,
        tags = tags,
    )

    # The engine's classes and the first ABI's libflutter.so.
    flutter_android_engine(
        name = "_%s_engine" % name,
        android_abi = abis[0],
        tags = tags,
    )

    # Flutter's assets are the same for every ABI.
    native.filegroup(
        name = "_%s_flutter_assets" % name,
        srcs = ["_%s_bundle_%s" % (name, abis[0])],
        output_group = "mobile_install",
        tags = tags,
    )

    flutter_android_registrant(
        name = "_%s_registrant_src" % name,
        application = application,
        tags = tags,
    )
    kt_android_library(
        name = "_%s_registrant" % name,
        srcs = ["_%s_registrant_src" % name],
        custom_package = "io.flutter.plugins",
        tags = tags,
        deps = ["_%s_engine" % name, plugins],
    )

    expand_template(
        name = "_%s_manifest" % name,
        template = manifest,
        out = "_%s_AndroidManifest.xml" % name,
        # Gradle resolves the manifest's relative class names (".MainActivity")
        # against the namespace before it renames the package to the
        # application ID; rules_android renames first, so they are made
        # absolute here.
        substitutions = {
            "${applicationName}": "android.app.Application",
            'android:name=".': 'android:name="%s.' % namespace,
        },
        tags = tags,
    )

    # The runner's code with the app's resources, so its R class resolves.
    kt_android_library(
        name = "_%s_runner" % name,
        srcs = kotlin_srcs,
        custom_package = namespace,
        manifest = "_%s_manifest" % name,
        resource_files = resources,
        tags = tags,
        deps = [
            "_%s_engine" % name,
            "_%s_registrant" % name,
            plugins,
        ] + runner_deps,
    )

    deps = [
        "_%s_native_libs" % name,
        "_%s_engine" % name,
        "_%s_registrant" % name,
        "_%s_runner" % name,
        plugins,
    ]

    flutter_android_manifest_merge(
        name = "_%s_manifest_merged" % name,
        base = "_%s_manifest" % name,
        libraries = deps,
        placeholders = ["applicationId", "minSdkVersion", "targetSdkVersion"],
        tags = tags,
    )

    android_binary(
        name = "_%s_binary" % name,
        manifest = "_%s_manifest_merged" % name,
        custom_package = namespace,
        manifest_values = manifest_values | {"applicationId": package_name},
        assets = ["_%s_flutter_assets" % name],
        assets_dir = "assets",
        deps = deps,
        tags = tags,
        **kwargs
    )

    release_apk(
        name = name,
        apk = "_%s_binary" % name,
        tags = tags,
    )

def _release_transition_impl(_settings, _attr):
    return {
        # Gradle's release build type: not debuggable, the AOT snapshot
        # stripped (rules_flutter's -c opt).
        "//command_line_option:compilation_mode": "opt",
        # No desugared JDK library: at the app's minSdk (29) Android has the
        # java.time and java.util.stream APIs it backports, and Gradle's L8
        # shrinks it away; unshrunk it is a second 2.6 MB dex.
        "//command_line_option:desugar_java8_libs": False,
    }

_release_transition = transition(
    implementation = _release_transition_impl,
    inputs = [],
    outputs = [
        "//command_line_option:compilation_mode",
        "//command_line_option:desugar_java8_libs",
    ],
)

def _release_apk_impl(ctx):
    binary = ctx.attr.apk[0]
    apks = [f for f in binary[DefaultInfo].files.to_list() if f.basename == binary.label.name + ".apk"]
    if len(apks) != 1:
        fail("%s gives no %s.apk" % (binary.label, binary.label.name))
    out = ctx.actions.declare_file(ctx.label.name + ".apk")
    ctx.actions.symlink(output = out, target_file = apks[0])
    return [DefaultInfo(files = depset([out]))]

release_apk = rule(
    implementation = _release_apk_impl,
    attrs = {"apk": attr.label(mandatory = True, cfg = _release_transition)},
    doc = "The release build of an android_binary's signed APK, as `<name>.apk`.",
)
