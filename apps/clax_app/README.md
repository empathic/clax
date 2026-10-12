# Clax app

The Clax Flutter app for Android and macOS: it runs `clax` on the device and
turns a spoken or typed request into a working app. The design is
`docs/superpowers/specs/2026-10-12-flutter-app-design.md`.

`tool/check.sh` runs the analyzer and the tests (the `app` lane of
`scripts/quality_gates.sh`). Tests that start the daemon need
`CLAX_TEST_BIN="$(tool/test-bin.sh)"`.
