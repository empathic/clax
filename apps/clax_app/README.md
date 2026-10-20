# Clax app

The Clax Flutter app for Android and macOS: it runs `clax` on the device and
turns a spoken or typed request into a working app. The design is
`docs/superpowers/specs/2026-10-12-flutter-app-design.md`.

`tool/check.sh` runs the analyzer and the tests (the `app` lane of
`scripts/quality_gates.sh`). The `*_real_test.dart` tests open the
embedded core the library `CLAX_EMBED_LIB` names (Bazel gives them the
host's `//crates/clax-embed:clax_embed_host`) and skip without it.
