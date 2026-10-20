# Building with Bazel

Clax builds everything with Bazel when Bazel is installed: every `just`
recipe (`ci`, `check`, `test`, `web-test`, `pi-test`, `web-e2e`, `lint`,
`build`, `web`, `dev`, `watch`, `serve`, `install`, the Clax app's
`app-test`, `app-apk`, `app-macos`, …), in every worktree, and CI. The point is the whole experience in the common case: about five
worktrees at once, with a shared cache one of them has already filled. A
new worktree then needs no setup (no `pnpm install`, no web build, no
warm-up step) and no `target/` or `node_modules`: `just ci` and `just test`
work at once, from the cache, and the worktree costs next to no disk until
it edits ("Five worktrees", below, has the numbers).

Cargo, `Cargo.lock`, each `package.json` and each `pnpm-lock.yaml` stay the
source of truth; Bazel's view of them is derived and checked. The native
tools still work: on a machine without Bazel the recipes fall back to them
and say so, the `-native` twins (`just ci-native`, `just test-native`, …)
and plain `cargo`/`pnpm` commands run them on purpose, and the release
workflow uses them.

## Use

Install [Bazelisk](https://github.com/bazelbuild/bazelisk) (`brew install
bazelisk`), which reads `.bazelversion` and fetches that Bazel, and
`ibazel` (`brew install ibazel`) for `just watch`. That is all the setup.

The recipes go through `scripts/run.sh`, which picks the build system by
what is on `PATH`:

| On `PATH` | What runs |
|---|---|
| `bazelisk`, or `bazel` at the version `.bazelversion` names | Bazel, for every recipe: what CI runs |
| Neither (a `bazel` of another version is not used: it would fail far from the cause) | The native tools (`scripts/quality_gates.sh`, `cargo`, `pnpm`), saying so before the task and in its last line |
| Neither Bazel nor the native tools | Nothing: one message names both ways to install |

`CLAX_BUILD` overrides the choice: `bazel` uses Bazel or fails with the
install command; `native` never looks for Bazel (what the `-native` twins
set); `auto` is the default. `scripts/run.sh which` says what `auto` picks,
and `scripts/run.sh --dry-run <task>` prints what a task would run
(`scripts/run.test.sh` tests the choice). CI does not go through
`scripts/run.sh`: it calls `scripts/bazel-test.sh --ci`, so a runner
without Bazel fails and never falls back.

In a worktree, use the recipes: no `cargo build`, `cargo test` or
`pnpm install` except through a `-native` twin. Each of those makes a
`target/` or a `node_modules` in the worktree, gigabytes Bazel's shared
cache does not need.

### The dev loop

- `just watch` runs `ibazel run //crates/clax-cli:clax -- serve …`: Bazel
  rebuilds the debug `clax`, which embeds the web UI, and restarts it on any
  Rust or web change; reload the browser after. Measured: about 12 s after a
  web shell edit, 30 s after a clax-core edit, on a loaded machine. Without
  Bazel it is the native watch (cargo-watch and Vite).
- `just dev <harness>` runs the harness with Bazel's debug `clax`
  (`scripts/run.sh bin` builds it and prints its path).
- `just serve`, `just stop`, `just doctor` run that `clax`
  (`scripts/run.sh clax …`).
- `just install` installs Bazel's optimized `clax` (`//crates/clax-cli:clax_opt`)
  as `$CARGO_HOME/bin/clax`, by a rename, then stops the agents' daemon if
  it ran the old one and runs `clax init`. Its `--version` names no commit
  ("Differences from Cargo"). `just install-native` is Cargo's build.

### rust-analyzer

`just rust-project` writes `rust-project.json` (git-ignored) from Bazel with
rules_rust's `gen_rust_project`; rerun it after adding a crate or a
dependency. Point rust-analyzer at it, and its check at Bazel, so the editor
never runs `cargo check` into a `target/`; for VS Code, in
`.vscode/settings.json`:

```json
{
  "rust-analyzer.linkedProjects": ["rust-project.json"],
  "rust-analyzer.check.overrideCommand": [
    "bazelisk", "build", "--@rules_rust//rust/settings:error_format=json", "//crates/..."
  ]
}
```

| Instead of | Run |
|---|---|
| `scripts/quality_gates.sh` | `just ci` (`scripts/bazel-test.sh --ci`) |
| `cargo nextest run --workspace` | `just test`, or `scripts/bazel-test.sh --rust` |
| `cargo nextest run -p clax-server -E 'test(shell)'` | `bazelisk test //crates/clax-server:integration --test_arg=shell` |
| `cargo build -p clax-cli` | `bazelisk build //crates/clax-cli:clax` (`bazel-bin/crates/clax-cli/clax`) |
| `pnpm run lint`, `typecheck`, `test` in `web/` | `just web-test`, or `bazelisk test //web:lint //web:typecheck //web:unit` |
| `pnpm run e2e` in `web/` | `just web-e2e` (`//web:e2e_gesture`, `//web:e2e_chrome_overlay`, `//web:e2e_rest`) |
| `scripts/build-web.sh` | `just web` (copies of Bazel's build into `web/dist` and `web/dist-extension`) |

Use `scripts/bazel-test.sh` rather than `--config=clippy` by hand: it gives
`crates/clax-commands` its own `clippy.toml`, which Bazel takes only one of per
command. It holds off idle sleep on macOS while it runs; a plain
`bazelisk test` does not (`caffeinate -i bazelisk test …` does).

## Keeping Bazel in step with Cargo

Most changes need nothing: a new source file, a new module under `tests/`,
a new use of a crate already in a `Cargo.toml`. A change to any
`Cargo.toml` or `Cargo.lock` needs `just bazel-sync`, committed with it.
`scripts/bazel-sync.sh` writes:

- `bazel/workspace.bzl`: each crate's version, targets (`[[test]]`
  included), features, path dependencies and dev-dependencies, its build
  script, and the workspace lints as rustc flags (a dev-dependency that
  turns a workspace crate's features on is refused, not derived);
- `bazel/release_files.bzl`: `scripts/test-release.files` as labels;
- `bazel/playwright/browsers.bzl`: the Chromium the locked Playwright runs,
  with checksums (rewritten only when the locked version moves);
- `MODULE.bazel.lock`.

`scripts/bazel-sync.sh --check` (the first step of every Bazel run, and of
CI) writes nothing and fails naming `just bazel-sync` when any of these is
stale. It also fails when `MODULE.bazel`'s Rust version differs from
`rust-toolchain.toml`, a workspace crate has no `BUILD.bazel`, `.bazelrc`
limits jobs or cores, or a third-party crate's features under Bazel differ
from `cargo tree --workspace`'s beyond `NATIVE_EXTRA` in
`scripts/bazel-derive.py`. A merge conflict in a derived file is resolved
by running the sync on the merged manifests. `scripts/bazel-sync.test.sh`
tests the check.

## Targets

Each crate's `BUILD.bazel` calls `cargo_package()` (`bazel/cargo.bzl`),
which declares, with `<lib>` the library's crate name:

- `<lib>`; no crate has a test-only feature or variant: test support is
  the `clax-testkit` crate, and `scripts/bazel-derive.py` refuses a
  dev-dependency that turns features on;
- `<lib>_lib_test` (unit tests), each binary and its `<bin>_bin_test`,
  and each `[[test]]` under its name (`//crates/clax-cli:descriptors`);
- for the `integration` tests of clax-server, clax-cli and clax-mcp, the
  modules of `tests/integration.rs` dealt into a few targets (six, three
  and two: `//crates/clax-server:integration__1` … `__6`; `split_tests`).
  Each compiles the shared `common` module and its own modules, so the
  groups build and run in parallel and an edit to one test file rebuilds
  one group. More groups would repeat more of what every test binary
  pays (codegen of the dependencies' generics, a link): one target per
  module made a clax-core edit compile eight times as much. The root gates
  each module with `#[cfg(any(not(clax_split), clax_split = "<module>"))]`;
  Cargo sets no `clax_split` and builds them all into one binary, as before;
- `build_script` when the crate has one;
- `rust_sources`, for source scans, and `manifest`, the crate's
  `Cargo.toml`, for the layering check.

A BUILD file names only what a crate reads outside `src/`: what it embeds,
what its tests read. `//crates/clax-cli:clax_warm` runs the binary once
("macOS first runs"); `//crates/clax-cli:clax_opt` is the optimized build
(`bazel/opt.bzl`) the perf benchmarks run and `--ci` builds, as the native
checks build the release binary. `//web:dist` (with `dist_extension`,
`dist_extension_test`, `dist_e2e`) is the web build; `//web:lint`,
`//web:typecheck`, `//web:unit`, `//web:bundle_size`,
`//plugins/pi:typecheck` and `//plugins/pi:test` the web and Pi checks;
`//scripts:*` the script gates. The perf benchmarks
(`//scripts:perf_daemon_full`, `//scripts:perf_clients_full`,
`//web:perf_usable_full`) are `manual`: performance is measured on demand
(`just perf`, which reruns them and prints their numbers) and gates nothing.

A crate whose compile inputs include a generated directory has its
sources placed in `bazel-bin` by rules_rust, so the repository files its
sources include sit there too (the `*_bin` copies in the BUILD files).
`clax-embedded` is the one such crate: it alone embeds the web UI, the
extension and the plugins, so a web edit recompiles it and the binary, not
the daemon. Which crate may depend on which is
`scripts/check-crate-layers.py` (`//scripts:crate_layers`).

## macOS first runs

macOS assesses every new executable on its first run, which can take
seconds on a loaded machine. Bazel makes a new file for `clax` in each
output base, so `warm_binary` (`bazel/warm.bzl`) runs it in an action that
is never taken from any cache (`no-cache`; `no-remote-cache` does not keep
an action out of the disk cache), and every test that runs `clax` depends
on that action's stamp. Once the file is assessed the action costs one
`clax --version`, and its unchanged stamp reruns nothing. Tests then run the same
file through runfiles links. The fake executables keep one store across
tests and runs (`CLAX_FAKE_EXE_DIR`, set by `scripts/bazel-test.sh`). Tests
find `clax` through `clax_fake_exe::clax_bin`, which makes `CLAX_TEST_BIN`
absolute.

## The web side and Pi

`web/` and `plugins/pi/` install with pnpm (`scripts/pnpm-install-stamped.sh`
natively). Under Bazel, rules_js fetches each project's packages from its
lockfile (`@npm_web`, `@npm_pi`) and runs the project's own `package.json`
scripts through `bazel/js/npm_script.mjs` (taken from `../changelist`): in
a private copy of exactly the declared files, so a deleted module or
package is really gone (`scripts/bazel-web.test.sh` checks this). Inputs
named in `skips` (the warmed `clax`) are read where Bazel put them rather
than copied. `scripts/pnpm-lock-check.py` fails when a `package.json` and
its lockfile disagree. Node is the toolchain `MODULE.bazel` pins (22, as
CI's native job uses); the real-Pi Rust tests get it as `CLAX_TEST_NODE`.

## Browser tests

The browser tests and time to usable run on a Chromium pinned by checksum
(`@chromium//:browsers`, `bazel/playwright/repo.bzl`): the build the locked
Playwright would download, laid out as `PLAYWRIGHT_BROWSERS_PATH` expects.
Nothing downloads while a test runs. `web/e2e/bazel-env.ts` makes the paths
Bazel hands the tests absolute. The targets are tagged `no-sandbox`: under
macOS's sandbox the `rest` project ran in 31 minutes instead of one, and
user-activation tests timed out. In CI,
`bazelisk run //web:playwright -- install-deps chromium` installs the
system libraries Chromium needs.

## The Clax app

`apps/clax_app` (Flutter, for Android, macOS, iPhone and iPad) builds and tests under
Bazel with [rules_flutter](https://github.com/aran/rules_flutter) (0.0.6)
and [rules_dart](https://github.com/aran/rules_dart) (0.7.3), patched and
extended here (`bazel/flutter/`, `bazel/android/`). rules_flutter was
chosen because it is maintained (a release a week, on Bazel 9 and the
Flutter 3.47.7 the app uses), fetches Flutter, its engines and the pub
packages by checksum, and builds an APK with rules_android rather than
Gradle; it is alpha, so its API may move with a version bump. Nothing
comes from `~/Library` or a `PATH` install but Xcode (rules_apple's, for
macOS) and the two keys below.

| Target (`//apps/clax_app:…`) | What it is | Native twin |
|---|---|---|
| `<dir>_<file>` (127, one per `test/**/*_test.dart`) | `flutter test` of that file, in a `flutter_tester` of its own; the `*_real_test.dart` ones against the warmed debug `clax` | `flutter test` |
| `dart_analyze` | `flutter analyze`: the app's library, and its tests, integration tests and tools (`:checked_sources`), by `analysis_options.yaml`; any diagnostic fails | `flutter analyze` |
| `prompt_sync`, `version_sync` | the prompt's copies of the contract; pubspec.yaml's version is the one the BUILD file gives the APK and the macOS app | `scripts/sync-app-prompt.sh --check` |
| `apk`, `apk_test` | the release APK for arm64-v8a and x86_64 (`bazel/flutter/android.bzl`), and a check of it against what Gradle makes | `flutter build apk` |
| `macos_app`, `macos_app_test` | `Clax.app` (a zip) with the embedded `clax` signed as a sandboxed helper (`bazel/flutter/macos.bzl`), and a check of it; macOS only | `flutter build macos` |
| `ios_app`, `ios_app_sim`, `ios_app_test` | `Clax.app` for iPhone and iPad (an `.ipa`; `manual`) with the daemon in its own process (`bazel/flutter/ios.bzl`), the simulator build of it (`-c dbg`), and a check of that build: its Info.plist, with pubspec.yaml's version (the iOS form of `:version_sync`), the `.clax` document type and the words of each permission prompt, Flutter's frameworks, each plugin's registration and privacy manifest, the home bundle, the daemon's four C functions, and every object of the daemon's static libraries for an iOS no newer than the app's minimum; macOS only | none |
| `ipa_dev`, `ipa_appstore` (manual) | the iOS app signed for a device with the team's development profile, and for TestFlight with its App Store profile (`@clax_apple_signing`); built with `-c opt --ios_multi_cpus=arm64`, never cached | none |
| `macos_about_test` | the About panel's credits (`macos/Runner/About.swift`), an XCTest bundle run by `xcodebuild`; macOS only | none |
| `macos_dialogs_test` | the Mac's menus and alerts (`macos/Runner/NativeDialogs.swift`): items, key equivalents, button order and keys, the suppression button's labels and the armed default, an XCTest bundle run by `xcodebuild`; macOS only | none |
| `integration_<name>` (manual) | `integration_test/<name>_test.dart` in the debug app, run in the logged-in session | `flutter test <file> -d macos` |
| `integration_ios_<name>` (manual) | `integration_test/ios_<name>_test.dart` in a debug iOS app of its own, installed fresh and run on the simulator "Clax iPhone" by `tool/ios-integration-run.sh` (`bazel/flutter/ios.bzl`); macOS only | none |
| `integration_android_<name>` (manual) | `integration_test/android_<name>_test.dart` in a debug APK of its own (arm64, the runner of `apk`), installed fresh and run on the emulator "Clax Pixel" by `tool/android-integration-run.sh` (`bazel/flutter/android.bzl`), which reads the test's result from the log; macOS on Apple silicon only | none |
| `scheme_host_sync` | `macos/Runner/SchemeHost.swift` and `ios/Runner/SchemeHost.swift` are the same file | none |
| `home_bundle`, `home_version_test` | the home app's bundle, `assets/home/home.clax` and `release.json`, packed from `home/` by `pack_home` (compiled by rules_dart) and signed: an asset of `app`, so of the APK and the macOS app; and a check of how its version is chosen | `scripts/build-home-bundle.sh` |
| `//crates/clax-cli:clax_android_arm64`, `:clax_android_x86_64` | `clax` optimized and stripped for each Android CPU with the NDK, as `<abi>/libclax.so` (`bazel/android/clax.bzl`) | `scripts/build-app-clax.sh android` |
| `//crates/clax-ios:clax_ios_c`, `:ios_build_test` | the in-process daemon as a static library for iOS (`libclax_ios.a`, optimized in the app's configuration) with its C header, and a check that it builds for iPhone and the simulator with every object for an iOS no newer than `--ios_minimum_os` (`bazel/apple/ios_rust.bzl`); macOS only | none |
| `//crates/clax-embed:clax_embed_android_arm64`, `:clax_embed_android_x86_64`, `:android_symbols_test` | the embedded core (`crates/clax-embed`) optimized and stripped for each Android CPU with the NDK, as `<abi>/libclax_embed.so` (`bazel/android/embed.bzl`), and a check with the NDK's `llvm-nm` that each exports exactly its four C functions | none |
| `//crates/clax-embed:clax_embed_macos`, `:clax_embed_host` | the embedded core as an optimized `libclax_embed.dylib` for the Mac app's `Frameworks` (macOS only), and the same library in the test configuration, which Dart tests load from `CLAX_EMBED_LIB` | none |
| `//crates/clax-embed:clax_embed_ios_c`, `:ios_build_test` | the embedded core as a static library for iOS, optimized in the app's configuration, with its C header and its four C functions kept and exported from the runner (`-exported_symbol`), and a check that it builds for iPhone and the simulator with every object for an iOS no newer than `--ios_minimum_os` (`bazel/apple/ios_rust.bzl`); macOS only | none |

Everything but the integration tests is in `just ci` (`tests(//...)`).
The integration tests open the app's window and take minutes each, so
`just app-integration` runs them on demand (`--ios` the simulator ones,
`--android` the emulator ones).

**The Android integration tests** run on an emulator Bazel fetches, never
on a plugged-in phone. `scripts/app-android-emulator.sh` (a dry run
without `--i-know-what-i-am-doing`; `//scripts:app_android_emulator_test`
tests it on stubs) makes the AVD "Clax Pixel" (android-36, Google APIs,
arm64-v8a) under `target/android-avd/` by writing its `config.ini` and
`.ini` (no `avdmanager`), links the pinned SDK parts into
`target/android-sdk/`, and keeps the emulator's and adb's home in
`target/android-home/`, so nothing reaches `~/.android`. `--start` boots
it headless (`--window` to look at it) on the first free even port from
5554 and prints `serial=emulator-<port> pid=<pid>`; `--stop <pid>` ends
only an emulator it started, by `adb emu kill`, then the PID. Every adb
call names the emulator's serial and goes to an adb server of its own on
the emulator's port + 10000, never the one on 5037 a phone is attached
to. The first run unpacks the system image into the output base (4.4 GB)
and the AVD's data image into `target/android-avd/`; it boots in about
30 s on an M-series Mac.

**What is fetched, and from where (`MODULE.bazel`).**

- Flutter 3.47.7 and its engines (`flutter.toolchain`), and every package
  of `apps/clax_app/pubspec.lock` by its pub.dev checksum (`flutter.pub`,
  the `@clax_app_pub` hub). Bazel never writes the lockfile:
  `bazelisk run @rules_flutter//flutter:pub -- get --directory apps/clax_app`
  resolves it with the pinned Flutter.
- The NDK r28c (`rules_android_ndk`, patched by
  `bazel/android/rules_android_ndk.patch` to fetch a pinned archive and to
  keep its C++ runtime archives apart from the static libc, which rules_rust
  would otherwise link), and the Rust standard library for
  `aarch64-linux-android` and `x86_64-linux-android`; crate_universe
  resolves the crates for those triples too.
- The Rust standard library for `aarch64-apple-ios` and
  `aarch64-apple-ios-sim`, and `apple_support`'s platforms and C toolchain
  (Xcode's). `.bazelrc`'s `--ios_minimum_os=17.0` is the one minimum: the
  C toolchain compiles for it, and `bazel/apple/ios_rust.bzl` passes it to
  rustc as `IPHONEOS_DEPLOYMENT_TARGET`. The web builds a library for iOS
  embeds are built for the host, as for Android (`bazel/dirs.bzl`).
- The Android SDK's build tools 36.0.0, platform android-36 and platform
  tools (`rules_android`, patched by `bazel/android/rules_android.patch` to
  fetch pinned archives instead of reading `ANDROID_HOME`).
- The Android emulator (37.2.12, the stable one for macOS on Apple
  silicon) and the android-36 Google APIs arm64-v8a system image
  (`@android_emulator`, `@android_system_image`), which only the Android
  integration tests fetch.
- A JDK (rules_java's remote JDKs: 21 for the app's Java, 25 for the tools),
  the Go toolchain rules_android builds its tools with, and the Maven
  libraries the plugins' Gradle builds name, pinned in
  `bazel/android/maven_install.json`.
- CwlCatchException, the Swift package speech_to_text's Apple code needs
  (`bazel/flutter/third_party`, added to it by the overlay in
  `bazel/flutter/plugin_overlays/speech_to_text`).
- webview_flutter_wkwebview's Apple sources with a patch
  (`bazel/flutter/plugin_overlays/webview_flutter_wkwebview/3/configuration-hook.patch`,
  applied by that overlay) that lets the runner set its `clax` scheme
  handler on each WebView's configuration before the view exists. The
  native builds get the same patch from `scripts/patch-pub.sh`, which
  applies it to the package in a pub cache of the checkout's own,
  `apps/clax_app/build-native/pub-cache`.

Three inputs are this machine's, not pinned: the Android debug key
(`~/.android/debug.keystore`, `bazel/android/debug_keystore.bzl`; without
one, rules_android's own signs the APK, which then does not install over
an APK signed on a developer's machine); the home app's release
(`bazel/flutter/home_release.bzl`, the `@clax_home_release` repository):
its signing key, `~/.config/clax/home-bundle/key`, and the ledger of the
versions this machine gave the home bundle, `releases` beside it; and the
iOS app's signing inputs (`bazel/apple/signing.bzl`, the
`@clax_apple_signing` repository): the team ID, the two provisioning
profiles and the App Store Connect API key in `~/.config/clax/apple/`
(`docs/usage.md`, "Signing for a device and TestFlight"). Only `ipa_dev`
and `ipa_appstore` use them, built here and never cached (`no-remote`,
`no-cache`); without them those two stop, naming each missing file, and
everything else builds.

**The home bundle's version** is chosen at each build, with nothing to
bump or commit: `apps/clax_app/tool/home-version.sh` takes a SHA-256 over
`home/` and `assets/kit/` and keeps the ledger's last version while the
sources have its digest, else records the next one (above
`home/manifest.json`'s `version` too). So an APK or macOS app whose home
sources changed carries a higher version than any earlier build on this
machine, and the installed app updates its home app from it (the trust
rules, `lib/bundle/trust.dart`); a rebuild of the same sources gives the
same version and the same bytes. The release is `<release>.<version>`
(`release` in the manifest names the series). `@clax_home_release` is a
`local` repository that watches the sources, the key and the ledger, so
it is evaluated again when one changes, a build in another checkout
included; the bundle is built locally (`no-remote`). Without the key, on
a machine with no ledger (CI), the bundle is signed by the development key
(`tool/home-dev.key`, whose seed is public) on the development lineage
(`dev_lineage`): a separate app to an install of the released home app,
which never updates from it. A machine with a ledger and no key stops,
naming the key's path. Keep `~/.config/clax/home-bundle` backed up: a lost
key means a new lineage, and a lost ledger means the versions start again
from the manifest's, below what installs have (raise `version` in
`home/manifest.json` past the last one shipped).

**rules_flutter's patches** (`bazel/flutter/rules_flutter.patch`), each
small enough to offer upstream: an app's assets are keyed by their path in
the app's package, not the workspace, so an app below the root
(`apps/clax_app`) finds `assets/kit/…`; a plugin gets the Android code of
its pub dependencies (image_picker_android needs
flutter_plugin_android_lifecycle's) and every Maven coordinate its Gradle
build names, not only those rules_flutter lists; `integration_test`
comes from the Flutter SDK as `flutter_test` does; and each pub package's
LICENSE, and the engine's, are exported (the hub's `<package>__license`).

**The licences page.** `flutter build` puts every pub package's licence and
the engine's into the bundle's NOTICES.Z; rules_flutter writes only the
`license_files` it is given. `//apps/clax_app:notices`
(`bazel/flutter/notices.bzl`) writes them in the form `LicenseRegistry`
reads: the pub packages of `APP_PUB_PACKAGES`, which must be exactly those
the app's Dart code reaches (the build names any it gains or loses), the
engine's, and the licence files of the Rust crates linked into the embedded
clax. The APK's and the macOS app's checks look for it; the Maven and
Swift libraries' notices are not in it.

**How the APK is assembled.** rules_flutter's `flutter_android_app` builds
one ABI and compiles the runner's Kotlin without the app's resources, so
`flutter_fat_apk` (`bazel/flutter/android.bzl`) composes its lower-level
rules: per ABI the AOT `libapp.so` (`flutter_android_bundle`) and
`libflutter.so`, the engine's classes once, `libclax.so` for each ABI,
the runner's Kotlin with `res/` under its namespace (`dev.empathic.clax_app`),
the plugin registrant, every plugin's Android code, and the manifest with
the plugins' permissions merged in and its relative class names made
absolute (Gradle resolves them against the namespace before it renames the
package to the application ID, `dev.empathic.clax`). `release_apk` builds
it with `-c opt` (not debuggable, the AOT snapshot stripped) and without
the desugared JDK library (the app's minSdk, 29, has the APIs it backports,
and Gradle's L8 shrinks it away), and `.bazelrc`'s
`--experimental_android_compress_java_resources` compresses the native
libraries as Gradle does for an app that extracts them. R8 shrinks the
code with AGP's optimized defaults (`bazel/android/proguard-android-optimize.txt`),
Flutter's rules (`bazel/flutter/flutter_proguard_rules.pro`, copied from
Flutter's Gradle plugin) and each plugin's and library's own, and the
resources with `shrink_resources`. Gradle is not run at all. Against
Gradle's APK (`aapt2 dump badging` and the file list, as `:apk_test`
checks): the same package, versions, SDK levels, permissions, launcher
activity, native libraries and debug signature, and one dex of 1.24 MB
against Gradle's 1.22 MB; it differs in what AGP adds or strips beyond
that: no baseline profile, Kotlin's `.knm` metadata files left in, and the
`.qual` files under `assets/` left out.

**The macOS app** is rules_flutter's `flutter_macos_app` (rules_apple),
named `clax_app` as Xcode's PRODUCT_NAME, with Runner's entitlements and
pubspec.yaml's version, and `clax` in `Contents/Resources` signed ad hoc
with `Runner/ClaxHelper.entitlements` by `macos_helper`, as the Xcode
build's "Embed clax" phase signs it (a debug app's helper without them).
It is ad hoc signed throughout; a Developer ID identity is a native-build
concern (`macos/Runner/Configs/AppInfo.xcconfig`, `Signing.xcconfig`).

**The iOS app** is rules_flutter's `flutter_ios_app` (rules_apple) for
iPhone and iPad from iOS 17.0, bundle ID `dev.empathic.clax`
(`IOS_BUNDLE_ID` in `bazel/flutter/ios.bzl`), with pubspec.yaml's version.
There is no Xcode project: `ios/Runner/` holds rules_flutter's runner
templates (`AppDelegate.swift`, `SceneDelegate.swift`, `Info.plist`, the
storyboards) and the app's own Swift, every `ios/Runner/*.swift` compiled
into the runner. The daemon is not a process but `crates/clax-ios`, a static
library built `-c opt` in the app's configuration, linked through
`ios_daemon` (`ios/Daemon/ClaxDaemon.swift`, module `ClaxDaemon`, over the C
header `ClaxIOS`); `ios/Runner/NativeChannel.swift` serves it to the Dart
supervisor on `dev.empathic.clax/native` (`daemonStart`, `daemonState`,
`daemonStop`, `daemonPaths`). App Transport Security allows plain HTTP to
loopback (`NSAllowsLocalNetworking`) and to `localhost` with its
subdomains, the apps' own origins (an exception domain in
`ios/Runner/Info.plist`); nothing off the device. A simulator app builds
only in debug (`bazelisk build //apps/clax_app:ios_app -c dbg`); a device
build is `ipa_dev` or `ipa_appstore`, signed with the team's profile
(`bazel/apple/signing.bzl`).

**The tests** are compiled a directory at a time (`tests_<dir>_all`,
`manual`; `bazel/flutter/tests.bzl`): a `.dill` carries the whole app and
Flutter, 64 MB, and one per file made 7.6 GB of them, written again to the
shared cache on every edit of the app. Each file still runs in its own
`flutter_tester`, as under `flutter test`: the suite's entry point runs
the file `CLAX_TEST_FILE` names, so no file sees what another left in a
global (the test binding's HTTP override is one). They run `flutter_tester` as `flutter test` does, with
`UNIT_TEST_ASSETS` and `APP_NAME` set as it sets them (so assets load
within a test's fake time). A test finds the app's files and its scratch
directory through `test/support/paths.dart`, the same under `flutter test`
(run in `apps/clax_app`) and Bazel (run in the runfiles tree). A test file
another test imports for its fakes is listed in `TEST_SUPPORT` in
`apps/clax_app/BUILD.bazel`. A new test directory under `test/` is added
to `SUITES` there. An integration test runs in the debug app
(`debug_app`, `-c dbg`: JIT, Runner's `#if DEBUG` test-input channel,
`DebugProfile.entitlements`), whose entry point (`integration_main`) runs
the test and exits with its result.

### Maintaining it

- **Flutter.** Change `flutter.toolchain(flutter_version = …)` in
  `MODULE.bazel` to a version rules_flutter knows (its release notes say
  which; a newer one may need a newer rules_flutter), resolve the lockfile
  with the new Flutter (above), and run `just app-test`. Each patch under
  `bazel/flutter/` must still apply: a failure names the file; regenerate
  the patch against the new release's sources. The Dart SDK of rules_dart
  (`dart.toolchain` in rules_dart's module) must match Flutter's.
- **A pub package.** `flutter pub add` (or edit `pubspec.yaml`) and resolve
  the lock as above; add the package to `:clax_app`'s `deps`. A plugin
  whose Android build names a Maven coordinate not yet pinned fails naming
  `@rules_android_maven//:<group>_<artifact>`: add it with its version to
  `maven.install` in `MODULE.bazel`, then
  `REPIN=1 bazelisk run @rules_android_maven//:pin`. A plugin whose
  Apple code needs a Swift package, or whose Android build compiles C with
  CMake, needs an overlay in `bazel/flutter/plugin_overlays/<package>/<major>/`
  (a copy of the BUILD file rules_flutter generates, with the
  dependency added; see the speech_to_text one, and its `android/` half,
  which an overlay must carry too).
- **The NDK and the SDK.** Each archive is a URL and a SHA-256 in
  `MODULE.bazel` (`android_ndk.configure`, `android_sdk.configure`; the
  SDK's in `<directory> <archive's top directory> <host> <url> <sha256>`
  lines, from Google's `repository2-3.xml`): replace both
  (`curl -sL <url> | shasum -a 256`). The NDK's API level (29) is the app's
  `minSdk`. The platform's version is also `targetSdkVersion` in the APK's
  `manifest_values`.
- **The JDKs** are rules_java's remote JDKs, named in `.bazelrc`
  (`--java_runtime_version`, `--tool_java_runtime_version`); the Kotlin
  toolchain's `jvm_target` is in `bazel/android/BUILD.bazel`.
- **AGP and Gradle** are not used; `android/app/build.gradle.kts` stays the
  native build's, and what it sets that the APK must match (application ID,
  namespace, SDK levels, ABIs, desugaring, extracted native libraries,
  signing) is set again in `apps/clax_app/BUILD.bazel` and checked by
  `:apk_test`. Change both together.
- **The app's version.** `VERSION_NAME` and `VERSION_CODE` in
  `apps/clax_app/BUILD.bazel` with `pubspec.yaml`'s `version`
  (`:version_sync` fails until they agree).

## Run outside Bazel

`scripts/bazel-test.sh --ci` runs these as they are, after the Bazel
passes, because each reads git, runs `just` or `cargo`, writes into the
checkout, or runs Bazel itself: `scripts/test-plugins.sh` (git tags),
`scripts/test-justfile.sh`, `scripts/test-dev.sh`,
`scripts/test-extension-signing.sh` (writes `web/dist-extension`),
`scripts/check-no-hard-links.sh` and `scripts/check-pipefail-pipes.sh` over
the tracked files (their own tests are Bazel tests),
`scripts/bazel-sync.test.sh` and `scripts/bazel-web.test.sh`. The slow
ones are skipped when they passed last time on the same files, as Bazel
caches a passed test: the plugin and dev-script checks are keyed on the
whole working tree and the `v*` tags, the two Bazel-setup tests on the
Cargo, Bazel and package manifests and Bazel's own files. A pass is
recorded under `~/.cache/clax-checks/` (`CLAX_CHECKS_CACHE`) by its key, so
a new worktree at contents some worktree checked skips them too; delete the
directory to rerun them. CI keeps no such record and runs every one. Tests that
need a real harness CLI, a model or credentials (`scripts/verify-harnesses.sh`
against real harnesses, the smoke scripts) stay native and manual, as
before.

## Gate for gate

What each gate of `scripts/quality_gates.sh` is under `scripts/bazel-test.sh --ci`.
Performance gates nothing in either (`just perf` measures on demand).

| `quality_gates.sh` gate | Under Bazel (`--ci`) |
|---|---|
| web pnpm install, pi pnpm install | `scripts/pnpm-lock-check.py web plugins/pi`; rules_js installs from each lockfile |
| web build | `//web:dist`, `//web:dist_extension` (inputs of the tests and the binaries that embed them) |
| web unit | `//web:unit` |
| web bundle size | `//web:bundle_size` |
| web lint | `//web:lint` |
| web typecheck | `//web:typecheck` |
| pi extension | `//plugins/pi:typecheck`, `//plugins/pi:test` |
| playwright browser, web e2e | `//web:e2e_gesture`, `//web:e2e_chrome_overlay`, `//web:e2e_rest` (Chromium from `@chromium//:browsers`) |
| build clax, build tests | the build of `//crates/...` in the one `bazel test` |
| cargo nextest (or cargo test) | every Rust test target, `tests(//crates/...)`, the integration tests in a few targets per crate |
| cargo fmt --check | `--config=fmt` over `//crates/...` |
| cargo clippy | `--config=clippy` over `//crates/...`: each library, binary and test target; `crates/clax-commands` again with its own `clippy.toml` |
| release build | `//crates/clax-cli:clax_opt` |
| justfile | `scripts/test-justfile.sh`, outside Bazel |
| release scripts | `//scripts:release` |
| tool hook gate | `//scripts:tool_hook` |
| extension signing | `scripts/test-extension-signing.sh`, outside Bazel |
| stable bin | `//scripts:stable_bin` |
| fake exe | `//scripts:fake_exe` |
| release installer | `//scripts:installer` |
| dev scripts | `scripts/test-dev.sh`, outside Bazel |
| plugins | `scripts/test-plugins.sh`, outside Bazel |
| plugin wrapper | `//scripts:plugin_wrapper` |
| comment loop | `//scripts:comment_loop` |
| worker npm ci, worker cargo fmt, worker cargo test, worker cargo clippy, worker wasm, worker e2e | CI's `worker` job, natively (`just worker-test`, then `e2e.mjs`): `crates/clax-worker` is a workspace of its own, outside Bazel |
| wasm32 check | CI's `worker` job, natively (`scripts/check-wasm.sh`, `just check-wasm`) |
| no unsafe code | `//scripts:no_unsafe` |
| no process launch on iOS | `//scripts:ios_spawns` (`scripts/check-ios-spawns.sh` against `scripts/ios-spawns.allow`), and the check's own test `//scripts:ios_spawns_test` |
| crate layers | `//scripts:crate_layers` |
| no hard links | `//scripts:no_hard_links` (the check's own test), and `scripts/check-no-hard-links.sh` over the tracked files outside Bazel |
| pipefail pipes | `//scripts:pipefail_pipes`, and `scripts/check-pipefail-pipes.sh` over the tracked files outside Bazel |
| app (flutter analyze + test) | `//apps/clax_app:dart_analyze` and the test files' targets, with `:prompt_sync`, `:version_sync`, `:apk_test`, `:macos_app_test` and `:ios_app_test` ("The Clax app") |
| build app clax | `//scripts:build_app_clax` (the native script's dry runs) |

Bazel adds: `scripts/bazel-sync.sh --check` first, `//scripts:bazel_gc`,
`//scripts:run_choice`, `//bazel/js:scratch_test`,
`scripts/bazel-sync.test.sh` and `scripts/bazel-web.test.sh`.

The hard-link rule covers the links Clax's own files make, not those of the
tools it runs. The owner's ruling of 2026-10-16: "Hard links that we create are banned, not those created by well tested tools". So `--ci` no longer
scans Bazel's output base or the disk cache for files with more than one
link; the source and rule scans (`scripts/check-no-hard-links.sh`) stay.

## CPU and memory

Bazel runs as many actions as there are cores. Rust tests run four libtest
threads each (`RUST_TEST_THREADS=4`); the browser projects take eight
workers each and declare it (`cpu:8`). The perf benchmarks are `exclusive`:
Bazel runs them one at a time with nothing beside them. To hold builds
back on a shared machine, copy `user.bazelrc.example` to `user.bazelrc`.

## CI

CI (`.github/workflows/ci.yml`) runs `scripts/bazel-test.sh --ci` on a
GitHub-hosted runner. `.github/actions/setup-bazel` installs Bazelisk and
restores `~/.cache/bazel-disk` and `~/.cache/bazel-repo` with
`actions/cache`, keyed by the lockfiles and the commit, falling back to the
newest cache of the same lockfiles and then of any; only a passing push to
main saves one, after `scripts/bazel-gc.sh` trims the disk cache to 6 GB
(Bazel trims only while its server sits idle, which a job never does). `common --config=ci` (`.bazelrc`) bounds the cache to what
fits a runner and fails on a stale lockfile. The native gates stay as a
manual workflow (`native-check.yml`). A second job, `worker`, runs what
Bazel does not build, natively: `scripts/check-wasm.sh` over the crates a
hosted Worker runs, and `crates/clax-worker` (a workspace of its own, for
wasm32 only): `just worker-test`, then its browser e2e (`e2e.mjs`) on a
web UI built with pnpm. A third job, `perf-log`, runs the
perf benchmarks after every green push to main and appends a record to the
`perf-log` branch (`docs/usage.md`, "Developing Clax"); it gates nothing, and is
the only job with write access.

## Disk

Bazel's files live outside the repository, in three places:

- **The shared compile cache**, `~/.cache/bazel-disk`: every action's
  outputs, for every worktree of this repository and of `../changelist`.
  Bazel trims it to 100 GB and to entries used in the last 7 days while a
  server sits idle; the 100 GB bound covers both repositories, whose
  `.bazelrc` files state the same bound. Change the two together.
- **Bazel's user root**, `~/Library/Caches/bazel/_bazel_$USER` on macOS:
  - one output base per worktree, of either repository (its
    `DO_NOT_BUILD_HERE` names the worktree), holding that worktree's
    outputs and its Bazel server. Files that came from the shared cache are
    APFS clones of its entries, so `du` counts them and the disk mostly
    does not;
  - `cache/repos/v1`, shared by every output base of every repository:
    `content_addressable` holds the downloads (toolchains, crates, npm
    packages, Chromium), `contents` the external repositories unpacked once
    for all output bases. Bazel removes an unpacked repository unused for
    a while on its own; nothing in this repository trims either;
  - `install`, Bazel's own binaries per version.
- **The checks' pass records**, `~/.cache/clax-checks`: empty files, one
  per passing keyed outside check ("Run outside Bazel").

`just bazel-gc` (`scripts/bazel-gc.sh`) applies the shared cache's bounds
now, removes the output bases of this repository's removed worktrees (a
server judged dead by `kill -0` on its PID file; changelist's output bases
are its own `bazel-gc`'s), and with `--report` prints real sizes. It warns
when `../changelist/.bazelrc` states another bound. Run it after removing
worktrees.

A worktree that only runs the recipes has no `target/` and no
`node_modules`: those come only from the native tools (a `-native` twin, a
plain `cargo` or `pnpm`, or a machine without Bazel), whose state is
per worktree and large (this branch's own worktree, used for native
comparisons, holds a 29 GB `target/`, 18 GB of it Cargo's incremental
state). `just clean` removes them.

## Speed

Measured 2026-10-16 on the owner's 12-core machine, shared with other
sessions' builds; each figure has the 1-minute load average beside it, and
any figure above a load of about 15 is noise more than signal.

| Case | Bazel | Native | Load |
|---|---|---|---|
| `just ci`, nothing changed | 12 s, 14 s, 20 s | 188 s, 192 s | 6–9, 38 / 10–49 |
| `just ci` in a fresh worktree at a commit another built | 34 s (155 s before the outside checks' passes were shared) | 644 s (cold `target/`, 6.7 GB) | 7–33 |
| `just test` after a clax-core comment edit | 96 s, 140 s | 86 s, 89 s | 21–35 |
| `just web-test` after a web shell edit | 43 s, 44 s | 26 s, 27 s | 10–17 |
| `just ci` after a clax-core edit | 433 s, 467 s | 306 s, 307 s | 7–14 |
| `just ci` after a web shell edit | 280 s, 293 s | 155 s, 262 s | 7–37 |
| `just watch`: restart after a web shell / clax-core edit | 12 s / 30 s | | 16–21 |

Bazel wins whenever the cache has the work: nothing changed, or a new
worktree. After an edit it is slower than Cargo. Where the time goes, after
a clax-core comment edit (Bazel's profile against `cargo test --no-run
--timings`, the same edit):

| Unit | Cargo (incremental) | Bazel |
|---|---|---|
| clax-core library | 6.8 s | 9.4 s (+ 6.2 s metadata) |
| clax-core unit tests | 8.0 s | 21.7 s |
| clax-mcp library | 3.2 s | 7.1 s (+ 5.2 s metadata) |
| clax-server library | 3.8 s | 31.6 s, and 31.7 s again for its test-feature variant |
| clax-server unit tests | 5.3 s | 33.4 s |
| clax-server integration tests | 6.0 s | 31.0 s |
| clax-cli binary / its unit tests | 3.9 s / 3.7 s | 12.1 s / 11.1 s |
| clax-cli, clax-mcp integration tests | 3.3 s, 3.6 s | 11.6 s, 14.1 s |
| whole build of the tests | 24 s | critical path 73 s of compiling, then 42 s running clax-server's integration tests |

The difference is rustc's incremental compilation, which Cargo's dev
profile uses and rules_rust does not: Bazel compiles every crate that
depends on the edited one whole. Turning it on (unsandboxed rustc with
`-Cincremental` in the output base) cut the compile path to about 25 s, but
it stays off: its directory grows with every build, and "Disk filling up
is a bigger issue right now than slightly slower incremental builds" (the
owner, 2026-10-16). Every source edit changes the crate's
metadata, so every dependent recompiles. What was done instead: the
integration tests run as a few targets per crate (`split_tests`), the
format check, clippy and the tests share one invocation, clax-cli's own
clippy pass runs after it (from 70 s on the critical path to 8 s), the web
UI's build no longer reads tests, and the slow checks outside Bazel are
skipped when their inputs passed before.

The tables above predate the crate layout, which took the store out of clax-core, split clax-server into five crates, put
every embedded file in clax-embedded and test support in clax-testkit, so
no crate is compiled twice. `just test` after an edit, measured
2026-10-16/17 on the same machine (1-minute load before → after):

| Edit | Before the layout | After it, first run | After it, second run |
|---|---|---|---|
| clax-core comment (`ids.rs`) | 105 s (9→19) | 74 s (23→21) | 93 s (11→42) |
| route comment (`clax-server/src/routes/threads.rs`) | 138 s (19→24) | 56 s (19→20) | 79 s (42→35) |
| web shell comment (`web/shell/index.html`) | 96 s (15→17) | 50 s (20→21) | 74 s (35→34) |

The second run shared the machine with a load above 30, which the test
runs (half of every critical path) feel most. What a clax-core edit
compiles now: clax-core (3.3 s; its unit tests 6.3 s), clax-store (7.5 s;
its tests 15.3 s), clax-mcp, then clax-server's library once (15–18 s, no
test-feature variant) and the test binaries. A web edit rebuilds
`web/dist`, clax-embedded (0.5 s) and the binary (3–4 s) and reruns the
tests that serve it; it recompiles no daemon crate.

## For agents

- Run checks through the recipes (`just ci`, `just test`, …); report a
  native result as native.
- After any `Cargo.toml` or `Cargo.lock` change, run `just bazel-sync` and
  commit the result.
- After removing worktrees, run `just bazel-gc`.
- Never `bazelisk clean --expunge` to fix a cache problem in a worktree
  others share a server with; report it.

## Differences from Cargo

- **`CARGO_MANIFEST_DIR`** is the crate's path from the workspace root, and
  tests run from the runfiles root, which stands for the repository. Tests
  make a repository path absolute (`clax_fake_exe::repo()`) before handing
  it to a child.
- **`CARGO`** names nothing under Bazel; a test fallback that would run it
  fails loudly. Tests get `clax` through `CLAX_TEST_BIN`.
- **`CLAX_BUILD_COMMIT`** is `unknown` in every Bazel build.
- **Embedded files in debug builds.** Bazel's debug `clax` embeds the web
  UI, the extension and the plugins (`--cfg clax_embedded`, rust-embed's
  `debug-embed`); Cargo's reads them from the checkout. The one test of
  that, `web::tests::an_embedded_build_carries_the_bridge` in
  clax-embedded, runs only
  under Bazel (`scripts/bazel-parity.py` knows it).
- **Features.** crate_universe keeps one feature list per crate, so Bazel's
  third-party crates have the workspace's merged features, as
  `cargo test --workspace` does, plus `debug-embed` on rust-embed.
- **The test binary.** Bazel's tests run `clax` built with its own
  features only, as the native gates' `CLAX_TEST_BIN` is.
- **The testkit in unit tests.** Bazel builds a crate's unit tests
  (`<lib>_lib_test`) without `clax-testkit`, which is for integration
  tests only (`unit_dev_deps` in `bazel/cargo.bzl`); Cargo gives every test
  target all dev-dependencies, so `cargo test -p clax-server --lib` builds
  the testkit and the server under it first, and a unit test that names
  `clax_testkit` compiles under Cargo and fails to under Bazel.
- **Debug information.** Workspace crates keep line tables, as Cargo's
  dev profile does; dependencies have none.
- **PATH.** Tests see `/usr/bin:/bin:/usr/sbin:/sbin` (`.bazelrc`), not the
  developer's PATH.
