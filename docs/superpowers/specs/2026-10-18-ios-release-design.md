# Clax on iPhone and iPad: a TestFlight build

Parts superseded by `docs/superpowers/specs/2026-10-19-in-process-core-design.md`: §1 and §2 (the in-process daemon, `crates/clax-ios` and its `clax_start`…`clax_trim_memory`, the key slot over HTTP, loopback and App Transport Security), §4.1's `clax-ios` and `ios_daemon` pieces and §10.3's `NSAllowsLocalNetworking`; the iOS app links `crates/clax-embed` instead.

Date: 2026-10-18. Branch: `flutter-app`. Status: draft for the owner's review.
Builds on `2026-10-18-professional-ui-design.md` (the "UI spec" below),
`2026-10-12-flutter-app-design.md` and `docs/bazel.md` ("The Clax app").

## Goal

The Clax app (`apps/clax_app`) runs on iPhone and iPad, from a TestFlight
build that `just app-iphone` makes and uploads. That makes three platforms
of one app: Android, macOS and iOS.

After this work:

- the daemon runs **inside the app's process** on iPhone and iPad. iOS lets
  an app start no other process, so the helper binary of the Mac and the
  `libclax.so` executable of Android have no iOS form;
- iOS has the **native Apple look** it shares with the Mac: SF type,
  navigation bars, sheets with detents, swipe-back, context menus and
  haptics;
- the iPad has the Mac's **sidebar and detail**, adapted for touch, and
  works in Split View, Slide Over and Stage Manager;
- a change that is running when the person leaves the app finishes in the
  time iOS gives. A longer build **pauses and resumes** on return, and says
  so in a notification;
- **Bazel builds it all** in one command, as `just app-phone` does for
  Android: `just app-ios` for the simulator and `just app-iphone` for a
  device and TestFlight.

Out of this round: the App Store listing, screenshots, the age rating
questionnaire and review preparation beyond what TestFlight's Beta App
Review needs. Internal TestFlight testers need no review at all (§10.6).

## Owner's decisions

1. **Native iOS look**, sharing the Apple-platform design with the Mac: SF,
   navigation bars, sheets, swipe-back, context menus and haptics. Android
   stays Material. Both are built on the same Clax tokens. The adaptive
   layer of the UI spec (§2 there, `lib/ui/kit/`) gets two families,
   Material and Apple. The Mac and iOS are the Apple family.
2. **iPad is in scope**, with the Mac's sidebar-and-detail structure
   adapted for touch, plus Split View and Stage Manager.
3. **Background: finish what fits, and notify.** A running change uses the
   extra background time iOS allows. A longer build pauses and resumes on
   return, with a notification: "Paused — open Clax to keep building."
4. **This round is a TestFlight-ready build**: the daemon in-process on
   iPhone and iPad, a Bazel build, the native iOS UI, and TestFlight setup.
   The App Store listing and review preparation come later. Signing needs
   the owner's Apple Developer certificate and team ID (record
   553cb43495, `apps/clax_app/macos/Runner/Configs/AppInfo.xcconfig:16-23`,
   is the same wait for the Mac).

## 1. How the daemon runs today

| | Android | macOS | iOS (this spec) |
|---|---|---|---|
| What runs | `clax` as `<nativeLibraryDir>/libclax.so`, a child process (`bazel/android/clax.bzl`) | `clax` in `Contents/Resources`, a sandboxed helper process (`bazel/flutter/macos.bzl`) | `libclax_ios.a` linked into the app; the daemon on a thread of the app |
| Found by | `AndroidExeLocator` over `dev.empathic.clax/native` (`lib/daemon/exe_locator.dart:22-40`) | `MacExeLocator` (`exe_locator.dart:11-19`) | nothing to find |
| Started as | `<exe> --port <P> serve --foreground --require-credential` (`supervisor.dart:553` builds its environment) | the same | `clax_start(port)` over the native channel |
| Port and key | `<home>/daemon.json`, once its PID is the child's and `/healthz` answers (`supervisor.dart:440`) | the same | the same file, with the app's own PID |
| Kept alive | `DaemonSupervisor`: backoff, `onResume` health checks, restart on a key change (`supervisor.dart:103-125`, `266-289`) | the same | the same supervisor over an in-process starter (§2.7) |
| Where `serve` is | `clax-cli/src/serve.rs:12-42` builds a `ServeConfig` and blocks on `clax_server::serve::serve` | the same | `crates/clax-ios` builds the same `ServeConfig` |

`clax_server::serve::serve(cfg, ready)` (`crates/clax-server/src/serve.rs:254`)
is already an `async fn` that takes its whole configuration as a value and
reports the bound port through `ready`. Nothing in it needs a process of
its own except three things (§2.5): it installs SIGTERM and SIGINT handlers
(`serve.rs:490`, `505`), the CLI's `close_inherited_fds`
(`clax-cli/src/serve.rs:75-96`) closes every descriptor above stdio, and
the environment carries the API key and paths.

## 2. The in-process daemon

### 2.1 The crate and its C ABI

A new crate, `crates/clax-ios`, built as a `staticlib` for
`aarch64-apple-ios` and `aarch64-apple-ios-sim`. It is the CLI's
`serve.rs` without the process: it builds the `ServeConfig` the CLI builds,
with the web UI the binary embeds (`clax-cli/src/bundle.rs`'s `WebUi`,
moved to `clax-embedded` so both use one copy), and runs it.

Its ABI is four functions with scalar arguments only:

```c
// clax_ios.h
// Starts the daemon on its own thread, asking for `port` on 127.0.0.1
// (0: any free port). Returns at once: 0 started, 1 already running.
int32_t clax_start(uint16_t port);
// Asks a running daemon to stop; returns at once. clax_state() says when
// it has.
void clax_stop(void);
// 0 never started, 1 starting, 2 up, 3 stopping, or 100 + the exit status
// `clax serve` would have exited with (100 a clean stop, 165 a failed
// migration, 101 a panic, 100 + 1 any other error).
int32_t clax_state(void);
// Under memory pressure and on entering the background: SQLite's caches
// shrink, the WAL is checkpointed (PASSIVE), the HTTP cache drops its
// memory tier. Safe from any thread; returns when done.
void clax_trim_memory(void);
```

- **No pointers cross the boundary.** The home is fixed: `$HOME/Library/
  Application Support/clax-home`. On iOS `$HOME` is the app's container,
  and that is the directory `main.dart` already uses
  (`getApplicationSupportDirectory()` + `clax-home`, `lib/main.dart:94-99`).
  The cache directory is `$HOME/Library/Caches/clax`. The token and the
  port are read from `daemon.json` by Dart, as on the other platforms.
  The API key arrives over loopback (§2.6).
- **`unsafe`.** The workspace forbids `unsafe_code` (`Cargo.toml`), and
  that lint also covers `#[unsafe(no_mangle)]`, which every exported C
  function needs. With scalar arguments nothing else in the crate is
  unsafe. The draft proposes that `crates/clax-ios` alone sets
  `unsafe_code = "deny"` in its own `[lints]` and allows it on exactly the
  four `#[unsafe(no_mangle)]` attributes, with a check that the crate has
  no other `allow(unsafe_code)` and no `unsafe` block. This is a change to
  an AGENTS.md rule, so it is the owner's (open question 1). The other
  route, a home path passed as pointer and length, needs an `unsafe`
  `from_raw_parts` and is not taken.
- **Called from Swift, not from Dart.** The runner (`ios/Runner/
  ClaxDaemon.swift`) imports the header as a Clang module and serves the
  calls on the existing `dev.empathic.clax/native` channel (`daemonStart`,
  `daemonStop`, `daemonState`). `dart:ffi` is not used: the Swift side has
  to watch the app's lifecycle and memory warnings anyway (§2.4, §2.8), and
  a symbol Swift references cannot be dead-stripped from the static
  library, where one `DynamicLibrary.process()` looks up can be.

### 2.2 Threads and the Tokio runtime

- `clax_start` spawns one OS thread, `clax-daemon`, and returns. That
  thread owns a multi-threaded Tokio runtime with **two workers**
  (`clax-rt-*`) and the default blocking pool, and blocks on `serve`. The
  store's own connection threads are `clax-store`'s, as in the CLI.
- Flutter's threads (platform, UI, raster, IO) never run daemon code. Dart
  talks to the daemon only over HTTP on loopback, as on Android and the
  Mac, so the app's code above the supervisor does not change.
- `clax_stop` sends on a `watch` channel that `ServeConfig` gains (§2.5);
  `serve` drains as it does for SIGTERM today (`serve.rs:495-540`: SSE
  streams end, connections drain within `DRAIN_TIMEOUT`, the journal
  closes, the store shuts down, `daemon.json` is removed). The thread then
  drops the runtime and sets the state. A later `clax_start` builds a new
  runtime; nothing global survives but the tracing subscriber
  (`try_init`, once a process) and `clax-embedded`'s lazily read files.
- A panic in a task is Tokio's to contain, as today. A panic on the
  `clax-daemon` thread itself ends that thread, and `clax_state` answers
  101; it never unwinds into Swift. The exit-status mapping reuses
  `clax-commands`' (`cli.rs:428`, `MIGRATION_FAILED_EXIT` 65), so the
  supervisor's rule that a failed migration is `Failed` at once
  (`supervisor.dart`, `migrationFailedExit`) still holds.
- Logs: the subscriber writes to `Caches/clax/daemon.log`, cut at 1 MB to
  `daemon.log.1`. The supervisor's `stderrTail` reads the file's last lines
  instead of a child's pipes.

### 2.3 Where CLAX_HOME lives, and backup

| Path (under the container) | What | Backed up | Protection class |
|---|---|---|---|
| `Library/Application Support/clax-home/` | the daemon's home: `clax.db` (WAL), `artifacts/`, `toolpath/`, `config.toml` | **yes**: it is the person's apps and their data | `NSFileProtectionCompleteUntilFirstUserAuthentication` |
| `…/clax-home/net-cache/`, `…/clax-home/logs/` | caches the daemon keeps in its home | no (`isExcludedFromBackup`, set by the runner at each start) | the same |
| `…/clax-home/daemon.json`, `starting.json` | this run's port, PID and token | no (excluded) | the same |
| `Library/Application Support/` (the rest) | `settings.json`, the ledgers, `pending-make.json`, `install-id` (`main.dart:65-75`) | yes | the same |
| `Library/Caches/clax/` | `daemon.log`, the home snapshot, `claude-cli/` scratch (unused on iOS) | no; iOS may purge it | the same |
| `tmp/` | `TMPDIR` | no | the same |

- **Why not `NSFileProtectionComplete`:** it makes the files unreadable
  while the device is locked. A change that finishes in the background
  after the person locks the phone would then fail to write its version,
  and SQLite's `-shm` file would fail to open. "Until first user
  authentication" is iOS's default for app files. The runner sets it
  explicitly on `clax-home` so that a later default change cannot move it.
- **The API key** stays in the Keychain (`flutter_secure_storage`), with
  `KeychainAccessibility.first_unlock_this_device`: never in a backup,
  never on another device.
- **No App Group.** The home lives in the app's own container. iOS ends a
  suspended app that holds a file lock in a *shared* container (exception
  `0xdead10cc`). Keeping `clax.db` in the app's own container, and taking
  no App Group in this round, avoids the case (§2.4).

### 2.4 Suspend and resume

iOS suspends an app shortly after it leaves the screen: every thread
stops, the daemon's included. The design takes each piece in turn.

- **The listener.** A listening socket can be reclaimed while its app is
  suspended (Apple TN2277, "Networking and Multitasking"). After resume,
  `accept` then fails on every call. The supervisor's `onResume`
  (`supervisor.dart:266-289`) already probes `/healthz` twice and restarts
  a daemon that does not answer. In-process, that restart is `clax_stop`,
  waiting for `clax_state() >= 100`, then `clax_start` on the **same
  port** (kept in `<support>/daemon-port`), so the WebView's origin,
  cookies and storage stay `http://127.0.0.1:<port>`. Tokio sets
  `SO_REUSEADDR`, so the port can be bound again at once. The probe on
  resume costs one loopback request; a restart costs the store's open
  (51–57 ms on a fresh home in the spike, §11).
- **`serve` on an accept error.** It must log and go on rather than spin.
  Phase 2 checks what axum's `serve` loop does when `accept` keeps failing
  and adds a test with a listener that fails.
- **The stream.** The shell's live stream (SSE) drops when the app is
  suspended. On `resumed` the app posts a `resumed` host message (UI spec
  §6.2 gains it), and the shell reconnects at once instead of waiting for
  `EventSource`'s retry. The Dart side's own streams reconnect as they do
  after an Android doze.
- **The database.** SQLite runs in WAL mode with its locks on files in the
  app's own container. Apple documents the `0xdead10cc` termination for
  locks held at suspension on files in a *shared* container (§2.3), and the
  device checklist (§9, item 6) confirms that a suspended Clax is not ended
  for its own. Either way, no write transaction should be open at
  suspension. The store's writes are short, and the runner holds a
  background task (§6.1) from `sceneDidEnterBackground` until
  `clax_trim_memory` has returned. That call also runs a `PASSIVE` WAL
  checkpoint, so a later kill of the suspended app loses nothing that was
  committed, and no fifth function is needed.
- **Timers.** The reaper, sweepers and stale check sleep on Tokio timers,
  and suspension stops them. On resume the reaper sees the wall clock
  jump past monotonic time and marks the store as resumed
  (`serve.rs:404-418`, `slept`), as after a Mac's sleep.
- **A kill while suspended.** iOS ends suspended apps freely. The next
  launch is a cold start: `daemon.json` names this process's earlier PID,
  which the supervisor already ignores when it is the app's own PID
  (`supervisor.dart:511`) or is not alive. Makes in flight are offered
  again by `PendingMake`, as after Android's task removal.

### 2.5 What `serve` gains

`ServeConfig` gets one field:

```rust
/// How the daemon is asked to stop. `Signals` is `clax serve`'s: SIGTERM,
/// Ctrl-C and the shutdown route. `Embedded` is a daemon inside another
/// app's process: only the receiver and the shutdown route, and no signal
/// handler is installed, since the process's signals are the app's.
pub stop: StopOn,
```

The reason for `Embedded` is that a Tokio SIGTERM handler, once installed,
stays for the process's life. iOS's own SIGTERM to an app would then no
longer end the app. `raise_open_file_limit` stays, because the app's limit
is the daemon's and Flutter's descriptors share it.

### 2.6 The API key

On Android and the Mac the key reaches the daemon in its environment
(`supervisor.dart`, `keyEnv`), and a new key restarts it (`DaemonKey`,
`lib/daemon/daemon_key.dart`). In-process there is no environment of its
own, and `setenv` in a multi-threaded process is unsound. Instead:

- `clax_sample::Sampler` reads the key through a closure today
  (`clax-cli/src/serve.rs:34-36`). `clax-ios` passes one that reads an
  in-memory slot.
- The supervisor fills the slot with `PUT /api/sample/key` (body: the key,
  or empty to clear it), which needs the owner's bearer token. The daemon
  serves the route only when `stop` is `Embedded`. It runs after each `Up`
  and on each key change, in place of the restart.
- The key is never written to disk by the daemon and never logged.
  Artifact origins (`<id>.localhost`) never hold the token
  (`supervisor.dart:79`), so no page can set or read it.

### 2.7 The Flutter side

- `lib/daemon/in_process.dart`: `InProcessStarter implements
  ProcessStarter`. `start()` calls `daemonStart` with the port from the
  arguments the supervisor builds and returns a `RunningProcess` whose
  `pid` is `io.pid`, whose `exitCode` completes when a `daemonState` poll
  (every 250 ms while starting or stopping, and on each `resumed`) answers
  100 or more, and whose `kill()` calls `daemonStop`. Its `stdout` and
  `stderr` are the new lines of `daemon.log`.
- `DaemonSupervisor` takes a `DaemonHost` (`android`, `mac`, `inProcess`)
  in place of `bool android`. With `inProcess` it writes no environment,
  never signals a PID (the guard at `supervisor.dart:511` already skips its
  own), and sends the key over `PUT /api/sample/key` (§2.6). Everything
  else stays: `config.toml` before each start, the kept port, `daemon.json`
  and `/healthz`, the backoff, `Failed` after five exits in a minute.
- `main.dart` picks the host by `Platform.isIOS`. `ClaudeCliBackend` is
  already off where it cannot run (`claude_cli_backend.dart:205-214`
  answers false off the Mac), so the agent on iOS is the Messages API
  backend with the owner's key.

### 2.8 Memory

- **Limits.** iOS ends a foreground app over its device's jetsam limit
  (about half the RAM on recent iPhones, less on older ones). The WebView's
  pages run in WebKit's own content processes, which have their own
  limits; the app's own footprint is Flutter, the Dart heap and the
  daemon.
- **What the daemon costs.** In the spike, the whole Mac test process
  (daemon, AppKit and a WKWebView host) peaked at a **27 MB footprint**
  (`/usr/bin/time -l`) on a fresh home. Phase 2 measures the iOS app on a
  device with a home of 50 apps, and the budget it sets goes into the
  timings gate (`timings.jsonl`).
- **Under pressure.** `didReceiveMemoryWarning` calls `clax_trim_memory`
  (`PRAGMA shrink_memory` on every connection, and the HTTP cache's memory
  tier dropped) and Flutter's own `didHaveMemoryPressure`. The runtime's two
  workers and SQLite's page cache (`cache_size`, set per connection by the
  store) keep the steady state small.
- **The WebView's content process** is ended under pressure more often on
  iOS than elsewhere. `webViewWebContentProcessDidTerminate` reloads the
  page and keeps the comment and threads state; this is the iOS half of
  record 1ee688df3a (Android's renderer crashes).
- The increased-memory-limit entitlement is not requested. It is a
  fallback if the device measurements need it.

### 2.9 Loopback, WKWebView and App Transport Security

- The daemon binds `127.0.0.1` only (`ServeConfig.bind`), with
  `--require-credential`'s gate on (`require_credential: true`), as on
  Android and the Mac.
- ATS: `NSAppTransportSecurity` › `NSAllowsLocalNetworking = YES`. It
  allows plain HTTP to loopback, `localhost` and `*.localhost` (the
  artifact origins) without the arbitrary-loads exception, so App Review
  asks for no justification. Everything off the device stays HTTPS.
- **Loopback is not the local network.** Binding and connecting to
  `127.0.0.1` does not trigger iOS's local network prompt, and the app
  declares no `NSLocalNetworkUsageDescription`. Phase 1's simulator run
  confirms there is no prompt (§11).
- **`*.localhost` origins.** WebKit resolves `localhost` subdomains to
  loopback on its own. Phase 1 confirms it on iOS by loading
  `http://<id>.localhost:<port>/` in the spike app. If WebKit did not
  resolve them, `WKURLSchemeHandler` cannot serve `http`, so the fallback
  would be a `clax-artifact:` scheme of the app's own. That would be a
  larger change and is recorded as an alternative, not designed here.
- The key cookie is set through `WKHTTPCookieStore` before the first load,
  as `DaemonKeyCookie` does through `webview_flutter` today. The spike
  proved this on the Mac's WebKit (§11).
- `WKWebView.isInspectable` is on in debug and profile builds only, so
  Safari's Web Inspector reaches pages on a test device.

## 3. Crates on iOS: what builds, and what is gated

The spike built the whole daemon (`clax-server` and everything under it)
for `aarch64-apple-ios` and `aarch64-apple-ios-sim` **with no source
change** and no warnings (§11). iOS is `target_os = "ios"`, which is
`unix` and is neither `macos` nor `linux`, so it falls into the same arms
Android does. What remains is behaviour at run time. iOS's sandbox refuses
`posix_spawn` and `fork`, so every path that starts a program fails there,
and each gets a compile-time gate.

The gating follows the wasm32 pattern in this repository: a
`[target.'cfg(…)'.dependencies]` table in the crate's `Cargo.toml` (for
example `crates/clax-server/Cargo.toml:45-58`) and `#[cfg]` on the code,
with the iOS arm returning what "not available" already means.

| Where | What it does | On iOS |
|---|---|---|
| `clax-realtime/src/push.rs:63`, `164` (`CodexPush`) | spawns `codex` for Codex tier 5 | `CodexPush::from_env` answers `Disabled`; `clax-ios` passes the disabled push (as the spike did). No `cfg` is needed in the crate, only in what `clax-ios` passes. |
| `clax-core/src/gitctx.rs:816-819` | runs `git` for an agent session's repository context | `#[cfg(not(target_os = "ios"))]`; the iOS arm returns the "no repository" outcome. No session on iOS has a working tree. |
| `clax-core/src/remote.rs:806-840` (`process_started`) | `sysinfo` on macOS and Linux, `ps` on other Unix | `#[cfg(target_os = "ios")]` returns `None`: no parent-PID credential files on iOS. |
| `clax-mcp/src/tools.rs:2786-2806` (`open_in_browser`) | runs `open` or `xdg-open` | `false` on iOS; the app opens links itself (`ExternalLinks`). |
| `clax-mcp/src/shim.rs`, `channel.rs` | the stdio MCP shim and channel helpers for local agents | not compiled for iOS (the module is `cfg(not(target_os = "ios"))`); the daemon's streamable-HTTP MCP route stays. |
| `clax-core/src/daemon.rs:146` (`pid_alive`) | `kill(pid, 0)` | builds and works for the app's own PID; another app's PID answers `EPERM`, which reads as alive. That is harmless: no other daemon can share the container. |
| `clax-commands`, `clax-hooks`, `clax-cli` | the CLI, hooks, plugin installs and `init` (they spawn `claude`, `codex` and `git`) | not linked: `clax-ios` depends on `clax-server`, `clax-core`, `clax-sample`, `clax-realtime`, `clax-http` and `clax-embedded` only. |
| `clax-embedded`'s debug disk tree (`web.rs:32-38`) | a debug build serves `web/dist` from the checkout | iOS builds always embed: Bazel sets `--cfg clax_embedded` for the iOS transition, since a checkout path means nothing on a phone. |
| file watching | none in the daemon; no `notify` crate in `Cargo.lock` | nothing to gate |
| the Keychain | not used by any crate; the app's key is Flutter's (`flutter_secure_storage`) | nothing to gate |
| `ring`, `rustls`, `rusqlite` (bundled SQLite) | TLS for `sample()` and the store | build as they are. Their C objects carry the deployment target, which Bazel sets (§4.2). |

**Unavailable on iOS**, written into `docs/usage.md`'s iOS section when
phase 2 lands:
- local agents (Claude Code, Codex, Pi) and their hooks, with nothing to
  run them;
- Codex tier-5 push;
- "Claude Code on this Mac" as the app's agent;
- the CLI and plugin installs;
- parent-PID credential files;
- the repository context of sessions.

What remains is the app's own agent (Messages API), every artifact,
version, thread and question route, `sample()`, rooms, MCP over HTTP, and
bundles.

A check, `//crates/clax-ios:ios_build_test`, builds the crate for both
triples in `just ci` on macOS hosts, the way `scripts/check-wasm.sh` keeps
the Worker's crates building for wasm32.

## 4. Bazel

### 4.1 Pieces

```
//crates/clax-ios:clax_ios          rust_static_library (lib.rs, 4 C functions)
//crates/clax-ios:clax_ios_c        cc_library: hdrs = clax_ios.h, deps = :clax_ios
//apps/clax_app:ios_daemon          swift_library ClaxDaemon (imports clax_ios_c)
//apps/clax_app:ios_app             flutter_ios_app (rules_flutter), runner_deps = [:ios_daemon, …]
//apps/clax_app:ipa                 the device .ipa, signed (§4.4)
//apps/clax_app:ios_app_test        checks the bundle's contents (as :macos_app_test does)
```

### 4.2 rules_rust for the iOS triples

- `MODULE.bazel`'s `rust.toolchain` gains `aarch64-apple-ios` and
  `aarch64-apple-ios-sim` in `extra_target_triples`, with the SHA-256 of
  each `rust-std-1.94.0-<triple>.tar.xz` beside the Android ones.
  crate_universe resolves the crates for those triples too.
  `scripts/bazel-sync.sh --check` already compares the versions with
  `rust-toolchain.toml`, which gains the two targets for Cargo.
- The platforms are `@build_bazel_apple_support//platforms:ios_arm64` and
  `:ios_sim_arm64`. rules_apple's `ios_application` split transition
  (`--ios_multi_cpus=arm64` or `sim_arm64`) puts the static library in the
  right one, and rules_rust maps them to the triples.
- **The deployment target.** In the spike, Cargo's C objects (`ring`, and
  SQLite through `libsqlite3-sys`) carried `minos 26.5`, the SDK's, while
  the app linked for 17.0. `ld` warned on each, and `IPHONEOS_DEPLOYMENT_
  TARGET=17.0` did not change those objects. Under Bazel the C objects
  come from the `apple_support` toolchain with `--ios_minimum_os=17.0`.
  `:ios_app_test` checks every object's `LC_BUILD_VERSION`
  (`vtool -show-build`) against it, so a mismatch fails the build rather
  than warning.
- The library is built `-c opt` with `-Cstrip=symbols` in a transition
  of its own, as `android_clax` does (`bazel/android/clax.bzl:11-25`). The
  spike's stripped app binary was 37.5 MB for the daemon and its harness;
  the App Store compresses it, and the measured download size goes into
  the phase 3 notes.

### 4.3 rules_flutter: what it has, and what we add

rules_flutter 0.0.6 already has **`flutter_ios_app`** (`@rules_flutter//
flutter:ios.bzl`): the AOT framework, the iOS engine xcframework (debug JIT
for the simulator, profile and release AOT for devices), the plugin
registrant, Swift runners from `ios/Runner/*.swift`, entitlements, a
provisioning profile, app icons, privacy manifests merged from the plugins
(`flutter_ios_privacy_manifests`), and `runner_deps` for extra Swift
libraries. It has not been run in this repository. What we add:

- `apps/clax_app/ios/`, from `flutter create --platforms=ios .` with the
  Flutter 3.47.7 `MODULE.bazel` pins. Its `Runner/` gains:
  - `ClaxDaemon.swift`;
  - `NativeChannel.swift` (the `native` channel's iOS half);
  - `Background.swift` (§6.1);
  - `Notices.swift`;
  - `BundleIntake.swift`;
  - `NativeDialogs.swift` (UIKit alerts, §7.3).
  The Xcode project stays the native build's, as Android's Gradle files
  do.
- `families = ["iphone", "ipad"]`. rules_flutter's docstring warns that
  `flutter create`'s icon set has a 76×76@1x entry that `actool` rejects
  for iPad. The UI spec's icon work (§5.11 there) renders the iOS set
  without it.
- `flutter_ios_app` sets `minimum_os_version` to 15.0 by default; we set
  17.0 (draft decision 6).
- Plugin overlays: the iOS halves of the plugins in `pubspec.yaml` are
  built by rules_flutter's Apple plugin rules, which already build the
  macOS halves. `speech_to_text` needs CwlCatchException on iOS as it does
  on the Mac, so its overlay
  (`bazel/flutter/plugin_overlays/speech_to_text`) gains the iOS
  dependency. A plugin whose iOS half needs something else fails naming
  it, and gets an overlay the same way (`docs/bazel.md`, "Maintaining
  it").
- If a fix to rules_flutter's iOS rules is needed, it goes into
  `bazel/flutter/rules_flutter.patch` in small hunks that can be offered
  upstream, as the existing ones are.
- `MODULE.bazel`'s `use_repo(flutter, …)` needs no iOS engine repositories:
  `_engine_xcframework_select` resolves them through rules_flutter's own
  mapping.

### 4.4 Signing inputs, declared

Signing material is this machine's and not pinned, like the Android debug
key and the home release key. It comes through one `local` repository
rule, `bazel/apple/signing.bzl` (`@clax_apple_signing`), which watches
each file and is evaluated again when one changes:

| Input | Default path | Used by |
|---|---|---|
| team ID | `~/.config/clax/apple/team-id` (10 characters) | the entitlements' `application-identifier` and `com.apple.developer.team-identifier` |
| provisioning profile, development | `~/.config/clax/apple/clax-development.mobileprovision` | device builds for `just app-iphone` |
| provisioning profile, App Store | `~/.config/clax/apple/clax-appstore.mobileprovision` | the TestFlight `.ipa` |
| App Store Connect API key | `~/.config/clax/apple/AuthKey_<KEYID>.p8`, with `api-key-id` and `api-issuer-id` beside it | the upload (§4.5) |

- The signing identities, "Apple Development" and "Apple Distribution",
  stay in the login keychain, where `codesign` reads them. rules_apple
  names the identity from the profile.
- The directory is `0700`, and the rule refuses group- or world-readable
  files, as the home release key is kept owner-only (commit `aacd3d9a7`).
- **Without the directory**, simulator builds work: they are unsigned, as
  rules_apple's simulator builds are. A device or TestFlight build stops,
  naming each missing file and the `docs/usage.md` section that explains
  how to get it.
- The actions that sign or upload are `no-remote` and `no-cache`, so
  neither a profile nor a key enters the shared cache.

### 4.5 The commands

**`just app-ios`** (`scripts/app-ios.sh`), the simulator:
1. builds `//apps/clax_app:ios_app` with `--ios_multi_cpus=sim_arm64`
   (`-c dbg` by default for JIT and hot restart, `--release` for `-c
   opt`);
2. boots a simulator named "Clax iPhone", creating it from the newest
   installed runtime and an iPhone 17 device type if it is missing
   (`APP_IOS_DEVICE=ipad` uses "Clax iPad", an iPad Pro 13-inch);
3. installs it with `xcrun simctl install`, keeping the app's data, and
   launches it;
4. checks that the daemon came up: it waits on `daemon.json` in the app's
   data container (`simctl get_app_container … data`), then on `/healthz`.

It prints the step it is on, as `app-phone.sh` does, and states what is
missing when the iOS simulator runtime is not installed (§11).

**`just app-iphone`** (`scripts/app-iphone.sh`), a device and TestFlight,
in the style of `app-phone.sh`:
- it builds HEAD exactly, or a clean copy of HEAD at
  `target/app-iphone/tree`;
- it prints what it would do and changes nothing without
  `--i-know-what-i-am-doing`;
- `just app-iphone` builds the development-signed `.ipa`
  (`--ios_multi_cpus=arm64 -c opt`) and installs it on the plugged-in or
  paired device with `xcrun devicectl device install app`, keeping the
  app's data. It then launches the app and checks that the daemon stays up
  (`devicectl device info files` reads `daemon.json`).
  `APP_IPHONE_DEVICE` picks the device when there are several;
- `just app-iphone --testflight` builds the App Store-signed `.ipa` and
  uploads it with `xcrun altool --upload-package` and the API key (§4.4).
  It then prints the build number and the App Store Connect link. The
  build number is chosen as the home bundle's version is: a ledger
  (`~/.config/clax/apple/builds`) of the last number per marketing
  version, the next one taken at each upload, so no number is ever reused
  and nothing is committed. `pubspec.yaml`'s version stays the marketing
  version, and `:version_sync` checks the iOS app against it as it checks
  the APK and the Mac app.

Transporter (the app) stays the manual fallback for an upload that
`altool` refuses. Xcode is not needed for either command, but its
toolchain is (rules_apple's).

## 5. Platform features

| Feature | Android today | iOS | What changes |
|---|---|---|---|
| Run notifications ("… is ready", "Couldn't …") | `Notices` on the `news` channel (`BackgroundRunService.kt`) | `UNUserNotificationCenter` through `flutter_local_notifications`, from Swift's `Notices.swift` for the paused notice | permission is asked at the first make, not at launch; tapping one opens the artifact (`launchArtifact`, `opened`) as on Android |
| Ongoing progress while making | a `dataSync` foreground service with a progress notification | none exists. A background task (§6.1), then the paused notice | `BackgroundWork.start` answers true while the task runs; `stopped` fires at its expiry |
| The persistent comment-mode notification | `AppNoticeChannel.kt`: Comment and Threads from the shade | iOS has no ongoing notification | §5.1 |
| Share sheet | `share_plus` | `share_plus` (`UIActivityViewController`) | **iPad needs an anchor**: every share passes `sharePositionOrigin` (the button's rect), or UIKit raises on iPad |
| File picker | `file_picker` | `file_picker` (`UIDocumentPickerViewController`) | none |
| Camera | `image_picker` | `image_picker`; no camera on the simulator | `NSCameraUsageDescription`, `NSPhotoLibraryUsageDescription` |
| Location | `geolocator` | `geolocator` (Core Location, when in use) | precise or approximate maps to iOS's full and reduced accuracy; a precise request on reduced accuracy uses `requestTemporaryFullAccuracy` with `NSLocationTemporaryUsageDescriptionDictionary` |
| Speech and dictation | `speech_to_text` | `speech_to_text` (`SFSpeechRecognizer`), on-device when the device supports it; the keyboard's own dictation in every field | `NSMicrophoneUsageDescription`, `NSSpeechRecognitionUsageDescription` |
| Voice cues' haptics | `platformHaptics = Platform.isAndroid` (`voice_cues.dart:46`) | `HapticFeedback` maps to `UIImpactFeedbackGenerator` | on iOS too; the Apple family adds selection and success haptics (§7) |
| Reminders | `flutter_local_notifications` | the same | iOS keeps at most 64 pending local notifications: `Reminders` schedules the nearest 60 and tops up on each launch |
| Opening a `.clax` bundle | `BundleReceiverActivity.kt`, `BundleIntake.kt` | `CFBundleDocumentTypes` and an exported type `dev.empathic.clax.bundle` (`.clax`); `scene(_:openURLContexts:)` → `dev.empathic.clax/bundles` | the channel's Dart side (`ChannelBundleIntake`) is unchanged |
| Secure key | `flutter_secure_storage` (Keystore) | the same (Keychain, first unlock, this device only) | none |
| External links | `ExternalLinks` | the same (`UIApplication.open`) | none |

### 5.1 Comment mode without a persistent notification

On Android, Comment and Threads live in the notification shade, with the
Comment tab when notifications are off (UI spec owner's decision 8).
iOS has no ongoing notification. The options:

| Option | For | Against |
|---|---|---|
| **The Clax bar** at the bottom, in the home indicator's band (recommended) | always one tap away; costs the page nothing, since it sits in the bottom safe area the page already leaves; reads as Safari's compact bar | covers the band a page drawn edge to edge (`viewport-fit=cover`) would use |
| A Live Activity | on the Lock Screen and in the Dynamic Island | a widget extension target; the HIG reserves Live Activities for an activity with a progress and an end, not a standing shortcut; it is gone after 8 hours |
| A long-press anywhere on the page | no chrome at all | the page owns long-press (text selection, its own menus), and nothing shows the gesture |
| An App Intent ("Comment in Clax") | the Action Button, Control Center, Siri and Shortcuts reach it | needs setup by the person; it is a second door, not the first |
| The Android Comment tab on the right edge | matches Android | not an iOS pattern, and it covers page content |

**Recommended: the Clax bar.** It is the iPhone's form of the UI spec's
Clax pill (§5.2 there), placed where iOS puts a browser's controls.

- **Compact (the default):** a capsule centred in the home indicator band,
  on the system material (Liquid Glass on iOS 26), 30 pt high in a 44 pt
  target. It shows the app's icon, its name and, while something is going
  on, the pill's states ("Working", "Building 2/5", "New version").
- **A tap** expands it into a toolbar above the band: Home, Comment,
  Threads with its count, Share, and ⋯ (Inspect, Versions, Star). It folds
  back after 4 s, or on the next tap on the page.
- **A long-press** on the bar enters comment mode at once, with a medium
  haptic.
- **Scrolling** the page down shrinks it to a 6 pt line over the home
  indicator, as Safari's bar does; scrolling up or a tap brings it back.
  This needs the shell's `chrome` message (UI spec §6.2) to report the
  page's scroll direction, which the shell adds.
- **Comment mode's own bar** (UI spec §5.3) replaces the Clax bar while it
  is on.

The App Intent is a later phase's second door (phase 7). It is cheap to
add once the bar's actions exist, and it gives the Action Button a "Comment
in Clax". The Live Activity is kept for builds in progress (§6.2), where
the HIG fits it, and is not in this round.

## 6. Background work

### 6.1 Finish what fits

- When the app leaves the screen with a run in progress,
  `Background.swift` calls `beginBackgroundTask(withName: "Clax run")`.
  `BackgroundWork.start` already exists for Android's service; on iOS it
  answers true while the task is held. iOS gives about 30 seconds, and
  less under load. `backgroundTimeRemaining` is posted to Dart every 5 s.
- **A change** (one edit to an app) that ends inside that time lands as it
  does in the foreground: its version is saved, and the "… is ready" or
  "Couldn't …" notification is posted (`BackgroundRuns`,
  `lib/platform/background_work.dart:199-212`).
- **When 5 s remain**, or at the task's expiration handler if that comes
  first, Swift sends `stopped`. On iOS, `stopped` means pause:
  - each run stops at once, keeping what it has: a change keeps its draft
    in `UnfinishedChanges` (as at a limit); a build keeps its plan's
    position (its step is run again, never skipped); a make keeps its
    request in `PendingMake` as `waiting`;
  - the in-flight Messages API request is cancelled, not left to fail, so
    nothing is billed for a reply that will not be read;
  - the daemon checkpoints (§2.4);
  - one notification is posted, replacing any earlier one: **"Paused —
    open Clax to keep building."**, with the app's name as its title.
    Tapping it opens that app's build view;
  - the task is ended.
- **On return** (`resumed`), every paused run starts again from where it
  stopped, with no tap: a build continues from its step, a change runs on
  from its draft (Keep going's path), and a make starts again. The paused
  notification is removed.
- **A kill while paused** (iOS ends suspended apps freely) is a cold start,
  and the same records resume the runs at the next launch: `PendingMake`
  already does this for makes, and the plan store and `UnfinishedChanges`
  do it for builds and changes.
- On Android nothing changes: `stopped` there still means the service
  stopped, and runs go on.

### 6.2 Paths not taken in this round

- **`BGContinuedProcessingTask`** (iOS 26) lets an app started on a
  person's tap keep running in the background, with a system progress UI,
  until it finishes or the person stops it. It could let a long build
  *finish* rather than pause. The owner's decision 3 says a longer build
  pauses, so it is not used. It is recorded as an alternative with
  `revisit:` after TestFlight feedback (open question 4).
- **A Live Activity for a build** ("Building Groceries · 3/5", then
  "Paused") on the Lock Screen and in the Dynamic Island. It needs a
  WidgetKit extension (rules_apple's `ios_extension`), and is a later
  round's.
- **Background fetch and processing tasks** (`BGAppRefreshTask`,
  `BGProcessingTask`) run at times iOS chooses, often hours later, and
  need the `UIBackgroundModes` key. They do not fit a build the person is
  waiting for.

## 7. The iOS interface

The UI spec's tokens (§1 there), components (§2) and surfaces (§5) apply as
written. This section names what iOS changes, against the Mac's Apple
family.

### 7.1 Two families in the kit

`lib/ui/kit/` (UI spec §2) chooses a family per platform: **Material**
(Android, the web shell's look in Flutter) and **Apple** (macOS and iOS).
Inside the Apple family each widget takes the idiom from
`defaultTargetPlatform` and the size class:

| Component | Mac (UI spec) | iPhone | iPad |
|---|---|---|---|
| `ClaxScaffold` | content view in the main window, `MacToolbar` | `CupertinoPageScaffold` with a navigation bar; large titles on Home, Inbox and Settings | sidebar and detail at regular width, as iPhone at compact width |
| `ClaxButton` | push buttons, 24 pt | filled, tinted and plain; 50 pt for a primary action at the bottom of a sheet, 44 pt targets | the same |
| `ClaxTextField` | bordered, 13 pt, focus ring | `CupertinoTextField`, 17 pt, rounded, no focus ring | the same, with a focus ring when a keyboard is attached |
| `ClaxSwitch` etc. | Mac look | `CupertinoSwitch`, in Clax's green | the same |
| `ClaxSheet.show` | window-attached sheet | a sheet with detents (medium, large) and a grabber (§7.3) | a form sheet, centred, 540 pt wide |
| `ClaxDialog.show` | `NSAlert` sheet | `UIAlertController` (`.alert`) through `NativeDialogs` | the same |
| `ClaxMenu.show` | `NSMenu` at the pointer | `CupertinoContextMenu` with a preview for tiles and rows; a pull-down menu for ⋯ buttons | the same; a right click or two-finger tap opens it with a pointer |
| `ClaxToast` | inline bar under the toolbar | a capsule under the Dynamic Island or the status bar, as iOS's own banners | top of the detail pane |
| type | SF Pro 13 pt body | SF Pro 17 pt body, Dynamic Type through `MediaQuery.textScaler` | the same |
| targets | 24 pt controls | **44×44 pt** at least | 44 pt; hover states with a pointer |

The tokens' colours, radii and motion are shared. The type scale gets an
iOS column in `design/tokens.json` (UI spec §1.3), keyed on Apple's text
styles (Large Title 34, Title 1 28, Headline 17 semibold, Body 17,
Callout 16, Subheadline 15, Footnote 13, Caption 12).

### 7.2 Structure by size class

- **iPhone, and iPad at compact width** (Slide Over, the narrow side of
  Split View, the smallest Stage Manager windows): one navigation stack.
  Home is the root, with a large title, the home app's grid and the Clax
  bar's New. An app pushes over it full screen with the Clax bar
  (§5.1). Inbox and Settings push from Home's navigation bar buttons.
  **Swipe-back** from the left edge pops, through `CupertinoPageRoute`. On
  an app page, swipe-back returns to Home, and the page's own history is
  the Clax bar's ⋯ › Back (the WebView's `allowsBackForwardNavigationGestures`
  is off, so one edge gesture never means two things).
- **iPad at regular width:** the Mac's sidebar and detail (UI spec §3.3,
  §4.2-4.4), drawn for touch:
  - The sidebar is 320 pt, inset grouped, with 44 pt rows: Starred, Apps
    (groups as disclosure rows), Making. Search is at its top.
  - It collapses as iPadOS's sidebars do: the navigation bar's sidebar
    button, or a swipe from the left edge.
  - The detail has a navigation bar in place of the Mac's unified toolbar,
    with back, title, Comment, Threads with its count, Share and ⋯.
  - Threads is a third column (320 pt) when the window is at least
    1180 pt wide, and a sheet below that.
  - Inbox and Settings are sheets (form sheets), not windows: one scene
    this round (open question 5).
- **Split View, Slide Over and Stage Manager.** The app supports every
  orientation and does not set `UIRequiresFullScreen`. It moves between
  the two structures at the 700 pt width breakpoint as the window is
  resized, keeping the selected app and its scroll position. Flutter
  resizes without a restart.
- **iPadOS 26's windows and menu bar.** The Mac's menu bar (UI spec §4.8)
  is declared once through `UIMenuBuilder` for iPad, with the same
  commands and ⌘ shortcuts. Holding ⌘ shows them.

### 7.3 Per surface

The surfaces are the UI spec's §5. Only iOS's differences are listed.

| Surface | iOS |
|---|---|
| Launcher and home (§5.1) | Large title "Clax"; long-press on a tile is a `CupertinoContextMenu` with the tile as its preview (Open, Inspect, Star, Share, Delete in red); arranging uses the home app's own drag, with a selection haptic on pick-up and a light impact on drop |
| App chrome (§5.2) | the Clax bar (§5.1) on iPhone; the detail's navigation bar on iPad; toasts at the top; the load failure is `ClaxEmpty` as specified |
| Comment mode (§5.3) | the comment bar sits above the home indicator, with the Clax bar gone. "Tap anything to comment on it." The composer is a sheet at the medium detent that rises with the keyboard (`MediaQuery.viewInsets`), Send in the keyboard's accessory row. Picks give a light impact |
| Inbox and threads (§5.4) | `CupertinoListSection.insetGrouped`; **swipe actions** on rows: trailing Resolve (green), leading Mark Read; pull to refresh |
| Settings (§5.5) | inset grouped, as iOS's Settings app; the API key field is a secure field with Paste; Notifications opens iOS's settings for Clax when they are off |
| Making (§5.6) | the build view is a sheet at the large detent over the app, or the detail on iPad; it says "Paused" with the notification's words when it was paused (§6.1) |
| Inspector and version page (§5.7) | pushed in the stack; versions with swipe-to-restore on rows |
| Sheets and dialogs (§5.8) | detents: the change sheet and questions at medium, export and update at medium, making and the inspector at large; a grabber and no close button, except on the large detent, which gets a Done button in its navigation bar; confirmations and destructive choices are action sheets (`UIAlertController` `.actionSheet`, anchored to their button on iPad) |
| Native overlays (§5.9) | the same Flutter overlays; the permission prompt is a UIKit alert |
| Errors and empty states (§5.10) | as specified |
| Splash and icon (§5.11) | the iOS icon set (1024 pt, with iOS 18's dark and tinted variants), and a launch screen of `bg` with the mark centred, matching the first Flutter frame |

**The keyboard.** Fields scroll above it. The WebView's own forms keep
WebKit's accessory bar. The composer submits on Return only when a
hardware keyboard is attached, and inserts a new line otherwise. With a
hardware keyboard on iPad, the Mac's shortcuts (UI spec §4.10) work.

**Haptics** come from one place, `ClaxHaptics` in the kit: selection
(pickers, drag pick-up), light impact (a pick, a drop), success (a version
landed while the app was in front), warning (a failed change). They play
on iPhone only (an iPad has no Taptic Engine) and honour the system's
setting.

## 8. App Review

### 8.1 How Clax fits

Clax makes small web apps with an AI agent and runs them in WebKit. The
guidelines that matter:

- **2.5.2** forbids an app that downloads, installs or executes code that
  introduces or changes the app's features. Clax's own features are in the
  binary: the daemon is compiled in, Dart is AOT, and the shell and kit
  are embedded. What runs from the network or the model is HTML and
  JavaScript, run only by WebKit, which 2.5.2 and 4.7 allow.
- **4.7** (mini apps, chatbots and plug-ins not embedded in the binary)
  allows HTML5 mini apps under conditions:
  - **4.7.1, privacy and content rules:** made apps fall under Clax's
    privacy policy and its terms.
  - **4.7.2, no native APIs beyond WebKit's without Apple's permission:**
    this is the main risk (§8.2).
  - **4.7.3, no data or permission shared with a mini app without
    explicit consent:** per-app consent already exists for reads (the
    read consent dialog) and for device features (`FileDeviceGrants`).
  - **4.7.4, an index of the mini apps:** the home screen and the sidebar
    are one.
  - **4.7.5, age:** the app's age rating covers what a person can ask the
    agent to make. This is the later listing's work.
- **5.1.2(i)**, personal data shared with a third-party AI, needs a clear
  disclosure and explicit permission. On first use, before the first
  request to the Claude API, a sheet says that what the person types or
  says, and the app they are changing, go to Anthropic under their own
  key, with Allow and Not Now. Settings shows the choice and can take it
  back.
- **3.1.1, payments:** the person brings their own Anthropic key, and
  nothing is sold or unlocked in the app.
- **1.2, user-generated content:** made apps are shared only as files
  through the share sheet, person to person; Clax hosts nothing in this
  round, so 1.2's reporting and blocking duties do not arise. Hosted
  sharing (clax.dev) would bring them in.

### 8.2 What to avoid

- **Exposing native APIs to pages beyond WebKit's.** The device host
  (`lib/platform/device/`) gives pages camera, location, files, share and
  reminders through the host protocol. On iOS:
  - camera and location go through WebKit's own (`getUserMedia`,
    `<input type=file capture>`, `navigator.geolocation`), each with
    iOS's own prompt and Clax's per-app grant in front;
  - share goes through `navigator.share`, which WKWebView supports;
  - files go through `<input type=file>`, which WebKit serves with the
    document picker.
  That leaves **reminders** (local notifications) as the one native
  capability, behind the per-app grant. The draft keeps it on for internal
  TestFlight and asks the owner whether it stays for external testers,
  with a line about it in the review notes (open question 3).
- **Downloading native code or a new Dart snapshot.** None, ever; the home
  bundle and kit updates are HTML, CSS and JS, signed (`lib/bundle/
  trust.dart`).
- **A browser in disguise.** The WebView follows no link out of the app
  (links open in Safari through `ExternalLinks`), and pages load only from
  the loopback daemon. 2.5.6 (browsers must use WebKit) is met by using
  WebKit only.
- **Local agents, the CLI, Codex push.** They are not in the iOS build at
  all (§3), so no review question about running other programs arises.
- **Background abuse.** Only `beginBackgroundTask`, no
  `UIBackgroundModes`, no audio trick to stay alive.
- **Placeholder or developer UI.** The UI spec's clean-up (its owner's
  decision 5) lands before an external TestFlight build.

## 9. Testing

**Unit and widget tests** run as today (`just app-test`). New tests:
- `test/daemon/in_process_test.dart`: the supervisor over a fake
  `InProcessStarter`, for start, the key over `PUT /api/sample/key`,
  resume with a dead listener restarting on the same port, and a failed
  migration;
- `test/platform/background_pause_test.dart`: `stopped` on iOS pauses each
  run kind, posts one notice, and resumes on `resumed`, with an injected
  clock;
- the kit's Apple family on iOS for every component in §7.1.

**Rust:**
- `crates/clax-ios` tests run on the host (macOS):
  - start, stop, start again in one process;
  - `StopOn::Embedded` installs no signal handler (a SIGTERM to the test
    process's own handler is unchanged);
  - the key route is absent under `Signals` and present under `Embedded`;
  - `clax_trim_memory` checkpoints the WAL.
- The `cfg(target_os = "ios")` arms are built by `:ios_build_test` (§3).

**Simulator integration tests** (`integration_test/ios_*_test.dart`,
`manual`, run by `just app-integration --ios` on a booted "Clax iPhone"
and "Clax iPad"):
- **cold start:** the daemon comes up, `daemon.json` names the app's PID,
  the home shows, and a made app opens in its WebView with the key cookie;
- **background:** `xcrun simctl` sends the app to the background and
  returns it. The listener is checked, the stream reconnects, and a
  `*.localhost` artifact origin loads;
- **pause:** a scripted build (`CLAX_SCRIPTED_AGENT`) is paused by a test
  channel's `expireBackgroundTask`, which shows the paused notification
  and resumes on return;
- **open a `.clax`:** `simctl openurl` on a bundle file opens the
  install sheet.

**Renders that someone looks at.** The UI spec's rig (§9 there) gains
three forms:
- iPhone, 402×874 pt at 3× (iPhone 17), SF Pro;
- iPad, 1032×1376 pt at 2× (iPad Pro 13-inch), sidebar and detail;
- iPad compact, 375×1376 pt (Split View's narrow side).
Each is rendered in light and dark, plus Dynamic Type at the largest
accessibility size, and written to
`target/flutter-app/shots/<surface>-<form>-<theme>.png`. Nothing compares
pixels. Each phase that changes a surface ends with its renders looked at.
The simulator's own screenshots (`xcrun simctl io booted screenshot`) of
the same states are taken by the integration tests. They are the check on
the parts Flutter does not draw: the status bar, the home indicator, alerts
and the share sheet.

**Device checklist** (phase 6, on the owner's iPhone and an iPad, from a
TestFlight build):
1. Install from TestFlight; the first launch shows Home within the
   time-to-usable budget (`homeUsable` in `timings.jsonl`).
2. Save an API key; make an app; it opens; comment on it; a change lands.
3. Leave during a change that fits: the "… is ready" notification arrives,
   and tapping it opens the app.
4. Leave during a long build: "Paused — open Clax to keep building."
   arrives, and on return the build continues from its step.
5. Lock the phone during a change; unlock; the version is there.
6. Leave the app for an hour, and return: the daemon answers, or restarts
   on the same port, and the open app's page and its local storage are
   intact.
7. Force-quit during a make; reopen; the make is offered again.
8. Camera, location (precise and approximate), file picker, share sheet
   (iPad: anchored), dictation, reminders: each prompt shows its usage
   string.
9. iPad: rotate; Split View at a third, a half and two thirds; Slide
   Over; a Stage Manager window resized through the breakpoint; a
   hardware keyboard's shortcuts; pointer hover and right click.
10. Memory: open ten apps in a row on the oldest supported device; no
    jetsam (Settings › Privacy › Analytics shows none for Clax). A page's
    content process killed under pressure reloads.
11. Airplane mode during a change: the change waits for the connection,
    as on Android.
12. Swipe-back on every pushed screen; VoiceOver reads the Clax bar and the
    comment bar.

## 10. TestFlight

### 10.1 Identity

- **Bundle ID:** `dev.empathic.clax`, the Android application ID and the
  Mac's `PRODUCT_BUNDLE_IDENTIFIER` (`AppInfo.xcconfig`). One ID across
  Apple platforms keeps a universal purchase open for later. The owner
  registers it as an explicit App ID in their team (open question 2).
- **Name:** "Clax" (UI spec owner's decision 6), if App Store Connect
  accepts the name; otherwise the owner picks a listed name such as
  "Clax — make small apps", with "Clax" under the icon.
- **Version:** `pubspec.yaml`'s version as `CFBundleShortVersionString`;
  the build number from the upload ledger (§4.5).

### 10.2 Entitlements

The release set is deliberately small:
- `application-identifier` and `com.apple.developer.team-identifier`,
  from the team ID;
- `keychain-access-groups`, the app's own group, which
  `flutter_secure_storage` uses;
- `get-task-allow` in development builds only.

No push notifications (`aps-environment`; every notification is local),
no App Groups (§2.3), no background modes, no increased memory limit
(§2.8), and no associated domains in this round.

### 10.3 Info.plist

- **Usage strings**, in Clax's voice (UI spec §7):
  - `NSCameraUsageDescription`: "An app you made in Clax can take a photo
    when you let it."
  - `NSPhotoLibraryUsageDescription`: "An app you made in Clax can use a
    photo you pick."
  - `NSMicrophoneUsageDescription`: "Clax listens while you speak a
    request or a comment."
  - `NSSpeechRecognitionUsageDescription`: "Clax turns what you say into
    text for your request or comment."
  - `NSLocationWhenInUseUsageDescription`: "An app you made in Clax can
    use your location when you let it."
  - `NSLocationTemporaryUsageDescriptionDictionary` › `precise`: "This
    app asked for your precise location."
- `NSAppTransportSecurity` › `NSAllowsLocalNetworking`: true (§2.9).
- `CFBundleDocumentTypes`, `UTExportedTypeDeclarations` (`.clax`), and
  `LSSupportsOpeningDocumentsInPlace`: false, so a bundle is copied in.
- `UISupportedInterfaceOrientations` (all on iPad; portrait and both
  landscapes on iPhone), `UILaunchScreen`, and
  `UIApplicationSceneManifest` with one scene.
- `ITSAppUsesNonExemptEncryption` (§10.5).

### 10.4 Privacy manifest (`ios/Runner/PrivacyInfo.xcprivacy`)

- `NSPrivacyTracking`: false; no tracking domains.
- **Collected data types:** Clax's developer receives nothing; nothing is
  sent to Empathic. What the person types or says, and their app's code,
  go to Anthropic under the person's own key, for the app's function. The
  draft declares this as "Other User Content", not linked to identity, not
  used for tracking, purpose App Functionality. The owner confirms the
  reading (open question 7).
- **Required-reason APIs** in the app's own code (the plugins' manifests
  are merged by `flutter_ios_privacy_manifests`):
  - File timestamp, `C617.1`: SQLite and the store `stat` files inside the
    container.
  - System boot time, `35F9.1`: Tokio's and Dart's monotonic clocks
    measure elapsed time.
  - Disk space, `E174.1`: only if phase 2 finds the store checks free
    space before a write. Otherwise it is not declared.
  - User defaults, `CA92.1`: only if a plugin's manifest does not already
    cover its own use.
  Phase 3 lists the symbols the binary actually links (`nm -u` on the
  app) and declares exactly those.

### 10.5 Export compliance

Clax uses encryption for HTTPS to the Claude API (rustls in the daemon,
`dart:io`'s TLS in the agent) and for authentication: Ed25519 signatures
on bundles and SHA-256 content hashes (`lib/bundle/signing.dart`). It
encrypts no user data for storage or for transmission beyond HTTPS. The
draft sets `ITSAppUsesNonExemptEncryption = NO`, on the reading that HTTPS
and authentication fall under the EAR's exemptions, so no documentation
is uploaded. Because rustls is not Apple's TLS, this is the owner's call
(open question 6). If the answer is yes, App Store Connect takes an annual
self-classification report instead, and the key flips.

### 10.6 The upload path

1. Owner, once (open question 2):
   - an Apple Developer Program membership;
   - the App ID `dev.empathic.clax`;
   - the Apple Development and Apple Distribution certificates in the
     login keychain;
   - two provisioning profiles (development with the owner's devices, and
     App Store);
   - an App Store Connect record for the app;
   - an App Store Connect API key with the Developer role.
   Each file goes where §4.4 names it.
2. `just app-iphone --testflight --i-know-what-i-am-doing`: build, sign,
   `altool --upload-package`, print the build number.
3. App Store Connect processes the build (minutes). Export compliance is
   answered by the plist key.
4. **Internal testing** (up to 100 people of the team) needs no review:
   the build is available once processed. This round's target is here.
5. **External testing** needs Beta App Review of the first build of each
   version, with review notes. The draft's notes cover:
   - what Clax is;
   - that the person's own Anthropic key is needed, with a review key
     provided through App Store Connect's field;
   - §8.1's points on 2.5.2 and 4.7.

## 11. Phase 1: the spike, run on 2026-10-18

**Toolchain on this Mac:**
- Xcode 26.6 (17F113), with the iPhoneOS and iPhoneSimulator 26.5 SDKs
  (`xcrun --sdk iphonesimulator --show-sdk-path` →
  `…/iPhoneSimulator26.5.sdk`).
- Rust 1.94.0. The `aarch64-apple-ios` and `aarch64-apple-ios-sim` std
  libraries were added with `rustup target add` for the spike.
- **Missing: an iOS simulator runtime.** `xcrun simctl list runtimes` and
  `xcrun simctl runtime list` list none, and `simctl list devices
  available` is empty. So no simulator could boot. It is installed by
  `xcodebuild -downloadPlatform iOS` (several GB), or by Xcode › Settings
  › Components › iOS 26.5 Simulator. It was not downloaded unasked.

**What was built** (code under `target/ios-spike/`, not committed):

1. `target/ios-spike/claxffi`, a `staticlib` crate (its own workspace, the
   repository's `Cargo.lock` copied) depending on `clax-core`,
   `clax-server`, `clax-sample`, `clax-realtime` and `clax-embedded` by
   path. It has `clax_start(port) -> port` and `clax_state()`, scalar
   only, with the home at `$HOME/Library/Application Support/clax-home`
   and the token left in `daemon.json`. It builds the `ServeConfig` as
   `clax-cli/src/serve.rs` does, with `require_credential: true`, codex
   push disabled and no key.
2. `cargo build --release` for `aarch64-apple-ios-sim`,
   `aarch64-apple-ios` and `aarch64-apple-darwin`: **all three built with
   no change to any crate and no warnings**, in 1 min 47 s cold for the
   simulator. `libclaxffi.a` is about 83 MB unstripped for each triple.
3. A minimal UIKit app (`target/ios-spike/app/main.swift`). It calls
   `clax_start(0)`, opens a `WKWebView` on `http://127.0.0.1:<port>/
   healthz`, and writes what the page says to `Documents/result.txt`. Its
   Info.plist has `NSAllowsLocalNetworking`. It was compiled and linked
   with `swiftc` against the static library for **both the simulator
   (`arm64-apple-ios17.0-simulator`) and a device (`arm64-apple-ios17.0`)**,
   with `-framework Security -framework SystemConfiguration
   -lresolv -lc++`.
   - Every symbol resolved. The stripped binary is 37.5 MB, and
     `vtool -show-build` reports `platform IOS, minos 17.0, sdk 26.5`.
   - `ld` warned that the C objects of `ring` and SQLite were built for
     iOS 26.5 while the app linked for 17.0. Setting
     `IPHONEOS_DEPLOYMENT_TARGET=17.0` rebuilt them, but the warnings
     stayed (§4.2 handles this under Bazel).
4. The same two calls in a minimal **AppKit** app with a `WKWebView`
   (`target/ios-spike/mac/main.swift`), run with `HOME` pointed at a
   scratch directory, to exercise in-process start and WebKit on loopback
   where a simulator could not:
   - `clax_start(0)` returned a listening port in **48–57 ms** on a fresh
     home. Migrations ran, and `daemon.json`, `clax.db` (WAL), `artifacts/`
     and `toolpath/` were created under the scratch home only.
   - The WKWebView loaded `/healthz` (`{"pid":<the app's own PID>,
     "version":"spike",…}`) **0.23–0.42 s** after launch.
   - Without the key cookie, `/` answered the credential gate's 401, as
     `--require-credential` intends.
   - With `clax_key_<port>` set from `daemon.json`'s token through
     `WKHTTPCookieStore`, `/` loaded the shell (`document.title` "Clax",
     two scripts).
   - The whole process peaked at a 27 MB memory footprint
     (`/usr/bin/time -l`).

**Proven:** the daemon builds as an iOS static library for device and
simulator without source changes. It links into an iOS app with a
scalar-only C ABI, and it runs in-process on the app's own thread and Tokio
runtime with the app's PID. WebKit loads it from `127.0.0.1` with the key
cookie.

**Not yet proven, for the first run once the simulator runtime is
installed** (the app in `target/ios-spike/app` runs as is):
- the WKWebView load on iOS under ATS with `NSAllowsLocalNetworking`;
- no local network prompt for loopback;
- `*.localhost` resolving in iOS's WebKit;
- the listener after a background and foreground cycle (`simctl` can
  suspend the app);
- the time to first page on a simulator.
Phase 2 starts by running these.

**Run on the simulator, 2026-10-19:** "Clax iPhone" (iPhone 17, iOS 26.5
(23F77)). The as-built app (`target/ios-spike/app/build-sim`) ran first; the
harness then gained a scene delegate, a `localhost` and a `spike.localhost`
load, a framed-picker page and a reload on `sceneWillEnterForeground`, and
was relinked with `swiftc` against the same `libclaxffi.a`. Logs and
screenshots are under `target/ios/spike-run/` and `target/ios/shots/`.
1. **ATS loopback: yes.** With only `NSAllowsLocalNetworking`,
   `http://127.0.0.1:<port>/healthz` and `http://localhost:<port>/healthz`
   answered 200 with the app's own PID.
2. **Local network prompt: none** in the screenshots of the first launch
   (`spike-healthz.png`) or of the framed page, with no
   `NSLocalNetworkUsageDescription`. The simulator is not known to enforce
   local network privacy, so the release device build still checks it
   (Group B).
3. **`*.localhost`: WebKit resolves it, but ATS blocks it under
   `NSAllowsLocalNetworking` alone.** `http://spike.localhost:<port>/
   healthz` failed with `NSURLErrorDomain -1022` ("App Transport Security
   policy requires the use of a secure connection"), contrary to §2.9. With
   `NSExceptionDomains` › `localhost` › `NSIncludesSubdomains` and
   `NSExceptionAllowsInsecureHTTPLoads` added, the same URL answered 200
   from the daemon, and a frame at `http://spike.localhost:<other port>/`
   loaded (its server logged `Host: spike.localhost:<port>`). Which ATS key
   the app ships is the owner's call (record on §2.9).
4. **A gesture in a cross-origin frame opens a picker in the top document:
   yes.** An XCUITest tap on a button in the `spike.localhost` frame posted
   a message to the `127.0.0.1` parent, which logged
   `navigator.userActivation.isActive` true and called `input.click()` on
   an `<input type=file>`; iOS showed its file menu (Photo Library, Take
   Photo or Video, Choose File) at once (`spike-picker.png`).
5. **The listener after a background cycle: answered.** With Settings in
   front, `curl` to the daemon timed out (the app suspended). Brought back
   after 72 s, the same process (same PID and port, `clax_state` 2) reloaded
   `/healthz` with 200 in 0.4 s. The simulator suspends but did not reclaim
   the socket; a reclaimed listener is still for a device (§2.4).
6. **Time to first page:** `clax_start(0)` returned the port in 12–71 ms
   (157 ms, measured from launch, on the first launch after install).
   `/healthz` finished loading 2.13 s after launch on that first launch
   (WebKit's processes starting cold) and 1.10–1.79 s on the five launches
   after.

## 12. Phases

Each phase ends with something to see or run. The iOS UI phases follow the
UI spec's: the Apple family is the Mac's (its phase 6) extended, so phase 5
here waits on it.

1. **The spike** (done, §11). The simulator half waits on the runtime
   being installed.
2. **The daemon crate.**
   - `crates/clax-ios` with the four functions, `StopOn` in `ServeConfig`,
     `PUT /api/sample/key`, and the iOS `cfg` arms of §3;
   - `:ios_build_test`, and the host tests of §9;
   - the AGENTS.md exception, if the owner approves it (open question 1).
   - *Run:* `cargo test -p clax-ios` and the spike app against the real
     crate on the simulator.
3. **Bazel and the bare app.**
   - The two triples, `apps/clax_app/ios/`, `flutter_ios_app` with
     `:ios_daemon`, `InProcessStarter` and `DaemonHost`;
   - `just app-ios`, and `:ios_app_test` (the bundle's files, the
     deployment target of every object, the plist keys);
   - the iOS form of `version_sync`.
   - *Visible:* today's app, in Material, running on the simulator with
     its daemon in-process.
4. **Platform glue.**
   - The native, background, notice and bundle channels in Swift; the
     pause and resume of §6.1;
   - the plugins' iOS halves and their usage strings; the
     WebKit-first device features of §8.2;
   - the simulator integration tests of §9.
   - *Visible:* a make that pauses and resumes on the simulator.
5. **The iOS interface.**
   - The Apple family's iOS idiom (§7.1); the navigation stack;
     swipe-back; sheets with detents; context menus; swipe actions;
     haptics;
   - the Clax bar (§5.1); the iOS icon set and launch screen;
   - the iPhone renders.
6. **iPad.**
   - Sidebar and detail for touch; the size-class switch; Split View,
     Slide Over and Stage Manager; the `UIMenuBuilder` menu bar;
     pointer and keyboard;
   - the iPad renders.
7. **TestFlight.**
   - `@clax_apple_signing`, `just app-iphone` and `--testflight`, the
     upload ledger;
   - the privacy manifest and encryption key; the first-use AI disclosure
     (§8.1); the App Intent "Comment in Clax";
   - the first internal build, and the device checklist (§9) on the
     owner's devices.
   - This phase waits on the owner's certificate, team ID, profiles and
     App Store Connect record. Phases 2–6 do not.

## Decisions taken in this draft (the owner may override)

1. The daemon is a static library linked into the app, called from Swift
   over the existing `native` channel, not through `dart:ffi`.
2. Its C ABI has four scalar-only functions. The home is fixed under
   `$HOME`, the token comes from `daemon.json`, and the key goes over
   loopback.
3. One OS thread owns a two-worker Tokio runtime; Dart never calls Rust
   directly.
4. `ServeConfig.stop` (`Signals` or `Embedded`): an embedded daemon
   installs no signal handler.
5. `clax-home` stays in Application Support and is backed up, except
   caches, logs and `daemon.json`. Protection is "until first user
   authentication". There is no App Group.
6. The minimum is iOS 17.0.
7. Loopback under `NSAllowsLocalNetworking`, no arbitrary loads, no local
   network usage string.
8. A resume that finds the daemon dead restarts it in-process on the same
   port, through the existing `onResume` path.
9. On iOS, `BackgroundWork.stopped` means pause: at 5 s left, every run
   stops cleanly, one "Paused — open Clax to keep building." notice is
   posted, and runs resume on return without a tap.
10. Comment entry on iPhone is the Clax bar in the home indicator band:
    tap for the toolbar, long-press for comment mode. The App Intent is a
    second door in phase 7. The Live Activity is not used for comment
    entry.
11. iPad switches between stack and sidebar at 700 pt and has one scene.
    Inbox and Settings are sheets.
12. On iOS, camera, location, files and share go through WebKit's own
    APIs. Reminders are the only native capability exposed to pages.
13. The bundle ID is `dev.empathic.clax`, shared with Android and the Mac.
14. Signing material comes from `~/.config/clax/apple/` through a watched
    `local` repository. Signing actions are never cached remotely.
15. TestFlight uploads use `altool` with an App Store Connect API key.
    Build numbers come from a local ledger.
16. `ITSAppUsesNonExemptEncryption = NO`, pending open question 6.

## Open questions

These block, each at the phase named.

1. **The `unsafe` exception (blocks phase 2).** AGENTS.md says "No
   `unsafe`", and the workspace forbids `unsafe_code`. A C function Swift
   can call needs `#[unsafe(no_mangle)]`, which the lint counts. May
   `crates/clax-ios` alone deny the lint instead of forbidding it, and
   allow it on its four exported functions only, with a check that
   nothing else in it is `unsafe`? Without it, there is no in-process
   daemon on iOS.
2. **Apple account inputs (block phase 7, and any device run).** From the
   owner:
   - an Apple Developer Program membership (the free tier cannot use
     TestFlight);
   - the **team ID**;
   - the **bundle ID**: `dev.empathic.clax`, or another;
   - the **Apple Development and Apple Distribution certificates** in this
     Mac's login keychain (the same wait as record 553cb43495 for the
     Mac);
   - the UDIDs of the iPhone and iPad for the development profile;
   - an App Store Connect app record named "Clax" (or the listed name if
     "Clax" is taken);
   - an App Store Connect API key with the Developer role.
3. **Reminders for pages (blocks external TestFlight).** Keep the one
   native capability (§8.2) for external testers and explain it in the
   review notes, or turn it off on iOS until the App Store round?
4. **`BGContinuedProcessingTask` (does not block).** iOS 26 can let a long
   build *finish* in the background with a system progress UI. The
   owner's decision 3 says pause, and the draft follows it. Revisit after
   TestFlight feedback?
5. **iPad windows (does not block this round).** One scene now, with Inbox
   and Settings as sheets. Should a later round give the iPad multiple
   windows, as the Mac has separate Inbox and Settings windows?
6. **Export compliance (blocks the first upload).** Is "No non-exempt
   encryption" right for an app that uses its own TLS library (rustls)
   for HTTPS, and Ed25519 for bundle signatures? This may want counsel.
7. **The privacy label (blocks external TestFlight).** The person's
   prompts and app code go to Anthropic under their own key. The draft
   declares them as "Other User Content" for App Functionality, not
   linked to identity. Is that the reading the owner wants?
8. **The simulator runtime (blocks phase 1's second half).** May the
   iOS 26.5 simulator runtime (several GB) be downloaded on this Mac?
