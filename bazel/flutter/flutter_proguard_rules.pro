# Flutter's R8 rules, which its Gradle plugin adds to every release build:
# packages/flutter_tools/gradle/flutter_proguard_rules.pro of Flutter 3.47.7
# (BSD 3-clause, Copyright 2014 The Flutter Authors).

# Build the ephemeral app in a module project.
# Prevents: Warning: library class <plugin-package> depends on program class io.flutter.plugin.**
# This is due to plugins (libraries) depending on the embedding (the program jar)
-dontwarn io.flutter.plugin.**

# The android.** package is provided by the OS at runtime.
-dontwarn android.**

# In some cases, R8 is incorrectly stripping plugin classes. Keep
# all implementations of FlutterPlugin until we can determine
# why this is the case.
# See https://github.com/flutter/flutter/issues/154580.
-if class * implements io.flutter.embedding.engine.plugins.FlutterPlugin
-keep,allowshrinking,allowobfuscation class <1>
