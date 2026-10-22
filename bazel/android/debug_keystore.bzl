"""The Android debug keystore, read from this machine: ~/.android/debug.keystore.

The Clax app's APKs are signed with the debug key, as its Gradle build
signs its release build (apps/clax_app/android/app/build.gradle.kts), so a
Bazel-built APK installs over a Gradle-built one, keeping the app's data.
That key is this machine's, not the repository's: the one input of the APK
not pinned by a checksum. The repository is `local`, refetched when the
file changes, and the APK that names it is built locally (`no-remote`).

`@android_debug_keystore//:key` is that file, or, on a machine without one
(CI), rules_android's own debug key: such an APK builds and is checked, but
does not install over one signed on a developer's machine.
"""

def _impl(ctx):
    home = ctx.getenv("HOME", "")
    key = ctx.path("%s/.android/debug.keystore" % home)
    ctx.watch(key)
    if key.exists:
        ctx.symlink(key, "debug.keystore")
        actual = ":debug.keystore"
    else:
        actual = "@rules_android//tools/android:debug_keystore"
    ctx.file("BUILD.bazel", """\
alias(
    name = "key",
    actual = "{actual}",
    visibility = ["//visibility:public"],
)
""".format(actual = actual))

_debug_keystore = repository_rule(
    implementation = _impl,
    environ = ["HOME"],
    local = True,
)

def _ext_impl(_ctx):
    _debug_keystore(name = "android_debug_keystore")

debug_keystore = module_extension(implementation = _ext_impl)
