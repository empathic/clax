# Clax on iPhone and iPad Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Clax app (`apps/clax_app`) runs on iPhone and iPad with its daemon inside the app's process, the Apple family's look in its iOS idiom, the iPad's sidebar and detail, a pause-and-resume rule for work the person leaves, a Bazel build (`just app-ios`, `just app-iphone`), and everything a TestFlight upload needs short of the owner's Apple account.

**Architecture:** A new crate, `crates/clax-ios`, builds the `ServeConfig` that `clax serve` builds and runs it on a thread of its own with a two-worker Tokio runtime, behind four scalar-only C functions (`clax_start`, `clax_stop`, `clax_state`, `clax_trim_memory`). `clax_server::serve` gains `StopOn::Embedded` (no signal handlers, a stop and a trim channel), and `clax_sample` gains a key slot that `PUT /api/sample/key` fills, so a key change needs no restart. Bazel builds the crate as a static library for `aarch64-apple-ios` and `aarch64-apple-ios-sim`, links it into rules_flutter's `flutter_ios_app` through a Swift library (`ClaxDaemon`), and the Dart supervisor drives it over the existing `dev.empathic.clax/native` channel through an `InProcessStarter`, so nothing above the supervisor changes. The iOS screens are the professional UI plan's adaptive layer (`ClaxFamily.apple`, `ClaxWidth`) given their iOS idiom; they build on that plan's tasks and never duplicate them.

**Tech Stack:** Rust 1.94.0 (rules_rust, crate_universe), Flutter 3.47.7 / Dart (`flutter_test`), Swift/UIKit (`apps/clax_app/ios/Runner`), rules_flutter 0.0.6 (`flutter_ios_app`), rules_apple 5.2.0, rules_swift 4.1.2, apple_support 2.9.1, the shell's app-mode TypeScript (vitest), Bash (`sh_test`), Xcode 26.6's `xcrun simctl`, `devicectl`, `altool`, `vtool`, `otool`.

**Spec:** `docs/superpowers/specs/2026-10-18-ios-release-design.md` (the "iOS spec"; section numbers below such as §2.5 are its). It builds on `docs/superpowers/specs/2026-10-18-professional-ui-design.md` (the "UI spec") and is built beside `docs/superpowers/plans/2026-10-18-professional-ui.md` (the "UI plan"; its tasks are written **UI T28**, its groups **UI 4c**). Read the section a task names before starting it. **Scope:** the Clax app only.

**The owner's answers to the iOS spec's open questions**, binding on this plan:
1. **The `unsafe` exception is approved**: only `crates/clax-ios`, only on its four exported C functions, with scalar arguments. A check fails if anything else in that crate is unsafe, or if any other crate relaxes the lint. `AGENTS.md` gets one line naming the exception (Task 6).
2. **The iOS simulator runtime is installed**: `xcrun simctl list runtimes` lists `iOS 26.5 (23F77)` (`target/ios/sim-download.log` ends "Done."). Phase 1's simulator half runs now (Task 1).
3. **`BGContinuedProcessingTask`: no.** A longer build pauses (Task 13 records the alternative).
4. **Reminders stay on iOS** (Task 17 keeps them within iOS's 64-notification cap).

**Still pending from the owner** (Apple Developer membership, team ID, the bundle ID `dev.empathic.clax`, Development and Distribution certificates, device UDIDs, an App Store Connect app record and API key, the export-encryption answer, the privacy label): only **Group B** at the end needs them. Every other task runs on this Mac without them.

## Global Constraints

- **Commits are unsigned:** `git -c commit.gpgsign=false commit …`. `git add` only the paths your task lists (never `git add -A` or `.`). Never push. Never file, close or comment on issues or pull requests. If a git operation fails on signing or 1Password, stop and report.
- **Git configuration:** never change shared git configuration (`rerere` or any other), and never use bare `git stash`.
- **Homes and daemons:** never run anything against `~/.clax` or the daemon on port 7480. Tests start their own daemons; a manual run uses `just dev` (`~/.clax-dev`) or a scratch `CLAX_HOME` on a free port, stopped by its PID. A simulator run uses only the simulators this plan creates ("Clax iPhone", "Clax iPad"); never erase or boot another. Never wait on processes with `pgrep -f`/`pkill -f`; use the PID you started, or an exit file, with a timeout.
- **Bazel only.** Build and test with `bazelisk build|test|run //…` (`docs/bazel.md`). No `cargo`, `pnpm`, `npm`, `flutter` or `dart` command in the worktree. After a `Cargo.toml` or `Cargo.lock` change, `just bazel-sync` and commit what it writes. Rust tests: `bazelisk test //crates/<crate>:<lib>_lib_test` (unit) and `bazelisk test //crates/clax-server:all --test_arg=<module>::` (integration, split into groups). Flutter tests: `bazelisk test //apps/clax_app:<dir>_<file>` (`test/daemon/in_process_test.dart` is `//apps/clax_app:daemon_in_process_test`; top-level `test/app_test.dart` is `//apps/clax_app:root_app_test`). Web unit tests: `bazelisk test //web:unit --test_arg=<path under web/>`. Anything longer than a couple of minutes runs in the background, logged to `target/plan-logs/ios-<task>.log`, waited on in bounded stretches.
- **Deterministic tests:** no sleeps standing in for a condition, no wall-clock budgets. Every timing rule (the 250 ms state poll, the 5 s pause margin, the 2 s trim and pause-ack waits, the 4 s Clax bar fold, the 1 MB log cut) runs on an injected clock or size: `Clock` (`apps/clax_app/lib/daemon/clock.dart`) with `tester.pump`, `tokio::time::pause()` in Rust, a `Duration`/size parameter in Swift helpers that tests set, `vi.useFakeTimers()` in vitest.
- **Prose:** write "ID" in prose, comments and commit messages; `id` only as a literal symbol in code. Doc comments and commit messages describe the contract or the change, never this plan, the conversation, or a rating of quality.
- **Qualifier** runs through `Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"; "$Q" exec <args>`. Read `"$Q" exec agents` and each topic it lists before your session's first record. Before editing a file, `"$Q" exec threads <path>` and read what is open on it. More than two writes go in one `"$Q" exec record --stdin` batch written to a file in the scratchpad (outside the repository), dry-run first. A record you are told to write is pinned to the lines it names; records cite paths, line ranges and command output, never this plan.
- **Closing records:** close only a record your session wrote or one your own commit fixed, with evidence (`"$Q" exec resolve <ID> "Fixed: <test> passes (<command>)" --reason fixed --ref git:<fix sha>`), in a **separate** commit. Propose every other close in the task report.
- **`unsafe`:** the workspace forbids it. The one exception is Task 6's: `#[unsafe(no_mangle)]` on the four exported functions of `crates/clax-ios/src/lib.rs`, each preceded by `#[allow(unsafe_code)]`. Nothing else anywhere. No hard links, anywhere.
- **Two families, not two platforms** (the UI plan's rule): components branch on `ClaxFamily.of(context)`, layout on `ClaxWidth`; only chrome that exists on one OS checks the platform, through one getter in `lib/ui/kit/family.dart` (`hasDesktopWindowChrome` for the Mac; this plan adds `isTouchApple` for iOS's own system UI: UIKit alerts, haptics, the Clax bar).
- **iOS native files:** every `apps/clax_app/ios/Runner/*.swift` is globbed by `flutter_ios_app` (`ios.bzl:556`); no BUILD change for a new one. Test-only native code is inside `#if DEBUG`.
- **Renders are looked at:** a UI task is done only once its renders have been opened with the Read tool and match the iOS spec §7 and the surfaces beside them. Simulator screenshots (`xcrun simctl io <udid> screenshot`) go to `target/ios/shots/`. "Tests pass" is not verification for UI.
- **Copy** follows the UI spec §7 (’ “ ” …, sentence case, "Clax", "app"). From UI T25 on, `//scripts:check_copy` enforces it.
- Before the branch merges, `just check` runs once (in the background) and `"$Q" exec diff main` is read; both results go in the final report.

## Decisions this plan takes where the code differs from the spec

Each is the closest working reading of the spec; the owner may override any.

1. **The key slot lives in the sampler, not in `ServeConfig.stop`.** `Sampler::from_config` reads the key once and bakes it into `AnthropicProvider` (`crates/clax-sample/src/lib.rs:123-138`; the provider clones `api_key` per call at `anthropic.rs:383-386`), so a closure over a slot (§2.6) would change nothing after start. Task 4 adds `clax_sample::KeySlot`, read on every call; `PUT /api/sample/key` answers 404 unless the daemon's sampler was built with a slot, which only `clax-ios` does. It is not a `StateParts` field: `crates/clax-worker/src/instance.rs:428` builds `StateParts` too.
2. **The web UI adapter is copied, not moved.** `scripts/check-crate-layers.py` lets `clax-embedded` depend on nothing, so it cannot implement `clax_core::embedded::FileTree`. `crates/clax-ios/src/web.rs` keeps its own copy of the 15-line `WebUi` of `crates/clax-cli/src/bundle.rs:30-45`.
3. **The exit statuses move to `clax_core::exit`.** `MIGRATION_FAILED_EXIT` and `exit_code` sit in `crates/clax-commands/src/cli.rs:428-440`, which `clax-ios` may not depend on (it spawns `claude`, `codex`, `git`). `clax-core` has no `anyhow`, so the helper walks a `std::error::Error`'s `source()` chain (anyhow's chain is that chain).
4. **A panic on the daemon thread is state 201** (100 + 101, the status a panicking `clax serve` exits with). The spec's list gives 101 to both "a panic" and "100 + 1 any other error"; this keeps its rule "100 + the exit status `clax serve` would have exited with".
5. **Trim is SQLite only.** The HTTP cache has no memory tier: `NetCache` keeps entry bodies on disk and only an index in memory (`crates/clax-http/src/net_cache.rs:262`). `clax_trim_memory` runs `PRAGMA shrink_memory` on every store connection and one `PASSIVE` checkpoint.
6. **Accept errors.** `clax_http::auth::TunedListener::accept` (`auth.rs:453-457`) delegates to axum 0.8's `Listener for tokio::net::TcpListener`, which retries every accept error, sleeping 1 s on any that is not a per-connection one. Task 3 confirms it from the fetched axum source and records it; there is no failing-listener test. The restart after a reclaimed listener is tested in Dart (Task 9) and on the simulator (Task 19).
7. **Simulator builds are debug only.** rules_flutter refuses a simulator app outside `-c dbg` (`flutter_ios_simulator_build`'s doc: the simulator engine runs only debug builds). `just app-ios` has no `--release`; profile and release builds are device builds.
8. **The local network prompt is checked on a release device build.** Debug and profile builds carry `NSLocalNetworkUsageDescription` and `NSBonjourServices` for the Dart VM service (rules_flutter merges `_DART_VM_SERVICE_PLIST` outside release, `ios.bzl` step 10b). Task 1's spike checks loopback without those keys; Group B checks the release build.
9. **No Xcode project for iOS.** The runner starts from rules_flutter's own templates (`@rules_flutter//flutter/private/runners/ios`: `AppDelegate.swift`, `SceneDelegate.swift`, `Info.plist`, the storyboards), not `flutter create`, and Bazel is the only iOS build: there is no `-native` twin.
10. **Every iOS notification goes through `flutter_local_notifications`.** The plugin is the one `UNUserNotificationCenter` delegate, so taps on a notification Swift posted would reach the plugin, not the app. The paused notice is posted from Dart when Swift sends `stopped`; Swift waits up to 2 s for Dart's `pauseDone` before it trims and ends the task. No `Notices.swift`.
11. **`backgroundTimeRemaining` is not posted to Dart:** nothing would read it. Swift owns the 5 s rule.
12. **Pause is a hold.** `ResumingTurns` and `ResumingBackend` (`lib/platform/network_pause.dart`) already abort a turn attempt and hold it until `AppPresence.whenForeground`, then send it again with no tool run twice (`network_pause.dart:131-227`, decided in record 8fd2656e: no time bound on a hold). `AppPresence.pause()` aborts the attempt in flight the same way. A pause does not count against `maxResumes`.
13. **iPad's stack-to-sidebar switch is `ClaxWidth.medium` (600 pt), not 700 pt** (§7.2), so the iPad shares the UI plan's one rule (`lib/ui/layout.dart`). Task 29 records the alternative with `revisit:after-ipad-testflight`.
14. **The bundle ID is one constant**, `IOS_BUNDLE_ID = "dev.empathic.clax"` in `bazel/flutter/ios.bzl`, until the owner confirms it (Group B1).
15. **The Apple platforms are `@apple_support//platforms:ios_arm64` and `:ios_sim_arm64`** (the spec's `build_bazel_apple_support` is the WORKSPACE name). `apple_support` becomes a root `bazel_dep` at 2.9.1, the version rules_flutter asks for.
16. **`--cfg clax_embedded` needs no iOS step:** `bazel/cargo.bzl` passes it to every Bazel build already.
17. **Stripping.** The static library is built `-c opt` in a transition (Task 8). `-Cstrip=symbols` has no effect on a staticlib (there is no link step), so the device app binary is stripped by rules_apple's `--objc_enable_binary_stripping` in `-c opt` device builds (`scripts/app-iphone.sh`, Task 32).
18. **Sheet detents are drawn in Flutter:** a modal route around a `DraggableScrollableSheet` snapping to medium (0.5) and large (1.0) with a grabber.
19. **iOS has no ongoing notification, so the Clax bar** (§5.1) replaces both the pill (UI T28) and the Comment tab (UI T29) on iPhone; Android keeps both.

## Review Focus

1. **Coming back after iOS reclaimed the listener while an app's page is open.** `/healthz` fails twice, the daemon stops and starts again on the same port, the page reloads at the same origin with its local storage, and the key slot is filled again. Test: Task 9 (`in_process_test.dart` "resume with a dead listener restarts on the same port and puts the key again"), Task 19 (`ios_background_test.dart`).
2. **Leaving the app over and over during one change.** Each leave pauses at the 5 s margin and the run resumes on return; four leaves in one turn must not fail it through `maxResumes`. Test: Task 13 (`background_pause_test.dart` "five pauses in one turn resend it five times and run no tool twice").
3. **A key saved, changed or cleared while the daemon starts, runs or restarts.** The slot ends with the last key, a clear turns `sample()` off, a key is never in a log line or a state, and a `PUT` before `Up` is not lost. Test: Task 4 (route tests, redaction), Task 9 ("a key changed while starting is put once the daemon is up").
4. **More than 64 reminders across apps.** iOS drops pending notifications past 64 silently; the nearest 60 are scheduled and the rest topped up as they fire, on every launch. Test: Task 17 (`reminders_test.dart` "on iOS 70 reminders schedule the nearest 60 and top up after one fires").
5. **A stop asked while the daemon is still starting, and a start while it stops.** `clax_stop` during migrations ends in 100, never a hang; `clax_start` while stopping answers 1 and changes nothing; a start after the stop ends starts a new runtime. Test: Task 7 (`lifecycle_races`).

---

## Order and parallel groups

Tasks in one group touch disjoint files and may run at the same time; groups run in order. A task never starts before the tasks (of either plan) its **Consumes** names.

| Phase | Groups (tasks in a group run in parallel) |
|---|---|
| 1 Spike | **1a** T1 |
| 2 Daemon crate | **2a** T2, T3, T4, T5, T6, T9 · **2b** T7 · **2c** T8 |
| 3 Bare app | **3a** T10 · **3b** T11 |
| 4 Platform glue | **4a** T12, T14 · **4b** T13, T15, T17 · **4c** T16, T18 · **4d** T19 |
| 5 iOS interface | **5a** T20, T21 · **5b** T22, T23 · **5c** T24, T25, T26, T28 · **5d** T27 |
| 6 iPad | **6a** T29, T30 · **6b** T31 |
| 7 TestFlight, no account | **7a** T32, T35 · **7b** T33, T34 · **7c** T36 |
| B Blocked on the owner's Apple inputs | **Ba** B1 · **Bb** B2 · **Bc** B3 · **Bd** B4 · **Be** B5 |

Task 9 (Dart, `lib/daemon/`) is in 2a because it needs only the channel's method names, not the crate.

### Beside the UI plan

The shared hot files are `apps/clax_app/lib/ui/app_screen.dart`, `apps/clax_app/lib/app.dart`, `apps/clax_app/BUILD.bazel` and `MODULE.bazel`; this plan also shares `apps/clax_app/lib/main.dart`, `lib/ui/host_protocol.dart`, `web/shell/src/app/host.ts`, `lib/ui/kit/*`, `lib/ui/theme.dart`, `design/tokens.json`, `test/ui/shots/rig.dart`, `scripts/build-app-icons.sh`, `lib/ui/launcher.dart`, `lib/ui/settings_screen.dart`, `lib/ui/inbox_screen.dart`, `lib/ui/split/*`, `lib/ui/comment_composer.dart`, `lib/ui/making_screen.dart`, `lib/platform/device/device_plugins.dart`, `lib/ui/webview_surface.dart` and `docs/usage.md` with it.

| iOS group | Its files the UI plan also edits | Must not run beside (UI) | Waits for (UI) |
|---|---|---|---|
| 1a | none | — | — |
| 2a | T9: `lib/main.dart` (two renamed arguments); T6: `scripts/BUILD.bazel` (UI T1, done) | UI 4a, 6a (T26, T41: `main.dart`) | — |
| 2b | none | — | — |
| 2c | `MODULE.bazel` | UI 2d (T10) | — |
| 3a | `apps/clax_app/BUILD.bazel`, `lib/main.dart` | UI 1a, 2b, 2c, 2d, 3d, 4a, 6a (T1, T9, T10, T12, T25, T26, T41) | — |
| 3b | `docs/usage.md` | UI 6i (T52) | — |
| 4a | T12: `lib/main.dart`, `lib/platform/device/device_plugins.dart`; T14: `app_screen.dart`, `host_protocol.dart`, `host.ts` | UI 4a, 6a (main.dart); every UI group with an `app_screen.dart`, `host_protocol.dart` or `host.ts` task (1c, 3a, 3c, 4a–4e, 5a–5d, 6b, 6f, 7b) | UI T34, T35 (T14 edits the host messages they rebuild) |
| 4b | T13: `app.dart`; T17: `webview_surface.dart`, `bundle_sheets.dart`, `device_plugins.dart` | UI groups with an `app.dart` task (1c, 2b, 3a, 3b, 5c, 6c, 6e, 6f, 7a, 7c); UI 1c and 5b (T5 `bundle_sheets.dart`, T36 `webview_surface.dart`) | — |
| 4c | T18: `webview_surface.dart` (`Init`), `lib/main.dart` | UI 5b (T36), 4a, 6a | UI T34, T35, T36 |
| 4d | `apps/clax_app/BUILD.bazel` via `bazel/flutter/macos.bzl` only | — | — |
| 5a, 5b | `design/tokens.json`, `rig.dart`, `lib/ui/kit/*` | UI 1a–1d, 5c (T39), 6g (T50), 6h (T51) | UI phases 1–5 (T1–T40), T50 |
| 5c | `theme.dart`, `app.dart`, `launcher.dart`, `app_screen.dart`, `host_protocol.dart`, `host.ts`, `inbox_screen.dart`, `settings_screen.dart`, `version_page.dart`, `scripts/build-app-icons.sh` | every UI group (run 5b when the UI plan is between phases or done) | UI T9, T26, T28–T33, T43 |
| 5d | `comment_composer.dart`, `making_screen.dart`, `launcher.dart` | UI 6f (T48), 6h (T51), 6g (T50), 7c (T56) | UI T30, T48, T51 |
| 6a | `lib/ui/split/*`, `threads_panel.dart`, `app.dart` | UI phase 7 (every task touches `split_shell.dart`) | UI phase 6 (T41–T52) |
| 6b | none (`ios/Runner` only) | — | UI T43 |
| 7a | T35: `app.dart`, `settings_screen.dart`; T32: `MODULE.bazel` | UI groups with an `app.dart` or `settings_screen.dart` task; UI 2d | — |
| 7b | none (`bazel/flutter/ios.bzl`, scripts) | — | — |
| 7c | T36: `app.dart` | UI groups with an `app.dart` task | — |

Phases 1–4 can run while the UI plan runs its phases 1–5, as long as each row above holds. Phase 5 and phase 6 follow the UI plan's phases they build on; the simplest safe order is to run iOS 5c and 6a when no UI task is in flight.

## Commands used throughout

```bash
Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"
SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
# A Flutter shots test (the UI plan's rig), writing PNGs into $SHOTS:
bazelisk test //apps/clax_app:ui_shots_<surface>_shots_test --nocache_test_results \
  --test_env=CLAX_SHOTS_DIR="$SHOTS" --sandbox_writable_path="$SHOTS"
# Every Dart task, before its commit:
bazelisk test //apps/clax_app:dart_analyze
# Every Rust task, before its commit (format and clippy over the crates it changed):
bazelisk build --config=fmt --config=clippy //crates/<crate>/...
# The simulator app (from Task 10 on), and a run of it (from Task 11 on):
bazelisk build //apps/clax_app:ios_app -c dbg --ios_multi_cpus=sim_arm64
just app-ios                       # "Clax iPhone"; APP_IOS_DEVICE=ipad for "Clax iPad"
xcrun simctl io "$(scripts/app-ios.sh --udid)" screenshot target/ios/shots/<state>.png
```

PNG names follow the UI plan (`<surface>-<form>-<theme>.png`); this plan adds the forms `iphone`, `ipad` and `ipad-compact` (Task 20).

---

## Phase 1: The spike on the simulator (§11)

### Task 1: Run the spike app on the iOS simulator

**Files:**
- Modify (not committed; the spike lives in the git-ignored `target/ios-spike/`): `target/ios-spike/app/main.swift`, `target/ios-spike/app/Info.plist`
- Modify (committed): `docs/superpowers/specs/2026-10-18-ios-release-design.md` (§11 gains "Run on the simulator, 2026-10-19" with the measured results; nothing else in the spec changes)

**Interfaces:**
- Consumes: the built spike (`target/ios-spike/app/build-sim/ClaxSpike.app`, `target/ios-spike/claxffi/target/aarch64-apple-ios-sim/release/libclaxffi.a`, built in §11; no Cargo is run now).
- Produces: one qualifier record per question below, on `docs/superpowers/specs/2026-10-18-ios-release-design.md`, which Tasks 10, 12, 18 and 19 rely on.

- [ ] **Step 1: Create the simulators** this plan uses, idempotently: `xcrun simctl list devices available | grep -q "Clax iPhone (" || xcrun simctl create "Clax iPhone" com.apple.CoreSimulator.SimDeviceType.iPhone-17 com.apple.CoreSimulator.SimRuntime.iOS-26-5`, and the same for "Clax iPad" with `iPad-Pro-13-inch-M5-12GB` (pick the newest iPad Pro 13-inch type `xcrun simctl list devicetypes` lists). Boot "Clax iPhone".
- [ ] **Step 2: Run the spike as built.** `xcrun simctl install <udid> target/ios-spike/app/build-sim/ClaxSpike.app`, `xcrun simctl launch --console-pty <udid> <bundle ID>` (`plutil -extract CFBundleIdentifier raw target/ios-spike/app/Info.plist`), read `result.txt` from `xcrun simctl get_app_container <udid> <bundle> data`/Documents. Record: started port and time, `/healthz` loaded under ATS with `NSAllowsLocalNetworking` (yes/no and the error if no), the time to first page. Screenshot (`simctl io … screenshot target/ios/shots/spike-healthz.png`) and read it: no local network prompt.
- [ ] **Step 3: Extend the harness** (`main.swift`), relinking with `swiftc` against the existing `libclaxffi.a` exactly as §11 item 3 did (`-target arm64-apple-ios17.0-simulator -framework Security -framework SystemConfiguration -lresolv -lc++`, the simulator SDK from `xcrun --sdk iphonesimulator --show-sdk-path`):
  - after `/healthz`, load `http://spike.localhost:<port>/healthz` and record whether WebKit resolved it;
  - load a page from the daemon whose body is an `<iframe src="http://spike.localhost:<port>/…">` (serve it by publishing nothing: use `data:` HTML that frames the `*.localhost` URL); in the frame, a button whose click posts a message to the parent; the parent's listener calls `input.click()` on an `<input type=file>` and records whether the document picker appeared (screenshot `spike-picker.png`). This answers whether a gesture in a cross-origin frame lets the top document open a picker (Task 18 needs it).
- [ ] **Step 4: Background and return.** With the app showing `/healthz`, `xcrun simctl launch <udid> com.apple.Preferences` (puts the spike in the background), wait for `xcrun simctl spawn <udid> launchctl list | grep claxspike` to show it suspended or up to 60 s, then relaunch the spike (`simctl launch`) and have it reload `/healthz` on `sceneWillEnterForeground` (add that). Record: did the listener answer after return; if not, the error.
- [ ] **Step 5: Record** each answer as a qualifier `comment` on `docs/superpowers/specs/2026-10-18-ios-release-design.md` §11's "Not yet proven" lines (`:1042-1050`), with the command and its output: ATS loopback, local network prompt, `*.localhost`, cross-origin activation for a picker, listener after return, time to first page. If `*.localhost` does **not** resolve, write the record as a `concern` tagged `status:needs-decision` ("the `clax-artifact:` scheme of §2.9 is not designed") and stop the plan after this task: Phase 3 would serve broken artifact origins. If the cross-origin activation does not open the picker, write a `concern` tagged `status:needs-decision` on §8.2's first bullet; Task 18 then waits for the owner and the rest of the plan goes on.
- [ ] **Step 6: Write §11's result paragraph** ("Run on the simulator, 2026-10-19: …", the six answers with numbers) and commit it with the `.qual` file: "Record the spike's simulator run: loopback under ATS, *.localhost, a picker from a framed gesture, and the listener after a background cycle".

---

## Phase 2: The daemon crate (§2, §3)

### Task 2: The exit statuses in clax-core

**Files:**
- Create: `crates/clax-core/src/exit.rs`
- Modify: `crates/clax-core/src/lib.rs` (`pub mod exit;`), `crates/clax-commands/src/cli.rs:424-457` (the constant and `exit_code` go; their test moves to `exit.rs`; `run`'s `std::process::exit(exit_code(&e))` becomes `std::process::exit(clax_core::exit::status_of(e.as_ref()))`)

**Interfaces:**
- Produces:
  ```rust
  //! The statuses a daemon run ends with, shared by `clax serve` and the
  //! daemon a host app runs in its own process (`clax-ios`).

  /// The status a run ends with when its database's schema could not be
  /// brought up to date ([`crate::CoreError::MigrationFailed`]): a failure
  /// that repeats on every start until the database or the binary changes,
  /// so a supervisor stops restarting on it (the Clax app's
  /// `DaemonSupervisor.migrationFailedExit`).
  pub const MIGRATION_FAILED: i32 = 65;

  /// The status for a run that failed with `e`: [`MIGRATION_FAILED`] when
  /// `e` or any error in its `source()` chain is a failed migration,
  /// otherwise 1.
  pub fn status_of(e: &(dyn std::error::Error + 'static)) -> i32 {
      let mut cur = Some(e);
      while let Some(c) = cur {
          if matches!(c.downcast_ref::<crate::CoreError>(), Some(crate::CoreError::MigrationFailed { .. })) {
              return MIGRATION_FAILED;
          }
          cur = c.source();
      }
      1
  }
  ```
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on `crates/clax-commands/src/cli.rs` and `crates/clax-core/src/lib.rs`.
- [ ] **Step 2: Move the test** `a_failed_migration_exits_with_its_own_status` (`cli.rs:447-457`) into `exit.rs`'s `mod tests`, rewritten for `status_of`: an `anyhow::Error` cannot be built in clax-core, so build the chain with a small `struct Ctx(Box<dyn Error>)` whose `source()` is the inner error, and assert `status_of(&Ctx(Box::new(CoreError::MigrationFailed { at: 27, detail: "duplicate column name: draft_of".into() })))` is 65, a plain `std::io::Error` is 1, and `CoreError::SchemaNewer { found: 40, known: 31 }` is 1. In `cli.rs`'s tests add one that `status_of(anyhow::Error::from(CoreError::MigrationFailed{..}).context("starting the daemon").as_ref())` is 65, pinning that anyhow's context layers are a `source()` chain.
- [ ] **Step 3: Run** `bazelisk test //crates/clax-core:clax_core_lib_test //crates/clax-commands:clax_commands_lib_test` — FAIL (no module). Implement; rerun — PASS — plus fmt and clippy over both crates.
- [ ] **Step 4: Commit**: "Give the exit statuses of a daemon run to clax-core, for every host of the daemon".

### Task 3: `ServeConfig.stop`, and a trim the host can ask for

**Files:**
- Modify: `crates/clax-server/src/serve.rs:186-217` (the field), `:254-545` (`serve`), `crates/clax-cli/src/serve.rs:24-41` (`stop: StopOn::Signals`), `crates/clax-server/tests/daemon.rs:120,163,260,304,333` (each config gains `stop: StopOn::Signals`), `crates/clax-store/src/exec.rs` (a job on every worker connection), `crates/clax-store/src/lib.rs` (`Store::trim_memory`, `TrimReport`)
- Test: `crates/clax-server/tests/daemon.rs` (three tests), `crates/clax-store/src/exec.rs` or `lib.rs` tests (one)

**Interfaces:**
- Produces (`clax_server::serve`):
  ```rust
  /// How the daemon is asked to stop. `Signals` is `clax serve`'s: SIGTERM,
  /// Ctrl-C and the shutdown route. `Embedded` is a daemon inside another
  /// app's process: only its control and the shutdown route, and no signal
  /// handler is installed, since the process's signals are the app's.
  pub enum StopOn {
      Signals,
      Embedded(EmbeddedControl),
  }

  /// What a host app holds to drive a daemon in its own process.
  pub struct EmbeddedControl {
      /// `true` asks the daemon to stop, as SIGTERM does for `clax serve`.
      pub stop: tokio::sync::watch::Receiver<bool>,
      /// Each sender asks for one [`clax_store::Store::trim_memory`]; its
      /// report is sent back on it.
      pub trim: tokio::sync::mpsc::Receiver<std::sync::mpsc::Sender<clax_store::TrimReport>>,
  }
  // ServeConfig gains: pub stop: StopOn,
  ```
  In `serve`: the `tokio::signal::unix::signal(SignalKind::terminate())?` at `:490` and the `tokio::signal::ctrl_c()` arm at `:505` exist only under `Signals`; under `Embedded` the `select!` waits on the shutdown route, `stale`, and `stop` turning true, and a task spawned beside the sweeper answers each `trim` request with `store.trim_memory()` run through `spawn_blocking` (a failure is logged and answered with `TrimReport::default()`). Everything after the `select!` (the drain, the journal, the store shutdown, `daemon.json`) is the same for both.
- Produces (`clax_store`):
  ```rust
  /// What [`Store::trim_memory`] did: the WAL's frames and how many the
  /// `PASSIVE` checkpoint copied into the database (equal when no reader held
  /// an older snapshot).
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct TrimReport { pub wal_frames: i64, pub checkpointed: i64 }
  impl Store {
      /// Releases what SQLite caches on every connection (`PRAGMA
      /// shrink_memory`) and runs one `PASSIVE` checkpoint, so a process
      /// killed afterwards loses nothing committed. Blocks the caller.
      pub fn trim_memory(&self) -> Result<TrimReport>;
  }
  ```
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Confirm decision 6** from the fetched source: `grep -n "fn handle_accept_error" -A25 "$(bazelisk info output_base)"/external/*crates*axum-0.8*/src/serve/listener.rs` (the path under crate_universe's repository). Record a qualifier `comment` on `crates/clax-http/src/auth.rs:453-457` quoting the lines: which errors are retried at once and which sleep 1 s, so a reclaimed listener on iOS costs one log line a second until the supervisor's resume probe restarts the daemon.
- [ ] **Step 3: Failing tests.** In `daemon.rs`:
  1. `an_embedded_daemon_stops_on_its_control_and_removes_daemon_json`: `StopOn::Embedded` with a `watch::channel(false)`; after `ready`, `/healthz` answers; `tx.send(true)`; `serve` returns `Ok(())` within 20 s; `daemon.json` is gone.
  2. `an_embedded_daemon_installs_no_signal_handler`: install the test's own handler first (`let mut term = tokio::signal::unix::signal(SignalKind::terminate()).unwrap();`), start an `Embedded` daemon, `nix::sys::signal::kill(nix::unistd::Pid::this(), nix::sys::signal::Signal::SIGTERM)` (nix is a normal dependency of clax-server with `signal`), assert `term.recv()` returns within 5 s **and** `/healthz` still answers afterwards; then stop it through the control. (Under `Signals` the same SIGTERM ends `serve`: keep that as case 3, `a_signals_daemon_stops_on_sigterm`, against the same test handler.)
  3. `an_embedded_daemon_trims_on_request`: publish nothing, send a `std::sync::mpsc::channel()` sender on `trim`, `recv_timeout(Duration::from_secs(5))` gives a report with `checkpointed == wal_frames`.
  In clax-store: `trim_memory_checkpoints_the_wal`: open a store on a temp home, write one artifact, `trim_memory()` answers `wal_frames > 0` and `checkpointed == wal_frames`; a write after it still lands.
  Run `bazelisk test //crates/clax-server:all --test_arg=daemon:: //crates/clax-store:clax_store_lib_test` — FAIL.
- [ ] **Step 4: Implement** (`exec.rs`'s `checkpoints` loop at `:844-858` shows the checkpoint call; a broadcast job runs `conn.execute_batch("PRAGMA shrink_memory")` on each worker's connection, then one `PRAGMA wal_checkpoint(PASSIVE)` read as `(busy, log, checkpointed)`). Rerun — PASS — plus fmt and clippy for clax-server, clax-store, clax-cli, and `bazelisk test //crates/clax-cli:all` (the CLI's own daemon tests).
- [ ] **Step 5: Commit**: "Let a host app stop and trim a daemon in its own process, with no signal handler of the daemon's".

### Task 4: The key slot and `PUT /api/sample/key`

**Files:**
- Modify: `crates/clax-sample/src/lib.rs:71-203` (`KeySlot`, `Sampler::from_home_with_slot`, `key_slot`, `available`/`reason` reading the slot), `crates/clax-sample/src/anthropic.rs:20-40,383-386` (`KeySource`), `crates/clax-server/src/routes/sample.rs` (`put_key`), `crates/clax-server/src/router.rs:90` (`.route("/api/sample", get(sample::daemon))` gains `.route("/api/sample/key", put(sample::put_key))` beside it), `crates/clax-server/src/routes/guards.rs:84` (`("sample::put_key", Token)`)
- Test: `crates/clax-sample/src/lib.rs` tests, `crates/clax-server/tests/api_sample.rs`

**Interfaces:**
- Produces (`clax_sample`):
  ```rust
  /// The owner's key, held in memory by a host app's daemon and set over
  /// loopback (`PUT /api/sample/key`). Never written to disk, never logged:
  /// `Debug` prints whether it is set, not the key.
  #[derive(Clone, Default)]
  pub struct KeySlot(std::sync::Arc<std::sync::RwLock<Option<String>>>);
  impl KeySlot {
      pub fn new() -> KeySlot;
      /// Sets the key; `None` or a blank key clears it.
      pub fn set(&self, key: Option<String>);
      pub fn get(&self) -> Option<String>;
  }
  impl std::fmt::Debug for KeySlot { /* KeySlot { set: true } */ }

  /// Where the `anthropic` provider reads its key on each call.
  pub enum KeySource { Fixed(String), Slot(KeySlot) }

  impl Sampler {
      /// The home's `[sample]` as [`Sampler::from_home`] reads it, with the
      /// `anthropic` provider's key read from `slot` on every call; sampling
      /// is off (`NoKey`) while the slot is empty. A `stub` provider ignores
      /// the slot.
      pub fn from_home_with_slot(home_root: &std::path::Path, slot: KeySlot) -> Sampler;
      /// The slot this sampler reads, when it was built with one.
      pub fn key_slot(&self) -> Option<&KeySlot>;
  }
  // AnthropicProvider::new(base_url, key) stays (KeySource::Fixed); AnthropicProvider::with_source(base_url, KeySource).
  ```
  `available()` is `provider.is_some()` and, for a slot sampler, a set slot; `reason()` answers `NoKey` for an empty slot. A call made while the slot is empty fails as `NoKey` does today.
- Produces (route): `PUT /api/sample/key` (bearer token, `DeniedToGrants`): body JSON `{"key": "<key>"}` or `{"key": null}`; `204` on success; `404 not_found` "this daemon takes its key from its environment" when `s.sample.key_slot()` is `None`; `400 bad_request` for any other body. The key never appears in a log line, an error message or a response.
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests.** clax-sample: a slot sampler is unavailable with reason `NoKey`, available after `set(Some("sk-test"))`, unavailable after `set(Some("  "))`; `format!("{:?}", slot)` contains no `sk-test`; an `AnthropicProvider::with_source` over a slot against the stub HTTP server the crate's tests use sends `x-api-key: sk-a`, then after `set(Some("sk-b"))` the next call sends `sk-b`. `api_sample.rs`: on a daemon whose sampler is `from_home_with_slot`, `PUT` without the token is 401, with it 204, and `GET /api/sample` then answers `available: true`; `{"key": null}` makes it `false` with `reason: "no_key"`; a body of `"sk"` is 400; on the default daemon (`Sampler::disabled()`) the route is 404; the daemon's captured log (the test subscriber the module uses) never contains the key. The route-table test in `guards.rs` (`GUARDS` must name every handler) fails until the entry is added.
- [ ] **Step 3: Run** `bazelisk test //crates/clax-sample:clax_sample_lib_test //crates/clax-server:all --test_arg=api_sample:: --test_arg=guards` — FAIL; implement; rerun — PASS — plus fmt and clippy.
- [ ] **Step 4: Commit**: "Let a host app's daemon take the owner's key over loopback, read on every call and kept only in memory".

### Task 5: What iOS cannot run, gated at compile time (§3)

**Files:**
- Modify: `crates/clax-core/src/gitctx.rs:383-399` (`find_git`: the unix arm becomes `#[cfg(all(unix, not(target_os = "ios")))]`, the `None` arm `#[cfg(any(not(unix), target_os = "ios"))]`) and `:674-680` (`capture` answers `GitField::Capture("unavailable")` first on iOS), `crates/clax-core/src/remote.rs:805-840` (`process_started`: an iOS arm returning `None`; the `ps` arm's cfg excludes iOS; the `cfg_attr` at `:842-845` follows), `crates/clax-mcp/src/tools.rs:2786-2797` (`open_in_browser` answers `false` on iOS without running anything), `crates/clax-mcp/src/lib.rs:16-21` (`probe`, `shim`, `standdown` are `#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]`; `channel` stays: it reads no process), `docs/usage.md` (a new "### Clax on iPhone and iPad" under "The Clax app", `:487`: what is unavailable, §3's list)
- Test: `crates/clax-core/src/gitctx_capture_tests.rs` (host behaviour unchanged), the iOS arms are compiled by Task 8's `:ios_build_test`

**Interfaces:**
- Produces: on `target_os = "ios"`, no path in clax-core, clax-mcp's tools, clax-server, clax-http, clax-realtime, clax-store or clax-sample starts a process except `CodexPush` (which `clax-ios` builds disabled, Task 7).
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Implement** the arms; keep each comment's contract ("off unix (a hosted build) there is no git to run" becomes "off unix, and on iOS, whose sandbox refuses `posix_spawn`, there is no git to run").
- [ ] **Step 3: Run** `bazelisk test //crates/clax-core:clax_core_lib_test //crates/clax-mcp:all` — PASS (nothing changes on the host) — plus fmt and clippy. `grep -rn 'Command::new' crates/clax-core/src crates/clax-server/src crates/clax-http/src crates/clax-realtime/src crates/clax-store/src crates/clax-sample/src crates/clax-mcp/src/tools.rs | grep -v test` and check each hit is gated or is `CodexPush`'s (`clax-realtime/src/push.rs:164`) or the store's test helper (`clax-store/src/lib.rs:884`, under `#[cfg(test)]`).
- [ ] **Step 4: Write the usage section**: what runs on iOS (the app's own agent over the Messages API, every artifact, version, thread and question route, `sample()`, rooms, MCP over HTTP, bundles) and what does not (§3's six bullets), with one line on why (iOS lets an app start no other program).
- [ ] **Step 5: Commit**: "Gate the daemon's process launches off on iOS, and say in the usage guide what Clax on iPhone and iPad leaves out".

### Task 6: The `unsafe` exception, and its check

**Files:**
- Modify: `AGENTS.md` (one line after "No `unsafe`: …"), `scripts/check-no-unsafe.sh` (rewrite), `scripts/BUILD.bazel:210-214` (`no_unsafe` gains the crate manifests and a test of the check), `scripts/bazel-derive.py:56-69,151-152` (a crate's own `[lints.rust]`), `bazel/cargo.bzl:118-129` (a crate's own lint flags)
- Create: `scripts/test-check-no-unsafe.sh`

**Interfaces:**
- Produces the `AGENTS.md` line (exactly):
  ```markdown
  - One exception, approved by the owner: `crates/clax-ios` denies `unsafe_code` instead of forbidding it, and allows it only on the `#[unsafe(no_mangle)]` of its four exported C functions (`clax_start`, `clax_stop`, `clax_state`, `clax_trim_memory`), whose arguments are scalars; `//scripts:no_unsafe` fails on any other `unsafe`, any other `allow(unsafe_code)`, and any other crate that does not take the workspace's lints.
  ```
- Produces `scripts/check-no-unsafe.sh [ROOT]` (default: the repository), failing with one line per finding:
  1. every `crates/*/Cargo.toml` but `crates/clax-ios/Cargo.toml` (and the separate `crates/clax-worker` workspace, which keeps its own `forbid`) has `[lints]` with `workspace = true`: `crates/<c>/Cargo.toml: does not take the workspace's lints`;
  2. `crates/clax-ios/Cargo.toml`, when present, sets `unsafe_code = "deny"` under `[lints.rust]`: else `crates/clax-ios/Cargo.toml: must deny unsafe_code`;
  3. outside `crates/clax-ios/src/lib.rs`, any `\bunsafe\b(?!-)` before a `//` comment fails as today: `<file>:<line>: <text>`;
  4. in `crates/clax-ios/src/lib.rs`, the only `unsafe` tokens are four lines that are exactly `#[unsafe(no_mangle)]`, each directly after a line `#[allow(unsafe_code)]` and directly before `pub extern "C" fn <name>(` with `<name>` one of the four, each name once, and every parameter of the four a scalar (`u16`, `i32` or none); `allow(unsafe_code)` appears nowhere else in the crate: else `crates/clax-ios/src/lib.rs:<line>: only the four exported functions may use unsafe`.
  The script is Bash plus `perl` (as today), `set -euo pipefail`, works from any directory.
- Produces `bazel-derive.py`: a crate whose `Cargo.toml` has its own `[lints.rust]` (not `workspace = true`) gets `"lint_flags": [<flags>]` in its `CRATES` entry, mapped by the same `lint_flags` logic; other crates get no key, so `bazel/workspace.bzl` does not change until Task 7 adds the crate. `bazel/cargo.bzl` uses `crate.get("lint_flags", WORKSPACE_RUSTC_FLAGS)` for the library, binaries, tests and build script.
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on `AGENTS.md`, `scripts/check-no-unsafe.sh`, `scripts/bazel-derive.py`, `bazel/cargo.bzl`.
- [ ] **Step 2: Write `test-check-no-unsafe.sh`** (shaped like `scripts/test-check-crate-layers.sh`): copy `scripts/check-no-unsafe.sh` and a minimal tree into `mktemp -d` (two crates' `Cargo.toml` and `src/lib.rs`, one `crates/clax-ios/` with the four functions as Task 7 writes them). Cases, each `_fail`ing with a message: the tree passes; an `unsafe { }` block in `clax-ios/src/lib.rs` fails naming its line; a fifth `#[unsafe(no_mangle)]` fails; `clax_start(p: *const u8)` fails; an `#[allow(unsafe_code)]` on a non-exported fn fails; `unsafe_code = "allow"` in `clax-ios/Cargo.toml` fails; another crate with `[lints.rust] unsafe_code = "deny"` fails as not taking the workspace's lints; `'unsafe-inline'` in a string still passes; `unsafe` in `crates/clax-core/src/x.rs` fails as today.
- [ ] **Step 3: Register** in `scripts/BUILD.bazel`: `no_unsafe` keeps running the check on `RUST_SOURCES` and gains `CRATE_MANIFESTS` (the list `crate_layers` uses at `:188`) in `data`; add `sh_test(name = "no_unsafe_test", srcs = ["test-check-no-unsafe.sh"], data = [":all"])`. Run `bazelisk test //scripts:no_unsafe //scripts:no_unsafe_test` — the test FAILS on the old script.
- [ ] **Step 4: Rewrite the script**, the derive and `cargo.bzl`; `just bazel-sync` must write no change (`git status --short bazel/` empty). Rerun — PASS — and `bazelisk test //scripts:crate_layers` — PASS.
- [ ] **Step 5: Edit `AGENTS.md`** (the line above, nothing else).
- [ ] **Step 6: Commit**: "Allow unsafe only on clax-ios's four exported C functions, and check that nothing else in any crate relaxes the lint".

### Task 7: `crates/clax-ios`

**Files:**
- Create: `crates/clax-ios/Cargo.toml`, `crates/clax-ios/BUILD.bazel` (`cargo_package()` only; Task 8 adds the iOS targets), `crates/clax-ios/src/lib.rs`, `crates/clax-ios/src/daemon.rs`, `crates/clax-ios/src/web.rs`, `crates/clax-ios/src/log.rs`, `crates/clax-ios/include/clax_ios.h`
- Modify: `Cargo.toml` (members gain `"crates/clax-ios"`), `Cargo.lock`, `scripts/check-crate-layers.py:38-55` (`"clax-ios": SERVER | {"clax-embedded"}`), `docs/superpowers/specs/2026-10-16-workspace-layout-design.md` §2.1 (the crate's row, beside `clax-cli`), `bazel/workspace.bzl` and `MODULE.bazel.lock` (written by `just bazel-sync`)

**Interfaces:**
- Produces `Cargo.toml`:
  ```toml
  [package]
  name = "clax-ios"
  version.workspace = true
  edition.workspace = true
  license.workspace = true

  # The workspace forbids unsafe_code; this crate denies it and allows it
  # only on the #[unsafe(no_mangle)] of its four exported C functions
  # (AGENTS.md; scripts/check-no-unsafe.sh checks it).
  [lints.rust]
  unsafe_code = "deny"
  unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)', 'cfg(clax_split)', 'cfg(clax_split, values(any()))'] }

  [dependencies]
  clax-core = { path = "../clax-core" }
  clax-server = { path = "../clax-server" }
  clax-sample = { path = "../clax-sample" }
  clax-realtime = { path = "../clax-realtime" }
  clax-http = { path = "../clax-http" }
  clax-embedded = { path = "../clax-embedded" }
  clax-store = { path = "../clax-store" }
  anyhow.workspace = true
  tokio = { workspace = true, features = ["rt-multi-thread", "sync", "time", "macros"] }
  tracing.workspace = true
  tracing-subscriber.workspace = true

  [dev-dependencies]
  tempfile.workspace = true
  reqwest.workspace = true
  ```
  (`clax-store` is for `TrimReport`; `ALLOWED`'s `SERVER` already includes it.)
- Produces `include/clax_ios.h`: the four declarations of §2.1 verbatim, with `#include <stdint.h>` and an include guard.
- Produces `src/lib.rs` (the only file with `unsafe`):
  ```rust
  //! The Clax daemon inside an iOS app's process: `clax serve`'s
  //! configuration ([`daemon::config`]) run on a thread of its own, behind
  //! four C functions with scalar arguments (`include/clax_ios.h`).

  mod daemon;
  mod log;
  mod web;

  pub use daemon::{Paths, State, start_at, state, stop, trim};

  /// Starts the daemon on its own thread, asking for `port` on 127.0.0.1
  /// (0: any free port), with the home under `$HOME`. Returns at once: 0
  /// started, 1 already running (starting, up or stopping), 2 no `$HOME`.
  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub extern "C" fn clax_start(port: u16) -> i32 {
      match std::env::var_os("HOME") {
          Some(h) => start_at(Paths::under(std::path::Path::new(&h)), port),
          None => 2,
      }
  }

  /// Asks a running daemon to stop and returns at once; [`clax_state`]
  /// says when it has.
  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub extern "C" fn clax_stop() { stop() }

  /// 0 never started, 1 starting, 2 up, 3 stopping, or 100 + the status
  /// `clax serve` would have exited with (100 a clean stop, 165 a failed
  /// migration, 101 another error, 201 a panic on the daemon's thread).
  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub extern "C" fn clax_state() -> i32 { state() }

  /// Shrinks SQLite's caches and checkpoints the WAL; returns when done, or
  /// after 2 s, or at once when no daemon is up. Safe from any thread.
  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub extern "C" fn clax_trim_memory() { let _ = trim(std::time::Duration::from_secs(2)); }
  ```
- Produces `src/daemon.rs`:
  ```rust
  /// The daemon's directories under an iOS app's container (`$HOME`).
  pub struct Paths { pub home: PathBuf, pub cache: PathBuf }
  impl Paths {
      /// `<home_dir>/Library/Application Support/clax-home` and
      /// `<home_dir>/Library/Caches/clax`.
      pub fn under(home_dir: &Path) -> Paths;
      pub fn log(&self) -> PathBuf; // <cache>/daemon.log
  }
  pub const IDLE: i32 = 0; pub const STARTING: i32 = 1; pub const UP: i32 = 2; pub const STOPPING: i32 = 3; pub const EXITED: i32 = 100;
  /// The state an ended run leaves: 100 + [`clax_core::exit::status_of`],
  /// 100 for `Ok`.
  pub fn final_state(r: &anyhow::Result<()>) -> i32;
  /// Starts a daemon at `paths` unless one is starting, up or stopping
  /// (then 1). Test entry point; [`crate::clax_start`] calls it.
  pub fn start_at(paths: Paths, port: u16) -> i32;
  pub fn stop();
  pub fn state() -> i32;
  /// Asks the running daemon for one trim and waits up to `wait`; `None`
  /// when none is up or it did not answer in time.
  pub fn trim(wait: Duration) -> Option<clax_store::TrimReport>;
  /// `clax serve`'s ServeConfig for `paths` (`crates/clax-cli/src/serve.rs:24-41`):
  /// 127.0.0.1, the 30 s stale check and 60 s reaper, Codex push disabled
  /// (`Default::default()`), the sampler [`Sampler::from_home_with_slot`],
  /// the embedded web UI ([`crate::web::web_ui`]), no hosted mode,
  /// `require_credential: true`, and `stop: StopOn::Embedded(control)`.
  pub fn config(paths: &Paths, port: u16, slot: KeySlot, control: EmbeddedControl) -> ServeConfig;
  ```
  One `static DAEMON: Mutex<Option<Running>>` (the stop `watch::Sender`, the trim `mpsc::Sender`, the thread's `JoinHandle`) and one `static STATE: AtomicI32`. `start_at` creates `paths.home` and `paths.cache`, initialises the log once (`log::init(paths.log())`, `try_init`), spawns the OS thread `clax-daemon`, and returns 0. The thread builds `tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("clax-rt").enable_all()`, runs `std::panic::catch_unwind(AssertUnwindSafe(|| rt.block_on(run(…))))` where `run` spawns a task that sets `UP` when `ready` fires and awaits `serve(config(…), Some(ready_tx))`; then `rt.shutdown_timeout(Duration::from_secs(2))`, clears `DAEMON`, and stores `final_state` (201 for a caught panic). The key slot lives as long as the process (`static SLOT: LazyLock<KeySlot>`), so a restarted daemon serves the key the supervisor puts again.
- Produces `src/log.rs`: `pub fn init(path: PathBuf)` — a `tracing_subscriber::fmt()` with `EnvFilter` default `warn,clax=info`, no ANSI, writing to a `MakeWriter` that appends to `path` and, when the file passes `CUT_AT` (1 MiB, a parameter of the writer for tests), renames it to `daemon.log.1` (replacing the old one) and starts a new file.
- Produces `src/web.rs`: `pub fn web_ui() -> Arc<dyn FileTree>`, the copy of `WebUi` (Decision 2; always the embed: `clax_embedded::web_ui_dir()` is `None` under Bazel's `--cfg clax_embedded`).
- Consumes: Tasks 2, 3, 4, 6.

- [ ] **Step 1: Read the threads** on `Cargo.toml`, `scripts/check-crate-layers.py` and the layout spec.
- [ ] **Step 2: Failing tests** in `src/daemon.rs`'s `mod tests` (one test binary shares the process-wide daemon, so the lifecycle tests are serialized through one `static LOCK: Mutex<()>`, each taking it first):
  1. `start_stop_start_in_one_process`: `start_at(Paths::under(tmp), 0)` is 0, `state()` reaches `UP` (poll every 20 ms for at most 30 s, the poll being the condition, not a sleep); `<home>/daemon.json` names `std::process::id()`; `GET /healthz` on its port is 200; a second `start_at` is 1; `stop()`; state reaches 100; `daemon.json` is gone; `start_at` again is 0 and reaches `UP` on a new port with a new token.
  2. `lifecycle_races` (Review Focus 5): `start_at` then `stop()` at once: the state reaches 100 within 30 s; `start_at` while `STOPPING` is 1; `stop()` with nothing running changes nothing; `trim` with nothing running is `None`.
  3. `the_key_slot_survives_a_restart`: up; `PUT /api/sample/key` `{"key":"sk-x"}` with the token from `daemon.json` is 204 and `GET /api/sample` says `available: true`; stop and start; `GET /api/sample` still says `available: true`.
  4. `trim_answers_with_a_checkpoint`: up; `trim(Duration::from_secs(5))` is `Some(r)` with `r.checkpointed == r.wal_frames`.
  5. `final_state_maps_statuses`: `Ok(())` → 100; a failed migration → 165; `anyhow!("x")` → 101.
  6. In `src/log.rs`: with `CUT_AT = 64`, writing 100 bytes in two lines leaves `daemon.log.1` with the first and `daemon.log` with the second.
  `src/lib.rs` has no tests (the C functions are three lines over the tested ones).
- [ ] **Step 3: Register the crate**: members, `check-crate-layers.py`, then `just bazel-sync` (writes `bazel/workspace.bzl` with `lint_flags` for clax-ios, `Cargo.lock`, `MODULE.bazel.lock`). Run `bazelisk test //crates/clax-ios:clax_ios_lib_test` — FAIL (no code).
- [ ] **Step 4: Implement**; rerun — PASS — plus `bazelisk test //scripts:no_unsafe //scripts:crate_layers` and fmt and clippy over `//crates/clax-ios/...`.
- [ ] **Step 5: Commit** the crate, the manifests, the lock and the derived files: "Run the Clax daemon inside a host app's process behind four scalar C functions".

### Task 8: Bazel for the iOS triples, the static library and its check

**Files:**
- Modify: `MODULE.bazel:13-48` (`rust.toolchain`'s `sha256s` gain `rust-std-1.94.0-aarch64-apple-ios.tar.xz` and `rust-std-1.94.0-aarch64-apple-ios-sim.tar.xz`; `extra_target_triples` gain both), `:59-71` (`supported_platform_triples` gain both), `:189-191` (`bazel_dep(name = "apple_support", version = "2.9.1")`), `rust-toolchain.toml` (`targets` gain both, with the comment "the Clax app's in-process daemon on iOS (crates/clax-ios)"), `crates/clax-ios/BUILD.bazel`, `bazel/cargo.bzl` (`cargo_package(staticlib = False)`), `MODULE.bazel.lock` (by `just bazel-sync`), `.bazelrc` (`build --ios_multi_cpus=sim_arm64` with a comment: rules_apple's simulator CPU on Apple silicon, a device build passes `arm64`; and `build --ios_minimum_os=17.0`, so every iOS configuration, the app's and `:clax_ios_builds`' alike, compiles ring's and SQLite's C for iOS 17.0, §4.2)
- Create: `bazel/apple/BUILD.bazel`, `bazel/apple/ios_rust.bzl`

**Interfaces:**
- Produces `cargo_package(staticlib = True)`: also declares `<lib>_static`, a `rust_static_library` over the library's `srcs`, `deps`, `rustc_flags`, `rustc_env` and `edition` (the same `common` dict), `visibility` as the library's.
- Produces `bazel/apple/ios_rust.bzl`:
  ```python
  # ios_opt_library(name, lib): `lib` (a rust_static_library) built -c opt
  # in the configuration it is depended on from (the app's split
  # transition sets the platform), forwarding its CcInfo and DefaultInfo.
  # ios_builds(name, lib, platforms): `lib` built for each of `platforms`
  # (labels), its files the rule's outputs: what a build_test checks.
  ```
  `ios_opt_library`'s transition sets `//command_line_option:compilation_mode` to `opt` only (decision 17); `ios_builds` uses a 1:N split transition on `//command_line_option:platforms`.
- Produces `crates/clax-ios/BUILD.bazel`:
  ```python
  load("@bazel_skylib//rules:build_test.bzl", "build_test")
  load("@rules_cc//cc:cc_library.bzl", "cc_library")
  load("@rules_swift//swift:swift_interop_hint.bzl", "swift_interop_hint")
  load("//bazel:cargo.bzl", "cargo_package")
  load("//bazel/apple:ios_rust.bzl", "ios_builds", "ios_opt_library")

  cargo_package(staticlib = True)

  # The static library in the app's configuration, always optimized.
  ios_opt_library(name = "clax_ios_opt", lib = ":clax_ios_static")

  swift_interop_hint(name = "clax_ios_module", module_name = "ClaxIOS")

  # The C header Swift imports as `ClaxIOS`, linked with the library and
  # the system libraries the daemon's dependencies need (the spike, iOS
  # spec §11: Security, SystemConfiguration, resolv, c++).
  cc_library(
      name = "clax_ios_c",
      hdrs = ["include/clax_ios.h"],
      aspect_hints = [":clax_ios_module"],
      includes = ["include"],
      linkopts = ["-framework", "Security", "-framework", "SystemConfiguration", "-lresolv", "-lc++"],
      target_compatible_with = ["@platforms//os:ios"],
      visibility = ["//apps/clax_app:__pkg__"],
      deps = [":clax_ios_opt"],
  )

  ios_builds(
      name = "clax_ios_builds",
      lib = ":clax_ios_static",
      platforms = ["@apple_support//platforms:ios_arm64", "@apple_support//platforms:ios_sim_arm64"],
      tags = ["manual"],
  )

  # The daemon keeps building for iPhone and its simulator (iOS spec §3).
  build_test(
      name = "ios_build_test",
      targets = [":clax_ios_builds"],
      target_compatible_with = ["@platforms//os:macos"],
  )
  ```
- Consumes: Task 7 (and Task 5, whose iOS arms this compiles).

- [ ] **Step 1: Read the threads** on `MODULE.bazel`, `bazel/cargo.bzl`, `.bazelrc`.
- [ ] **Step 2: Checksums:** `curl -sL https://static.rust-lang.org/dist/rust-std-1.94.0-aarch64-apple-ios.tar.xz.sha256` and the same for `-sim`; compare each with `curl -sL <tarball> | shasum -a 256`; use them only if both agree. A failing download stops the task: report it.
- [ ] **Step 3: Failing check:** add the BUILD targets first and run `bazelisk test //crates/clax-ios:ios_build_test` — FAIL (no toolchain for the triples).
- [ ] **Step 4: Implement** the module changes, `just bazel-sync` (crate_universe resolves for the new triples; `MODULE.bazel.lock` changes), `ios_rust.bzl` and `cargo_package(staticlib)`. Run `bazelisk test //crates/clax-ios:ios_build_test` in the background (the daemon compiles twice for iOS; logged to `target/plan-logs/ios-8.log`). Expected: PASS, with no warning in the log naming a deployment target (`grep -i "built for newer" target/plan-logs/ios-8.log` empty: apple_support's C toolchain compiles ring's and SQLite's C at `--ios_minimum_os`).
- [ ] **Step 5: Check** `scripts/bazel-sync.sh --check` passes, `bazelisk test //scripts:no_unsafe //crates/clax-ios:clax_ios_lib_test` PASS, and `otool -l "$(bazelisk cquery --output=files --platforms=@apple_support//platforms:ios_sim_arm64 //crates/clax-ios:clax_ios_static 2>/dev/null)" | grep -A3 LC_BUILD_VERSION | grep platform | sort -u` names only platform 7 (iOS simulator), and `otool -l` of the same archive shows `minos 17.0` on every member with an `LC_BUILD_VERSION`.
- [ ] **Step 6: Commit**: "Build the in-process daemon for iPhone and its simulator under Bazel, with a check that it keeps building".

---

## Phase 3: The bare app (§2.7, §4)

### Task 9: The supervisor drives an in-process daemon

**Files:**
- Create: `apps/clax_app/lib/daemon/in_process.dart`, `apps/clax_app/lib/daemon/sample_key.dart`, `apps/clax_app/test/daemon/in_process_test.dart`
- Modify: `apps/clax_app/lib/daemon/supervisor.dart:126-200` (`DaemonHost` replaces `bool android`; `keyPort`), `:267-289` (unchanged rule, the kill reaches `daemonStop`), `:395-470` (`_startOnce`: no key in the environment for `inProcess`; after `Up`, the key is put), `:553-558` (`_environment` by host), `apps/clax_app/lib/daemon/daemon_key.dart` (doc: `onChanged` is the supervisor's `keyChanged`), `apps/clax_app/lib/main.dart:100-112,166` (only the renamed arguments: `host: android ? DaemonHost.android : DaemonHost.mac`, `onChanged: supervisor.keyChanged`; Task 10 adds the iOS branch), `apps/clax_app/test/daemon/supervisor_test.dart` (every `android: true` becomes `host: DaemonHost.android`), `apps/clax_app/test/support/fake_supervisor.dart` if it constructs one

**Interfaces:**
- Produces:
  ```dart
  /// How the daemon runs: a child process found by an [ExeLocator]
  /// (Android's `libclax.so`, the Mac's helper), or inside this process
  /// (iOS, `crates/clax-ios`), where nothing is spawned or signalled.
  enum DaemonHost { android, mac, inProcess }

  /// Puts the owner's key into an in-process daemon's memory
  /// (`PUT /api/sample/key`); null clears it. True on 204.
  abstract interface class SampleKeyPort { Future<bool> put(Uri base, String token, String? key); }
  class HttpSampleKeyPort implements SampleKeyPort { HttpSampleKeyPort({HttpClient Function()? client}); }

  /// An [ExeLocator] for [DaemonHost.inProcess]: there is no executable.
  class InProcessLocator implements ExeLocator { const InProcessLocator(); /* clax() answers 'in-process' */ }

  /// A [ProcessStarter] over the daemon inside this app (`clax_start` and
  /// friends, served by `ios/Runner/NativeChannel.swift` on
  /// `dev.empathic.clax/native`): `start` sends `daemonStart {port}` with
  /// the port of `--port <P>` in [args]; the [RunningProcess]'s `pid` is
  /// this process's, its `exitCode` completes with `daemonState − 100` once
  /// a `daemonState` poll (every [pollEvery] on [clock]) answers 100 or
  /// more, `kill` sends `daemonStop` (any signal), and `stderr` carries the
  /// lines appended to [logPath] since the start (read on each poll);
  /// `stdout` is empty. `daemonStart` answering 1 (already running) is a
  /// start that exited with 1.
  class InProcessStarter implements ProcessStarter {
    InProcessStarter({required this.logPath, this.channel = const MethodChannel('dev.empathic.clax/native'), this.clock = const SystemClock(), this.pollEvery = const Duration(milliseconds: 250)});
  }

  // DaemonSupervisor({…, DaemonHost host = DaemonHost.mac, SampleKeyPort? keyPort, …}); `android:` goes.
  /// The saved key changed: an in-process daemon gets it over
  /// [SampleKeyPort] (no restart); a child daemon restarts to read it from
  /// its environment ([restart]).
  Future<void> keyChanged();
  ```
  With `inProcess`: `_environment()` is not built and no key is put in it; once a start is `Up`, `_putKey(endpoint)` reads the key (`_keyOrNull`) and calls `keyPort.put`; a `keyChanged` while `Starting` or `Restarting` is remembered and put at the next `Up` (Review Focus 3); `_stopStale` is skipped (the guard at `:511` already returns for the app's own PID); `PidKiller` is never called. `migrationFailedExit` (65) still makes `Failed` at once (state 165 → exit code 65).
- Consumes: the channel's method names (Task 10 serves them): `daemonStart {port} → int`, `daemonStop`, `daemonState → int`.

- [ ] **Step 1: Read the threads** on `supervisor.dart` (9fbb9e03: the fixed 15 s health limit; a fresh in-process home opens in about 50 ms, §11, so nothing changes here) and `daemon_key.dart`.
- [ ] **Step 2: Failing tests** in `in_process_test.dart`, over a mock `MethodChannel` handler (`TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler`) that plays a fake daemon (writes `daemon.json` with `io.pid` under a temp home and answers states as scripted) and a `FakeClock` (`test/daemon/supervisor_test.dart`'s):
  1. start: `ensureUp` sends `daemonStart {port: <picked>}`, answers `Up` once `daemon.json` names `io.pid` and the fake `HealthProbe` answers, and puts the saved key (`FakeSampleKeyPort` records `('http://127.0.0.1:<port>', token, 'sk-1')`); no environment carries the key;
  2. `keyChanged` while `Up` puts the new key and starts nothing; `keyChanged` while `Starting` puts it once at `Up` (Review Focus 3);
  3. resume with a dead listener (Review Focus 1): the probe answers false twice, the supervisor sends `daemonStop`, the fake state goes 3 then 100, the next `daemonStart` asks for the **same port** (kept in `<support>/daemon-port`), and the key is put again;
  4. a failed migration: `daemonState` answers 165 while starting → `Failed` at once, with `daemon.log`'s last lines (the fake writes two) in `stderrTail`;
  5. `stop()` sends `daemonStop` and completes when the state reaches 100, with no `PidKiller` call (a `PidKiller` fake that throws on any call).
  Run `bazelisk test //apps/clax_app:daemon_in_process_test` — FAIL.
- [ ] **Step 3: Implement**; update the callers and `supervisor_test.dart`; run `bazelisk test //apps/clax_app:daemon_in_process_test //apps/clax_app:daemon_supervisor_test //apps/clax_app:daemon_daemon_key_test //apps/clax_app:root_app_test //apps/clax_app:dart_analyze` — PASS.
- [ ] **Step 4: Commit**: "Supervise a daemon inside the app's own process, putting the owner's key over loopback instead of restarting".

### Task 10: The iOS runner and its Bazel app

**Files:**
- Create: `apps/clax_app/ios/Runner/AppDelegate.swift`, `SceneDelegate.swift`, `Info.plist`, `Base.lproj/LaunchScreen.storyboard`, `Base.lproj/Main.storyboard` (each from `@rules_flutter//flutter/private/runners/ios`, decision 9), `apps/clax_app/ios/Daemon/ClaxDaemon.swift` (outside `Runner/`, which the runner globs), `apps/clax_app/ios/Runner/NativeChannel.swift`, `bazel/flutter/ios.bzl`, `apps/clax_app/tool/ios-app-check.sh`
- Modify: `apps/clax_app/BUILD.bazel` (one `load("//bazel/flutter:ios.bzl", "clax_ios_app")` and one call after `macos_app_test`, `:394`: `clax_ios_app(name = "ios_app", version_name = VERSION_NAME, version_code = VERSION_CODE)`), `apps/clax_app/lib/main.dart:85-112` (the iOS branch), `docs/bazel.md` ("The Clax app": the table gains `ios_app`, `ios_app_test`; a paragraph "The iOS app")

**Interfaces:**
- Produces `bazel/flutter/ios.bzl`:
  ```python
  IOS_BUNDLE_ID = "dev.empathic.clax"  # pending the owner's confirmation (iOS spec open question 2)
  IOS_MINIMUM_OS = "17.0"

  def clax_ios_app(name, version_name, version_code):
      """The iOS app and its checks, in apps/clax_app:
        <name>             flutter_ios_app (iPhone and iPad, iOS 17.0) with the
                           in-process daemon (`:ios_daemon`), manual
        ios_daemon         swift_library ClaxDaemon importing ClaxIOS
        ios_version        apple_bundle_version from pubspec.yaml's version
        ios_app_sim        the simulator app built -c dbg (flutter_ios_simulator_build)
        ios_app_test       tool/ios-app-check.sh over it (macOS only)
      """
  ```
  `ios_daemon` is `swift_library(name = "ios_daemon", srcs = ["ios/Daemon/ClaxDaemon.swift"], module_name = "ClaxDaemon", deps = ["//crates/clax-ios:clax_ios_c"])`, and the runner's Swift `import ClaxDaemon`. `flutter_ios_app(name, application = ":app", bundle_id = IOS_BUNDLE_ID, app_name = "Clax", families = ["iphone", "ipad"], minimum_os_version = IOS_MINIMUM_OS, version = ":ios_version", runner_deps = [":ios_daemon"], tags = ["manual"])`. `ios_app_test` is an `sh_test` with `data = [":ios_app_sim", ":assets/home/home.clax", "//crates/clax-ios:clax_ios_builds"]`, `env` `VERSION_NAME`/`VERSION_CODE`/`IOS_MINIMUM_OS`, `tags = ["no-remote"]`, `target_compatible_with = ["@platforms//os:macos"]`.
- Produces `ClaxDaemon.swift` (module `ClaxDaemon`):
  ```swift
  import ClaxIOS
  import Foundation

  /// The daemon inside this app (crates/clax-ios), and the files it keeps
  /// under the app's container.
  public enum ClaxDaemon {
    /// `<container>/Library/Application Support/clax-home`, as the daemon
    /// computes it from $HOME.
    public static var home: URL
    /// `<container>/Library/Caches/clax/daemon.log`.
    public static var log: URL
    /// Creates the home with "complete until first user authentication"
    /// protection, and starts the daemon: 0 started, 1 running, 2 no $HOME.
    public static func start(port: UInt16) -> Int32
    public static func stop()
    public static func state() -> Int32
    /// Off the main thread; calls back on the main queue when done.
    public static func trimMemory(_ done: @escaping () -> Void)
    /// Marks `daemon.json`, `starting.json`, `net-cache/` and `logs/`
    /// under the home as excluded from backup (each run's files).
    public static func excludeRunFilesFromBackup()
  }
  ```
- Produces `NativeChannel.swift`: `enum NativeChannel { static func install(_ registrar: FlutterPluginRegistrar) }`, registered from `AppDelegate.didInitializeImplicitFlutterEngine` with `engineBridge.pluginRegistry.registrar(forPlugin: "ClaxNative")`, serving on `dev.empathic.clax/native`: `daemonStart {port}` → Int, `daemonStop`, `daemonState` → Int (on the first 2 after a start, `excludeRunFilesFromBackup()`), `daemonPaths` → `{home, log}`, `openExternal {url}` → Bool (http, https and mailto through `UIApplication.shared.open`), `openAppSettings` (`UIApplication.openSettingsURLString`), `snapshotWebView {id}` → `{png, width, height}` or nil (the Mac's at `macos/Runner/MainFlutterWindow.swift:26-56`, with `UIGraphicsImageRenderer` instead of `CGContext`/`NSBitmapImageRep`).
- Produces `Info.plist` (from the template): `CFBundleDisplayName` and `CFBundleName` "Clax", `UISceneDelegateClassName` `$(PRODUCT_MODULE_NAME).SceneDelegate`, `UIApplicationSupportsMultipleScenes` false, `NSAppTransportSecurity` › `NSAllowsLocalNetworking` true (and no `NSAllowsArbitraryLoads`), `UISupportedInterfaceOrientations` (portrait, both landscapes) and `UISupportedInterfaceOrientations~ipad` (all four), no `UIRequiresFullScreen`, no `UIBackgroundModes`, no `NSLocalNetworkUsageDescription`.
- Produces `main.dart`'s iOS branch: `final inProcess = Platform.isIOS;` the support and cache directories as today; with `inProcess` the home and the log come from `daemonPaths` (`MethodChannel('dev.empathic.clax/native').invokeMapMethod`), and the supervisor gets `exe: const InProcessLocator()`, `starter: InProcessStarter(logPath: log)`, `pids: IoPidKiller()` (never called), `host: DaemonHost.inProcess`, `keyPort: HttpSampleKeyPort()`. `background` and `notice` stay `NoBackgroundWork()`/null on iOS until Task 12.
- Produces `tool/ios-app-check.sh APP_ZIP HOME_BUNDLE STATIC_LIB…`: fails, naming the check, unless: `Clax.app/Info.plist` (`plutil -extract … raw`) has `CFBundleIdentifier` `dev.empathic.clax`, `CFBundleShortVersionString` `$VERSION_NAME`, `CFBundleVersion` `$VERSION_CODE`, `MinimumOSVersion` `$IOS_MINIMUM_OS`, `UIDeviceFamily` `[1, 2]`, `NSAppTransportSecurity.NSAllowsLocalNetworking` true, no `NSAllowsArbitraryLoads`, no `UIRequiresFullScreen`, no `UIBackgroundModes`; `Frameworks/App.framework` and `Frameworks/Flutter.framework` exist; `Frameworks/App.framework/flutter_assets/assets/home/home.clax` equals `HOME_BUNDLE` (`cmp`); the main binary's `LC_BUILD_VERSION` is platform 7 with `minos $IOS_MINIMUM_OS`; `nm -gU` of the binary lists `_clax_start`, `_clax_stop`, `_clax_state`, `_clax_trim_memory`; and every member of each `STATIC_LIB` (the device and simulator archives of `:clax_ios_builds`; `otool -l` per member) with an `LC_BUILD_VERSION` has `minos` at most `$IOS_MINIMUM_OS` (§4.2: a mismatch fails here rather than warning at link). This is also the iOS form of `version_sync`.
- Consumes: Tasks 8, 9.

- [ ] **Step 1: Read the threads** on `apps/clax_app/BUILD.bazel`, `lib/main.dart`, `docs/bazel.md`.
- [ ] **Step 2: Copy the runner templates** (`cp` from `"$(bazelisk info output_base)/external/rules_flutter+/flutter/private/runners/ios/"`; text files, not executables) and edit `Info.plist` as above. `GeneratedPluginRegistrant.swift` is not copied: the macro generates the registrant.
- [ ] **Step 3: Write the check first**, `ios-app-check.sh`, and the macro with `ios_app_test`; run `bazelisk test //apps/clax_app:ios_app_test` — FAIL (no Swift daemon, or the check's first missing key).
- [ ] **Step 4: Implement** `ClaxDaemon.swift`, `NativeChannel.swift`, the `AppDelegate` registration and `main.dart`'s branch. Run `bazelisk test //apps/clax_app:ios_app_test` in the background (logged). Expected: PASS. If rules_flutter's iOS rules need a fix, it goes into `bazel/flutter/rules_flutter.patch` as a small hunk (`docs/bazel.md`, "rules_flutter's patches"), named in the commit message. If `swift_library` cannot import the `cc_library` through `swift_interop_hint`, use rules_swift's `@rules_swift//swift:module_name` hint form the fetched rules_swift documents and say which in the commit.
- [ ] **Step 5: Run it by hand** (Task 11 automates this): `xcrun simctl install <Clax iPhone> <unzipped Clax.app>`, `xcrun simctl launch <udid> dev.empathic.clax`; `daemon.json` appears under `$(xcrun simctl get_app_container <udid> dev.empathic.clax data)/Library/Application Support/clax-home/` naming the app's PID (`xcrun simctl spawn <udid> launchctl list | grep dev.empathic.clax`); screenshot the home screen into `target/ios/shots/bare-home.png` and read it: today's app, in Material, with its home.
- [ ] **Step 6: Run** `bazelisk test //apps/clax_app:dart_analyze //apps/clax_app:root_app_test //apps/clax_app:version_sync //apps/clax_app:macos_app_test` — PASS (the Mac and the APK are unchanged).
- [ ] **Step 7: Commit**: "Build the Clax app for iPhone and iPad with Bazel, its daemon running in the app's own process".

### Task 11: `just app-ios`

**Files:**
- Create: `scripts/app-ios.sh`, `scripts/test-app-ios.sh`
- Modify: `justfile` (after `app-phone-native`, `:128`: `# Build the Clax app for the iOS simulator and run it on "Clax iPhone" (APP_IOS_DEVICE=ipad for "Clax iPad"), keeping its data` / `app-ios:` / `    ./scripts/app-ios.sh`), `scripts/BUILD.bazel` (`sh_test(name = "app_ios", srcs = ["test-app-ios.sh"], data = ["app-ios.sh"])`), `docs/usage.md` ("Clax on iPhone and iPad": `just app-ios`)

**Interfaces:**
- Produces `scripts/app-ios.sh [--udid | --dry-run]`: with no argument, in order, printing each step as `app-phone.sh` does: (1) checks `xcrun simctl list runtimes` names an iOS runtime, else stops: "No iOS simulator runtime: run `xcodebuild -downloadPlatform iOS` (about 8.5 GB)"; (2) `bazelisk build //apps/clax_app:ios_app -c dbg --ios_multi_cpus=sim_arm64`; (3) finds or creates the simulator ("Clax iPhone" from the newest iOS runtime and the newest `iPhone-17` device type; with `APP_IOS_DEVICE=ipad`, "Clax iPad" from the newest iPad Pro 13-inch type), and boots it (`simctl bootstatus -b`); (4) unzips the app into `target/app-ios/<sha>/` and `simctl install`s it (data kept); (5) `simctl launch` and waits, polling every 0.5 s for at most 60 s, for `daemon.json` in the data container, then for `/healthz` on its port answering with the launched PID; prints the port and "Clax is up on <device>". `--udid` prints the chosen device's UDID and does nothing else. `--dry-run` prints the commands of each step and runs none. It never touches another simulator.
- Produces `test-app-ios.sh`: runs `app-ios.sh --dry-run` with a `PATH` whose `xcrun` and `bazelisk` are recording stubs in a `mktemp -d` and checks the commands in order (the build line, `simctl create` when the stub lists no "Clax iPhone", `simctl install`, `simctl launch dev.empathic.clax`); and that with the stub listing no runtime it exits 1 printing the download line.
- Consumes: Task 10.

- [ ] **Step 1: Read the threads** on `justfile`, `scripts/app-phone.sh`, `docs/usage.md`.
- [ ] **Step 2: Write the test**, register it; `bazelisk test //scripts:app_ios` — FAIL.
- [ ] **Step 3: Write the script**; rerun — PASS — and `scripts/test-justfile.sh` passes.
- [ ] **Step 4: Run** `just app-ios` (background, logged): "Clax is up on Clax iPhone"; then `APP_IOS_DEVICE=ipad just app-ios`. Screenshot both into `target/ios/shots/app-ios-{iphone,ipad}.png` and read them.
- [ ] **Step 5: Commit**: "Add just app-ios: build the app for the simulator and run it with its daemon up".

---

## Phase 4: Platform glue (§2.3, §2.4, §5, §6, §8.2, §9)

### Task 12: Background time, memory and files in Swift; one notification hub

**Files:**
- Create: `apps/clax_app/ios/Runner/Background.swift`, `apps/clax_app/lib/platform/local_notifications.dart`, `apps/clax_app/test/platform/local_notifications_test.dart`
- Modify: `apps/clax_app/ios/Runner/AppDelegate.swift` (install `Background`; `applicationDidReceiveMemoryWarning` → `ClaxDaemon.trimMemory`), `apps/clax_app/ios/Runner/SceneDelegate.swift` (`sceneDidEnterBackground`, `sceneWillEnterForeground` → `Background`), `apps/clax_app/lib/platform/background_work.dart:11-155` (`paused`, `pauseDone`, `pausesRuns`; `IosBackgroundWork`), `apps/clax_app/lib/platform/device/device_plugins.dart:27-110` (`PluginNotifier` over the hub, with `iOS:` settings), `apps/clax_app/lib/main.dart:155-170` (`background: IosBackgroundWork(hub)` and the hub on iOS), `apps/clax_app/lib/platform/secure_key.dart` (`IOSOptions`), `apps/clax_app/test/platform/background_work_test.dart`, `apps/clax_app/test/platform/secure_key_test.dart`

**Interfaces:**
- Produces (`background_work.dart`; `NoBackgroundWork` and `ChannelBackgroundWork` gain no-op `paused`/`pauseDone` and `pausesRuns => false`):
  ```dart
  // BackgroundWork gains:
  /// Whether [stopped] means "pause the runs now" (iOS: the background time
  /// is nearly spent) rather than "the service stopped" (Android).
  bool get pausesRuns;
  /// Posts the paused notice for [title], replacing any earlier one;
  /// tapping it opens [artifactId]'s build view (or the app when null).
  Future<void> paused(String title, String? artifactId);
  /// Tells the platform the runs are paused, so it may end its background
  /// time.
  Future<void> pauseDone();

  /// iOS: `start`/`update`/`stop` and `stopped`/`pauseDone` over the
  /// `dev.empathic.clax/background` channel that `ios/Runner/Background.swift`
  /// serves (a `beginBackgroundTask` held while runs go on); the news —
  /// `ready`, `failed`, `paused`, permission, taps — through [hub].
  class IosBackgroundWork implements BackgroundWork { IosBackgroundWork(NotificationHub hub, {MethodChannel channel = const MethodChannel('dev.empathic.clax/background')}); }
  ```
  Notice wording: `paused` posts title = the app's name, body **"Paused — open Clax to keep building."**, payload `run:<artifactId>`; `ready`/`failed` as Android's (`BackgroundRunService.kt`), payload `run:<artifactId>`.
- Produces (`local_notifications.dart`):
  ```dart
  /// The app's one `flutter_local_notifications` plugin (iOS allows one
  /// notification delegate): initialised once, with taps dispatched by
  /// their payload's kind (`run:<aid>`, `reminder:<…>`).
  class NotificationHub {
    static Future<NotificationHub> init({FlutterLocalNotificationsPlugin? plugin});
    /// Taps on notifications whose payload is `<kind>:<data>`.
    Stream<String> taps(String kind);
    /// The `<data>` of the notification of [kind] that launched the app, once.
    Future<String?> launch(String kind);
    Future<bool> requestPermission();
    Future<void> show(int id, String title, String body, {required String kind, required String data});
    Future<void> cancel(int id);
  }
  ```
  `PluginNotifier` uses the hub with kind `reminder` (its existing payload as `<data>`), and keeps working on Android and macOS as before.
- Produces `Background.swift` (`final class Background`, installed with the registrar): on `start {title, text}` it records the newest title and answers true; `update` replaces it; `stop` clears it and ends a held task. On `sceneDidEnterBackground` it always begins a task named "Clax trim", calls `ClaxDaemon.trimMemory` and ends that task when it returns (§2.4); with runs recorded it also begins "Clax run" (`beginBackgroundTask(withName:expirationHandler:)`) and checks `UIApplication.shared.backgroundTimeRemaining` once a second; at `margin` (5 s, a stored property tests set) or at the expiration handler, whichever first, it **pauses once**: invokes `stopped` on the channel, waits up to `ackWait` (2 s) for `pauseDone`, calls `ClaxDaemon.trimMemory`, then ends the task. On `sceneWillEnterForeground` it cancels the timer and ends any held task. `#if DEBUG`, `expireBackgroundTask` on the same channel runs the pause at once (Task 19's test hook).
- Produces: `ClaxDaemon.home` is created with `FileProtectionType.completeUntilFirstUserAuthentication` (Task 10) and the Keychain key is written with `first_unlock_this_device` (`lib/platform/secure_key.dart`'s `FlutterSecureKey`: `IOSOptions(accessibility: KeychainAccessibility.first_unlock_this_device)`; add it here, with a test that the options say so).
- Consumes: Task 10.

- [ ] **Step 1: Read the threads** on `background_work.dart`, `device_plugins.dart` (c3f4359f: the plugin ports unverified on a device; this task adds iOS and leaves the record open), `secure_key.dart`, `main.dart`.
- [ ] **Step 2: Failing tests.** `local_notifications_test.dart` over a fake `FlutterLocalNotificationsPlugin` subclass (the plugin's platform interface set to a fake): one `initialize`; a tap with payload `run:a1` reaches `taps('run')` and not `taps('reminder')`; `launch('run')` answers `a1` once and null after; `show` passes title, body and `run:a1`. `background_work_test.dart`: `IosBackgroundWork.pausesRuns` is true and the others false; `paused('Groceries', 'a1')` shows "Paused — open Clax to keep building." titled "Groceries" with payload `run:a1`, and a second `paused` uses the same notification ID (replacing it); a `stopped` call from the mock channel reaches `stopped`; `pauseDone` sends `pauseDone`. `secure_key_test.dart`: the iOS options' accessibility is `first_unlock_this_device`.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:platform_local_notifications_test //apps/clax_app:platform_background_work_test //apps/clax_app:platform_secure_key_test` — FAIL; implement Dart and Swift; rerun — PASS — plus `//apps/clax_app:ui_app_screen_device_test`, `dart_analyze`, and `bazelisk test //apps/clax_app:ios_app_test`.
- [ ] **Step 4: Look** on the simulator (`just app-ios`): send the app to the background (`xcrun simctl launch <udid> com.apple.Preferences`) and confirm with `xcrun simctl spawn <udid> log show --last 1m --predicate 'subsystem == "dev.empathic.clax"'` (Background.swift logs through `os.Logger(subsystem: "dev.empathic.clax", category: "background")`) that "Clax trim" began and ended. The pause itself is checked in Task 19.
- [ ] **Step 5: Commit**: "Hold background time on iOS while runs go on, trim the daemon on leaving and under memory pressure, and post every notification through one hub".

### Task 13: Pause and resume the runs (§6.1)

**Files:**
- Modify: `apps/clax_app/lib/platform/network_pause.dart:15-46` (`AppPresence.pause`), `:74-140` (`ResumingBackend`), `:190-330` (`ResumingTurns`), `apps/clax_app/lib/app.dart:593-600` and `:760-770` (on `_s.background.stopped` with `pausesRuns`: `_presence.pause()`, post `paused` for the newest run, then `pauseDone`), `:1231-1245` (the paused notice is cancelled on `resumed`)
- Create: `apps/clax_app/test/platform/background_pause_test.dart`
- Test: `apps/clax_app/test/platform/network_pause_test.dart`, `apps/clax_app/test/app_background_test.dart`

**Interfaces:**
- Produces (`AppPresence`):
  ```dart
  /// Pauses the runs while the app is out of the foreground (iOS, when
  /// its background time is nearly spent): every turn or request in flight
  /// is aborted as a background cut is and held until [whenForeground],
  /// then sent again. Does nothing in the foreground.
  void pause();
  /// Whether [pause] was called since the app last left the foreground.
  bool get paused;
  /// Completes at the next [pause]; at once while [paused].
  Future<void> get whenPaused;
  ```
  `ResumingTurns.turn` adds `presence.whenPaused` to the attempt's `cancelled` (beside `watch.stalled`); an attempt that ends by the pause is held (`_hold`) and sent again **without counting against `maxResumes`**, and its events are told to `holds.onCutAttempt` as a cut attempt's are. `ResumingBackend.run` races `inner.run` against `presence.whenPaused` through a request whose `cancelled` includes it (an `AgentRequest` copy with `cancelled: Future.any([?request.cancelled, presence.whenPaused])`), and on the pause holds and runs it again, also uncounted. A pause is not a cancel: `CancelWatch` still reads only the request's own `cancelled`.
- Produces (app): with `pausesRuns`, a `stopped` event calls `_presence.pause()`, then `_s.background.paused(<the newest run's app name>, <its artifact ID>)` and `pauseDone()`; Android's handling of `stopped` (`BackgroundRuns._down`) is unchanged. On `resumed`, the paused notice is cancelled through the hub.
- Consumes: Task 12.

- [ ] **Step 1: Read the threads** on `network_pause.dart` (8fd2656e decided: holds have no time bound; 9ca0b154: a cut attempt is not priced, which a pause inherits; 2422b328: `onPause` has no matching `onResume` on a cancel) and on `app.dart`'s background region.
- [ ] **Step 2: Failing tests** in `background_pause_test.dart`, over `ResumingTurns` with a scripted `TurnBackend` whose turns stream events until cancelled, an injected `Clock` and an `AppPresence`:
  1. a turn in flight when the app leaves and `pause()` is called is aborted at once (the fake sees its `cancelled` fire), held while the app is away for 5 simulated hours, and sent again unchanged on `onLifecycle(resumed)`; the runner adds it to the history once and runs no tool twice;
  2. five pauses in one turn send it five times and the turn succeeds (Review Focus 2), while a sixth **network** cut after three network resumes still throws;
  3. `pause()` in the foreground does nothing;
  4. a cancel during a pause hold throws `AgentError.cancelled` and calls no `onResume`;
  5. `ResumingBackend`: a single call in flight is aborted by the pause and run again on return.
  `app_background_test.dart` (the app over `FakeBackgroundWork` with `pausesRuns: true`): a change running when `stopped` arrives posts one `paused('Groceries', 'a1')` then `pauseDone`, and on `resumed` the paused notice is cancelled and the change lands; with `pausesRuns: false` the same `stopped` posts nothing.
  Run `bazelisk test //apps/clax_app:platform_background_pause_test //apps/clax_app:platform_network_pause_test //apps/clax_app:root_app_background_test` — FAIL.
- [ ] **Step 3: Implement**; rerun — PASS — plus `//apps/clax_app:agent_worker_offline_test //apps/clax_app:agent_worker_restart_test //apps/clax_app:root_app_test //apps/clax_app:dart_analyze`.
- [ ] **Step 4: Record** an `alternative` on `docs/superpowers/specs/2026-10-18-ios-release-design.md:617-622` (§6.2's first bullet): "`BGContinuedProcessingTask` (iOS 26) would let a long build finish in the background with a system progress UI; not used: the owner chose pause and resume (§ Owner's decisions 3, answered again for open question 4). revisit: after-testflight-feedback". And a `waiver` on `network_pause.dart`'s pause hold: "A pause aborts a turn attempt that may already have been billed (as 9ca0b154 accepts for a network cut); a pause near the end of a long answer costs that answer again."
- [ ] **Step 5: Commit**: "Pause every run when iOS's background time is nearly spent, and resume each from where it stopped when Clax is back".

### Task 14: The shell reconnects on `resumed`

**Files:**
- Modify: `apps/clax_app/lib/ui/host_protocol.dart` (a `Resumed` host call: `window.claxHost?.resumed()`), `apps/clax_app/lib/ui/app_screen.dart` (on `AppLifecycleState.resumed`, the open app's surface runs `Resumed`), `web/shell/src/app/host.ts` (`resumed()`: the app-mode stream's `EventSource` is closed and opened again at once, and the bridge's `pageWait`/notice state is left as it is), `docs/contract.md` and `web/contract/clax-extensions.d.ts` only if app mode's host calls are listed there (`grep -n "claxHost" docs/contract.md`)
- Test: `apps/clax_app/test/ui/host_protocol_test.dart`, `apps/clax_app/test/ui/app_screen_test.dart`, `web/shell/src/app/host.test.ts`

**Interfaces:**
- Produces `final class Resumed extends HostCall { const Resumed(); }` whose `toJs()` is `window.claxHost?.resumed()`; the shell's `claxHost.resumed()` (no argument, idempotent).
- Consumes: Task 10; UI T34, T35 (the host call set and `init` they rebuild).

- [ ] **Step 1: Read the threads** on the three files.
- [ ] **Step 2: Failing tests:** `Resumed().toJs()` is `window.claxHost?.resumed()`; `app_screen_test.dart`: a `resumed` lifecycle event sends `Resumed` to the `FakeSurface` once, an `inactive` sends nothing; `host.test.ts` (fake timers, a fake `EventSource` class): `resumed()` closes the stream and opens a new one within the same tick, and a second `resumed()` while it opens makes no third.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_host_protocol_test //apps/clax_app:ui_app_screen_test //web:unit --test_arg=shell/src/app/host.test.ts` — FAIL; implement; rerun — PASS — plus `//web:typecheck //web:lint //apps/clax_app:dart_analyze`.
- [ ] **Step 4: Commit**: "Reconnect an open app's live stream at once when Clax comes back to the foreground".

### Task 15: `.clax` bundles on iOS

**Files:**
- Create: `apps/clax_app/ios/Runner/BundleIntake.swift`
- Modify: `apps/clax_app/ios/Runner/Info.plist` (`CFBundleDocumentTypes` naming `dev.empathic.clax.bundle`, `UTExportedTypeDeclarations` for it: conforms to `public.data`, extension `clax`, MIME `application/vnd.empathic.clax`; `LSSupportsOpeningDocumentsInPlace` false), `apps/clax_app/ios/Runner/SceneDelegate.swift` (`scene(_:openURLContexts:)` and the `connectionOptions.urlContexts` of a cold launch → `BundleIntake.opened`), `apps/clax_app/tool/ios-app-check.sh` (the two plist keys)

**Interfaces:**
- Produces `BundleIntake.swift`: the `dev.empathic.clax/bundles` channel exactly as the Mac's `AppDelegate.swift:33-62` serves it (`initial` → `[{path}]`, then `opened {path}`), for file URLs whose extension is `clax`, each copied first into `<tmp>/bundles/<UUID>.clax` (the file iOS hands over may vanish once the call returns); `ChannelBundleIntake` (`lib/platform/bundle_intake.dart`) is unchanged.
- Consumes: Task 10.

- [ ] **Step 1: Read the threads** on `lib/platform/bundle_intake.dart` and `macos/Runner/AppDelegate.swift`.
- [ ] **Step 2: Failing check:** the two plist assertions in `ios-app-check.sh`; `bazelisk test //apps/clax_app:ios_app_test` — FAIL.
- [ ] **Step 3: Implement**; rerun — PASS.
- [ ] **Step 4: Look:** export a `.clax` from the Mac app or take `apps/clax_app/test/fixtures/` 's bundle fixture if one exists (`ls apps/clax_app/test/fixtures`), `xcrun simctl openurl <udid> file://<path>` with Clax running and with it quit; both open the install sheet; screenshot `target/ios/shots/bundle-open.png` and read it.
- [ ] **Step 5: Commit**: "Open .clax bundles in Clax on iOS from Files, Mail and the share sheet".

### Task 16: The plugins' iOS halves and their usage strings

**Files:**
- Modify: `bazel/flutter/plugin_overlays/speech_to_text/` (its BUILD gains the iOS library's dependency on `@cwl_catch_exception`, as the macOS half has), `apps/clax_app/ios/Runner/Info.plist` (§10.3's usage strings verbatim: `NSCameraUsageDescription`, `NSPhotoLibraryUsageDescription`, `NSMicrophoneUsageDescription`, `NSSpeechRecognitionUsageDescription`, `NSLocationWhenInUseUsageDescription`, `NSLocationTemporaryUsageDescriptionDictionary` › `precise`), `apps/clax_app/tool/ios-app-check.sh` (each key and its exact string), any further overlay a plugin's iOS build fails on (`docs/bazel.md`, "Maintaining it"), `bazel/flutter/rules_flutter.patch` only if a plugin rule needs a fix

**Interfaces:**
- Produces: the iOS app builds with every plugin of `pubspec.yaml` (`file_picker`, `flutter_local_notifications`, `flutter_secure_storage`, `flutter_timezone`, `geolocator`, `image_picker`, `path_provider`, `share_plus`, `speech_to_text`, `webview_flutter`), and the bundle carries each plugin's `PrivacyInfo.xcprivacy` (rules_flutter's `flutter_ios_privacy_manifests`).
- Consumes: Task 10.

- [ ] **Step 1: Read the threads** on `bazel/flutter/plugin_overlays/`.
- [ ] **Step 2: Failing check:** the usage-string assertions and one that `Frameworks/` or the app bundle holds at least one plugin privacy manifest (`find Clax.app -name PrivacyInfo.xcprivacy | grep -c .` ≥ 1); `bazelisk test //apps/clax_app:ios_app_test` — FAIL.
- [ ] **Step 3: Implement**; rerun in the background — PASS. A plugin whose iOS half needs something new gets an overlay as `speech_to_text`'s; name each in the commit message.
- [ ] **Step 4: Look:** in the simulator, trigger dictation (the comment composer's microphone) and a location request from the kit example's location call; screenshot each system prompt (`target/ios/shots/prompt-{mic,speech,location}.png`) and read them: each shows its usage string.
- [ ] **Step 5: Commit**: "Build every plugin's iOS half, with Clax's own words in each permission prompt".

### Task 17: Platform behaviour in Dart: share anchors, haptics, reminders, accuracy, the WebView

**Files:**
- Modify: `apps/clax_app/lib/ui/bundle_sheets.dart:975-990` (`SharePlus.instance.share(ShareParams(…, sharePositionOrigin: <the button's global rect>))`; the callers pass the rect from the tapped button's `RenderBox`), `apps/clax_app/lib/platform/device/device_plugins.dart:120-140` (`PluginShare`'s two shares take an anchor: the frame's rect from the device request, else the screen's centre point), `:230-262` (`PluginLocation`: a precise request on reduced accuracy calls `Geolocator.requestTemporaryFullAccuracy(purposeKey: 'precise')` first, on iOS), `apps/clax_app/lib/platform/device/reminders.dart` (iOS's cap), `apps/clax_app/lib/platform/voice_cues.dart:45-46` (`platformHaptics ?? (Platform.isAndroid || Platform.isIOS)`), `apps/clax_app/lib/ui/webview_surface.dart` (on iOS: `WebKitWebViewController.setInspectable(kDebugMode || kProfileMode)`; back-forward gestures stay off, and a test pins it)
- Test: `apps/clax_app/test/ui/bundle_flows_test.dart`, `apps/clax_app/test/platform/device/reminders_test.dart` (create if absent), `apps/clax_app/test/platform/voice_cues_test.dart`, `apps/clax_app/test/ui/webview_surface_test.dart` (if present; else the assertion goes in `app_screen_test.dart`)

**Interfaces:**
- Produces (`reminders.dart`): `Reminders(port, {file, now, int? pendingCap})`, `pendingCap` defaulting to `60` on iOS (`Platform.isIOS`) and null elsewhere; with a cap, the port has at most `pendingCap` notifications pending, the nearest next fire times first, and `Future<void> topUp()` (called at each launch from `DeviceHost`'s construction and after every `schedule`, `cancel` and fired reminder) cancels pending ones past the cap and schedules those now within it. `NotifierPort` gains `Future<Set<int>> pendingIds()`.
- Consumes: Task 12 (`PluginNotifier` over the hub).

- [ ] **Step 1: Read the threads** on each file (c3f4359f stays open: the device run is Group B's).
- [ ] **Step 2: Failing tests:** an export share passes a non-null `sharePositionOrigin` equal to the button's rect; with `pendingCap: 60` and an injected `now`, 70 reminders across three apps leave the fake port with exactly the 60 nearest pending, and after the nearest fires and `topUp` runs, the 61st is pending (Review Focus 4); without a cap nothing changes; the default of `platformHaptics` is true for an iOS host (the constructor reads `Platform.isAndroid || Platform.isIOS` through a `bool Function() isTouchHost` parameter that tests set, since `debugDefaultTargetPlatformOverride` does not change `Platform`); the surface sets inspectable in debug.
- [ ] **Step 3: Run** the tests — FAIL; implement; rerun — PASS — plus `//apps/clax_app:platform_device_device_host_test //apps/clax_app:ui_app_screen_device_test //apps/clax_app:dart_analyze`.
- [ ] **Step 4: Commit**: "Anchor shares for iPad, keep reminders within iOS's pending limit, ask for precise location once, and give iOS haptics and an inspectable WebView in debug".

### Task 18: WebKit's own camera, location, files and share on iOS (§8.2)

**Files:**
- Modify: `web/shell/src/caps/device.ts:56-130` (on `platform: "ios"`, `camera.take`/`pick`, `file.pick`/`save`, `share.text`/`file` and `location.current`/`watch` ask the app for the grant only, then run through WebKit), `web/shell/src/caps/device-link.ts` (the `grant` call), `web/shell/src/app/host.ts` (`init`'s `platform` reaches `device.ts`), `apps/clax_app/lib/platform/device/device_host.dart` (`grant` requests: consent and the stored grant only, answered `{allowed, precise?}`), `apps/clax_app/lib/ui/host_protocol.dart` (the `grant` device method parses), `apps/clax_app/lib/ui/webview_surface.dart` (`Init(platform: 'ios')` on iOS), `apps/clax_app/lib/main.dart` (iOS's `DeviceHost` gets only `reminders` among the ports: share, files, camera and location are WebKit's)
- Test: `web/shell/src/caps/device.test.ts`, `apps/clax_app/test/platform/device/device_host_test.dart`

**Interfaces:**
- Produces the shell's iOS paths (the contract to pages, `web/contract/clax-extensions.d.ts` `Device`, is unchanged): `camera.take` → a hidden `<input type=file accept="image/*" capture="environment">`; `camera.pick` and `file.pick` → `<input type=file>` (with the page's `accept`); `file.save` and `share.file` → `navigator.share({files: [new File(…)]})` (the share sheet's Save to Files); `share.text` → `navigator.share({text})`; `location.current`/`watch` → `navigator.geolocation` (`enableHighAccuracy` only when the grant is precise), with the app's coarsening (`location_coarsen.dart`'s rule) applied in the shell for an approximate grant: a port of `Coarsener` to `web/shell/src/caps/coarsen.ts` with the same grid shift, sent by the app in the grant answer. Every path keeps `frameGesture()`, the 4 MiB limit (`deviceMaxBytes`) and the error codes (`not_granted`, `cancelled`, `unavailable`, `too_large`).
- Consumes: Task 1's answer on a framed gesture opening a picker (if it was "no", this task waits for the owner's decision recorded there); UI T34, T35, T36 (`Init.platform`, the host set).

- [ ] **Step 1: Read the threads** on `device.ts`, `device_host.dart`, `location_coarsen.dart`.
- [ ] **Step 2: Failing tests:** `device.test.ts` (jsdom, a fake `navigator.share`, `navigator.geolocation` and input clicks): on `platform: "ios"` each method asks `grant` first and never `call`s the native method; a denied grant rejects `not_granted` without touching WebKit; a 5 MiB picked file rejects `too_large`; an approximate grant returns a coarsened fix equal to the Dart `Coarsener`'s for the same input and shift (a shared fixture of three points); on `platform: "android"` nothing changes. `device_host_test.dart`: `grant` asks consent once per app and capability and answers `{allowed: true, precise: false}` for Approximate.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=shell/src/caps/device.test.ts //apps/clax_app:platform_device_device_host_test` — FAIL; implement; rerun — PASS — plus `//web:typecheck //web:lint //apps/clax_app:dart_analyze`.
- [ ] **Step 4: Look** on the simulator: the kit example's photo pick and location calls show iOS's own sheets and prompts after Clax's grant dialog; screenshots `target/ios/shots/device-{pick,location}.png`, read.
- [ ] **Step 5: Commit**: "On iOS, let apps' camera, files, share and location go through WebKit's own pickers and prompts, behind Clax's per-app grant".

### Task 19: The simulator integration tests (§9)

**Files:**
- Create: `apps/clax_app/integration_test/ios_cold_start_test.dart`, `ios_background_test.dart`, `ios_pause_test.dart`, `ios_open_bundle_test.dart`, `apps/clax_app/tool/ios-integration-run.sh`, `apps/clax_app/ios/Runner/TestChannel.swift` (`#if DEBUG`: `dev.empathic.clax/test` with `expireBackgroundTask` forwarded to `Background`, and `screenshot(path)` is not needed: the runner script takes them)
- Modify: `bazel/flutter/ios.bzl` (`clax_ios_integration_tests()`: per `integration_test/ios_*_test.dart`, an `integration_main`, a `flutter_application`, a `flutter_ios_app` built -c dbg, and `sh_test(name = "integration_ios_<name>")` tagged `manual`, `exclusive`, `no-sandbox`, `no-remote`), `bazel/flutter/macos.bzl:90-110` (the generated main also prints `CLAX_INTEGRATION_RESULT=<0|1>` before it exits, which the iOS runner reads), `apps/clax_app/BUILD.bazel` (the macro's call beside `clax_ios_app`), `apps/clax_app/BUILD.bazel:465-523` (the macOS comprehension's glob excludes `integration_test/ios_*_test.dart`), `scripts/run.sh` or `justfile` (`just app-integration --ios` runs `bazelisk test $(bazelisk query 'attr(name, "integration_ios_", //apps/clax_app:all)')`), `docs/usage.md` (the `--ios` form)

**Interfaces:**
- Produces `ios-integration-run.sh APP_ZIP`: boots "Clax iPhone" (through `scripts/app-ios.sh --udid`), installs the app fresh (`simctl uninstall` first: each test starts from an empty container), `simctl launch --console-pty`, reads the console until `CLAX_INTEGRATION_RESULT=` (at most 10 minutes, the only bound, as a test timeout), saves `simctl io screenshot` at each `CLAX_SHOT=<name>` line into `$TEST_UNDECLARED_OUTPUTS_DIR/<name>.png`, and exits with the result. Backgrounding is done by the test asking the runner: a `CLAX_BACKGROUND=<seconds>` line makes the script `simctl launch com.apple.Preferences`, wait that long, and relaunch the app (`simctl launch` of a running app brings it forward).
- Tests (each drives the real app through `integration_test/rig.dart`'s helpers where they fit):
  - **cold start:** the daemon comes up, `daemon.json` names `io.pid`, Home shows, and a made app (the scripted agent's, `CLAX_SCRIPTED_AGENT` through `--dart-define` in the macro's `flutter_application`) opens in its WebView with the key cookie (its page's `document.title`);
  - **background:** after `CLAX_BACKGROUND=30`, `/healthz` answers (restarted on the same port if it did not), the open app's stream reconnected (a version published by the test through the API reaches the page), and `http://<aid>.localhost:<port>/` loads;
  - **pause:** a scripted build's first step is held, the test sends `expireBackgroundTask` through the test channel, the paused notification is delivered (the plugin's `getActiveNotifications()` lists one titled with the app's name and the paused body), and on return (`CLAX_BACKGROUND=0`) it is gone and the build continues from its step and completes;
  - **open a `.clax`:** the runner `simctl openurl`s a fixture bundle (`CLAX_OPENURL=<path>`), and the install sheet shows its app's name.
- Consumes: Tasks 11–18.

- [ ] **Step 1: Read the threads** on `bazel/flutter/macos.bzl` and `integration_test/rig.dart`.
- [ ] **Step 2: Write the runner and the macro**; `bazelisk query 'attr(name, "integration_ios_", //apps/clax_app:all)'` lists four targets, all `manual` (`just ci` runs none).
- [ ] **Step 3: Write the four tests**; run each with `bazelisk test //apps/clax_app:integration_ios_<name> --test_output=streamed` in the background (logged). Expected: PASS. Read every PNG they wrote: the status bar, the home indicator, the notification banner and the install sheet.
- [ ] **Step 4: Check** the macOS integration targets are unchanged: `bazelisk query 'attr(name, "integration_", //apps/clax_app:all)'` lists the macOS ones as before plus the four.
- [ ] **Step 5: Commit**: "Test the iOS app on the simulator: cold start, a background cycle, a paused build and an opened bundle".

**Phase 4 ends** with the four integration tests passing and their screenshots read.

---

## Phase 5: The iOS interface (§5.1, §7)

Every task here builds on the UI plan's kit (UI T5–T8), overlays (UI T34–T40) and phone surfaces (UI T26–T33); none re-creates them. A component changes only its Apple family's iOS idiom, chosen by `defaultTargetPlatform == TargetPlatform.iOS` inside the Apple family, through `isTouchApple` (Task 21).

### Task 20: iOS type sizes and the iOS render forms

**Files:**
- Modify: `design/tokens.json` (`type.<role>.apple.ios` for every role: `large-title` [34, 41, 700], `title-1` [28, 34, 600], `title-2` [22, 28, 600], `title-3` [20, 25, 600], `headline` [17, 22, 600], `body` [17, 22, 400], `body-l` [17, 22, 400], `callout` [16, 21, 400], `caption` [12, 16, 400], `mono` [15, 20, 400], `haiku` [17, 24, 400]), `apps/clax_app/lib/ui/tokens.g.dart` (regenerated: `python3 scripts/sync-tokens.py`), `apps/clax_app/test/ui/shots/rig.dart` (forms `iphone` 402×874 at 3, `TargetPlatform.iOS`, top 62, bottom 34; `ipad` 1032×1376 at 2, top 24, bottom 20; `ipadCompact` 375×1376 at 2; `iosForms = [iphone, ipad, ipadCompact]`; `allForms` unchanged)
- Test: `apps/clax_app/test/ui/theme_test.dart`, `apps/clax_app/test/ui/shots/rig_shots_test.dart`

**Interfaces:**
- Produces `ClaxType.of(ClaxRole.body, TargetPlatform.iOS).fontSize == 17` (the generator already reads `apple.ios`, `scripts/sync-tokens.py:327-330`); `shots(…, forms: iosForms)`; the iOS forms load SF Pro as `.AppleSystemUIFont` as the Mac form does.
- Consumes: UI T1, T2, T5.

- [ ] **Step 1: Failing tests:** `ClaxType.of(ClaxRole.largeTitle, TargetPlatform.iOS).fontSize == 34` and `body` 17; `rig_shots_test` checks the three new forms' sizes and DPRs.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_theme_test //apps/clax_app:ui_shots_rig_shots_test //design:tokens_sync` — FAIL; implement and regenerate; rerun — PASS.
- [ ] **Step 3: Look:** `rig` at the three forms, light and dark; read `rig-iphone-light.png`: the status bar band at 62 and the home indicator band at 34.
- [ ] **Step 4: Commit**: "Give the Apple family iOS's text sizes, and render surfaces at iPhone and iPad sizes".

### Task 21: The Apple family's iOS idiom for controls, scaffold and toasts (§7.1)

**Files:**
- Modify: `apps/clax_app/lib/ui/kit/family.dart` (`bool get isTouchApple => defaultTargetPlatform == TargetPlatform.iOS;`), `kit/scaffold.dart` (iOS: `CupertinoPageScaffold` with a `CupertinoNavigationBar`, or `CupertinoSliverNavigationBar` when `largeTitle`; colours from the palette), `kit/button.dart` (iOS: filled, tinted and plain at 17 pt; 44 pt minimum targets; a primary at the bottom of a sheet 50 pt high), `kit/text_field.dart` (iOS: `CupertinoTextField`, 17 pt, rounded, no focus ring until a hardware keyboard is seen: `KeyboardAttached`, a notifier in `family.dart` that the first `KeyEvent` from `HardwareKeyboard.instance` sets), `kit/toggles.dart` (`CupertinoSwitch` in `agent`), `kit/toast.dart` (iOS: a capsule under the status bar or Dynamic Island, `MediaQuery.paddingOf(context).top + 8`), `kit/badge.dart` (iOS sizes)
- Test: `apps/clax_app/test/ui/kit/controls_test.dart`, `apps/clax_app/test/ui/kit/surfaces_test.dart`, `apps/clax_app/test/ui/shots/kit_shots_test.dart` (`forms: [...allForms, ...iosForms]`)

**Interfaces:**
- Produces `isTouchApple`, `KeyboardAttached.of(context)` (`bool`), and the iOS forms of the UI plan's `ClaxScaffold`, `ClaxButton`, `ClaxTextField`, `ClaxSwitch`, `ClaxCheckbox` (iOS: a checkmark row), `ClaxToast`, `ClaxBadge`, with their constructors unchanged.
- Consumes: UI T5, T6, T7, T50 (hover and cursors already in `button.dart`).

- [ ] **Step 1: Read the threads** on `lib/ui/kit/`.
- [ ] **Step 2: Failing tests** (`debugDefaultTargetPlatformOverride = TargetPlatform.iOS`): every control's hit area is at least 44×44; `ClaxScaffold(largeTitle: true)` builds a `CupertinoSliverNavigationBar`; `ClaxTextField` is a `CupertinoTextField` at 17 pt with no focus ring until a `KeyDownEvent` arrives; `ClaxSwitch` is a `CupertinoSwitch` whose active colour is the palette's `agent`; a top `ClaxToast` sits 8 pt under `padding.top`; on macOS and Android every existing test still passes unchanged.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_kit_controls_test //apps/clax_app:ui_kit_surfaces_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** `kit-controls` and `kit-surfaces` at `iphone` and `ipad`, light and dark, and at the largest accessibility text size (`textScales: [1, 3.1]` for `iphone`); read them: iOS controls in Clax's colours, nothing clipped.
- [ ] **Step 5: Commit**: "Draw Clax's controls, scaffold and toasts in iOS's idiom inside the Apple family".

### Task 22: Sheets with detents, alerts, action sheets and context menus (§7.1, §7.3)

**Files:**
- Create: `apps/clax_app/ios/Runner/NativeDialogs.swift` (registered from `AppDelegate.swift` beside `NativeChannel`; the Mac's `dev.empathic.clax/dialogs` contract, UI T39: `alert` as `UIAlertController(.alert)` with `armMs` disabling the default action until then and `suppression` as a second "Always for this app" action; `actionSheet` as `.actionSheet`, anchored on iPad at the `at` rect the call passes; `menu` is not served on iOS: menus are Flutter's)
- Modify: `apps/clax_app/lib/ui/kit/sheet.dart` (`ClaxSheet.show(…, {ClaxDetent detent = ClaxDetent.medium})`: on iOS at compact width, decision 18's route with a grabber and no close button, and a Done button in the navigation bar on the large detent; on iPad at medium and up, a 540 pt form sheet centred), `kit/dialog.dart` (on iOS, `ClaxDialog.show` routes to `NativeDialogs.alert`; a destructive confirmation with `asSheet: true` to `NativeDialogs.actionSheet`), `kit/menu.dart` (on iOS, `ClaxMenu.show` opens a pull-down styled menu at the anchor; `ClaxMenu.contextFor(child, items)` wraps a tile or row in `CupertinoContextMenu` with the child as its preview), `kit/native_dialogs.dart` (`actionSheet`; the routing getter becomes `hasNativeAlerts => hasDesktopWindowChrome || isTouchApple`)
- Test: `apps/clax_app/test/ui/kit/surfaces_test.dart`, `apps/clax_app/test/ui/kit/native_dialogs_test.dart`

**Interfaces:**
- Produces `enum ClaxDetent { medium, large }`; `ClaxSheet.show<T>(context, {required builder, title, actions, detent})`; `NativeDialogs.actionSheet({required String title, String? body, required List<String> buttons, int? destructiveIndex, int? cancelIndex, Rect? at}) → Future<int?>`; `ClaxMenu.contextFor(Widget child, List<ClaxMenuItem> items, {required void Function(String id) onSelected}) → Widget`.
- Consumes: UI T7, T36, T39; Task 21 (`isTouchApple`).

- [ ] **Step 1: Failing tests** (iOS override): `ClaxSheet.show` at 402 wide shows a grabber, no `Icons.close`, and its sheet's height is half the screen within 1 pt; `detent: large` fills it and shows Done; at 1032 it is a 540-wide form sheet; `ClaxDialog.show` on iOS calls the mock `dialogs` channel's `alert` with `armMs` and never builds an `AlertDialog`; `NativeDialogs.actionSheet` sends `at` as `{x, y, w, h}`; `ClaxMenu.contextFor` builds a `CupertinoContextMenu`; on Android none of these call the channel.
- [ ] **Step 2: Run** — FAIL; implement Dart and Swift; rerun — PASS — plus `//apps/clax_app:ios_app_test`, `dart_analyze`.
- [ ] **Step 3: Look:** a sheet at medium and large, an alert before and after arming, an action sheet on iPad anchored to its button (simulator screenshots, since UIKit draws the alerts), light and dark; read them.
- [ ] **Step 4: Commit**: "Give iOS sheets with detents, UIKit alerts and action sheets, and context menus with previews".

### Task 23: `ClaxHaptics`

**Files:**
- Create: `apps/clax_app/lib/ui/kit/haptics.dart`, `apps/clax_app/test/ui/kit/haptics_test.dart`
- Modify: `apps/clax_app/lib/ui/kit/kit.dart` (export)

**Interfaces:**
- Produces:
  ```dart
  /// The one place haptics come from: on iPhone only (an iPad has no Taptic
  /// Engine; `MediaQuery.sizeOf(context).shortestSide >= 600` is an iPad),
  /// and never under `MediaQuery.disableAnimationsOf` (iOS's Reduce Motion
  /// stands in for the system haptics switch Flutter does not expose).
  abstract final class ClaxHaptics {
    static void selection(BuildContext c); // pickers, a drag's pick-up
    static void light(BuildContext c);     // a pick, a drop
    static void medium(BuildContext c);    // a long-press into comment mode
    static void success(BuildContext c);   // a version landed in front
    static void warning(BuildContext c);   // a failed change
  }
  ```
  over `HapticFeedback.selectionClick`, `lightImpact`, `mediumImpact`, and for success and warning `HapticFeedback.vibrate` is not used: they call `dev.empathic.clax/native`'s `notificationHaptic {kind: "success"|"warning"}`, which `NativeChannel.swift` serves with `UINotificationFeedbackGenerator` (add the method there).
- Consumes: Task 21 (`isTouchApple`), Task 10 (`NativeChannel.swift`).

- [ ] **Step 1: Failing tests** (a mock `SystemChannels.platform` recording `HapticFeedback.vibrate` calls and a mock native channel): on iOS at 402 wide each call plays once; at 1032 wide none plays; with `disableAnimations` none plays; on Android and macOS none plays.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_kit_haptics_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`, `//apps/clax_app:ios_app_test`.
- [ ] **Step 3: Commit**: "Play haptics from one place, on iPhone only and never under Reduce Motion".

### Task 24: The navigation stack and swipe-back (§7.2)

**Files:**
- Modify: `apps/clax_app/lib/ui/theme.dart` (`pageTransitionsTheme` gains `TargetPlatform.iOS: CupertinoPageTransitionsBuilder()`), `apps/clax_app/lib/app.dart` (Home at compact width on iOS is a `ClaxScaffold(largeTitle: true, title: 'Clax')` with Inbox and Settings as navigation bar buttons pushing `CupertinoPageRoute`s; an app page pushes full screen; a pop from an app page returns to Home, and the page's own history is the Clax bar's ⋯ › Back, Task 25), `apps/clax_app/lib/ui/launcher.dart` (the fallback launcher's top bar on iOS is the large title), `apps/clax_app/lib/ui/webview_surface.dart` is not touched (back-forward gestures stay off, Task 17)
- Test: `apps/clax_app/test/app_test.dart`, `apps/clax_app/test/ui/launcher_test.dart`, `apps/clax_app/test/ui/theme_test.dart`

**Interfaces:**
- Consumes: UI T5, T27 (the app screen's `PopScope` turns a back inside the page into `GoBack` on Android; on iOS a swipe-back pops the route instead), T31, T33, T43; Tasks 21, 22.

- [ ] **Step 1: Read the threads** on `app.dart`, `launcher.dart`, `theme.dart`.
- [ ] **Step 2: Failing tests** (iOS override, 402×874): Home shows a large "Clax" title and Inbox and Settings buttons; tapping Inbox pushes a `CupertinoPageRoute`; a left-edge drag (`tester.dragFrom(Offset(2, 400), Offset(300, 0))`) pops it; an opened app's route pops to Home on the same drag, and a `HistoryMsg(canGoBack: true)` does not change that (on Android the UI plan's T27 test still sends `GoBack`); the theme's iOS builder is Cupertino's.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:root_app_test //apps/clax_app:ui_launcher_test //apps/clax_app:ui_theme_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look** on the simulator: Home, Inbox pushed, a swipe-back half way (screenshot mid-gesture with `simctl io` while a `simctl` scripted touch is not available: take Home and Inbox only), light and dark; read them.
- [ ] **Step 5: Commit**: "Navigate the iPhone as one stack with large titles and swipe-back, the page's own history kept off the edge gesture".

### Task 25: The Clax bar (§5.1)

**Files:**
- Create: `apps/clax_app/lib/ui/clax_bar.dart`, `apps/clax_app/test/ui/clax_bar_test.dart`
- Modify: `apps/clax_app/lib/ui/app_screen.dart` (on iPhone, `isTouchApple` and compact width: the `ClaxBar` in the bottom safe area replaces the `ClaxPill` and the `CommentTab`; comment mode's bar replaces it while on), `web/bridge/src/` (the chrome sampler of UI T26 adds `scroll: "up" | "down"` on a 24 px movement of the page's scroll, debounced to one message per direction change), `web/shell/src/app/host.ts` (relays it), `apps/clax_app/lib/ui/host_protocol.dart` (`ChromeMsg.scroll`, optional)
- Test: `apps/clax_app/test/ui/app_screen_test.dart`, `apps/clax_app/test/ui/host_protocol_test.dart`, `web/bridge/test/chrome.test.ts`, `apps/clax_app/test/ui/shots/app_shots_test.dart` (`forms: iosForms` for the bar)

**Interfaces:**
- Produces:
  ```dart
  enum ClaxBarMode { compact, expanded, line }
  class ClaxBar extends StatefulWidget {
    const ClaxBar({super.key, required this.state, this.progress, required this.threads, required this.onHome, required this.onComment, required this.onThreads, required this.onShare, required this.onMore, this.clock = const SystemClock(), this.foldAfter = const Duration(seconds: 4)});
    final ClaxPillState? state; // UI T28's states: working, building, newVersion, location
  }
  ```
  Compact: a capsule centred in the home indicator band, 30 pt to see in a 44 pt target, on the system material (a `BackdropFilter` blur over `overlay` at 80% when Reduce Transparency is off; solid `raised` when on), the mark (`ClaxMark`, UI T8), "Clax" and the pill's state text. A tap expands it to a toolbar above the band (Home, Comment, Threads with its `ClaxBadge`, Share, ⋯ with Inspect, Versions, Star, Back when the page has history); it folds after `foldAfter` on `clock` or on the next tap on the page. A long-press enters comment mode with `ClaxHaptics.medium`. A `ChromeMsg(scroll: down)` shrinks it to a 6 pt line over the home indicator; `up` or a tap brings it back. `Semantics`: a button "Clax" with the state as its value; VoiceOver reads each toolbar item.
- Consumes: UI T8, T26, T28, T29, T30; Tasks 21, 22, 23.

- [ ] **Step 1: Read the threads** on `app_screen.dart` (the ten UI T26 names, and whatever UI T28–T30 left) and the bridge's chrome sampler.
- [ ] **Step 2: Failing tests:** `clax_bar_test.dart` (FakeClock): compact by default, 44 pt hit target, centred in the bottom padding; a tap expands with five items; after `tester.pump(const Duration(seconds: 4))` it folds; a long-press calls `onComment` and plays `medium` (mock channel); `ChromeMsg(scroll: down)` makes it 6 pt high, `up` restores it; at `TextScaler.linear(2)` the label is not clipped. `app_screen_test.dart` (iOS override, 402 wide): no `ClaxPill` and no `CommentTab`, one `ClaxBar`; in comment mode no `ClaxBar`; on Android at 412 the UI plan's pill and tab are unchanged. `chrome.test.ts`: a scroll of 30 px down posts `scroll: "down"` once, 10 px posts nothing.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_clax_bar_test //apps/clax_app:ui_app_screen_test //apps/clax_app:ui_host_protocol_test //web:unit --test_arg=bridge/test/chrome.test.ts` — FAIL; implement; rerun — PASS — plus `dart_analyze`, `//web:typecheck`.
- [ ] **Step 4: Look:** the app shot at `iphone`: compact, expanded, line, and building 2/5, light and dark and at the largest text size; then a simulator screenshot of the real bar over a page (the home indicator drawn by iOS); read them.
- [ ] **Step 5: Commit**: "Put Clax in the iPhone's home indicator band: a bar that expands to the app's actions and long-presses into comment mode".

### Task 26: Lists and forms: Inbox, Settings and versions (§7.3)

**Files:**
- Modify: `apps/clax_app/lib/ui/inbox_screen.dart` (iOS: `CupertinoListSection.insetGrouped`; rows with a trailing Resolve swipe action in `agent` and a leading Mark Read, through `lib/ui/swipe.dart`'s existing swipe if it fits, else `Dismissible` with `confirmDismiss` that does not remove the row; `CupertinoSliverRefreshControl` pull to refresh), `apps/clax_app/lib/ui/settings_screen.dart` (iOS: inset grouped sections; the API key a secure `CupertinoTextField` with a Paste button; Notifications, when off, shows "Turn on in Settings" calling `openAppSettings`), `apps/clax_app/lib/ui/versions_tab.dart` (iOS: a trailing Restore swipe action on each row, asking `ClaxDialog` first)
- Test: `apps/clax_app/test/ui/inbox_screen_test.dart`, `apps/clax_app/test/ui/settings_test.dart`, `apps/clax_app/test/ui/versions_tab_test.dart` (create if absent), shots `inbox_shots_test.dart`, `screens_shots_test.dart` (`iosForms`)

**Interfaces:**
- Consumes: UI T14, T29, T32, T49 (`SettingsTabs` bodies), T55; Tasks 21, 22.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests** (iOS override): an inbox row swiped left shows Resolve and its tap calls `resolveThread` (FakeApi); swiped right shows Mark Read; a pull past 100 pt calls the refresh once; Settings' sections are `CupertinoListSection`s, the key field obscures its text, Paste fills it from a mock clipboard, and with notifications off "Turn on in Settings" sends `openAppSettings`; a version row's Restore asks before restoring.
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** inbox, a swiped row, Settings and versions at `iphone` and `ipad`, light and dark, 100% and the largest size; read them.
- [ ] **Step 5: Commit**: "Lay out Inbox, Settings and versions as iOS lists, with swipe actions and pull to refresh".

### Task 27: Comment mode, making and tiles on iOS (§7.3)

**Files:**
- Modify: `apps/clax_app/lib/ui/comment_composer.dart` (iOS compact: the composer is a `ClaxSheet` at the medium detent that rises with `MediaQuery.viewInsetsOf`; Send in the keyboard's accessory row; Return sends only when `KeyboardAttached` (Task 21), else inserts a newline; picks play `ClaxHaptics.light`), `apps/clax_app/lib/ui/making_screen.dart` (iOS compact: a sheet at the large detent over the app; a paused build reads "Paused — open Clax to keep building." until it resumes, from Task 13's presence), `apps/clax_app/lib/ui/launcher.dart` (iOS: a tile's long-press is `ClaxMenu.contextFor` with the tile as preview: Open, Inspect, Star, Share, Delete in red; a drag's pick-up plays `selection`, a drop `light`)
- Test: `apps/clax_app/test/ui/comment_composer_test.dart`, `apps/clax_app/test/ui/making_screen_test.dart`, `apps/clax_app/test/ui/launcher_test.dart`

**Interfaces:**
- Consumes: UI T30, T31, T33, T48, T51; Tasks 13, 21, 22, 23.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests** (iOS override): the composer is a sheet at half height which moves up by the keyboard's inset (`MediaQueryData(viewInsets: EdgeInsets.only(bottom: 300))`); Return inserts a newline without a hardware keyboard and sends with one; a paused presence shows the paused line in the making sheet and it goes on `resumed`; a tile's long-press builds a `CupertinoContextMenu` with five actions, Delete destructive.
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** comment mode with the composer and keyboard (simulator: `simctl` with the software keyboard shown), the making sheet, paused, and a tile's context menu (simulator screenshot), light and dark; read them.
- [ ] **Step 5: Commit**: "Comment, make and arrange on iOS with sheets that follow the keyboard, a paused build that says so, and tile previews".

### Task 28: The iOS icon set and launch screen (§7.3, the UI spec §5.11)

**Files:**
- Modify: `scripts/build-app-icons.sh` (an iOS target: `apps/clax_app/ios/Runner/Assets.xcassets/AppIcon.appiconset/` with the 1024 pt single-size icon in light, dark and tinted variants and its `Contents.json` in Xcode 26's single-size form, no 76×76@1x entry (rules_flutter's `families` note); `SOURCE.sha256` covers it), `apps/clax_app/ios/Runner/Base.lproj/LaunchScreen.storyboard` (the view's background the named colour `LaunchBackground`, the mark image centred at 132 pt; `Assets.xcassets/LaunchBackground.colorset` with `bg` light and dark; `LaunchMark.imageset` rendered by the script), `apps/clax_app/tool/ios-app-check.sh` (the compiled `Assets.car` exists and `Info.plist` has `CFBundleIcons`)
- Test: `//design:icons_sync`, `//apps/clax_app:ios_app_test`

**Interfaces:**
- Consumes: UI T9 (`scripts/build-app-icons.sh`, `design/mark.json`), UI T11 (`ClaxSplash` centres the mark at the window's centre, which the launch screen matches).

- [ ] **Step 1: Read the threads** on `scripts/build-app-icons.sh`.
- [ ] **Step 2: Failing check:** `ios-app-check.sh`'s icon assertions; `bazelisk test //apps/clax_app:ios_app_test //design:icons_sync` — FAIL.
- [ ] **Step 3: Implement**; run the script; rerun — PASS.
- [ ] **Step 4: Look:** read `AppIcon.appiconset`'s three PNGs; install with `just app-ios`, screenshot the home screen icon and the launch (`simctl launch` then a screenshot within the first frame is not reliable: read the storyboard render instead by `ibtool --compile` into `target/ios/` only if the script cannot render it); the launch's mark sits where `ClaxSplash` draws it.
- [ ] **Step 5: Commit**: "Give the iOS app the Clax mark as its icon, light, dark and tinted, and a launch screen that matches the first frame".

**Phase 5 ends** with every surface's `iphone` renders read in both themes and at the largest text size, and the simulator screenshots of the Clax bar, alerts and sheets read.

---

## Phase 6: iPad (§7.2)

### Task 29: Sidebar and detail for touch, and the size-class switch

**Files:**
- Modify: `apps/clax_app/lib/ui/split/split_shell.dart` (on `isTouchApple`: the sidebar 320 pt, rows 44 pt, inset grouped; it collapses with the navigation bar's sidebar button or a left-edge swipe; the detail has a navigation bar with back, title, Comment, Threads with its count, Share and ⋯), `apps/clax_app/lib/ui/split/sidebar.dart` (touch metrics on iOS; search at its top), `apps/clax_app/lib/ui/threads_panel.dart` (on iOS a third column of 320 pt at a window at least 1180 pt wide, a sheet below that), `apps/clax_app/lib/app.dart` (on iPad, Inbox and Settings open as `ClaxSheet` form sheets, not routes; the selection and the selected app's scroll position survive a width change across `ClaxWidth.medium`)
- Test: `apps/clax_app/test/ui/split/split_shell_test.dart`, `apps/clax_app/test/app_test.dart`, `apps/clax_app/test/ui/shots/split_shots_test.dart` (`iosForms`)

**Interfaces:**
- Consumes: UI T45, T46, T47, T48, T49, T51; Tasks 21, 22, 24.

- [ ] **Step 1: Read the threads** on the split files and `app.dart`.
- [ ] **Step 2: Failing tests** (iOS override): at 1032×1376 a 320 sidebar with 44 pt rows and the detail's navigation bar; at 1180 wide with threads open, three columns; at 1100, threads as a sheet; resizing `tester.view.physicalSize` from 1032 to 375 and back keeps the selected app selected and its `ScrollController.offset`; at 375 the iPhone stack (Task 24) shows; Inbox opens a form sheet at 1032.
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Record** the `alternative` (decision 13) on `docs/superpowers/specs/2026-10-18-ios-release-design.md:686-690` (§7.2's Split View bullet): "The stack-to-sidebar switch is ClaxWidth.medium (600 pt, lib/ui/layout.dart), shared with every platform, not 700 pt. revisit: after-ipad-testflight".
- [ ] **Step 5: Look:** `split` at `ipad` and `ipad-compact`, light and dark; on the "Clax iPad" simulator, Split View is not scriptable by `simctl`: rotate (`xcrun simctl io <udid> … ` has no rotate; use the Simulator app's Device › Rotate by hand once) and take screenshots in both orientations; read them.
- [ ] **Step 6: Commit**: "Give the iPad the sidebar and detail drawn for touch, moving to one stack and back as its window narrows".

### Task 30: The iPad's menu bar, hardware keyboard and pointer

**Files:**
- Create: `apps/clax_app/ios/Runner/MenuBuilder.swift` (`AppDelegate.buildMenu(with:)`: Clax, File, Edit, View, App, Window, Help with the UI spec §4.8's items and ⌘ shortcuts as `UIKeyCommand`s; each sends `claxMenuAction {command}` on `dev.empathic.clax/menu`, the UI plan's channel)
- Modify: `apps/clax_app/ios/Runner/AppDelegate.swift` (install), `apps/clax_app/lib/ui/app_menu.dart` (`MacMenu.commands` also listens on iOS; renamed in place to `NativeMenu` only if UI T43's name is `MacMenu` and every caller is updated in this task)
- Test: `apps/clax_app/test/ui/app_menu_test.dart`

**Interfaces:**
- Consumes: UI T43 (`ClaxCommand`, `claxShortcuts(apple)`, the `menu` channel), T50 (hover); Task 29.

- [ ] **Step 1: Failing tests:** on iOS a `commands` event of `home` from the mock channel pops to Home; ⌘K (`claxShortcuts(apple)`) toggles threads on iOS at 1032 wide; a secondary tap on a sidebar row opens its `ClaxMenu`; a mouse hover fills a row with `hover`.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — plus `//apps/clax_app:ios_app_test`.
- [ ] **Step 3: Look:** on "Clax iPad" with the Mac's keyboard connected (I/O › Keyboard › Connect Hardware Keyboard), hold ⌘ and screenshot the shortcut overlay; read it against §4.8.
- [ ] **Step 4: Commit**: "Give the iPad Clax's menu bar and keyboard shortcuts, and pointer hover and right clicks".

### Task 31: The iPad renders

**Files:**
- Modify: the shots tests Tasks 21–29 touched gain `ipad` and `ipad-compact` where they lack them; no product code.

- [ ] **Step 1: Render** every surface at `ipad` and `ipad-compact`, light and dark, plus the largest text size at `ipad-compact`.
- [ ] **Step 2: Look** at each PNG; fix any defect in the task that owns the surface (a separate commit naming it).
- [ ] **Step 3: Commit** the test changes: "Render every surface at iPad sizes".

---

## Phase 7: TestFlight, everything that needs no account (§4.4, §4.5, §8, §10)

### Task 32: The signing inputs, declared

**Files:**
- Create: `bazel/apple/signing.bzl` (`clax_apple_signing` repository rule and its module extension), `bazel/apple/signing-test.sh`
- Modify: `MODULE.bazel` (`signing = use_extension("//bazel/apple:signing.bzl", "signing")`, `use_repo(signing, "clax_apple_signing")`), `bazel/apple/BUILD.bazel` (`sh_test(name = "signing_test")`), `bazel/flutter/ios.bzl` (`ipa_dev` and `ipa_appstore`: `flutter_ios_app`s with `provisioning_profile` from `@clax_apple_signing//:development` / `:appstore` and entitlements from `@clax_apple_signing//:entitlements_<kind>`, tagged `manual`, `no-remote`, `no-cache`), `docs/usage.md` ("Signing for a device and TestFlight": each file, where it goes, how the owner gets it)

**Interfaces:**
- Produces `@clax_apple_signing`: a `local` repository that watches `~/.config/clax/apple/` (`CLAX_APPLE_DIR` overrides it, for the test), and exposes `:team_id` (a file with the 10-character ID), `:development` and `:appstore` (the profiles), `:api_key` (`AuthKey_<KEYID>.p8` with `api-key-id` and `api-issuer-id`), `:entitlements_development` (`application-identifier` `<team>.dev.empathic.clax`, `com.apple.developer.team-identifier`, `keychain-access-groups` `[<team>.dev.empathic.clax]`, `get-task-allow` true) and `:entitlements_appstore` (the same without `get-task-allow`). When a file is missing, each target is a `fail()`-on-use rule whose message names every missing file and `docs/usage.md`'s section; a directory not `0700` or a file readable by group or others is refused with its path. Simulator builds never depend on it.
- Consumes: Task 10.

- [ ] **Step 1: Write `signing-test.sh`**: with `CLAX_APPLE_DIR` at a `mktemp -d` holding fake files (a 10-character team ID, two files named as profiles, a key and its two IDs), `bazelisk build @clax_apple_signing//:entitlements_development` succeeds and the plist names the team; with the profile removed, the build fails naming `clax-development.mobileprovision`; with the directory `0755`, it fails naming the mode. Register it (`tags = ["manual", "no-sandbox"]`: it runs Bazel). Run it — FAIL.
- [ ] **Step 2: Implement**; rerun — PASS. `bazelisk build //apps/clax_app:ios_app` still needs nothing from the directory (`CLAX_APPLE_DIR=/nonexistent`).
- [ ] **Step 3: Commit**: "Declare the device and TestFlight signing inputs, naming each missing file and keeping them out of every cache".

### Task 33: `just app-iphone` and its build-number ledger

**Files:**
- Create: `scripts/app-iphone.sh`, `scripts/test-app-iphone.sh`
- Modify: `justfile` (`app-iphone` and `app-iphone-testflight`, each with a description), `scripts/BUILD.bazel` (`sh_test(name = "app_iphone")`), `docs/usage.md`

**Interfaces:**
- Produces `scripts/app-iphone.sh [--testflight] [--i-know-what-i-am-doing]`, in `app-phone.sh`'s style: builds HEAD exactly, or a clean copy at `target/app-iphone/tree`; without the flag prints each step and changes nothing. Device: `bazelisk build //apps/clax_app:ipa_dev -c opt --ios_multi_cpus=arm64 --objc_enable_binary_stripping --features=dead_strip`, `xcrun devicectl list devices` (`APP_IPHONE_DEVICE` picks one when several), `devicectl device install app`, `devicectl device process launch`, and `devicectl device info files` reading `daemon.json` until it names the launched PID or 60 s. `--testflight`: the next build number from `~/.config/clax/apple/builds` (one line per marketing version, `<version> <last number>`; the next is last + 1, written before the upload so a failed upload never reuses it), the `ipa_appstore` build with `--define=clax_build_number=<n>` read by `ios_version`, `xcrun altool --upload-package <ipa> --type ios --apiKey <id> --apiIssuer <issuer>` (the key from `@clax_apple_signing`'s directory, through `API_PRIVATE_KEYS_DIR`), then prints the build number and `https://appstoreconnect.apple.com/apps`. It prints Transporter as the fallback when `altool` refuses.
- Produces `test-app-iphone.sh`: with stub `bazelisk`, `xcrun` and a temp ledger, the dry run prints the build, install and launch lines in order and writes nothing; `--testflight --i-know-what-i-am-doing` with a ledger `1.0.0 4` writes `1.0.0 5` before calling the stub `altool`, and a failing stub `altool` leaves `5` (never reused); without the signing directory it stops naming the missing files.
- Consumes: Task 32.

- [ ] **Step 1: Write the test**; register; run — FAIL. **Step 2: Write the script**; rerun — PASS — and `scripts/test-justfile.sh`. **Step 3: Commit**: "Add just app-iphone for a device and TestFlight, with build numbers that are never reused".

### Task 34: The privacy manifest (§10.4)

**Files:**
- Create: `apps/clax_app/ios/Runner/PrivacyInfo.xcprivacy`
- Modify: `bazel/flutter/ios.bzl` (the app's `resources` gain it), `apps/clax_app/tool/ios-app-check.sh` (it is in the bundle and parses with `plutil -lint`)

**Interfaces:**
- Produces: `NSPrivacyTracking` false, `NSPrivacyTrackingDomains` empty, `NSPrivacyCollectedDataTypes` empty (Group B3 adds the entry once the owner answers the privacy label), and `NSPrivacyAccessedAPITypes` with exactly the required-reason categories the app's own binary links: list them with `nm -u <Clax.app/Clax> | grep -E 'stat|fstat|getattrlist|NSFileModificationDate|systemUptime|mach_absolute_time|statfs|NSUserDefaults|volumeAvailableCapacity'` on the simulator build and map each hit to its category (`C617.1` file timestamps, `35F9.1` system boot time, `E174.1` disk space, `CA92.1` user defaults), writing the `nm` output into the commit message.
- Consumes: Task 16.

- [ ] **Step 1: Failing check** in `ios-app-check.sh`; run — FAIL. **Step 2: List the symbols**, write the manifest; rerun — PASS. **Step 3: Commit**: "Declare the iOS app's required-reason APIs and that it tracks no one".

### Task 35: The first-use AI disclosure (§8.1, guideline 5.1.2(i))

**Files:**
- Create: `apps/clax_app/lib/agent/ai_consent.dart`, `apps/clax_app/lib/ui/ai_consent_sheet.dart`, `apps/clax_app/test/agent/ai_consent_test.dart`
- Modify: `apps/clax_app/lib/platform/settings_store.dart` (`bool? aiConsent`, key `aiConsent`), `apps/clax_app/lib/app.dart` (before the first Messages API request of a make or change, when `required` and `aiConsent != true`, the sheet asks; Not Now cancels the run as a cancel does), `apps/clax_app/lib/ui/settings_screen.dart` (the choice shown, with Turn off)

**Interfaces:**
- Produces `class AiConsent { AiConsent({required bool required, required SettingsStore settings}); Future<bool> ensure(BuildContext context); }` with `required: Platform.isIOS` from `main.dart`; the sheet (a `ClaxSheet` at the medium detent): title "Clax sends your request to Anthropic", body "What you type or say, and the app you’re changing, go to Anthropic’s Claude API under your own key, to make and change your apps.", **Allow** and Not Now.
- Consumes: Task 22.

- [ ] **Step 1: Failing tests:** with `required: true` the first make shows the sheet and sends nothing until Allow; Not Now cancels and sends nothing; after Allow no sheet again; Settings' Turn off brings it back; with `required: false` (Android, macOS) no sheet ever.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:agent_ai_consent_test //apps/clax_app:root_app_test //apps/clax_app:ui_settings_test` — FAIL; implement; rerun — PASS. **Step 3: Look:** the sheet at `iphone`, light and dark. **Step 4: Commit**: "Ask once on iOS before anything goes to the Claude API, and let Settings take it back".

### Task 36: The App Intent "Comment in Clax"

**Files:**
- Create: `apps/clax_app/ios/Intents/CommentInClax.swift` (an `AppIntent` with `openAppWhenRun = true` that posts `comment` on `dev.empathic.clax/intents` once the engine is up, queued until Dart asks `initial`)
- Modify: `bazel/flutter/ios.bzl` (`swift_library(name = "ios_intents", srcs = ["ios/Intents/CommentInClax.swift"])` in `runner_deps` and in `ios_application`'s `app_intents` through `**kwargs`, as `flutter_ios_app`'s doc says), `apps/clax_app/lib/app.dart` (a `comment` intent enters comment mode on the app on top, or opens the most recent app first), `apps/clax_app/tool/ios-app-check.sh` (the bundle's `Metadata.appintents` exists)
- Test: `apps/clax_app/test/app_test.dart`

**Interfaces:**
- Produces `ChannelIntents` in `lib/platform/intents.dart`: `Stream<String> get intents; Future<List<String>> initial();`.
- Consumes: Task 25 (comment mode from the Clax bar's path).

- [ ] **Step 1: Failing tests:** an `intents` event `comment` with an app on top enters its comment mode; with Home on top it opens the most recent app and then enters comment mode; the check fails without the metadata.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — plus `//apps/clax_app:ios_app_test`. **Step 3: Look:** on the simulator, Shortcuts › Clax lists "Comment in Clax"; screenshot and read. **Step 4: Commit**: "Let the Action Button, Siri and Shortcuts start a comment in Clax".

---

## Group B: blocked on the owner's Apple inputs

**Every task here waits for:** an Apple Developer Program membership; the team ID; the bundle ID confirmed (`dev.empathic.clax`, or another to set in `bazel/flutter/ios.bzl`'s `IOS_BUNDLE_ID`); the Apple Development and Apple Distribution certificates in this Mac's login keychain; the iPhone's and iPad's UDIDs in a development profile; the App Store profile; an App Store Connect app record and an API key with the Developer role; the export-encryption answer (open question 6); and the privacy label (open question 7). Each file goes where `docs/usage.md`'s "Signing for a device and TestFlight" (Task 32) says. None of these tasks starts until the owner says the inputs are in place.

### B1: The signing inputs in place, and the first device build

- [ ] `bazelisk test //bazel/apple:signing_test` against the real directory (`CLAX_APPLE_DIR` unset) passes the mode checks.
- [ ] `scripts/app-iphone.sh` (dry run) lists no missing file; `bazelisk build //apps/clax_app:ipa_dev -c opt --ios_multi_cpus=arm64` succeeds; `codesign -d --entitlements - <app>` shows the team's `application-identifier`.
- [ ] If the owner changed the bundle ID, change `IOS_BUNDLE_ID` and `ios-app-check.sh`'s expectation in one commit.
- [ ] Propose to the owner (do not close) a comment on record 553cb43495 that the same certificates now exist, for the Mac's team-signed build.
- [ ] Commit any change: "Build the development-signed iOS app with the team's profile".

### B2: The first install on a device, and the release-only checks

- [ ] `just app-iphone` (with `--i-know-what-i-am-doing`) installs and launches on the iPhone; the daemon stays up (`daemon.json` names the launched PID).
- [ ] On the release build (the App Store-signed `.ipa` installed through TestFlight in B3, or `-c opt` with the development profile here): no local network prompt appears (decision 8); record the answer on the spec's §2.9 line as a qualifier comment.
- [ ] Measure and write into a qualifier comment on §2.8 (`docs/superpowers/specs/2026-10-18-ios-release-design.md:284-288`): the app's footprint with a home of 50 apps (Xcode's Instruments is not needed: `devicectl device info processes` and the memory report of `xcrun devicectl device process` if it gives one, else Settings › Privacy › Analytics), and the `.ipa`'s size and App Store Connect's estimated download size (§4.2).

### B3: Export compliance, the privacy label, and the first TestFlight build

- [ ] With the owner's export answer: `ITSAppUsesNonExemptEncryption` in `ios/Runner/Info.plist` (`NO` if the answer is that HTTPS and authentication are exempt) and `ios-app-check.sh`'s check of it.
- [ ] With the owner's privacy label: `NSPrivacyCollectedDataTypes` in `PrivacyInfo.xcprivacy` (the draft: Other User Content, not linked, not tracking, App Functionality), and the same answers in App Store Connect's App Privacy.
- [ ] `just app-iphone --testflight --i-know-what-i-am-doing`: uploads; App Store Connect processes it; it reaches internal testers.
- [ ] Commit: "Answer export compliance and the privacy label for TestFlight".

### B4: The device checklist (§9)

- [ ] Run the spec's twelve items on the owner's iPhone and an iPad from the TestFlight build, in order, recording each result (pass, or the defect with a `blocker` record whose detail opens with `Failure:`) as a qualifier record on `docs/superpowers/specs/2026-10-18-ios-release-design.md:840-866`. Item 1 compares with `homeUsable` in `timings.jsonl`; item 10 runs on the oldest iPhone available.
- [ ] Record on `apps/clax_app/lib/platform/device/device_plugins.dart:21-245` (record c3f4359f's span) what the iOS run verified, as a comment on c3f4359f; propose its close to the owner if every port passed.

### B5: External TestFlight

- [ ] Draft the Beta App Review notes in `docs/usage.md`'s signing section (or wherever the owner asks): what Clax is; that it needs the person's own Anthropic key, with a review key through App Store Connect's field (the owner provides it); guidelines 2.5.2 and 4.7 as §8.1 reads them; and reminders as the one native capability behind a per-app grant (the owner's answer 4: kept).
- [ ] The owner submits the first external build for Beta App Review.

---

## Done

The branch is ready to merge when phases 1–7 are done: `just check` (background) passes, `"$Q" exec diff main` has been read, and both results and the list of Group B's open items are in the final report. Group B runs when the owner's inputs arrive, on this branch or after it merges.
