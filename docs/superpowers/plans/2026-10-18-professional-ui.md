# A Professional Clax App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Clax app (`apps/clax_app`) looks like one finished product with its own brand on the Android phone, Android wide screens and the Mac: one token source, an adaptive component layer with a Material family and an Apple family, native Mac chrome, Material 3 on every Android size, no prototype text, and the haikus and easter eggs given a deliberate place.

**Architecture:** `design/tokens.json` is the one source; `scripts/sync-tokens.py` writes the home app's, the kit's and Flutter's token files, and `//design:tokens_sync` keeps them and their contrast in step. Screens are built on `apps/clax_app/lib/ui/kit/`, which draws each component in one of two families chosen by platform (`ClaxFamily.material` on Android, `ClaxFamily.apple` on macOS now and iOS next), and lay themselves out by window size class (`ClaxWidth`), never by platform, so tablets, foldables, Chromebooks, the Mac and the iPad share one sidebar-and-detail structure. Mac window chrome, menus, alerts and context menus are AppKit (`apps/clax_app/macos/Runner/*.swift`) over method channels. In app mode the shell running in the app's WebView declares which overlays the host draws (`claxHost.init({native})`) and posts them as messages; Flutter draws them natively.

**Tech Stack:** Flutter 3.47.7 / Dart (`flutter_test`), Swift/AppKit (macOS Runner), Kotlin (Android runner), the shell's app-mode TypeScript and the bridge (vitest, Playwright), Preact + htm (home app, `home_unit` vitest, the inspect harness), Python 3 (generators and checks), Bazel through `bazelisk` (rules_flutter, rules_js, `sh_test`).

**Spec:** `docs/superpowers/specs/2026-10-18-professional-ui-design.md`. Section numbers below (§1.2, §5.4, …) are the spec's. Read the section a task names before starting it; this plan does not restate it. **Scope:** the Clax app only (see "Out of scope: browser-only" at the end); web code changes only where the app shows it: the home app (`apps/clax_app/home/`), the starter kit (`apps/clax_app/assets/kit/`), and the shell's app-mode pieces and the bridge, which run inside the app's WebView.

## Global Constraints

- **Commits are unsigned:** `git -c commit.gpgsign=false commit …`. `git add` only the paths your task lists (never `git add -A` or `.`). Never push. Never file, close or comment on issues or pull requests. If a git operation fails on signing or 1Password, stop and report.
- **Homes and daemons:** never run anything against `~/.clax` or the daemon on port 7480. Tests start their own daemons; a manual run uses `just dev` (`~/.clax-dev`) or a scratch `CLAX_HOME` on a free port, stopped by its PID. Never wait on processes with `pgrep -f`/`pkill -f`; use the PID you started or an exit file, with a timeout.
- **Deterministic tests:** no sleeps standing in for a condition, no wall-clock budgets. Every timing rule (the 500 ms arming, the 3 s pill fold, the 600 ms splash breath, the 8 s haiku, the 4 s Mac toast, the 400 ms loading mark) runs on an injected clock: a `Clock` (`lib/daemon/clock.dart`) or `DateTime Function() now` parameter with `tester.pump(duration)` in widget tests, `vi.useFakeTimers()` in vitest, `page.clock` in Playwright, `manualClock()` from `apps/clax_app/home/physics.js` in the home tests.
- **Prose:** write "ID" in prose, comments and commit messages; `id` only as a literal symbol in code. Doc comments and commit messages describe the contract or the change, never this plan, the conversation, or a rating of quality.
- **Qualifier** runs through `Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"; "$Q" exec <args>`. Read `"$Q" exec agents` and each topic it lists before your session's first record. Before editing a file, `"$Q" exec threads <path>` and read what is open on it. More than two writes go in one `"$Q" exec record --stdin` batch written to a file in the scratchpad (outside the repository), dry-run first. A `show <short-ID>` can answer "No records found" for a 10-character prefix; find the record with `grep -rln <short-ID> --include=.qual .` and use its full ID.
- **Closing records:** each task lists the records it resolves. After the task's fix commit, close each with evidence in a **separate** commit: `"$Q" exec resolve <ID> "Fixed: <test> passes (<bazel command>)" --reason fixed --ref git:<fix sha>`, `git add` the touched `.qual` files, and commit "Close the records that <change> fixed". Records this plan only touches (§11's owner decisions and out-of-scope table) are never closed; propose any other close in the task report. Spec IDs that are not qualifier records (the audit codes H1…, S1…, A1…, X1…, and 45f2d35d25, 9b71118ca5, which `grep` finds in no `.qual` file) are named in commit messages only.
- **Renders are looked at:** a UI task is done only once its renders have been opened with the Read tool (the PNG files themselves) and match the spec and the surfaces beside them. "Tests pass" is not verification for UI. Renders go under the worktree's `target/` (git-ignored).
- **Bazel only.** Build and test with `bazelisk build|test|run //…` (`docs/bazel.md`). No `cargo`, `pnpm`, `npm`, `flutter` or `dart` command in the worktree except through a `-native` twin (`just app-test-native`, `just app-integration-native`). Web unit tests: `bazelisk test //web:unit --test_arg=<path under web/>`. Flutter tests: `bazelisk test //apps/clax_app:<dir>_<file>` (`test/ui/foo_test.dart` is `//apps/clax_app:ui_foo_test`; `test/ui/shots/foo_shots_test.dart` is `//apps/clax_app:ui_shots_foo_shots_test`; a new directory under `test/` joins `SUITES` in `apps/clax_app/BUILD.bazel:126-147`). Anything longer than a couple of minutes runs in the background, logged to `target/plan-logs/<task>.log`, waited on in bounded stretches.
- **Whimsy is kept.** No task removes a haiku, Rally of 10, the home springs or the toss. Whimsy goes only where §7.4 places it: at most one per screen, never where the person decides, is warned or something failed, off in comment mode and under reduced motion.
- **The home bundle is a build output; nothing is packed by hand.** Edit `apps/clax_app/home/` and `apps/clax_app/assets/kit/`; `//apps/clax_app:home_bundle` packs and versions them at build time (`docs/bazel.md`, "The home bundle's version"). Never run `pack_home` or `scripts/build-home-bundle.sh`, and never edit `apps/clax_app/assets/home/`. Task 4 raises `"release"` in `apps/clax_app/home/manifest.json` to `"1.6"`, the one raise in this plan.
- **Generated files are never edited by hand:** `apps/clax_app/home/tokens.css`, `apps/clax_app/home/mark.css`, the `/* tokens:begin */…/* tokens:end */` region of `apps/clax_app/assets/kit/kit.css`, `apps/clax_app/lib/ui/tokens.g.dart` and `apps/clax_app/assets/haiku.json` (`python3 scripts/sync-tokens.py`); the prompt copies under `apps/clax_app/assets/prompt/` (`sh scripts/sync-app-prompt.sh`); the icons (`scripts/build-app-icons.sh`). Change the source and rerun.
- **Contract changes** (new host messages, the home capability's `chrome()`) change `docs/contract.md`, `web/contract/clax-extensions.d.ts`, the kit's fake host (`apps/clax_app/assets/kit/fake-home.js`) and the prompt copies (checked by `//apps/clax_app:prompt_sync`) in the same task.
- **Two families, not two platforms.** Components branch on `ClaxFamily.of(context)` (`apple` for `TargetPlatform.macOS` and `TargetPlatform.iOS`, `material` otherwise), never on `Platform.isMacOS` or `TargetPlatform.macOS` directly. Layout branches on `ClaxWidth` (window size class), never on platform. Only window chrome that exists on one OS (the NSToolbar, the menu bar, `NSAlert`, Settings and Inbox windows) checks `defaultTargetPlatform == TargetPlatform.macOS`, through one getter, `hasDesktopWindowChrome` in `lib/ui/kit/family.dart`.
- **macOS native files:** a new `apps/clax_app/macos/Runner/*.swift` needs no BUILD change (rules_flutter globs `macos/Runner/*.swift`) but must be added to `apps/clax_app/macos/Runner.xcodeproj/project.pbxproj` (PBXFileReference, PBXBuildFile, the Runner group and the Sources phase, as `MainFlutterWindow.swift` is at lines 29, 73, 170, 377) so the native twin compiles it. Test-only native code is inside `#if DEBUG || PROFILE`.
- **Copy** follows §7 (’ “ ” …, sentence case, "Clax", "app", "checks"). From Task 25 on, `//scripts:check_copy` enforces it.
- No `unsafe`, no hard links, anywhere.
- Before the branch merges, `just check` runs once (in the background) and `"$Q" exec diff main` is read; both results go in the final report.

## Decisions this plan takes where the code differs from the spec

Each is the closest working reading of the spec; the owner may override any.

1. **Multiple windows on the Mac (§4.7).** Flutter 3.47.7's windowing API (`RegularWindowController` in `packages/flutter/lib/src/widgets/_window.dart`) is `@internal`, not exported from `widgets.dart`, and throws unless `flutter config --enable-windowing` set the `windowing` feature flag (`foundation/_features.dart:23`), with breaking changes allowed in patch releases. The plan takes the spec's fallback as the design: one `FlutterEngine`, a further `FlutterViewController` per window made in `AppDelegate.swift`, and Dart's public multi-view API (`runWidget`, `View`, `PlatformDispatcher.instance.views`). Task 41's spike confirms it first; if it fails, Settings and Inbox open as window sheets on the main window and the spike's qualifier record says so.
2. **`layout/main` writes from Flutter (§4.3).** The page's writer (`apps/clax_app/home/store.js:26`) takes a 2 s lease and writes last-writer-wins (waiver b1d1c9e3); a page cannot pin a version. Flutter writes as the owner through `ClaxApi.batchDocs` with `DocWrite(op: 'set', collection: 'layout', docId: 'main', ifVersion: <version read>)` (`lib/clax/models.dart:451-474`): pinned, as the spec says; a 409 re-reads and retries once. That a page write inside its own lease can still overwrite a Flutter write is recorded as a waiver on `lib/home/home_layout.dart` in Task 45.
3. **The web renders (§9).** `web/e2e/zz-ui-audit.spec.ts` is not in the repository, and `web/e2e/shots.spec.ts` is the browser shell's (out of scope). Task 2 adds `web/e2e/app-shots.spec.ts` for the app's web pieces only (the home app, the kit example, and app mode with the fake host), in a `shots` Playwright project left out of `rest`, run by the `manual` target `//web:e2e_shots`. `shots.spec.ts` is left as it is.
4. **Renders under Bazel.** The Flutter rig and `app-shots.spec.ts` write to the directory `CLAX_SHOTS_DIR` names, passed as `--test_env=CLAX_SHOTS_DIR=$PWD/target/flutter-app/shots --sandbox_writable_path=$PWD/target/flutter-app/shots` (the pattern `just perf` uses for `CLAX_PERF_JSON`); without it every shots test skips.
5. **No shell token file.** App mode's own colours (pins, the comment frame, the picking overlay) are the shell's `theme.css` values, which are §1.2's values already; the browser shell's theming is out of scope. `sync-tokens.py` writes no shell file; `//design:tokens_sync` instead checks that `web/shell/src/theme.css`'s `--you`, `--agent`, `--pin`, `--on-pin`, `--pin-ring` and `--comment-hl` equal the source, so app mode cannot drift from the app.
6. **The Android splash (§5.11).** `androidx.core:core-splashscreen` is not pinned and `flutter_fat_apk` (`bazel/flutter/android.bzl:88`, runner at `:203-216`) gives the runner no Maven dependencies. Task 10 pins it in `MODULE.bazel`'s `maven.install` (`:208-233`; repin with `REPIN=1 bazelisk run @unpinned_rules_android_maven//:pin`), adds a `runner_deps` attribute to `flutter_fat_apk`, and adds the dependency to `android/app/build.gradle.kts:64` for the native twin.
7. **`ClaxSplash` (§5.11)** is today the private `_Splash` (`lib/app.dart:2445-2473`); Task 11 replaces it with a public `ClaxSplash` in `lib/ui/splash.dart`.
8. **Prompt kinds and "always" (§6.2).** The shell's `Prompt` (`web/shell/src/caps/grants.ts:10-11`) has no `kind` and no "always" answer. Task 35 adds `kind` at each enqueue site (`grants.ts:104-126`, `caps/sample.ts:53`, `caps/downloads.ts:109`, `caps/net.ts:88`, `caps/cross-read.ts:102`) and an optional `always` label where a grant persists; `askAnswer("always")` answers `allow` and stores the grant as each path does today.
9. **The home capability's `chrome()` arrives in phase 3** (Task 19), not phase 6: §7.5's "Tap again to open" needs its pointer before the Mac exists. It answers `toolbar: false` until Task 47.
10. **The agent's name in the app (§7.2).** The daemon has no session display name: `RegisterSession` (`crates/clax-core/src/model.rs:316-340`) derives "app", then "app-2" (`crates/clax-core/src/names.rs:88-104`). Task 16 has the worker rename its session to "Clax" with the existing `PATCH /api/sessions/{id}` (`crates/clax-server/src/routes/sessions.rs:238-300`), keeping the derived name on `409 name_taken`, and the shell's app-mode labels name the `app` harness "Clax" whatever its session name. No Rust changes.
11. **`keepGoingHint` is matched as text** (`lib/agent/failure.dart:63`, read by `offersKeepGoing` at `:265`): Task 17 changes the copy through `ClaxVerbs` and keeps `offersKeepGoing` matching both the old and new sentences, since stored replies keep the old one.
12. **The macOS product name.** `app_name = "clax_app"` (`apps/clax_app/BUILD.bazel:349`) also names the bundle, which `tool/macos-app-check.sh:16-17` and `tool/integration-run.sh:11` expect; Task 12 changes all three to `Clax`. `tool/apk-check.sh:20` expects `label='' icon=''` on the launchable activity; Task 9 updates it to the adaptive icon.

## Review Focus

1. **An older app, or an older home bundle, next to a newer one.** A host that sends no `native` in `init` keeps every web overlay; a home release from before `chrome()` runs under the new app, and the new home app under an app that refuses `chrome()` as `unavailable`. Test: Task 35 (vitest "without native the shell draws every overlay"), Task 19 (home harness "chrome() unavailable keeps the phone layout").
2. **A permission prompt answered by a press that began before it armed** (a held Return, a double-tap that started on the page). Allow must not count; a press after arming must. Test: Task 36 (injected clock: pointer down at 499 ms, up at 520 ms does nothing; key down at 501 ms counts; Escape answers `dismiss`).
3. **Text at 200% and long unbroken titles** (a German compound, a URL as an app title). Tile labels stay two lines and ellipsize; the pill and the Comment tab never clip; sheets keep their buttons reachable. Test: Task 20 (`tileLabel` and `TileLabel` with a 40-character word), Tasks 28 and 29 (pill and tab at `TextScaler.linear(2)`), and the 130%/200% renders in Tasks 14, 32 and 37.
4. **Pages Clax does not style.** The status band painted in a page's colour must pick icon brightness by luminance, and a transparent or missing background must fall back to the body's colour, then `bg`. Test: Task 26 (`ChromeMsg` refuses non-`#rrggbb`; the bridge's sampler on `background: transparent`; `statusStyleFor` on #ffffff, #000000, #777777).
5. **`layout/main` changed by the home app while the sidebar drags.** No star, order or group may be lost, and keys the sidebar does not know survive. Test: Task 45 (`HomeLayout` with injected versions: a 409 re-reads and re-applies once; unknown keys and `opens` survive a reorder; a missing or unparsable document lists apps newest first and writes nothing).

## Order and parallel groups

Tasks in one group touch disjoint files and may run at the same time; groups run in order. A task never starts before the tasks its **Consumes** names.

| Phase | Groups (tasks in a group can run in parallel) |
|---|---|
| 1 Brand tokens | **1a** T1 · **1b** T2, T3, T4 · **1c** T5 · **1d** T6, T7 |
| 2 Identity | **2a** T8 · **2b** T11, T12 · **2c** T9 · **2d** T10 |
| 3 Clean-up and copy | **3a** T13, T15, T16, T20 · **3b** T14, T17, T18, T19 · **3c** T21, T22, T23, T24 · **3d** T25 alone (it touches every UI file) |
| 4 Android phone | **4a** T26, T32, T33 · **4b** T27, T31 · **4c** T28 · **4d** T29 · **4e** T30 |
| 5 Native overlays | **5a** T34, T35 · **5b** T36, T40 · **5c** T37, T39 · **5d** T38 |
| 6 The Mac and the split layout | **6a** T41 (spike) · **6b** T42, T45 · **6c** T43 · **6d** T44 · **6e** T46 · **6f** T47, T48, T49 · **6g** T50 · **6h** T51 · **6i** T52 |
| 7 Android wide | **7a** T53 · **7b** T54, T55 · **7c** T56 |

Shared files never share a group: `lib/ui/app_screen.dart` (T5, T15, T21, T26–T30, T36–T38, T42, T48, T54), `lib/app.dart` (T5, T11, T13, T19, T37, T43, T46, T49, T53, T56), `lib/ui/split/split_shell.dart` (T46, T47, T50, T51, T53, T55, T56), `macos/Runner.xcodeproj/project.pbxproj` (T12, T39, T43, T44, T52), `MainMenu.xib` (T12, T42, T43), `apps/clax_app/BUILD.bazel` (T1, T9, T10, T12, T25), `home/home.js` (T19, T20, T21, T33, T47). Phase 3 runs in four groups because Task 25 rewrites strings in nearly every UI file.

## Commands used throughout

```bash
Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"
SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
# A Flutter shots test, writing PNGs into $SHOTS:
bazelisk test //apps/clax_app:ui_shots_<surface>_shots_test --nocache_test_results \
  --test_env=CLAX_SHOTS_DIR="$SHOTS" --sandbox_writable_path="$SHOTS"
# The app's web pieces (Task 2), writing PNGs into $SHOTS:
bazelisk test //web:e2e_shots --nocache_test_results --test_env=CLAX_SHOTS_DIR="$SHOTS" \
  --sandbox_writable_path="$SHOTS" --test_arg=--grep --test_arg='<scene>'
# Every Dart task, before its commit:
bazelisk test //apps/clax_app:dart_analyze
```

PNG names are `<surface>-<form>-<theme>.png`, form `phone`, `wide` or `mac`, theme `light` or `dark` (§9); text-scale renders add `-130` or `-200`.

---

## Phase 1: Brand tokens (§1, §2, §10.1)

### Task 1: The token source, its generator and the sync check

**Files:**
- Create: `design/tokens.json`, `design/BUILD.bazel`, `design/tokens-sync.sh`, `scripts/sync-tokens.py`, `scripts/test-sync-tokens.sh`
- Generate (committed): `apps/clax_app/home/tokens.css`, `apps/clax_app/lib/ui/tokens.g.dart`, `apps/clax_app/assets/haiku.json`
- Modify: `scripts/BUILD.bazel` (export the script, `sh_test(name = "sync_tokens")`), `apps/clax_app/BUILD.bazel` (`filegroup(name = "token_outputs")`; `assets/haiku.json` in `APP_ASSETS` at `:226-234` and in the suites' `assets` at `:154-160`), `apps/clax_app/pubspec.yaml:35-40` (`- assets/haiku.json`), `web/BUILD.bazel` (`filegroup(name = "token_inputs", srcs = ["shell/src/view/haiku.json", "shell/src/theme.css"])`, visible to `//design:__pkg__` and `//scripts:__pkg__`)

**Interfaces:**
- Produces `design/tokens.json`, keys: `color` (`{name: {light, dark}}`, every row of §1.2, names as written there; `focus` = `agent`, `agent` = `accent`), `contrastMore` (§8: `border` takes `border-strong`), `surfaceContainers` (§1.2's two five-step ramps), `scheme` (`onError` and `inversePrimary`, §1.2), `type` (`{role: {material: [size, line, weight], apple: {macos: [size, line, weight]}, family?: "serif"|"mono"}}`; §1.3's "Web and Android" column is `material`, the macOS column `apple.macos`; the iOS spec adds `apple.ios`), `space`, `layout`, `radius`, `elevation`, `motion` (§1.4–§1.7), `contrast` (`[{fg, bg, min, themes: ["light","dark"], note}]`: the pairs `web/shell/src/theme.css:1-17,37-42` lists, plus §8's three: `muted` on `comment-hl`, `muted` on `hover`, `bg` on `fg`), `kit` (`{}` until Task 3), `appMode` (`["you", "agent", "pin", "on-pin", "pin-ring", "comment-hl"]`, Decision 5).
- Produces `python3 scripts/sync-tokens.py [--check]`: writes the outputs and copies `web/shell/src/view/haiku.json` to `apps/clax_app/assets/haiku.json`; `--check` writes nothing and exits 1 printing a unified diff per stale file; both modes exit 1 on a contrast pair below its `min` (WCAG 2.x, alpha composited on the pair's `bg`) printing `contrast: <fg> on <bg> (<theme>) is <ratio:.2f>, below <min>`, and on an `appMode` token whose `theme.css` value differs, printing `theme.css: --<name> is <value>, design/tokens.json says <value>`. The root resolves as `scripts/sync-app-prompt.sh:17-22` does. Standard library only; output deterministic (sorted keys, `\n`, fixed number format).
- Produces `apps/clax_app/home/tokens.css`: `--<name>` in `:root`, the dark set in `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { … } }` (the nesting `home.css:35-57` uses) and `:root[data-theme="dark"]`, then `@media (prefers-contrast: more)`; `--type-<role>`, `--space-<n>`, `--radius-*`, `--elev-1..3`, `--t-fast`, `--t`, `--t-enter`, `--t-exit`, `--font`, `--mono`. First line `/* Generated by scripts/sync-tokens.py from design/tokens.json. Do not edit. */`.
- Produces `apps/clax_app/lib/ui/tokens.g.dart` (first line `// Generated by scripts/sync-tokens.py from design/tokens.json. Do not edit.`):
  ```dart
  final class ClaxPalette {
    const ClaxPalette({required this.bg, required this.card, required this.raised, required this.overlay, required this.hover, required this.fg, required this.muted, required this.border, required this.borderHover, required this.borderStrong, required this.agent, required this.agentInk, required this.accentHover, required this.onAccent, required this.accentTint, required this.you, required this.youInk, required this.onYou, required this.commentHl, required this.selectionBg, required this.danger, required this.dangerTint, required this.primaryHover, required this.scrim, required this.pin, required this.onPin, required this.pinRing, required this.onError, required this.inversePrimary, required this.surfaceContainers});
    final Color bg, card, raised, overlay, hover, fg, muted, border, borderHover, borderStrong, agent, agentInk, accentHover, onAccent, accentTint, you, youInk, onYou, commentHl, selectionBg, danger, dangerTint, primaryHover, scrim, pin, onPin, pinRing, onError, inversePrimary;
    final List<Color> surfaceContainers; // lowest … highest
  }
  abstract final class ClaxTokens { static const ClaxPalette light = …, dark = …, lightMore = …, darkMore = …; }
  enum ClaxRole { largeTitle, title1, title2, title3, headline, body, bodyL, callout, caption, mono, haiku }
  abstract final class ClaxType {
    /// [role] on [platform]: the material column, or apple's entry for the platform (macos; ios once the iOS spec adds it, else macos).
    static TextStyle of(ClaxRole role, TargetPlatform platform);
    static TextStyle mono(TargetPlatform platform); // `ui-monospace` families: '.AppleSystemUIFontMonospaced', 'Menlo' on Apple; 'monospace' on Android
  }
  abstract final class ClaxSpace { static const double s0 = 0, s2 = 2, s4 = 4, s8 = 8, s12 = 12, s16 = 16, s20 = 20, s24 = 24, s32 = 32, s48 = 48, s64 = 64; }
  abstract final class ClaxLayout { static const double gutterCompact = 16, gutterWide = 24, reading = 720, form = 640, grid = 960, sheet = 560, appleSheetMin = 480, appleSheetMax = 640, medium = 600, expanded = 840, large = 1200, sidebar = 220, sidebarMin = 180, sidebarMax = 320, threadsPane = 320, threadsPaneMin = 280, threadsPaneMax = 420, listPane = 360, rail = 80, target = 48, appleTargetMin = 24; }
  abstract final class ClaxRadius { static const double xs = 4, sm = 8, md = 10, lg = 16, xl = 28, full = 999, controlApple = 6; static double tile(double size) => size * 0.25; }
  abstract final class ClaxElevation { static List<BoxShadow> shadows(int level, Brightness b); static BoxDecoration of(int level, Brightness b, ClaxPalette p, {Color? color, BorderRadius? radius}); /* dark: one surface step lighter and a 1 px border (level 1) or border-hover (2, 3) */ }
  abstract final class ClaxMotion { static const fast = Duration(milliseconds: 120), base = Duration(milliseconds: 200), enter = Duration(milliseconds: 300), exit = Duration(milliseconds: 200), markBreath = Duration(milliseconds: 1600); static const Curve standard = Cubic(.4, 0, .2, 1), decelerate = Cubic(.05, .7, .1, 1), accelerate = Cubic(.3, 0, .8, .15); static Duration of(BuildContext context, Duration d); /* 120 ms when MediaQuery.disableAnimationsOf(context) and d > 120 ms */ }
  ```
- Consumes: nothing.

- [ ] **Step 1: Read the threads** — `for p in web/shell/src/view/haiku.json apps/clax_app/lib/ui/theme.dart apps/clax_app/home/home.css scripts/BUILD.bazel apps/clax_app/BUILD.bazel; do "$Q" exec threads "$p"; done`.
- [ ] **Step 2: Write the generator's test** `scripts/test-sync-tokens.sh` (bash, `set -uo pipefail`, shaped like `scripts/test-check-crate-layers.sh`): copy `design/`, `scripts/sync-tokens.py`, `web/shell/src/view/haiku.json`, `web/shell/src/theme.css` and the outputs into a `mktemp -d` tree, run the copy's script there. Cases, each `_fail`ing with a message:
  1. `--check` exits 0 on the copy as committed;
  2. after `sed -i '' 's/#f7f7f5/#f7f7f6/' design/tokens.json`, it exits 1 naming `apps/clax_app/home/tokens.css` and `apps/clax_app/lib/ui/tokens.g.dart`;
  3. with `color.muted.light` set to `#b0aea8`, both modes exit 1 and print `muted on card (light)` and a ratio below `4.5`;
  4. a line appended to `apps/clax_app/assets/haiku.json` makes `--check` exit 1 naming it;
  5. `--you: #e0532e` in the copy's `theme.css` makes `--check` exit 1 printing `theme.css: --you`;
  6. after a plain run, `--check` exits 0 and a second plain run changes no byte (`cmp` each output).
- [ ] **Step 3: Register the tests** and see them fail. `scripts/BUILD.bazel`, beside `crate_layers`:
  ```python
  exports_files(["sync-tokens.py"], visibility = ["//design:__pkg__"])

  # scripts/sync-tokens.py on a copy of its inputs: in step, stale, low contrast, drifted app-mode colours.
  sh_test(
      name = "sync_tokens",
      srcs = ["test-sync-tokens.sh"],
      data = [":all", "//design:tokens", "//apps/clax_app:token_outputs", "//web:token_inputs"],
  )
  ```
  `design/BUILD.bazel`:
  ```python
  load("@rules_shell//shell:sh_test.bzl", "sh_test")

  filegroup(name = "tokens", srcs = ["tokens.json"], visibility = ["//scripts:__pkg__"])

  # The generated token files and the haiku copy are what design/tokens.json
  # writes, app mode's colours match it, and every listed pair meets its
  # contrast (scripts/sync-tokens.py --check).
  sh_test(
      name = "tokens_sync",
      srcs = ["tokens-sync.sh"],
      data = [":tokens", "//scripts:sync-tokens.py", "//apps/clax_app:token_outputs", "//web:token_inputs"],
  )
  ```
  (Task 8 adds `mark.json` to `:tokens`.) `design/tokens-sync.sh`: `exec python3 "$TEST_SRCDIR/$TEST_WORKSPACE/scripts/sync-tokens.py" --check`. `apps/clax_app/BUILD.bazel`: `filegroup(name = "token_outputs", srcs = ["home/tokens.css", "lib/ui/tokens.g.dart", "assets/haiku.json", "assets/kit/kit.css"], visibility = ["//design:__pkg__", "//scripts:__pkg__"])`.
  Run: `bazelisk test //scripts:sync_tokens //design:tokens_sync` — Expected: FAIL (no script, no outputs).
- [ ] **Step 4: Write `design/tokens.json`** from §1.2–§1.7 exactly, with the `contrast` list above (`fg`, `muted`, `agent-ink`, `you-ink`, `danger` on `bg`, `card`, `hover` at 4.5; `on-accent` on `agent`, `on-you` on `you`, `bg` on `fg` at 4.5; `border-strong` and `focus` on `bg` and `card` at 3.0; §8's pairs).
- [ ] **Step 5: Write `scripts/sync-tokens.py`**: one function per output (`home_css`, `dart`, the haiku copy, `check_app_mode`; Task 3 adds `kit_region`, Task 8 `mark_outputs`), each returning text; `main` compares or writes. Colours in Dart as `Color(0xAARRGGBB)`.
- [ ] **Step 6: Generate and test.** `python3 scripts/sync-tokens.py && bazelisk test //scripts:sync_tokens //design:tokens_sync`. Expected: PASS. Check three values in `tokens.g.dart` against §1.2 by eye (`bg` light, `you` dark, `surfaceContainers` dark).
- [ ] **Step 7: Bundle the haiku asset** and build: `bazelisk build //apps/clax_app:app && bazelisk test //apps/clax_app:dart_analyze`. Expected: PASS.
- [ ] **Step 8: Commit.**
  ```bash
  git add design/ scripts/sync-tokens.py scripts/test-sync-tokens.sh scripts/BUILD.bazel web/BUILD.bazel \
    apps/clax_app/home/tokens.css apps/clax_app/lib/ui/tokens.g.dart apps/clax_app/assets/haiku.json \
    apps/clax_app/pubspec.yaml apps/clax_app/BUILD.bazel
  git -c commit.gpgsign=false commit -m "Generate the brand's tokens for the home app and Flutter from design/tokens.json, with a sync and contrast check"
  ```

### Task 2: The render rigs

**Files:**
- Create: `apps/clax_app/test/ui/shots/rig.dart` (from the git-ignored `apps/clax_app/target/ui_audit/audit.dart`, 198 lines: its `Form`, `SystemBars`, `Audit.app/settle/shot` and `audit()`), `apps/clax_app/test/ui/shots/rig_shots_test.dart`, `web/e2e/app-shots.spec.ts`
- Modify: `web/playwright.config.ts:29-33` (`{ name: "shots", testMatch: /app-shots\.spec\.ts$/ }`; `rest`'s `testIgnore` gains `app-shots`), `web/BUILD.bazel:254-276` (the `e2e_` comprehension gains `"shots"`, and that target is tagged `manual`)

**Interfaces:**
- Produces (`test/ui/shots/rig.dart`):
  ```dart
  class ShotForm { const ShotForm(this.name, this.size, this.dpr, this.platform, {this.top = 0, this.bottom = 0, this.toolbar = 0}); final String name; final Size size; final double dpr; final TargetPlatform platform; final double top, bottom, toolbar; }
  const phone = ShotForm('phone', Size(412, 915), 2.625, TargetPlatform.android, top: 24, bottom: 16);
  const wide = ShotForm('wide', Size(1280, 800), 2, TargetPlatform.android, top: 24, bottom: 16);
  const mac = ShotForm('mac', Size(1200, 800), 2, TargetPlatform.macOS, toolbar: 52);
  const allForms = [phone, wide, mac];
  String? get shotsDir; // CLAX_SHOTS_DIR, or null
  class Shot {
    Shot(this.tester, this.form, this.dark, this.textScale);
    final WidgetTester tester; final ShotForm form; final bool dark; final double textScale;
    Widget app(Widget home, {bool chrome = true}); // MaterialApp with claxTheme, SystemBars on Android forms, a 52 pt MacToolbarBand stand-in (traffic lights, title) on mac
    Future<void> settle([int frames = 8]);
    Future<void> take(String surface, {Finder? of}); // <surface>-<form>-<theme>[-<scale×100>].png in shotsDir
  }
  /// One testWidgets per form × theme × text scale; skipped when shotsDir is null.
  void shots(String surface, Future<void> Function(Shot s) body, {List<ShotForm> forms = allForms, List<double> textScales = const [1]});
  ```
  `take` uses `matchesGoldenFile` with `autoUpdateGoldenFiles = true` and `goldenFileComparator = LocalFileComparator(Uri.file('$shotsDir/x.png'))`. Fonts as `audit.dart:20-46` loads them: Roboto and MaterialIcons from `$FLUTTER_ROOT/bin/cache/artifacts/material_fonts`, SF Pro (`/System/Library/Fonts/SFNS.ttf`) as `.AppleSystemUIFont`, New York (`/System/Library/Fonts/NewYork.ttf`, `NewYorkItalic.ttf`), Menlo.
- Produces `//web:e2e_shots` (manual): `app-shots.spec.ts` renders the home app (served as `web/e2e/home-app.spec.ts` serves it, with `fake-home.js` data), the kit example (`apps/clax_app/assets/kit/example/`), and an artifact in app mode with the fake host (`web/e2e/app-host.ts`: `initHost`) in comment mode, at `phone` (412×915, `isMobile`, `hasTouch`, DPR 2.625), `wide` (1280×800, `hasTouch`) and `mac` (1200×800, DPR 2), light and dark, reduced motion, into `$CLAX_SHOTS_DIR/<scene>-<form>-<theme>.png`; it fails if a page scrolls sideways; skipped without `CLAX_SHOTS_DIR`.
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on `web/playwright.config.ts`, `web/BUILD.bazel`, `web/e2e/home-app.spec.ts`.
- [ ] **Step 2: Write `rig_shots_test.dart`:** `shots('rig', (s) async { await s.tester.pumpWidget(s.app(const Scaffold(body: Center(child: Text('Clax'))))); await s.settle(); await s.take('rig'); });` and an always-run `test('shotsDir is null without CLAX_SHOTS_DIR, and the forms are §9's', …)` checking the three sizes and DPRs.
- [ ] **Step 3: Run it twice:** `bazelisk test //apps/clax_app:ui_shots_rig_shots_test` (PASS, shots skipped), then with the variable (Commands above). Expected: six PNGs `rig-{phone,wide,mac}-{light,dark}.png`. Read `rig-phone-light.png` and `rig-mac-dark.png`: status and gesture bars on the phone, the toolbar band on the Mac.
- [ ] **Step 4: Write `app-shots.spec.ts`** and the config and BUILD changes; run `bazelisk test //web:e2e_shots …--test_arg=--grep --test_arg=home`. Expected: six `home-*.png`; read the phone dark one. Then `bazelisk test //web:lint //web:typecheck` — PASS — and `bazelisk query 'attr(tags, "\bmanual\b", //web:e2e_shots)'` prints the target (so `just ci` never runs it).
- [ ] **Step 5: Commit** `apps/clax_app/test/ui/shots/`, `web/e2e/app-shots.spec.ts`, `web/playwright.config.ts`, `web/BUILD.bazel`: "Render the app's surfaces at phone, Android wide and Mac sizes, light and dark, on demand".

### Task 3: The kit reads the brand

**Files:**
- Modify: `scripts/sync-tokens.py` (`kit_region`), `scripts/test-sync-tokens.sh` (two cases), `design/tokens.json` (`kit`), `apps/clax_app/assets/kit/kit.css:10-140` (the mapped `--k-*` declarations move between `/* tokens:begin */` and `/* tokens:end */`; every `color-mix()` at `:17, :87-89, :119-121, :306-307, :335, :466-468, :497, :553` gets a plain fallback declaration before it)
- Test: `web/e2e/home-app.spec.ts`'s kit cases, `//web:unit`

**Interfaces:**
- Produces `design/tokens.json` `kit`: `{"--k-bg": "bg", "--k-surface": "card", "--k-surface-2": "hover", "--k-surface-3": "raised", "--k-bar": "card", "--k-fg": "fg", "--k-fg-2": "muted", "--k-fg-3": "border-strong", "--k-line": "border", "--k-line-strong": "border-strong", "--k-accent": "agent", "--k-on-accent": "on-accent", "--k-accent-soft": "accent-tint", "--k-danger": "danger", "--k-scrim": "scrim", "--k-radius": "radius", "--k-radius-sm": "radius-sm", "--k-radius-lg": "radius-lg", "--k-radius-full": "radius-full", "--k-fast": "t-fast", "--k-med": "t", "--k-ease": "standard", "--k-font": "font", "--k-font-mono": "mono"}` plus `--k-text-<kit name>` for each type role. Kit tokens with no brand counterpart (`--k-accent-2`, `--k-hero-*`, `--k-chart-1..6`, `--k-success`, `--k-warning`, `--k-s1..6`, `--k-touch`, `--k-safe-*`, `--k-ease-spring`, `--k-bar-h`, `--k-tab-h`) stay outside the markers, hand-kept. Apps still override `--k-accent` (Spec decision 3); apps made earlier keep their copy of the kit until their next change.
- Consumes: Task 1.

- [ ] **Step 1: Read the threads** on `apps/clax_app/assets/kit/kit.css` (2f7a336048: `color-mix` in older Android WebViews).
- [ ] **Step 2: Failing cases** in `test-sync-tokens.sh`: a value edited between the markers makes `--check` exit 1 naming `apps/clax_app/assets/kit/kit.css`; a missing `/* tokens:end */` exits 1 printing `kit.css: tokens:end marker missing`. Run `bazelisk test //scripts:sync_tokens` — FAIL.
- [ ] **Step 3: Implement** `kit_region` (it rewrites only the text between the markers: `:root`, both dark blocks, `prefers-contrast: more`), add the markers and map, delete the duplicated declarations outside, add the fallbacks. `python3 scripts/sync-tokens.py`.
- [ ] **Step 4: Run** `bazelisk test //scripts:sync_tokens //design:tokens_sync //web:unit //web:e2e_rest --test_arg=--grep --test_arg=kit`. Expected: PASS.
- [ ] **Step 5: Look** at the kit example (`//web:e2e_shots`, scene `kit`), phone light and dark: warm neutrals, green accent, nothing slate blue.
- [ ] **Step 6: Commit** the listed files: "Give the starter kit the brand's neutrals, type, radii and motion, with plain fallbacks for every colour mix". Close 2f7a336048 separately, with the e2e run as evidence.

### Task 4: The home app reads the tokens

**Files:**
- Modify: `apps/clax_app/home/index.html:7-8` (link `./tokens.css` first), `apps/clax_app/home/home.css:1-78` (the `--h-*` hexes become aliases: `--h-surface: var(--bg)`, `--h-on-surface: var(--fg)`, `--h-on-variant: var(--muted)`, `--h-primary: var(--agent)`, `--h-pc: var(--accent-tint)`, `--h-on-pc: var(--agent-ink)`, `--h-ec: var(--danger-tint)`, `--h-on-ec: var(--danger)`, `--h-error: var(--danger)`, `--h-on-error: var(--on-accent)`, `--h-sc: var(--hover)`, `--h-on-sc: var(--fg)`, `--h-track: var(--border)`, `--h-menu-bg: var(--overlay)`, `--h-inverse: var(--fg)`, `--h-on-inverse: var(--bg)`, `--h-state: var(--hover)`, the shadows from `--elev-*`, each commented `alias for release 1.6; removed in 1.7`; both dark blocks `:35-57, :59-78` go except `.h-face`'s tone swap; `:root { accent-color: var(--accent) }`; `::selection { background: var(--selection-bg); color: var(--fg) }` (H6); `@media (pointer: coarse) { button { min-height: 44px; min-width: 44px } }` (X3)), `apps/clax_app/home/manifest.json` (`"release": "1.6"`)
- Test: `apps/clax_app/home/tests/home.test.js` (one test), `web/e2e/home-app.spec.ts:56` (the count: `grep -c "^test(" apps/clax_app/home/tests/home.test.js`)

**Interfaces:**
- Consumes: Task 1 (`home/tokens.css`).

- [ ] **Step 1: Read the threads** on `apps/clax_app/home/` (nine open; none blocks this).
- [ ] **Step 2: Failing harness test:** `test("the home screen takes the brand's colours", …)` — `getComputedStyle(document.documentElement).getPropertyValue("--h-primary")` resolves (through a probe element's `color: var(--h-primary)`) to `rgb(47, 111, 42)`, and `accent-color` is set on `:root`. Bump the count.
- [ ] **Step 3: Run** `bazelisk test //web:e2e_rest --test_arg=--grep --test_arg='home app'` — FAIL; implement; rerun — PASS; `bazelisk test //apps/clax_app:home_home_bundle_test` — PASS.
- [ ] **Step 4: Look** at `//web:e2e_shots` scene `home` in all six forms: warm page, green New app container, both themes.
- [ ] **Step 5: Commit**: "Draw the home screen in the brand's colours from the generated tokens".

### Task 5: Flutter's theme, families and size classes

**Files:**
- Modify: `apps/clax_app/lib/ui/theme.dart:1-35` (rewrite), `apps/clax_app/lib/app.dart:2271-2272`, `apps/clax_app/test/ui/shots/rig.dart` (`Shot.app` passes `form.platform`), `apps/clax_app/lib/ui/version_page.dart:522,577,690,756` (`ClaxType.mono(Theme.of(context).platform)`) and `:593-625` (`DiffColors` from the palette: `agent` and `danger` at 14% over `card`; the seed `0xFF2E7D32` goes)
- Modify (breakpoints, §1.4): `lib/ui/bundle_sheets.dart:31,41`, `lib/ui/question_sheet.dart:12,114`, `lib/ui/change_sheet.dart:15,48` (each constant goes; `ClaxWidth.of(context) >= ClaxWidth.medium`), `lib/ui/app_screen.dart:1766` (`ClaxWidth.fromWidth(box.maxWidth) >= ClaxWidth.expanded` replaces `>= 720`)
- Create: `apps/clax_app/lib/ui/kit/family.dart`, `apps/clax_app/lib/ui/layout.dart`, `apps/clax_app/test/ui/theme_test.dart`, `apps/clax_app/test/ui/layout_test.dart`, `apps/clax_app/test/ui/shots/screens_shots_test.dart`

**Interfaces:**
- Produces (`lib/ui/kit/family.dart`):
  ```dart
  enum ClaxFamily { material, apple;
    static ClaxFamily forPlatform(TargetPlatform p) => p == TargetPlatform.macOS || p == TargetPlatform.iOS ? apple : material;
    static ClaxFamily of(BuildContext context) => forPlatform(Theme.of(context).platform);
  }
  /// The Mac's own window chrome (NSToolbar, menu bar, NSAlert sheets, extra windows): macOS only, never iOS.
  bool get hasDesktopWindowChrome => defaultTargetPlatform == TargetPlatform.macOS;
  ```
- Produces (`lib/ui/layout.dart`):
  ```dart
  enum ClaxWidth { compact, medium, expanded, large;
    bool operator >=(ClaxWidth other) => index >= other.index;
    bool operator <(ClaxWidth other) => index < other.index;
    static ClaxWidth fromWidth(double w) => w < 600 ? compact : w < 840 ? medium : w < 1200 ? expanded : large;
    static ClaxWidth of(BuildContext context) => fromWidth(MediaQuery.sizeOf(context).width);
  }
  abstract final class ClaxBreakpoints { static double gutter(BuildContext context); /* 16 at compact, 24 otherwise */ }
  ```
- Produces (`lib/ui/theme.dart`):
  ```dart
  ThemeData claxTheme(Brightness brightness, TargetPlatform platform, {bool highContrast = false});
  final class ClaxColors extends ThemeExtension<ClaxColors> { const ClaxColors(this.palette); final ClaxPalette palette; /* copyWith, lerp */ }
  ClaxPalette claxPalette(BuildContext context);
  TextTheme claxTextTheme(TargetPlatform platform, ClaxPalette p); // §1.3's mapping
  ```
  The `ColorScheme` is §1.2's second table by hand (`claxSeed` goes). Themes, all from the tokens: `filledButtonTheme` ink (`fg` fill, `bg` text, `primary-hover` hovered); `inputDecorationTheme` outlined (`radius-sm`, 1 px `border-strong`, 2 px `focus` focused, filled `card`, the label floating into the outline's gap — fixes the defect at the old `theme.dart:26-33`); `snackBarTheme` floating, width 560; `dialogTheme`; `listTileTheme` (`contentPadding: EdgeInsets.symmetric(horizontal: 16)`); `chipTheme`; `menuTheme` (`overlay`, level 2); `tooltipTheme`; `badgeTheme` (`you`/`on-you`); `bottomSheetTheme` (`raised`, top corners `radius-xl`, `showDragHandle: true`); `checkboxTheme`/`radioTheme`/`switchTheme` green; `pageTransitionsTheme` (`TargetPlatform.android: PredictiveBackPageTransitionsBuilder()`); `visualDensity` standard for material, compact for apple; `surfaceTintColor` transparent; `extensions: [ClaxColors(palette)]`. `app.dart` sets `theme`, `darkTheme`, `highContrastTheme`, `highContrastDarkTheme` from `claxTheme(…, defaultTargetPlatform, highContrast: …)`.
- Consumes: Task 1, Task 2.

- [ ] **Step 1: Read the threads** on each modified file.
- [ ] **Step 2: Failing tests.** `layout_test.dart`: `ClaxWidth.fromWidth` at 599, 600, 839, 840, 1199, 1200; `ClaxFamily.forPlatform` for android, macOS, iOS. `theme_test.dart`:
  ```dart
  test('the scheme is the brand, by hand', () {
    final t = claxTheme(Brightness.light, TargetPlatform.android);
    expect(t.colorScheme.primary, ClaxTokens.light.agent);
    expect(t.colorScheme.tertiary, ClaxTokens.light.you);
    expect(t.colorScheme.surface, ClaxTokens.light.bg);
    expect(t.colorScheme.surfaceContainerHighest, ClaxTokens.light.surfaceContainers[4]);
    expect(t.colorScheme.surfaceTint, Colors.transparent);
    expect(claxTheme(Brightness.dark, TargetPlatform.android).colorScheme.onError, const Color(0xFF141413));
  });
  test('fields are outlined and filled with card', () {
    final d = claxTheme(Brightness.light, TargetPlatform.android).inputDecorationTheme;
    expect(d.enabledBorder, isA<OutlineInputBorder>());
    expect((d.enabledBorder! as OutlineInputBorder).borderSide.color, ClaxTokens.light.borderStrong);
    expect((d.focusedBorder! as OutlineInputBorder).borderSide.width, 2);
    expect(d.fillColor, ClaxTokens.light.card);
  });
  test('the primary button is ink', () {
    final s = claxTheme(Brightness.light, TargetPlatform.android).filledButtonTheme.style!;
    expect(s.backgroundColor!.resolve({}), ClaxTokens.light.fg);
    expect(s.backgroundColor!.resolve({WidgetState.hovered}), ClaxTokens.light.primaryHover);
  });
  test('Android pages go back predictively', () {
    expect(claxTheme(Brightness.light, TargetPlatform.android).pageTransitionsTheme.builders[TargetPlatform.android], isA<PredictiveBackPageTransitionsBuilder>());
  });
  test('the Apple family is compact with SF sizes', () {
    final t = claxTheme(Brightness.light, TargetPlatform.macOS);
    expect(t.visualDensity, VisualDensity.compact);
    expect(t.textTheme.bodyMedium!.fontSize, 13);
  });
  testWidgets('a filled field\'s label sits in the outline\'s gap', (tester) async {
    await tester.pumpWidget(MaterialApp(theme: claxTheme(Brightness.light, TargetPlatform.android), home: Scaffold(body: TextField(controller: TextEditingController(text: 'sk-ant'), decoration: const InputDecoration(labelText: 'API key')))));
    final label = tester.getRect(find.text('API key'));
    final field = tester.getRect(find.byType(InputDecorator));
    expect(label.center.dy, closeTo(field.top, 2));
  });
  ```
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_theme_test //apps/clax_app:ui_layout_test` — FAIL.
- [ ] **Step 4: Implement**; replace the breakpoints; run `bazelisk test //apps/clax_app:ui_theme_test //apps/clax_app:ui_layout_test //apps/clax_app:ui_bundle_sheets_test //apps/clax_app:ui_question_sheet_test //apps/clax_app:ui_change_sheet_shots_test //apps/clax_app:ui_app_screen_test //apps/clax_app:ui_version_page_test //apps/clax_app:dart_analyze`. Expected: PASS (a test that asserted the side panel at 720 now uses 840).
- [ ] **Step 5: Look.** `screens_shots_test.dart`: `shots('settings', …)` (the `SettingsScreen` as `test/ui/settings_test.dart` builds it) and `shots('launcher', …)` (as `test/ui/launcher_icons_shots_test.dart` builds it). Render and read all twelve PNGs: warm page, outlined fields with the label in the gap, green switches, ink buttons, dark cards a step above the page.
- [ ] **Step 6: Commit**: "Build Flutter's theme from the brand's tokens, with Material and Apple families and window size classes".

### Task 6: Components, part 1: controls and states

**Files:**
- Create: `apps/clax_app/lib/ui/kit/button.dart`, `kit/text_field.dart`, `kit/toggles.dart`, `kit/badge.dart`, `kit/empty.dart`, `kit/banner.dart`, `kit/verbs.dart`, `kit/kit.dart` (exports `family.dart` and the rest), `apps/clax_app/test/ui/kit/controls_test.dart`, `apps/clax_app/test/ui/shots/kit_shots_test.dart`

**Interfaces:**
- Produces (each component draws its `ClaxFamily.of(context)` form, §2's two columns, with "Apple" for the spec's "macOS" column):
  ```dart
  enum ClaxButtonKind { primary, tonal, plain, destructive }
  class ClaxButton extends StatelessWidget { const ClaxButton({super.key, required this.label, required this.onPressed, this.kind = ClaxButtonKind.plain, this.icon, this.isDefault = false}); /* material: 40 dp in a 48 dp target; apple: a 24 pt push button, `isDefault` filled in the accent (the system accent once Task 50 reads it) */ }
  class ClaxTextField extends StatelessWidget { const ClaxTextField({super.key, this.controller, required this.label, this.hint, this.obscure = false, this.maxLines = 1, this.onSubmitted, this.autofocus = false, this.focusNode}); /* apple: bordered field, radius controlApple, 13 pt, focus ring in the accent */ }
  class ClaxSwitch extends StatelessWidget { const ClaxSwitch({super.key, required this.value, required this.onChanged, this.label}); }
  class ClaxCheckbox extends StatelessWidget { const ClaxCheckbox({super.key, required this.value, required this.onChanged, required this.label}); }
  class ClaxRadio<T> extends StatelessWidget { const ClaxRadio({super.key, required this.value, required this.group, required this.onChanged, required this.label, this.detail}); }
  class ClaxBadge extends StatelessWidget { const ClaxBadge({super.key, required this.count, this.child}); static String text(int count); /* '' at 0 and hidden, '99+' above 99 */ }
  class ClaxEmpty extends StatelessWidget { const ClaxEmpty({super.key, this.icon, this.art, required this.title, this.body, this.actions = const [], this.extra}); /* centred column, max 360, title-2, body in muted, centred text; `art` takes a ClaxMark (Task 8); `extra` takes a haiku (Task 21) */ }
  class ClaxBanner extends StatelessWidget { const ClaxBanner({super.key, required this.icon, this.iconColor, required this.title, this.body, this.actions = const []}); /* radius-lg card on `card`; actions inline from ClaxWidth.medium, below at compact; max width ClaxLayout.reading */ }
  final class ClaxVerbs { const ClaxVerbs(this.tap, this.longPress, this.swipe); final String tap, longPress, swipe; /* 'Tap'/'Click', 'Long-press'/'Right-click', 'Swipe'/'Drag' */
    static ClaxVerbs of(BuildContext context); /* Click when the last pointer event was a mouse (a Listener at the app's root records PointerDeviceKind) or, before any, when the family is apple and the platform macOS; Tap otherwise (§7.5) */ }
  ```
- Consumes: Task 5.

- [ ] **Step 1: Failing tests** in `controls_test.dart`: `ClaxBadge.text(0) == ''`, `(1) == '1'`, `(99) == '99'`, `(100) == '99+'`, and a 0 badge has no `Text`; on Android a `ClaxButton` has a 48×48 minimum hit size and a 40 dp visual height; `destructive` paints its label in `danger`; `ClaxEmpty` texts are `TextAlign.center` and its width at most 360 on a 1200-wide screen; `ClaxBanner`'s actions share the title's row at 900 wide and sit below it at 400; `ClaxVerbs.of` says 'Tap' on Android until a mouse pointer event, 'Click' after one, and 'Click' on macOS.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_kit_controls_test` — FAIL (`test/ui/kit/` is in the `ui` suite; no BUILD change).
- [ ] **Step 3: Implement**; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** `kit_shots_test.dart` renders one page with every control in every state (`shots('kit-controls', …)`); read the six PNGs; the mac ones read as Mac controls, the phone ones as Material.
- [ ] **Step 5: Commit** `lib/ui/kit/` and the tests: "Add Clax's adaptive buttons, fields, toggles, badges, empty states, banners and verbs".

### Task 7: Components, part 2: surfaces

**Files:**
- Create: `apps/clax_app/lib/ui/kit/scaffold.dart`, `kit/sheet.dart`, `kit/dialog.dart`, `kit/menu.dart`, `kit/toast.dart`, `apps/clax_app/test/ui/kit/surfaces_test.dart`
- Modify: `apps/clax_app/lib/ui/kit/kit.dart` (exports), `apps/clax_app/test/ui/shots/kit_shots_test.dart` (`kit-surfaces`)

**Interfaces:**
- Produces:
  ```dart
  class ClaxScaffold extends StatelessWidget { const ClaxScaffold({super.key, this.title, this.subtitle, this.actions = const [], this.leading, required this.body, this.floatingActionButton, this.largeTitle = false}); /* material: Scaffold + AnnotatedRegion<SystemUiOverlayStyle>(statusStyleFor(app bar colour)); apple: the content of a window or pane, no AppBar when hasDesktopWindowChrome (the toolbar carries title and actions from Task 44) */ }
  SystemUiOverlayStyle statusStyleFor(Color under); // transparent bars; light icons when under.computeLuminance() < 0.5
  abstract final class ClaxSheet { static Future<T?> show<T>(BuildContext context, {required WidgetBuilder builder, String? title, List<Widget> actions = const []}); /* compact: modal bottom sheet, radius-xl, handle, no close button; medium and up: centred Dialog, max 560; apple at medium and up: a sheet sliding from under the toolbar, 480–640, actions bottom-right (Cancel, then default) */ }
  class ClaxDialogAction { const ClaxDialogAction(this.label, {this.kind = ClaxButtonKind.plain}); final String label; final ClaxButtonKind kind; }
  abstract final class ClaxDialog { static Future<int?> show(BuildContext context, {required String title, String? body, required List<ClaxDialogAction> actions, int? defaultIndex, int? cancelIndex}); /* material: AlertDialog, actions right, stacked full-width when one label does not fit; apple: Flutter-drawn alert until Task 39 routes it to NSAlert where hasDesktopWindowChrome */ }
  class ClaxMenuItem { const ClaxMenuItem(this.id, this.title, {this.icon, this.symbol, this.key, this.destructive = false, this.enabled = true}) : separator = false; const ClaxMenuItem.separator() : id = '', title = '', icon = null, symbol = null, key = null, destructive = false, enabled = false, separator = true; final String id, title; final IconData? icon; final String? symbol /* SF Symbol name */, key; final bool destructive, enabled, separator; }
  abstract final class ClaxMenu { static Future<String?> show(BuildContext context, {required List<ClaxMenuItem> items, required Rect anchor}); /* an anchored popup at [anchor] (global coordinates); Task 39 routes it to NSMenu where hasDesktopWindowChrome */ }
  enum ClaxToastPlace { bottom, top, inline }
  abstract final class ClaxToast { static void show(BuildContext context, String text, {String? actionLabel, VoidCallback? onAction, ClaxToastPlace place = ClaxToastPlace.bottom}); /* bottom/top: floating card, 16 dp margins, max 560; inline: a bar under the pane's top bar or toolbar that dismisses itself after 4 s on the context's clock */ }
  ```
- Consumes: Tasks 5, 6.

- [ ] **Step 1: Failing tests** in `surfaces_test.dart`: `ClaxSheet.show` at 412 wide is a `BottomSheet` with a drag handle and no `Icons.close`; at 900 a `Dialog` at most 560 wide; `ClaxDialog.show` returns the tapped index and `cancelIndex` on Escape; `ClaxMenu.show` puts its first item within 8 dp of the anchor, returns the tapped ID and null on a tap outside; `ClaxToast.show(place: top)` sits under `MediaQuery.paddingOf(context).top`, 16 dp from the sides, at most 560 wide; `inline` disappears after `tester.pump(const Duration(seconds: 4))`; `statusStyleFor(Colors.white).statusBarIconBrightness == Brightness.dark`, `(Colors.black)` light, `(Color(0xFF777777))` light.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_kit_surfaces_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 3: Look** at `kit-surfaces` (a sheet, a dialog, a menu and each toast in its own shot), all six forms.
- [ ] **Step 4: Commit**: "Add Clax's adaptive scaffold, sheets, dialogs, anchored menus and toasts".

**Phase 1 ends** when `grep -rn -i "4a5d8f\|claxSeed" apps/clax_app/lib apps/clax_app/home` prints nothing and the Task 4–7 renders have been read in both themes.

---

## Phase 2: Identity (§5.11, §7.4's mark, §10.2)

### Task 8: The mark: one geometry, `ClaxMark` and its CSS twin

**Files:**
- Create: `design/mark.json`, `design/mark.css` (the CSS twin's rules, hand-written), `apps/clax_app/lib/ui/kit/mark.dart`, `apps/clax_app/test/ui/kit/mark_test.dart`
- Generate (committed): `apps/clax_app/home/mark.css` (a copy of `design/mark.css` with the generated header), `ClaxMarkGeometry` appended to `apps/clax_app/lib/ui/tokens.g.dart`
- Modify: `scripts/sync-tokens.py` (`mark_outputs`), `scripts/test-sync-tokens.sh` (a mark case), `design/BUILD.bazel` (`:tokens` gains `mark.json`, `mark.css`), `apps/clax_app/BUILD.bazel` (`token_outputs` gains `home/mark.css`), `apps/clax_app/lib/ui/kit/kit.dart`, `apps/clax_app/test/ui/shots/kit_shots_test.dart` (`kit-mark`)

**Interfaces:**
- Produces `design/mark.json`: `{"viewBox": [22, 16], "stroke": 2.6, "cap": "round", "you": {"d": "M1.5 2.5a5.5 5.5 0 0 1 0 11"}, "agent": {"d": "M20.5 2.5a5.5 5.5 0 0 0 0 11"}, "dot": {"cx": 11, "cy": 8, "r": 1.7}}` (copied from `web/shell/public/_clax/mark.svg`, which stays the browser's and is not regenerated).
- Produces (`lib/ui/kit/mark.dart`):
  ```dart
  enum ClaxMarkState { resting, working, sent, landed, asking, rally }
  class ClaxMark extends StatefulWidget {
    const ClaxMark({super.key, this.size = 24, this.state = ClaxMarkState.resting, this.monochrome, this.semanticsLabel});
    final double size; // the mark's width; height is size × 16/22
    final ClaxMarkState state;
    final Color? monochrome; // one colour for all three shapes (icons, the pill on `fg`)
    final String? semanticsLabel; // defaults by state: null (resting), 'Clax is working', 'Sent to Clax', 'New version ready', 'Clax has a question'
  }
  class ClaxMarkPainter extends CustomPainter { ClaxMarkPainter({required this.you, required this.agent, required this.dot, required this.dotShift, required this.agentScale}); /* draws ClaxMarkGeometry; dotShift in -1…1 moves the dot toward the orange (−) or green (+) arc */ }
  ```
  States (§7.4): `working` and `asking` loop on `ClaxMotion.markBreath` (dot toward green / toward orange and back, ease-in-out); `sent` slides once over 300 ms; `landed` nods the green arc (scale 1.08) over 300 ms; `rally` bounces the dot between the arcs ten times (Task 21's easter egg). Under `MediaQuery.disableAnimationsOf` and under `ClaxQuiet(quiet: true)` (an `InheritedWidget` in `mark.dart`, `static bool of(BuildContext c)`, which Task 30 sets while comment mode is on) every state draws still.
- Produces `apps/clax_app/home/mark.css`: `.clax-mark` (an inline SVG built by the home app from the same three shapes) with `.is-working`, `.is-sent`, `.is-landed`, `.is-asking` keyframes and `@media (prefers-reduced-motion: reduce)` stopping them.
- Consumes: Task 1, Task 6.

- [ ] **Step 1: Failing tests** (`mark_test.dart`): the painter's three paths match `mark.json` (the you arc's bounds are `Rect.fromLTRB(1.5 - 1.3, 2.5 - 1.3, 7 + 1.3, 13.5 + 1.3)` scaled by `size / 22`); `working` at t = 0 and t = 800 ms on `tester.pump` has `dotShift` 0 and ≈ +0.35; with `MediaQueryData(disableAnimations: true)` the dot never moves; `semanticsLabel` defaults per state. In `test-sync-tokens.sh`, editing `design/mark.css` without regenerating fails `--check` naming `apps/clax_app/home/mark.css`.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_kit_mark_test //scripts:sync_tokens` — FAIL; implement; `python3 scripts/sync-tokens.py`; rerun with `//design:tokens_sync` and `dart_analyze` — PASS.
- [ ] **Step 3: Look:** `kit-mark` renders each state at 16, 24, 48 and 132 dp (the working state at its widest drift), light and dark; read the phone and mac PNGs.
- [ ] **Step 4: Commit**: "Draw the Clax mark from one geometry, with its resting, working, sent, landed and asking states".

### Task 9: The app icons

**Files:**
- Create: `scripts/build-app-icons.sh`, `apps/clax_app/icons/SOURCE.sha256`, `design/icons-sync.sh`, `apps/clax_app/android/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml`, `…/mipmap-anydpi-v26/ic_launcher_round.xml`, `…/drawable/ic_launcher_fg.xml`, `…/drawable/ic_launcher_mono.xml`, `…/values/ic_launcher_colors.xml` (`ic_launcher_bg` #f7f7f5), `apps/clax_app/icons/play-store-512.png`, `apps/clax_app/macos/Runner/AppIcon.icon/` (Icon Composer bundle: background, two arcs, dot; light, dark, tinted)
- Regenerate (committed renders): `…/res/mipmap-{m,h,xh,xxh,xxxh}dpi/ic_launcher.png` and `ic_launcher_round.png` (48–192 px), `apps/clax_app/macos/Runner/Assets.xcassets/AppIcon.appiconset/app_icon_{16,32,64,128,256,512,1024}.png` and `Contents.json`
- Modify: `apps/clax_app/android/app/src/main/AndroidManifest.xml:27` (`android:roundIcon="@mipmap/ic_launcher_round"`), `apps/clax_app/tool/apk-check.sh:20` (the badging line's `icon='…'` now names the adaptive icon), `design/BUILD.bazel` (`sh_test(name = "icons_sync")`)

**Interfaces:**
- Produces `scripts/build-app-icons.sh [--check]` (dry run by default is not wanted here: it writes the renders; `--check` writes nothing and compares `SOURCE.sha256` with `sha256(design/mark.json + design/tokens.json's you, agent, fg, bg, raised)`): builds an SVG per target from `mark.json` (§5.11's sizes: 52 dp mark on the 108 dp adaptive canvas; 560 px mark in an 824 px body of radius 185 on Apple's 1024 grid, filled #ffffff→#f2f2ef with Apple's standard shadow; the dark variant on #232321 with the dark tokens), renders them with `rsvg-convert` (`/Users/alex/.local/homebrew/bin/rsvg-convert` or the first on `PATH`; the script fails naming it when absent), writes the PNGs and `SOURCE.sha256`. Works from any directory.
- Produces `//design:icons_sync`: runs `build-app-icons.sh --check` (hashes only, no pixels: a renderer upgrade cannot flake it).
- Consumes: Task 8 (`design/mark.json`).

- [ ] **Step 1: Read the threads** on `AndroidManifest.xml`, `apk-check.sh`, `Assets.xcassets`.
- [ ] **Step 2: Failing check:** add `icons_sync` to `design/BUILD.bazel` (`data` = `mark.json`, `tokens.json`, `//apps/clax_app:icons_hash` (a filegroup of `icons/SOURCE.sha256`), the script); `bazelisk test //design:icons_sync` — FAIL (no hash).
- [ ] **Step 3: Write the script, the vector drawables** (`ic_launcher_fg.xml`: the three shapes as `<path>`s in a 108×108 viewport, translated so the 52 dp mark is centred; `ic_launcher_mono.xml`: the same in `#000000`, tinted by the launcher) **and the adaptive XMLs** (background, foreground, `<monochrome>`). Run `scripts/build-app-icons.sh`.
- [ ] **Step 4: Run** `bazelisk test //design:icons_sync //apps/clax_app:apk_test //apps/clax_app:macos_app_test` — PASS (in the background; `apk_test` takes minutes).
- [ ] **Step 5: Look:** read `mipmap-xxxhdpi/ic_launcher.png`, `app_icon_1024.png`, `app_icon_32.png` and `play-store-512.png`; render the adaptive foreground over a circle mask and a squircle (`rsvg-convert` of the drawable's SVG twin into `target/flutter-app/shots/icon-mask-*.png`) and read both: the mark inside the 66 dp safe circle.
- [ ] **Step 6: Commit** the listed files: "Give the app the Clax mark as an Android adaptive icon with a monochrome layer and a macOS icon on Apple's grid".

### Task 10: The Android splash

**Files:**
- Modify: `MODULE.bazel:212-224` (`"androidx.core:core-splashscreen:1.2.0"` in `maven.install`'s artifacts) and `bazel/android/maven_install.json` (repinned), `bazel/flutter/android.bzl:88` (`flutter_fat_apk(…, runner_deps = [])`, passed into the runner's `kt_android_library` deps at `:203-216`), `apps/clax_app/BUILD.bazel:265-294` (`runner_deps = ["@rules_android_maven//:androidx_core_core_splashscreen"]`), `apps/clax_app/android/app/build.gradle.kts:64` (`implementation("androidx.core:core-splashscreen:1.2.0")`), `apps/clax_app/android/app/src/main/res/values/styles.xml` and `values-night/styles.xml` (`Theme.Clax.Starting` parent `Theme.SplashScreen`: `windowSplashScreenBackground` `@color/clax_bg`, `windowSplashScreenAnimatedIcon` `@drawable/splash_mark`, `postSplashScreenTheme` `@style/Theme.Clax`; `Theme.Clax` parent `Theme.DeviceDefault.DayNight.NoActionBar` with transparent status and navigation bars; `LaunchTheme`/`NormalTheme` names kept as aliases the manifest uses), `apps/clax_app/android/app/src/main/AndroidManifest.xml:35,43-46`, `apps/clax_app/android/app/src/main/kotlin/dev/empathic/clax_app/MainActivity.kt:16-21` (`installSplashScreen()` before `super.onCreate`, with `setKeepOnScreenCondition` false: Flutter's first frame draws the same mark, Task 11)
- Create: `…/res/drawable/splash_mark.xml` (the foreground's shapes on the 288 dp splash canvas, the mark 132 dp wide), `…/res/values/clax_colors.xml` and `values-night/clax_colors.xml` (`clax_bg` #f7f7f5 / #141413)
- Delete: `…/res/drawable/launch_background.xml`, `…/res/drawable-v21/launch_background.xml`

**Interfaces:**
- Produces `flutter_fat_apk(runner_deps = [labels])`.
- Consumes: Task 9 (`ic_launcher_fg.xml`'s shapes).

- [ ] **Step 1: Read the threads** on `bazel/flutter/android.bzl`, `MODULE.bazel`, `MainActivity.kt`, the styles.
- [ ] **Step 2: Pin** the artifact: edit `MODULE.bazel`, run `REPIN=1 bazelisk run @unpinned_rules_android_maven//:pin`, and `just bazel-sync` if it reports drift. A failing pin (network) stops the task: report it.
- [ ] **Step 3: Implement** the attribute, the themes, the drawable and `MainActivity`; run `bazelisk test //apps/clax_app:apk_test` (background, logged). Expected: PASS; `tool/apk-check.sh` still passes the launchable activity check.
- [ ] **Step 4: Look:** install isn't possible here; render `splash_mark.xml`'s SVG twin over `clax_bg` at 288 dp in both themes into `target/flutter-app/shots/splash-android-{light,dark}.png` and read them; record the on-device check for phase 8 as a `concern` on `MainActivity.kt` ("The splash's mark size and the jump to ClaxSplash are unverified on a device") tagged `phase:8`.
- [ ] **Step 5: Commit**: "Open the Android app on a splash of the Clax mark on the brand's background, light and dark".

### Task 11: `ClaxSplash` in Flutter

**Files:**
- Create: `apps/clax_app/lib/ui/splash.dart`, `apps/clax_app/test/ui/splash_test.dart`
- Modify: `apps/clax_app/lib/app.dart:2215-2224` (`return ClaxSplash(key: const ValueKey('splash'), clock: _s.clock)`), `:2445-2473` (`_Splash` goes), `:2476-2493` (`_UpdatingCover` draws `ClaxMark(state: working)` and "Updating…" rather than a `CircularProgressIndicator`), `apps/clax_app/test/ui/shots/screens_shots_test.dart` (`splash`)

**Interfaces:**
- Produces `class ClaxSplash extends StatefulWidget { const ClaxSplash({super.key, this.clock = const SystemClock(), this.breatheAfter = const Duration(milliseconds: 600)}); }`: the mark 132 dp wide, centred where Android's splash puts it (the window's centre), resting; after `breatheAfter` it turns `working` and "Starting…" fades in under it (`callout`, `muted`). No word "Clax".
- Consumes: Task 8.

- [ ] **Step 1: Failing tests:** at 0 ms one `ClaxMark` in `resting`, no "Starting…"; after `tester.pump(const Duration(milliseconds: 600))` the mark is `working` and "Starting…" shows; the mark's centre equals the screen's centre; no `Text('Clax')`; no `CircularProgressIndicator` anywhere under `ClaxSplash`.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_splash_test` — FAIL; implement; rerun with `//apps/clax_app:root_app_test` and `dart_analyze` — PASS.
- [ ] **Step 3: Look:** `splash` at 0 and 700 ms, all forms; the mark sits where `splash-android-*.png` (Task 10) puts it.
- [ ] **Step 4: Commit**: "Draw the splash's mark in Flutter's first frame, breathing with Starting… when the service is slow".

### Task 12: The name "Clax" on the Mac, the copyright, and About

**Files:**
- Modify: `apps/clax_app/macos/Runner/Configs/AppInfo.xcconfig` (`PRODUCT_NAME = Clax`, `PRODUCT_COPYRIGHT = Copyright © 2026 Empathic. All rights reserved.`), `apps/clax_app/BUILD.bazel:349,474` (`app_name = "Clax"` in `macos_app` and the integration macro's `flutter_macos_app`), `apps/clax_app/tool/macos-app-check.sh:3,16-17` (`Clax.app`), `apps/clax_app/tool/integration-run.sh:11` (`Clax.app/Contents/MacOS/Clax`), `apps/clax_app/macos/Runner/AppDelegate.swift` (an `orderFrontStandardAboutPanel` override with `.credits` from a haiku), `apps/clax_app/macos/Runner/Base.lproj/MainMenu.xib:29` (About wired to `showAbout:`)
- Create: `apps/clax_app/macos/Runner/About.swift` (reads `flutter_assets/assets/haiku.json` from the bundle's `App.framework`, picks one, returns an `NSAttributedString` in New York italic 13 pt, secondary label colour; a new one each time the panel opens. Another haiku on demand is Settings › About's, Task 14)

**Interfaces:**
- Produces `About.credits(seed: Int?) -> NSAttributedString` (Swift; nil seed is random).
- Consumes: Task 1 (`assets/haiku.json` in the bundle).

- [ ] **Step 1: Read the threads** on the files (and on `AppDelegate.swift`).
- [ ] **Step 2: Implement**; add `About.swift` to `project.pbxproj` (Global Constraints).
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:macos_app_test` (background). Expected: PASS with `Clax.app`, `CFBundleName` `Clax`, and the copyright line (extend `macos-app-check.sh` to check `NSHumanReadableCopyright` equals the new line and `CFBundleName` equals `Clax`).
- [ ] **Step 4: Look:** `bazelisk build //apps/clax_app:macos_app`, unzip to `target/flutter-app/mac-app/`, `open` it with a scratch `CLAX_HOME` (never `~/.clax`), choose Clax › About Clax, and `screencapture -l<windowid>` the About panel to `target/flutter-app/shots/about-mac-light.png` (the window ID from `osascript` or `CGWindowListCopyWindowInfo`); read it: "Clax", the copyright, a haiku. Quit by the PID you started.
- [ ] **Step 5: Commit**: "Name the Mac app Clax everywhere, with Empathic's copyright and a haiku in About".

**Phase 2 ends** with the icon, splash and About renders read.

---

## Phase 3: Clean-up and copy (§5.4–§5.7, §5.10, §7, §10.3)

### Task 13: The Developer switch

**Files:**
- Modify: `apps/clax_app/lib/platform/settings_store.dart:31-111` (`bool developer = false`, key `developer`), `apps/clax_app/lib/ui/settings_screen.dart:273-274,366-398` (Recent timings moves into a Developer section shown only when on; the switch "Developer tools")
- Create: `apps/clax_app/lib/ui/developer.dart`, `apps/clax_app/test/ui/developer_test.dart`, `apps/clax_app/test/platform/settings_store_test.dart`
- Modify: `apps/clax_app/lib/app.dart` (wrap the app in `DeveloperScope(settings: …)` beside the existing settings plumbing at `:754`)

**Interfaces:**
- Produces:
  ```dart
  class DeveloperScope extends InheritedNotifier<ValueNotifier<bool>> { const DeveloperScope({super.key, required ValueNotifier<bool> notifier, required super.child}) : super(notifier: notifier); static bool of(BuildContext context); }
  ```
  Later tasks gate developer views with `if (DeveloperScope.of(context)) …`: Recent timings (every name labelled; `send-first-step` labelled, 15fafe1f35), the inspector's Files, Code and Data tabs and raw JSON, MIME types, capability names, the Coming rows, raw error text under Details.
- Consumes: Task 6 (`ClaxSwitch`).

- [ ] **Step 1: Read the threads** on both files (15fafe1f35 on the timings).
- [ ] **Step 2: Failing tests:** `settings_store_test.dart`: `AppSettings()` has `developer == false`; it round-trips through `toJson`/`fromJson`; a non-bool value is ignored. `developer_test.dart`: `SettingsScreen` shows no "Recent timings" until "Developer tools" is switched on, then shows it, with `send-first-step` reading its label (find the label table in `lib/platform/timings.dart` and add the missing entry).
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:platform_settings_store_test //apps/clax_app:ui_developer_test //apps/clax_app:ui_settings_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Commit**: "Put Clax's developer views behind a Developer tools switch, off by default". Close 15fafe1f35 separately.

### Task 14: Settings (§5.5)

**Files:**
- Modify: `apps/clax_app/lib/ui/settings_screen.dart:156-505` (rewrite of `build` and `_Header`; `_keyTile` `:280-365` takes `ClaxTextField`; `_HomeSection` `:417-505` keeps its actions)
- Create: `apps/clax_app/lib/ui/haiku.dart`, `apps/clax_app/test/ui/haiku_test.dart`, `apps/clax_app/test/ui/shots/settings_shots_test.dart`
- Test: `apps/clax_app/test/ui/settings_test.dart`

**Interfaces:**
- Produces (`lib/ui/haiku.dart`):
  ```dart
  /// The shared haiku list (assets/haiku.json, copied from the shell's).
  Future<List<String>> loadHaikus(AssetBundle bundle);
  String pickHaiku(List<String> list, {String? seed, Random? random}); // FNV-1a of seed mod length, as web/shell/src/view/haiku.ts:4 does; random otherwise
  class HaikuLine extends StatelessWidget { const HaikuLine({super.key, required this.text, this.onAnother}); /* ClaxRole.haiku, muted; Semantics(label: 'Haiku: $text', liveRegion: false); long-press (material) or Option-click (apple) calls onAnother */ }
  ```
- Consumes: Tasks 6, 7, 13.

- [ ] **Step 1: Read the threads** on `settings_screen.dart` and its test.
- [ ] **Step 2: Failing tests.** `haiku_test.dart`: `pickHaiku(['a','b','c'], seed: 'x')` equals the shell's `pickHaiku` for the same seed (hard-code the shell's answer computed by `bazelisk test //web:unit --test_arg=shell/src/view/haiku.test.ts` with a temporary `console.log`, or compute FNV-1a by hand in the test); a `HaikuLine` exposes `Haiku: …` and is not a live region. `settings_test.dart`: sections in order Claude, Dictation, Sharing, Notifications (Android only), Home screen, About, Developer; every `ListTile`'s leading inset is 16; the API key field is a `ClaxTextField`; Model is one row opening a menu of "Fast", "Balanced", "Best", each with its model name (map `modelChoices` in `settings_store.dart:12` fastest first to those words); About shows Clax's version, the home screen's version, "Licences" (opens `showLicensePage`) and a haiku that changes between two visits; a long-press on it shows another.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_haiku_test //apps/clax_app:ui_settings_test` — FAIL; implement (the large title collapses on scroll with a 1 px `border` line under content, `SliverAppBar.large`; max width 640 centred from `ClaxWidth.medium`); rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** `settings_shots_test.dart` with `textScales: [1, 1.3, 2]` on the phone; read all; nothing clips at 200%.
- [ ] **Step 5: Commit**: "Lay Settings out in one aligned column with Claude, Dictation, Sharing, Home screen, About and Developer".

### Task 15: `userMessage`: no raw errors on screen

**Files:**
- Create: `apps/clax_app/lib/ui/user_message.dart`, `apps/clax_app/test/ui/user_message_test.dart`
- Modify: `apps/clax_app/lib/ui/make_service.dart:146`, `lib/ui/build_plan.dart:442`, `lib/ui/app_screen.dart:1100,1275,1661`, `lib/ui/thread_detail.dart:513`, `lib/ui/bundle_sheets.dart:1046,1088,1100,1261,1315`, `lib/home/home_recovery.dart:210`, `lib/ui/webview_surface.dart:169` (keep the raw `description` on `SurfaceFault.detail` for Developer)

**Interfaces:**
- Produces:
  ```dart
  /// One short line for [e] (§7.3): what happened, what you can do.
  String userMessage(Object e);
  /// The raw text for the log and Developer's Details.
  String rawError(Object e);
  ```
  Mapping: `SocketException`, `http.ClientException`, a `SurfaceFault` whose detail starts `net::ERR_` → "Clax’s service didn’t answer."; `TimeoutException` → "That took too long. Try again."; a `ClaxApiError` or `AgentError` whose code or text says overloaded (529) or rate limited (429) → "Claude is busy right now. Try again in a minute."; an invalid key (401, `authentication_error`) → "Your Claude API key wasn’t accepted. Check it in Settings."; another `AgentError` → its `message`; anything else → "Something went wrong. Try again." Every call site logs `rawError(e)` with the app's existing logger and passes it to Details only under `DeveloperScope.of(context)`.
- Consumes: Task 13.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests** (`user_message_test.dart`), one `test` per row of the mapping, including `StateError('The Claude API said: overloaded (529)')` → the busy line (the §7.3 "make failure" row), and `'net::ERR_CONNECTION_REFUSED'` → "Clax’s service didn’t answer."; and a widget test that `make_service`'s failure text for a `StateError` is never `Bad state:…`.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_user_message_test` — FAIL; implement and replace each site; run the suites of the touched files (`ui_make_service_test`, `ui_build_view_test`, `ui_app_screen_test`, `ui_thread_detail_test`, `ui_bundle_sheets_test`, `home_home_recovery_test`) and `dart_analyze` — PASS.
- [ ] **Step 4: Commit**: "Show one plain line for every failure, keeping the raw text for the log and Developer tools".

### Task 16: Clax is "Clax" everywhere in the app

**Files:**
- Modify: `apps/clax_app/lib/clax/api.dart:64-68` (`Future<void> renameSession(String sid, String name)`), `lib/clax/http_api.dart:183-201` (PATCH `/api/sessions/<sid>` `{"name": name}`), `lib/clax/fake_api.dart` (records the rename; `name_taken` on demand), `lib/agent/worker.dart:736-775` (after `_sid = await _session()`, `renameSession(_sid, 'Clax')`, ignoring `ClaxApiError` with code `name_taken` or `name_locked`), `web/shell/src/view/working-model.ts:12,18` (`app: "Clax"`), `web/shell/src/q/model.ts:71-76` (`agentLabel` is "Clax" for harness `app`), `web/shell/src/q/QuestionCard.svelte:220,291` (no "in <project>" for harness `app`; the button reads "Answer" for it)
- Test: `apps/clax_app/test/agent/worker_test.dart`, `apps/clax_app/test/clax/http_api_test.dart` (or the file that tests `registerAppSession`), `web/shell/src/view/working-model.test.ts`, `web/shell/src/q/model.test.ts`

**Interfaces:**
- Produces `ClaxApi.renameSession(String sid, String name)`.
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests:** the worker renames its session "Clax" once after registering and again after re-registering on `unknown_session`, and keeps working when the rename answers `name_taken`; the HTTP client sends `PATCH /api/sessions/s1` with `{"name":"Clax"}`; `harnessName("app") === "Clax"`; `agentLabel` for an `app` agent named `app-2` is "Clax"; the card's markup for an `app` question has no `in tmp` and its button reads "Answer".
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:agent_worker_test //apps/clax_app:clax_http_api_test //web:unit --test_arg=shell/src/view/working-model.test.ts --test_arg=shell/src/q/model.test.ts` — FAIL; implement; rerun — PASS — plus `dart_analyze`, `//web:typecheck`.
- [ ] **Step 4: Commit**: "Name the app's agent Clax in its session and wherever the app shows it".

### Task 17: A failed change reads as one reply with Details

**Files:**
- Modify: `apps/clax_app/lib/ui/thread_detail.dart:729-945` (`_statusCard`: the failed case's banner goes), `:969-984` (`_comment`: a failure reply is a `card` bubble with a 3 dp `danger` left rule and a `danger` icon; no `errorContainer`), `:882-890` (Retry hidden once the change has spent `changeCapUsd`), `apps/clax_app/lib/agent/failure.dart:12-272` (`parseFailureReply(String body) → FailureReply`; `keepGoingHint` through `ClaxVerbs`, Decision 11)
- Test: `apps/clax_app/test/ui/thread_detail_test.dart`, create `apps/clax_app/test/agent/failure_test.dart`, `apps/clax_app/test/ui/shots/thread_shots_test.dart`

**Interfaces:**
- Produces:
  ```dart
  final class FailureReply { const FailureReply({required this.line, this.costUsd, this.capUsd, this.files = const [], this.notes = const [], this.raw}); final String line; final double? costUsd, capUsd; final List<({String path, int added, int removed})> files; final List<String> notes; final String? raw; bool get spentLimit => costUsd != null && capUsd != null && costUsd! >= capUsd!; }
  FailureReply parseFailureReply(String body); // reads what limitFailureReply (:186), plannedStopReply (:237) and the failure prefix (:12) write
  String keepGoingHintFor(ClaxVerbs v); // '${v.tap} Keep going to pick up where it stopped.'
  bool offersKeepGoing(String body); // matches both the 'Tap' sentence stored replies carry and keepGoingHintFor's
  ```
  The bubble shows `line`, then Retry (hidden when `spentLimit`) and a **Details** disclosure holding "$2.10 of $6", the files with their lines, and the notes rendered as Markdown (bullets as bullets); `raw` last, only under `DeveloperScope`.
- Consumes: Tasks 6, 13, 15.

- [ ] **Step 1: Read the threads** on both files.
- [ ] **Step 2: Failing tests:** `failure_test.dart`: `parseFailureReply(limitFailureReply(…))` gives the cost, cap, files and notes it was built from; `offersKeepGoing` is true for the old and the new sentence. `thread_detail_test.dart`: a failed change shows exactly one failure (no status banner above it), no widget with `errorContainer` colour in the thread, a Details disclosure closed by default whose open state shows "$2.10 of $6" and a bullet list, and no Retry when the cost has reached the cap.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:agent_failure_test //apps/clax_app:ui_thread_detail_test` — FAIL; implement; rerun with `ui_keep_going_shots_test` and `dart_analyze` — PASS.
- [ ] **Step 4: Look:** `thread_shots_test.dart` (`thread-failed`, Details closed and open); read phone light and dark.
- [ ] **Step 5: Commit**: "Show a failed change once, as Clax’s reply, with its cost, files and notes under Details".

### Task 18: Thread titles from names, and the threads panel (§5.4)

**Files:**
- Modify: `web/bridge/src/protocol.ts:19-38` (`Anchor.name?: string`), `web/bridge/src/anchor.ts:169,185` and `web/bridge/src/area.ts:130` (record `name` at pick time: the element's accessible name, else its text's first 60 characters, else its role with the nearest heading, "Chart under ‘p95 by four-hour window’"), `web/bridge/src/describe.ts:17` (the helper `nameOf(el)`), `web/shell/src/app/label.ts:65-79` (`anchorLabel` in app mode: the quote, then `name`, then the kind word "Table", "Chart", "Image", "Area on the page"; an old anchor with none reads "A spot on the page"; quotes curly), `apps/clax_app/lib/ui/threads_panel.dart:122-197` (open threads first, then a collapsed "Resolved (n)" section in `muted`; meta line "3 comments · not sent"; "Send to Clax" an icon button with a tooltip; a wide clip on its own line cropped 16:9; the empty state's "Tap anything…" through `ClaxVerbs`)
- Test: `web/bridge/test/anchor.test.ts`, `web/bridge/test/area.test.ts`, `web/bridge/test/describe.test.ts`, `web/shell/src/app/label.test.ts`, `apps/clax_app/test/ui/threads_panel_test.dart`

**Resolves:** ca36e76845 (the clip); S2, S16 (audit codes).

- [ ] **Step 1: Read the threads** on each file (ca36e76845 on `threads_panel.dart`).
- [ ] **Step 2: Failing tests:** a `<button aria-label="Save">` anchors with `name: "Save"`; a `<div>` of 80 characters of text with its first 60; an unnamed `<canvas>` under `<h2>p95 by four-hour window</h2>` with "Chart under ‘p95 by four-hour window’"; `anchorLabel({selector: "#x > div:nth-child(3)"})` is "A spot on the page" and never contains `#` or `>`; the panel lists open before resolved, "Resolved (2)" collapsed, the meta "3 comments · not sent", the send button an `IconButton` with tooltip "Send to Clax", and a 1600×400 clip on its own row at 16:9.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=bridge/test --test_arg=shell/src/app/label.test.ts //apps/clax_app:ui_threads_panel_test` — FAIL; implement; rerun — PASS — plus `//web:typecheck`, `dart_analyze`.
- [ ] **Step 4: Commit**: "Title threads by the picked element’s name, never its selector, and list resolved threads apart". Close ca36e76845 separately.

### Task 19: Tap or click, and the home capability's `chrome()`

**Files:**
- Modify (Tap strings, §7.5): `apps/clax_app/lib/ui/comment_bar.dart:53` (the hint built from `ClaxVerbs`), `apps/clax_app/lib/app.dart:1410,1533,1537`, `apps/clax_app/lib/ui/build_notices.dart:70`, `apps/clax_app/home/home.js:1723,1755` (`say(\`${verbs.tap} again to ${what}\`)` with `verbs` from `chrome()`)
- Modify (`chrome()`, §4.3): `docs/contract.md:5264-5300` (the home capability's call table gains `chrome()` → `{platform, form, toolbar, pointer: "touch"|"mouse"}` and the `chrome` topic for `on`), `web/contract/clax-extensions.d.ts:340-405` (`interface Home`), `web/shell/src/caps/home.ts` (forwards `chrome` and the topic), `apps/clax_app/lib/home/home_host.dart:391-453` (`call` answers `chrome` from a `HomeChrome` value the app sets; emits `chrome` on change), `apps/clax_app/assets/kit/fake-home.js:87-135` (`chrome` with `set({chrome})`), the prompt copies (`sh scripts/sync-app-prompt.sh`)
- Test: `web/shell/src/caps/home.test.ts`, `apps/clax_app/test/home/home_host_test.dart`, `apps/clax_app/home/tests/home.test.js` and the count in `web/e2e/home-app.spec.ts:56`, `apps/clax_app/test/ui/comment_bar_test.dart`

**Interfaces:**
- Produces (Dart): `typedef HomeChrome = ({String platform, String form, bool toolbar, String pointer}); void HomeHost.setChrome(HomeChrome c);` — `platform` `"android"`, `"macos"` (and `"ios"` later), `form` `"phone"` below `ClaxWidth.medium` else `"wide"`, `toolbar` false until Task 47, `pointer` from `ClaxVerbs`' pointer tracking. The app sets it from its root's `MediaQuery` and pointer listener.
- Produces (page): `home.chrome(): Promise<{platform: string; form: "phone" | "wide"; toolbar: boolean; pointer: "touch" | "mouse"}>`, and `home.on("chrome", cb)`. The home app treats `unavailable` (an older app) as `{form: "phone", toolbar: false, pointer: "touch"}`.
- Consumes: Task 6 (`ClaxVerbs`).

- [ ] **Step 1: Read the threads** on each file (8c0474a5 on `home.ts`; cea3964d1b on `app.dart`).
- [ ] **Step 2: Failing tests:** `home.test.ts`: `chrome` is forwarded and its answer returned; `home_host_test.dart`: `call(home, 'chrome', {})` answers the set value and a change emits one `chrome` event; `home.test.js`: "Tap again to open" with `pointer: "touch"`, "Click again to open" with `"mouse"`, and "chrome() unavailable keeps the phone layout" (Review Focus 1); `comment_bar_test.dart`: the hint reads "Click anything to comment on it" after a mouse event.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=shell/src/caps/home.test.ts //apps/clax_app:home_home_host_test //apps/clax_app:ui_comment_bar_test //apps/clax_app:prompt_sync //web:e2e_rest --test_arg=--grep --test_arg='home app'` — FAIL; implement; rerun — PASS — plus `dart_analyze`, `//web:typecheck`.
- [ ] **Step 4: Commit**: "Say tap or click by the pointer in use, and tell the home app its platform, form and pointer through chrome()".

### Task 20: Tile labels, make tiles and the first run (§5.1)

**Files:**
- Modify: `apps/clax_app/home/grid.js` (export `tileLabel(text, {measure, width})`), `apps/clax_app/home/home.js:177-291` (`AppTile`, `MakeTile`: two-line labels from `tileLabel`; a make's request on line 1 and its state on line 2 in its colour; the breathing mark on `accent-tint` for a new make; an existing app's icon at full strength with a 3 dp `agent` ring and a "2/5" badge for a build; the `.h-over` 82% scrim at `:221` goes), `apps/clax_app/home/home.css:187-197` (`.h-label { overflow-wrap: normal; word-break: normal; hyphens: auto }`; `lang` on `<html>` in `index.html`), `apps/clax_app/lib/ui/launcher.dart:404-590` (`TileLabel`; `_AppTile`'s `surface @0.82` scrim at `:472-480` goes; `MakingTile`'s two lines and states, "Couldn’t make it" in `danger`), `apps/clax_app/lib/home/home_host.dart:256-264` and `apps/clax_app/lib/home/home_install.dart:113-147` (the ID being installed is excluded from `summaries` before `homeAppId` is saved), `ClaxBadge` for counts in both (99+)
- Create: `apps/clax_app/lib/ui/tile_label.dart`, `apps/clax_app/home_unit/label.test.js`, `apps/clax_app/test/ui/tile_label_test.dart`

**Resolves:** 6833bf1756, 7b568e1991 is an owner decision (kept "Offline"); H1, H4, H7.

**Interfaces:**
- Produces: `tileLabel(text: string, {measure: (s) => number, width: number}): {lines: string[], ellipsis: boolean}` (breaks only between words, at most two lines; a word wider than the cell is that word alone on one line, ellipsized) and its Dart twin `TileLabelLayout tileLabelLayout(String text, TextStyle style, double width, TextScaler scaler)` used by `class TileLabel extends StatelessWidget { const TileLabel(this.text, {super.key, this.color}); }`.
- Consumes: Tasks 6, 8.

- [ ] **Step 1: Read the threads** on the files (6833bf1756 on `app.dart:881-898`, drifted; 7b568e19 on `launcher.dart`).
- [ ] **Step 2: Failing tests:** `label.test.js`: "Weekly budget" fits one line; "A tide table for the harbour" breaks after "table"; "Donaudampfschifffahrtsgesellschaftskapitän" (42 characters) is one ellipsized line; nothing breaks mid-word. `tile_label_test.dart`: the same three, plus at `TextScaler.linear(2)` still two lines at most. `home_host_test.dart`: an artifact created by `ensureHomeApp` is absent from `summaries` before `homeAppId` is saved.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=../apps/clax_app/home_unit/label.test.js //apps/clax_app:ui_tile_label_test //apps/clax_app:home_home_host_test //apps/clax_app:ui_launcher_test` — FAIL; implement; rerun — PASS — plus `//web:e2e_rest --test_arg=--grep --test_arg='home app'`, `dart_analyze`.
- [ ] **Step 4: Look:** `//web:e2e_shots` scene `home-making` (a make, a build on an existing app, a long title) and the Flutter `launcher` shots; read phone and wide, both themes.
- [ ] **Step 5: Commit**: "Break tile labels between words only, and show makes and builds by state without scrims". Close 6833bf1756 separately.

### Task 21: Empty and error states, and the haikus' places (§5.10, §7.4)

**Files:**
- Modify: `apps/clax_app/lib/ui/error_screen.dart:1-72` (`ClaxEmpty`: "Clax couldn’t start", "It stopped while starting. Try again, or copy the details for a report.", **Try again** and Copy details; the log under a Details disclosure in `ClaxType.mono`, scrolling sideways with an edge fade; "The last lines clax wrote" goes), `apps/clax_app/lib/daemon/supervisor.dart:361` (the reason stays for the log only), `apps/clax_app/lib/ui/app_screen.dart:2055-2101` (`_cover`: loading shows the app's icon and title, the working mark after 400 ms; failure is `ClaxEmpty` with the app's icon, "This app didn’t load", "Clax’s service didn’t answer.", Try again and Back to Home; `net::ERR_*` under Details with Developer on), `apps/clax_app/lib/ui/launcher.dart:287-311` (load error "Couldn’t show your apps" / "Clax’s service didn’t answer." / Try again, no cloud icon, no New app; empty state with the mark, "Say what you need, and Clax makes it.", New app), `:1111-1124` ("This wasn’t finished." with **Make it again** and Cancel), `apps/clax_app/lib/ui/inbox_screen.dart:469-482,825` (empty: "All caught up." and a `HaikuLine`), `apps/clax_app/home/home.js:293` (`Message`: empty and error copy as the launcher's)
- Create: `apps/clax_app/test/ui/error_screen_test.dart`, `apps/clax_app/test/ui/shots/states_shots_test.dart`
- Test: `apps/clax_app/test/ui/app_screen_test.dart`, `apps/clax_app/test/ui/launcher_test.dart`, `apps/clax_app/test/ui/inbox_screen_test.dart`

**Interfaces:**
- Consumes: Tasks 6, 8, 13, 14 (`HaikuLine`), 15.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests:** the start error has **Try again** and Copy details (which puts the log on the clipboard: `tester.binding.defaultBinaryMessenger` mock for `Clipboard.setData`), the log closed under Details, and no "clax stopped"; the app cover shows no mark before 400 ms and the working mark after (`tester.pump`), and on failure the four texts with no `net::ERR` unless Developer is on; the launcher's error has no cloud icon and no New app; the empty inbox shows "All caught up." and one `HaikuLine`; every `ClaxEmpty`'s texts are centred.
- [ ] **Step 3: Run** the four tests — FAIL; implement; rerun — PASS — plus `//web:e2e_rest --test_arg=--grep --test_arg='home app'` and `dart_analyze`.
- [ ] **Step 4: Look:** `states_shots_test.dart`: start error, app loading, app failed, launcher empty, launcher error, inbox empty; read all six forms of each.
- [ ] **Step 5: Commit**: "Draw every empty and error state as one centred, plain state, with a haiku when the inbox is empty".

### Task 22: Making and the build view (§5.6)

**Files:**
- Modify: `apps/clax_app/lib/ui/making_screen.dart:150-161` (“A tide table for…” with the ellipsis inside the quotes, `title-1`, left-aligned, at most two lines), `:702` (an app-bar close (X) on the phone rather than "Back to apps"), `:803-810` (`shrinkWrap` goes), `:912-918` (the Live badge's dot inside it), a `HaikuLine` under the progress line after 8 s of waiting (`clock` parameter), `:1011` (copy), `apps/clax_app/lib/agent/progress.dart:123-148` (`toolLabel`: "Getting started", "Writing the page", "Checking it works", "Waiting for you"; paths only under Developer, passed in), `apps/clax_app/lib/ui/build_plan.dart:196-213` ("Building: Start and stop a session"), `:587-595` ("Added by you" in `caption`, `muted`, not a tertiary chip), `:442` (Task 15 already maps the error; the line reads "Couldn’t add it to the plan." with Details), `apps/clax_app/lib/agent/tools/finish.dart:419` (a new app stopped at a cap is titled from the request's first words, at most 40 characters, not "New app"), a `ClaxBanner` on a new app whose starting documents were refused ("Some of its starting data couldn’t be added." with Details; find the refused-docs result in `lib/agent/tools/finish.dart` `_create`)
- Test: `apps/clax_app/test/ui/making_screen_test.dart`, `apps/clax_app/test/ui/build_view_test.dart`, `apps/clax_app/test/agent/` (the finish tool's test file), `making_screen_shots_test.dart`, `build_view_shots_test.dart`

**Resolves:** 9b71118ca5, 3cb5573fc9 (if `grep` finds them in `.qual` files; else commit message only).

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests:** `makingTitle('A tide table for the harbour and the bay at dawn')` is `“A tide table for…”`; no `MaterialTapTargetSize.shrinkWrap` under `MakingScreen`; no haiku before 8 s, one after (`tester.pump(const Duration(seconds: 8))`), none in comment mode; `toolLabel` for a write of `index.html` is "Writing the page" and names the file only when asked for developer labels; `buildHeadline` reads "Building: …"; the person's feature shows "Added by you"; a cap stop on "a tide table for the harbour" titles the app "A tide table for the harbour".
- [ ] **Step 3: Run** them — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** the making and build shots in all forms, with the haiku state.
- [ ] **Step 5: Commit**: "Speak plainly while making and building, with the request in curly quotes and a haiku while waiting".

### Task 23: The inspector and the version page (§5.7)

**Files:**
- Modify: `apps/clax_app/lib/ui/app_inspector.dart:165-170` (tabs Overview, Versions, Permissions, Plan; Files, Code and Data only under Developer), each tab loading its own files with Try again in that tab alone, `:298-436` (`_Overview`: "Saves its own data", "Uses the internet: <hosts>", "Sends notifications" from the capabilities; the app bar reads "Inspect", the hero shows the icon and title), `:1031-1145` (`_Permissions`: no Coming rows unless Developer), `apps/clax_app/lib/ui/app_inspector_permissions.dart:186-192` (`ClaxMark` for Clax's reads, "Clax reads", not `smart_toy` and "Agent reads"), the Data tab's key/value table with a raw JSON toggle, `apps/clax_app/lib/ui/version_page.dart:403` (no note heading when none), `:564-569` ("Changed" when the sizes match)
- Test: `apps/clax_app/test/ui/app_inspector_test.dart`, `apps/clax_app/test/ui/version_page_test.dart`, `app_inspector_shots_test.dart`

**Resolves:** 6eb839c54f.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests:** four tabs with Developer off, seven with it on; a failed read of the plan's file shows Try again in Plan only while Overview still renders; Overview for `{db, net: ["api.weather.example"], notify}` lists the three plain lines; no "Coming" without Developer; the version page shows no "No note" and a same-size change reads "Changed".
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** the inspector shots, Overview and Permissions, all forms.
- [ ] **Step 5: Commit**: "Show the inspector’s plain tabs by default, each loading on its own". Close 6eb839c54f separately.

### Task 24: Sheets and dialogs (§5.8)

**Files:**
- Modify: `apps/clax_app/lib/ui/bundle_sheets.dart` (update sheets: the body says what each choice does, "Keep mine" and **Update** side by side, stacked full width when they do not fit; export: **Share** and "Share a copy with my data", "Include my data" goes, never two rows of three; the update progress sheet gains Cancel; "Tests" → "Checks"; Details a disclosure), `apps/clax_app/lib/ui/read_consent_dialog.dart:118-221` ("Let Clax read Groceries?", "Don’t allow" and **Allow once**, "Always for this app" a `ClaxCheckbox`; never "the agent"), the location consent dialog (`grep -rn "Precise" apps/clax_app/lib` finds it; Precise and Approximate as `ClaxRadio`s, "Don’t allow" and **Allow**), `apps/clax_app/lib/ui/change_sheet.dart` (Stop `ClaxButton.destructive`, Open thread tonal, both 40 dp; a handle and no X on the phone; "No errors" goes), the background run's notification text when it waits on consent: "Clax needs your OK" (`lib/platform/app_notice.dart` and its caller)
- Test: `apps/clax_app/test/ui/bundle_sheets_test.dart`, `read_consent_dialog_test.dart`, `change_sheet_shots_test.dart`, the location dialog's test, `bundle_sheets_shots_test.dart`

**Resolves:** c1dc9d72cc, 874d8a6753, e2b8fca37b.

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests** for each rule above: at 320 wide the update sheet's two buttons stack full width, at 412 they sit side by side; export shows no "Include my data"; the progress sheet has Cancel and cancelling stops the update (the fake updater records it); consent's title is one line containing "Clax"; the change sheet has no "No errors" and no close icon on the phone; the waiting notification says "Clax needs your OK".
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** the sheet shots in all forms.
- [ ] **Step 5: Commit**: "Give every sheet one primary action, short labels and bodies that say what each choice does". Close the three records separately.

### Task 25: Curly copy everywhere, and the copy check (§7.1)

**Files:**
- Create: `scripts/check-copy.py`, `scripts/test-check-copy.sh`
- Modify: `scripts/BUILD.bazel` (`sh_test(name = "check_copy")` running the check on `//apps/clax_app:copy_sources` and the self-test), `apps/clax_app/BUILD.bazel` (`filegroup(name = "copy_sources", srcs = glob(["lib/ui/**/*.dart", "lib/home/**/*.dart", "lib/agent/failure.dart", "lib/app.dart", "home/*.js"]))`), every file the check flags: the agent's sweep lists `lib/ui/making_screen.dart:1011`, `error_screen.dart:29`, `settings_screen.dart:139,478,481`, `bundle_sheets.dart:727,880,929,1087,1159,1280,1282`, `inbox_screen.dart:316,350,482`, `build_notices.dart:77`, `launcher.dart:292,531,839`, `lib/home/home_recovery.dart:105`, `lib/agent/failure.dart:12,45,93,173`, `lib/app.dart:346,1042,1398,1402,1469,1540` (rerun the check for the current list)

**Interfaces:**
- Produces `python3 scripts/check-copy.py <files…>`: for each Dart string literal (single-, double- and triple-quoted, raw included) and each JS string or template literal in `home/*.js`, fails on `n't`, `'s`, `'re`, `'ll`, `'ve`, `'d` with a straight apostrophe, on `...`, and on `Text('$e')`, `'$e'`, `"$e"`, `${e}` interpolations (`$e`/`${e}` where `e` is a `catch` variable: match `\$\{?(e|err|error)\}?` inside a literal), printing `path:line: <rule>: <literal>`; skips lines ending `// copy-check: allow` (for prompt text and stored-reply matching, Decision 11). Exit 1 on any finding.
- Consumes: every phase 3 task (it runs last so their copy is final).

- [ ] **Step 1: Failing self-test** `test-check-copy.sh`: fixtures under a temp dir: `Text('Don\'t')` fails; `Text('Don’t')` passes; `'Loading...'` fails; `Text('$e')` fails; `'$count apps'` passes; a line with `// copy-check: allow` passes; a JS template `` `Can't ${what}` `` fails. `bazelisk test //scripts:check_copy` — FAIL.
- [ ] **Step 2: Implement the check**; run it on the tree; fix every finding (curly ’, “ ”, …), marking the prompt and matcher lines with the allow comment.
- [ ] **Step 3: Run** `bazelisk test //scripts:check_copy //apps/clax_app:dart_analyze` and the suites whose expected strings changed (`bazelisk test //apps/clax_app:all` in the background, logged). Expected: PASS.
- [ ] **Step 4: Commit**: "Use curly apostrophes, quotes and ellipses in all app copy, checked by scripts/check-copy.py".

**Phase 3 ends** when `bazelisk test //scripts:check_copy` passes and the phase's renders have been read: no prototype text left.

---
## Phase 4: The Android phone (§3.1, §5.1–§5.4, §10.4)

### Task 26: Edge to edge, with the band in the page's colour

**Files:**
- Modify: `apps/clax_app/lib/main.dart` (`SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge)` and transparent bars on Android), `apps/clax_app/lib/ui/app_screen.dart:328-332` (`pillHeight`, `pillStrip` go), `:1872-1932` (`_page`: the SafeArea's `pillStrip` top goes; the page has the full height), `:811-967` (`_onMessage` handles `ChromeMsg`), `lib/ui/host_protocol.dart:12-90` (`ChromeMsg` in the sealed set), `web/bridge/src/` (a `chrome` sampler: on first paint and on a `ResizeObserver`/`MutationObserver` debounce of the elements at the viewport's top and bottom edges, the first opaque `background-color` walking up to `body` then `html`; posts `{top, bottom}` as `#rrggbb`), `web/shell/src/app/host.ts:149-207,436` (relays it as `chrome` with the nonce; header comment documents it), `docs/contract.md` (app mode's host messages, if listed there)
- Create: `apps/clax_app/lib/ui/system_bands.dart`, `apps/clax_app/test/ui/system_bands_test.dart`, `web/bridge/test/chrome.test.ts`
- Test: `apps/clax_app/test/ui/host_protocol_test.dart`, `web/shell/src/app/host.test.ts`

**Resolves:** c8cac2b3ff (with Task 28's pill: snapshot and live page share one frame).

**Interfaces:**
- Produces `final class ChromeMsg extends HostMessage { const ChromeMsg(this.top, this.bottom); final Color top, bottom; }` (parse refuses anything but `#rrggbb` with `TypeError`).
- Produces `class SystemBands extends StatelessWidget { const SystemBands({super.key, required this.top, required this.bottom, required this.child}); }`: paints the status-bar band in `top` and the navigation-bar band in `bottom` over the page's full height, with an `AnnotatedRegion<SystemUiOverlayStyle>` from `statusStyleFor` for each (Task 7). Before the first `ChromeMsg` both are the palette's `bg`.
- Consumes: Tasks 5, 7.

- [ ] **Step 1: Read the threads** on `app_screen.dart` (ten open: 2521833c, 86741946, 4c2bfc90, 098fa267, 664cefd4, 0fb89b9e, e6aad054, e2d39b88, 93b32b9e, 287cd716; none blocks this) and `host.ts`.
- [ ] **Step 2: Failing tests:** `chrome.test.ts` (jsdom): a page with `header { background: #102030 }` at the top reports `top: "#102030"`; a transparent element falls back to `body`'s colour, then `#ffffff` for nothing set; `rgba(…, 0.5)` is skipped as not opaque. `host.test.ts`: the bridge's chrome reaches the host as `{type: "chrome", top, bottom, nonce}`. `host_protocol_test.dart`: parses `#a0b0c0`, refuses `red`, `#abc`, a number. `system_bands_test.dart`: `top: Color(0xFFFFFFFF)` gives dark status icons, `Color(0xFF102030)` light; the page child's height equals the screen's.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=bridge/test/chrome.test.ts --test_arg=shell/src/app/host.test.ts //apps/clax_app:ui_host_protocol_test //apps/clax_app:ui_system_bands_test //apps/clax_app:ui_app_screen_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`, `//web:typecheck`.
- [ ] **Step 4: Look:** an app shot (`test/ui/shots/app_shots_test.dart`, created here: `AppScreen` over `FakeSurface` with a snapshot whose top is dark blue and bottom white) on the phone in both themes; the status band is the page's blue with light icons, the bottom band white with dark ones, and no strip.
- [ ] **Step 5: Commit**: "Draw the app edge to edge, painting the system bars in the page’s own top and bottom colours".

### Task 27: Predictive back

**Files:**
- Modify: `apps/clax_app/android/app/src/main/AndroidManifest.xml:30-38` (`android:enableOnBackInvokedCallback="true"` on the application), `apps/clax_app/lib/ui/app_screen.dart:774-780,1842-1853` (`PopScope(canPop: !_canGoBack && …)` and `onPopInvokedWithResult` turning a back inside the page into `GoBack()`), `apps/clax_app/tool/apk-check.sh` (checks the manifest attribute)
- Test: `apps/clax_app/test/ui/app_screen_test.dart`

**Interfaces:**
- Consumes: Task 5 (the predictive builder in `pageTransitionsTheme`).

- [ ] **Step 1: Failing tests:** with `HistoryMsg(canGoBack: true)` received, `tester.binding.handlePopRoute()` sends `GoBack` to the surface and keeps the route; with `false`, the route pops; `apk-check.sh` fails on an APK whose manifest lacks the attribute.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_app_screen_test` — FAIL; implement; rerun — PASS — and `//apps/clax_app:apk_test` (background).
- [ ] **Step 3: Commit**: "Go back predictively on Android, through the page’s own history first". The device check of the gesture is phase 8's.

### Task 28: The Clax pill, top toasts, the first-run hint and loading (§5.2)

**Files:**
- Modify: `apps/clax_app/lib/ui/working_chip.dart:33-588` (`WorkingPill` becomes `ClaxPill`; `WorkingChip`'s spinner at `:132-135` and the `BuildChip` spinners become `ClaxMark(state: working)`), `apps/clax_app/lib/ui/app_screen.dart:454-509,1084,1225-1264` (one `ClaxPill` at the top right, 8 dp under the status bar, replacing `_changePill`, `_topPill`, `_locationPill`, `_buildPill`), `:1432-1525` (`_snack`, `_toast`, `_offerNewer` show `ClaxToast(place: top)` over an app; a Restore or Undo suppresses "New version" until its own reload lands, 93b32b9ec2), `:2002-2053` (`_hintCard`: one line, one action, at the top: "Turn on notifications to comment from the shade." with **Turn on** (`AppNotice` permission request), or after a refusal "Use the Comment tab on the right to comment."), `:2165-2205` (`LocationPill` merges into the pill's "Location" state with its symbol and label)
- Modify: `apps/clax_app/lib/platform/settings_store.dart:31-111` (`List<String> rallied`, key `rallied`)
- Create: `apps/clax_app/test/ui/clax_pill_test.dart`
- Test: `working_chip_test.dart`, `app_screen_test.dart`, `app_shots_test.dart`

**Resolves:** 2521833ce5, 93b32b9ec2, 2634bc3a (its phone case), c8cac2b3ff (with Task 26).

Also here (§7.4): the mark's `landed` state on "New version ready", `sent` on the "Sent to Clax" toast (`lib/ui/thread_detail.dart:796`), and the app's Rally of 10: the first time an app reaches version 10 its toast reads "Version 10. A rally!" and the mark's `rally` state plays, once per app, remembered in `AppSettings` (`rallied: List<String>` of app IDs).

**Interfaces:**
- Produces `enum ClaxPillState { working, building, newVersion, location }` and `class ClaxPill extends StatefulWidget { const ClaxPill({super.key, required this.state, this.progress, this.onTap, this.onShow, this.clock = const SystemClock()}); }`: 32 dp to see in a 48 dp target, `fg` fill with `bg` text at level 2; "Working" with the breathing mark, "Building 2/5", "New version" with Show, "Location"; folds to a 32 dp mark-only circle after 3 s; hidden when nothing goes on; `Semantics(liveRegion: true, label: 'Clax is working')` and the others.
- Consumes: Tasks 7, 8, 26.

- [ ] **Step 1: Failing tests** (`clax_pill_test.dart`; and in `app_screen_test.dart` the rally: a `Landed` at version 10 shows "Version 10. A rally!" once, and a second app's version 10 shows it again for that app only): visible width with label at 0 s, 32 dp circle after `tester.pump(const Duration(seconds: 3))`; 48×48 hit target; at `TextScaler.linear(2)` the label is not clipped (`tester.getSize` of the `Text` within the pill); semantics label per state. `app_screen_test.dart`: one pill while working, no `pillStrip` offset, "New version ready" appears as a top toast; after `Restore` no "New version" until the reload's `Landed`; the hint offers **Turn on** when notifications are off and the Comment-tab line after a refusal.
- [ ] **Step 2: Run** them — FAIL; implement; rerun with `ui_working_chip_test`, `dart_analyze` — PASS.
- [ ] **Step 3: Look:** `app_shots_test.dart` states working, folded, building, new version, hint; phone, both themes, plus 200%.
- [ ] **Step 4: Commit**: "Replace the three pills and their strip with one Clax pill that folds to the mark, and show toasts at the top over an app". Close 2521833ce5 and 93b32b9ec2 separately.

### Task 29: The Comment tab, and Settings › Notifications

**Files:**
- Modify: `apps/clax_app/lib/ui/app_screen.dart:1995-2001,2103-2130` (`_handle` becomes `CommentTab`), `apps/clax_app/lib/platform/settings_store.dart` (`bool showCommentTab = false`, `double commentTabAt = 0.62`), `apps/clax_app/lib/ui/settings_screen.dart` (Notifications section: on or off, **Open settings** calling `openAppSettings` over `dev.empathic.clax/native` (`MainActivity.kt:70-77`), "Show the Comment tab")
- Create: `apps/clax_app/lib/ui/comment_tab.dart`, `apps/clax_app/test/ui/comment_tab_test.dart`
- Test: `apps/clax_app/test/ui/settings_test.dart`, `app_screen_test.dart`

**Resolves:** da2ee5a94f.

**Interfaces:**
- Produces `class CommentTab extends StatefulWidget { const CommentTab({super.key, required this.threads, required this.onComment, required this.onThreads, this.change, required this.at, required this.onMoved}); }` (§5.3: "Comment" with a 16 dp mark, 44 dp high in a 48 dp target, `fg` fill and `bg` text, level 2; right edge at `MediaQuery.systemGestureInsetsOf(context).right + 4`; at `at` × height; long-press then vertical drag moves it and `onMoved` saves; a tap opens a `ClaxMenu` with Comment, "Threads (n)", and the change in progress when there is one). Shown when notifications are off or `showCommentTab` is on.
- Consumes: Tasks 7, 8, 13.

- [ ] **Step 1: Read the threads** (da2ee5a94f).
- [ ] **Step 2: Failing tests:** with `systemGestureInsets.right = 32`, the tab's right edge is at `width - 36`; a horizontal drag does nothing, a long-press and vertical drag moves it and reports the new fraction; a tap opens the menu with three items when a change runs; at 200% the label is not clipped; Settings shows Notifications with Open settings (the channel mock records `openAppSettings`) and the switch; the tab shows with notifications on only when the switch is on.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_comment_tab_test //apps/clax_app:ui_settings_test //apps/clax_app:ui_app_screen_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** the tab over a white and over a black page, phone, both themes.
- [ ] **Step 5: Commit**: "Replace the edge handle with a labelled Comment tab outside the back-gesture zone". Close da2ee5a94f separately.

### Task 30: Comment mode (§5.3)

**Files:**
- Modify: `apps/clax_app/lib/ui/comment_bar.dart:81-165` (one row: the instruction, **Whole app**, Threads with its count, Voice, Close; 48 dp targets with 24 dp glyphs; Threads icon-only below 360 dp; the top rule and the touch icon in `tertiary` (`you`)), `apps/clax_app/lib/ui/comment_composer.dart:132-136,233-248` (the quote's rule in `tertiary`; "Your pick"; `ClaxTextField`; a page-opened composer labelled "From this app" with the app's icon), `lib/ui/host_protocol.dart` (`PageWaitMsg`), `web/shell/src/app/host.ts` (posts `pageWait` while comment mode or `use()` waits on a slow page), `apps/clax_app/lib/ui/app_screen.dart` (`PageWaitMsg` sets the bar's "Waiting for the page…"; `ClaxQuiet` around the page while comment mode is on, Task 8)
- Test: `comment_bar_test.dart`, `comment_composer_test.dart`, `host_protocol_test.dart`, `web/shell/src/app/host.test.ts`

**Resolves:** 5e0be2eadd, 0e9f60d93e; A2.

**Interfaces:**
- Produces `final class PageWaitMsg extends HostMessage { const PageWaitMsg(this.waiting); final bool waiting; }`; wraps the page in `ClaxQuiet(quiet: true)` (Task 8) while comment mode is on.
- Consumes: Tasks 6, 7, 8, 19 (`ClaxVerbs` hint).

- [ ] **Step 1: Read the threads** on each file.
- [ ] **Step 2: Failing tests:** the bar's rule colour equals `colorScheme.tertiary`; every control is at least 48×48; at 340 wide Threads shows icon and count only; the composer caption reads "Your pick" and a `byPage` composer "From this app"; `pageWait` parses and the bar shows "Waiting for the page…" until `waiting: false`; the shell posts `pageWait` true then false around a slow `use()` (fake timers).
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`, `//web:typecheck`.
- [ ] **Step 4: Look:** comment mode with the composer open, phone, both themes; and `//web:e2e_shots` scene `app-comment` (the pins and frame in `you`).
- [ ] **Step 5: Commit**: "Draw comment mode in people’s red-orange throughout, with a one-row bar and a Your pick composer". Close the two records separately.

### Task 31: Anchored menus and 48 dp targets

**Files:**
- Modify: `apps/clax_app/lib/ui/launcher.dart:312-350` (the tile's long-press passes the tile's global rect, not `Rect.zero`), `:1190-1258` (`HomeFlows._menu` is a `ClaxMenu` anchored at the rect: the home's items, then Open, Inspect, Share, Resume build; the bottom sheet and its title go), every `IconButton` and chip under `lib/ui/` given at least `kMinInteractiveDimension` (audit with a test that walks the launcher, app screen, inbox, thread and settings trees)
- Create: `apps/clax_app/test/ui/targets_test.dart`
- Test: `launcher_test.dart`, `apps/clax_app/test/home/home_menu_test.dart`

**Interfaces:**
- Consumes: Task 7 (`ClaxMenu`).

- [ ] **Step 1: Failing tests:** a long-press on a tile opens a menu whose first item is within 8 dp of the tile and no `BottomSheet`; the home page's `menu(id, rect, items)` opens it at `rect` (converted from page to global coordinates by `HomeHost`'s caller); `targets_test.dart` finds no tappable widget smaller than 48×48 on Android in the five screens.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 3: Commit**: "Open long-press menus at the tile, and give every control a 48 dp target on Android".

### Task 32: Inbox and the thread page (§5.4)

**Files:**
- Modify: `apps/clax_app/lib/ui/inbox_screen.dart:322-460,514-540,655-797` (the title "Inbox" without the count; an M3 `SearchBar` 56 dp at `radius-xl`; chips All, Unread, Questions, Changes; rows with a 40 dp app icon, the title in `headline`, one summary line naming the kind in words ("Clax asked: …", "Version 4 is ready"), the time trailing; unread a bold title and an 8 dp `you` dot, no row tint), `apps/clax_app/lib/ui/thread_detail.dart:127-…` (the app's name as title, "On the p95 chart · Version 3" as subtitle; "‹ Run 2 of 3 ›" with two icon buttons; content and reply bar at most 720 wide, centred; "Waiting for connection" with **Try now** tonal and Cancel plain)
- Create: `apps/clax_app/test/ui/shots/inbox_shots_test.dart`
- Test: `inbox_screen_test.dart`, `thread_detail_test.dart`

**Interfaces:**
- Consumes: Tasks 6, 7, 18 (titles from names).

- [ ] **Step 1: Read the threads** on both files.
- [ ] **Step 2: Failing tests:** no "Inbox ·"; a `SearchBar`; four chips; an unread row has a bold title, an 8 dp dot in `you` and no background tint; a question row's summary starts "Clax asked:"; the thread's subtitle is "Whole app · Version 1" for a whole-app thread; the run switcher reads "Run 2 of 3"; at 1280 wide the conversation is 720 wide.
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** `inbox_shots_test.dart` (inbox, a thread, the waiting state) at 100%, 130% and 200% on the phone, and the wide and mac forms.
- [ ] **Step 5: Commit**: "Lay out the inbox and threads as Material lists, titled by app and anchor".

### Task 33: The home screen's look (§5.1)

**Files:**
- Modify: `apps/clax_app/home/home.js:48-65` (Lucide's `EXTRA` and `svg()` go; Material Symbols Rounded glyphs as inline SVG paths in a new `apps/clax_app/home/symbols.js`, Apache 2.0, the notice added to `apps/clax_app/assets/kit/LICENSES.txt`), `:328-344` (`Bar`: 56 dp, the mark at 24 dp and "Clax" in `title-1`, search, inbox with `ClaxBadge`-style count (99+), settings, ⋮; the focus ring with 8 px padding, H8), `:2273` (the FAB: extended, `accent-tint`/`agent-ink`, mic and "New app"; phone only), banners (one layout for the key and recovery banners; recovery "Your home screen didn’t load" with a `danger` icon on `card`), `Folder` `:1566` (the rename field selects all and starts scrolled to the start, H6; the panel `raised` at level 2; scrim 40%/60%), notices `:2272` (full width less 16 px, max 560, H12), hover (tiles lift: scale 1.03, level 1, on a fine pointer, H3), `apps/clax_app/home/home.css` (the matching rules; `--h-*` aliases left for 1.6), `apps/clax_app/lib/ui/launcher.dart:235-311,789-930` (the fallback launcher matches: top bar with the mark, the FAB, `ClaxBanner` for key and recovery, the grid centred at max 960, icons 60 dp on the phone and 64 wide, cells 112)
- Test: `apps/clax_app/home/tests/home.test.js` (and the count), `launcher_test.dart`, `home_recovery_test.dart`

**Resolves:** eababe2045; H2, H3, H5 (phone part), H6, H8, H9, H10, H11, H12.

**Interfaces:**
- Produces `apps/clax_app/home/symbols.js`: `export function symbol(name: string, size?: number): string` for the names the home app uses (search, inbox, settings, more_vert, mic, home, folder, star, history, key, error, refresh, close).
- Consumes: Tasks 4, 7, 8, 20.

- [ ] **Step 1: Read the threads** on `home/` and `launcher.dart` (eababe20: Lucide vs Material glyphs).
- [ ] **Step 2: Failing harness tests:** no element carries a Lucide path (`stroke-linecap` attributes absent from the bar's SVGs); the bar shows the mark and "Clax"; the inbox badge reads "99+" at 120; the rename field's `selectionStart` is 0 and `selectionEnd` its length on open, `scrollLeft` 0; a notice is at most 560 wide on a 1280 page. `launcher_test.dart`: the grid's width is at most 960 on a 1280 screen; icons 64 dp at wide; banners are `ClaxBanner`.
- [ ] **Step 3: Run** `bazelisk test //web:e2e_rest --test_arg=--grep --test_arg='home app' //apps/clax_app:ui_launcher_test //apps/clax_app:home_home_recovery_test` — FAIL; implement; rerun — PASS — plus `//apps/clax_app:home_home_bundle_test`, `dart_analyze`.
- [ ] **Step 4: Look:** `//web:e2e_shots` home scenes (home, folder open, menu open, banner, empty, error) and the Flutter launcher shots, all forms, both themes; the two homes look alike.
- [ ] **Step 5: Commit**: "Give the home screen Material Symbols, the mark in its bar, a tonal New app button and the brand’s banners, folders and notices". Close eababe2045 separately.

**Phase 4 ends** with the phone renders of Tasks 26–33 read at 100%, 130% and 200%.

---

## Phase 5: Native overlays (§5.9, §6, §10.5)

### Task 34: The host messages in Dart, and one overlay at a time

**Files:**
- Modify: `apps/clax_app/lib/ui/host_protocol.dart:12-90` (parse `ask`, `askCancel`, `draft`, `gone`; `QuestionsMsg` gains `ids` and `raise` at `:444-452`; `Notice` gains `tone` and `action` at `:299-303`), `:483-861` (calls `AskAnswer`, `NoticeAction`, `ShowDraft`, `HideDraft`; `Init` at `:598-612` gains `native` and `platform`, edited into the IIFE's `{nonce:n}` literal at `:607` and its arguments at `:611`), `:876-983` (`ShellSession.documentLoaded` passes them)
- Create: `apps/clax_app/lib/ui/overlay_gate.dart`, `apps/clax_app/test/ui/overlay_gate_test.dart`
- Test: `apps/clax_app/test/ui/host_protocol_test.dart`

**Interfaces:**
- Produces (§6.2, §6.3):
  ```dart
  enum AskKind { capability, download, crossRead, sample, comments }
  final class AskMsg extends HostMessage { const AskMsg({required this.id, required this.kind, required this.title, required this.body, required this.allow, required this.deny, this.always, required this.armMs}); final String id; final AskKind kind; final String title, body, allow, deny; final String? always; final int armMs; }
  final class AskCancelMsg extends HostMessage { const AskCancelMsg(this.id); final String id; }
  // QuestionsMsg(open, {showing, ids, raise}): List<String> ids, bool raise
  enum NoticeTone { info, error }
  // Notice(text, {tone = NoticeTone.info, ({String id, String label})? action})
  sealed class DraftState {} final class Drafting extends DraftState { Drafting(this.by, this.note); } final class Showing extends DraftState { Showing(this.by, this.base, this.detail); }
  final class DraftMsg extends HostMessage { const DraftMsg(this.state); final DraftState? state; }
  final class GoneMsg extends HostMessage { const GoneMsg(); }
  enum AskAnswerValue { allow, deny, always, dismiss }
  final class AskAnswer extends HostCall { const AskAnswer(this.id, this.answer); }   // window.claxHost?.askAnswer(id, "allow")
  final class NoticeAction extends HostCall { const NoticeAction(this.id); }
  final class ShowDraft extends HostCall { const ShowDraft([this.by]); } final class HideDraft extends HostCall { const HideDraft(); }
  // Init(nonce, {visible, waitMs, native = const <String>[], platform}) → h.init({nonce:n, native:[…], platform:"android"})
  enum OverlayKind { composer, ask, questions }
  class OverlayGate extends ChangeNotifier { bool request(OverlayKind k); void release(OverlayKind k); OverlayKind? get showing; Stream<OverlayKind> get freed; }
  ```
  `OverlayGate` grants one overlay at a time; `questions` waits while `composer` or `ask` shows and is granted when they release (8fa8b99217's rule); `ask` waits only for another `ask`.
- Consumes: nothing beyond phase 4.

- [ ] **Step 1: Read the threads** on `host_protocol.dart`.
- [ ] **Step 2: Failing tests** in `host_protocol_test.dart`, one per message: valid shapes parse; each bad shape (a missing `id`, an unknown `kind`, `armMs` as a string, `ids` not a list of strings, `tone: "loud"`, a `draft` with `kind: "other"`) returns null through the `TypeError` path (`fromJson`'s try at `:85-89`); `AskAnswer('a1', AskAnswerValue.always).toJs()` is `window.claxHost?.askAnswer("a1","always")`; `Init(…, native: ['ask','questions','notice','draft'], platform: 'android').toJs()` contains `native:["ask","questions","notice","draft"]` and `platform:"android"`. `overlay_gate_test.dart`: questions requested while composer shows is not granted, and is granted (via `freed`) when the composer releases.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_host_protocol_test //apps/clax_app:ui_overlay_gate_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Commit**: "Parse the shell’s native overlay messages and answer them, with one overlay shown at a time".

### Task 35: The shell's app mode declares native overlays

**Files:**
- Modify: `web/shell/src/app/host.ts:149-300,422-945` (`init({nonce, native?, platform?})`: store the declared set; with `ask` native, the controller's prompt queue posts `ask` instead of setting `state.ask`, and `askAnswer(id, a)` resolves it; `askCancel` on unload or page loss; a late answer after `askCancel` is ignored; with `questions` native, the dock at `:869-891` is never built and `questions` carries `ids` and `raise`; with `notice` native, `notice {text, tone, action?}` and nothing drawn; `noticeAction(id)` runs it; with `draft` native, `draft {state}` from `view/draft-model.ts` and `showDraft`/`hideDraft` call `ctl.showDraftOf`/`ctl.backFromDraft`; `gone` when the artifact is deleted (`view/artifact-controller.ts:62,684`); `platform: "macos"` sets `cursor: default` on controls (§4.10)), `web/shell/src/view/prompt-queue.ts:3-18` (an `Ask` carries `{id, kind, always?}`), `web/shell/src/caps/grants.ts:10-11,104-126`, `caps/sample.ts:53`, `caps/downloads.ts:109`, `caps/net.ts:88`, `caps/cross-read.ts:88-109` (each prompt names its `kind`; persisting paths give `always`; a `dismiss` never stores a refusal, 44415b8019), `web/shell/src/view/artifact-controller.ts:319,656`, `web/shell/src/ui/StageIsland.svelte:70-90` (each web overlay renders only when not native), `docs/contract.md` and `web/contract/clax-extensions.d.ts` only if app mode's host messages are documented there (`grep -n "claxHost" docs/contract.md`)
- Test: `web/shell/src/app/host.test.ts`, `web/shell/src/view/prompt-queue.test.ts`, `web/shell/src/caps/grants.test.ts`, `web/shell/src/caps/cross-read.test.ts` (create if absent)

**Resolves:** 44415b8019 (in the app; the browser keeps its dialog), 6a28a0de6a with Task 37.

**Interfaces:**
- Produces the shell side of §6.2/§6.3 exactly as Task 34's Dart names them; `ALLOW_DELAY_MS` (500) is posted as `armMs`.
- Consumes: Task 34 (names only; the two can be written side by side).

- [ ] **Step 1: Read the threads** on each file (44415b8019 on `cross-read.ts:88-92`, 8fa8b99217 on `web/shell/src/app/app.css:29-39`).
- [ ] **Step 2: Failing vitest cases** (`host.test.ts`, under a new `describe("native overlays")`, fake timers): for each of the four, declared → nothing drawn and the message posted with the nonce; not declared → the web version drawn (Review Focus 1: an `init({nonce})` with no `native` draws all four); two queued prompts, `askAnswer` of the second's ID before the first is answered is ignored and the first resolves on its own ID; `askCancel` posted on unload, and an `askAnswer` after it changes nothing; `always` stores the grant as Allow-with-persist does; `dismiss` on a cross-read prompt stores nothing and the next `db.artifact()` asks again; `gone` posted once on delete.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=shell/src/app/host.test.ts --test_arg=shell/src/view/prompt-queue.test.ts --test_arg=shell/src/caps` — FAIL; implement; rerun — PASS — plus `//web:typecheck //web:lint`.
- [ ] **Step 4: Commit**: "Let the app’s host draw the prompt, questions, notices and draft banner, and post each to it". Close 44415b8019 separately (the app's half; note the browser's in the close message).

### Task 36: The native permission prompt (Android)

**Files:**
- Create: `apps/clax_app/lib/ui/ask_prompt.dart`, `apps/clax_app/test/ui/ask_prompt_test.dart`
- Modify: `apps/clax_app/lib/ui/app_screen.dart:811-967` (`AskMsg` → `OverlayGate.request(ask)` → `AskPrompt.show`; the answer becomes `AskAnswer`; `AskCancelMsg` closes it), `apps/clax_app/lib/ui/webview_surface.dart:152-155` (`Init` with `native: ['ask', 'questions', 'notice', 'draft']` and the platform)

**Resolves:** A1, A3 (audit codes); the app's half of S5.

**Interfaces:**
- Produces `abstract final class AskPrompt { static Future<AskAnswerValue> show(BuildContext context, AskMsg m, {Clock clock = const SystemClock()}); }`: an M3 `AlertDialog` route; "Don’t allow" focused; **Allow** disabled for `armMs` from the dialog's first frame, with "Allow in a moment…" under it until it arms; Allow counts only for a pointer whose down came after arming, or a Return/Space key whose down came after arming; `always` as a `ClaxCheckbox` ("Always for this app") whose checked Allow answers `always`; Escape or a tap outside answers `dismiss`.
- Consumes: Tasks 7, 34, 35.

- [ ] **Step 1: Failing tests** (`ask_prompt_test.dart`, `tester.pump` with an injected clock): Allow disabled at 0 and 499 ms, "Allow in a moment…" shown; a pointer down at 499 ms and up at 520 ms answers nothing; a tap at 600 ms answers `allow`; a Return key down at 400 ms and up at 600 ms answers nothing, a Return down at 501 ms answers `allow`; Escape answers `dismiss`, "Don’t allow" `deny`; with `always` checked, Allow answers `always`; the first frame focuses "Don’t allow".
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_ask_prompt_test` — FAIL; implement; wire into `app_screen.dart`; rerun with `ui_app_screen_test`, `dart_analyze` — PASS.
- [ ] **Step 3: Look:** the prompt before and after arming, phone and wide, both themes.
- [ ] **Step 4: Commit**: "Ask for capabilities in a native dialog that arms after half a second and accepts only presses that began after it".

### Task 37: The questions sheet, native (§5.9 Questions)

**Files:**
- Create: `apps/clax_app/lib/ui/questions/question_card.dart`, `apps/clax_app/lib/ui/questions/questions_sheet.dart`, `apps/clax_app/test/ui/questions_sheet_test.dart`, `apps/clax_app/test/ui/shots/questions_shots_test.dart`
- Modify: `apps/clax_app/lib/app.dart:1823` and `:2120-2128` (open `QuestionsSheet` for the app's artifact on `QuestionsMsg(raise: true)`, through `OverlayGate`), `apps/clax_app/lib/ui/app_screen.dart:926-937` (likewise; the "Clax has a question" toast stays as the quiet path when `raise` is false)
- Delete: `apps/clax_app/lib/ui/question_sheet.dart` (225 lines) and `apps/clax_app/test/ui/question_sheet_test.dart`, once nothing imports them

**Resolves:** 6a28a0de6a, 8fa8b99217; A4, A5, A6.

**Interfaces:**
- Produces `class QuestionCard extends StatefulWidget { const QuestionCard({super.key, required this.question, required this.onAnswer, required this.onSkip}); }` (single and multiple choice as rows with label and detail, "Other…" with a `ClaxTextField`, Skip and **Answer**; "Clax" in every line) and `abstract final class QuestionsSheet { static Future<void> show(BuildContext context, {required ClaxApi api, required String artifactId, String? focus}); }` (titled with `ClaxMark(state: asking)` beside "Clax has a question"; reads `api.questions()` filtered on `QuestionView.artifactId` (`lib/clax/models.dart:786,852`), answers through `api.answerQuestion` and `api.declineQuestion`; a 409 `question_closed` drops that card; compact: a bottom sheet titled "Clax has a question", `radius-xl`, handle; medium and up: a centred dialog; apple at medium and up: a 560 sheet).
- Consumes: Tasks 6, 7, 34, 35.

- [ ] **Step 1: Read the threads** (6a28a0de6a on `question_sheet.dart:45-73` and its three comments; 8fa8b99217).
- [ ] **Step 2: Failing tests** over `FakeApi` (`askQuestion` then `holdQuestions` as its tests use them): two questions for the artifact and one for another show two cards; choosing a single-choice row and **Answer** calls `answerQuestion` with `{selected: [label], text: ''}`; Skip calls `declineQuestion`; "Other…" sends its text; the sheet shows no "app-2" and no "in tmp"; with the composer granted in `OverlayGate`, a `raise` waits and opens when the composer closes; a 409 removes the card. At `TextScaler.linear(2)` the Answer button is reachable by scrolling.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_questions_sheet_test` — FAIL; implement; switch the callers; delete the old sheet and test; rerun with `root_app_test`, `ui_app_screen_test`, `dart_analyze` — PASS.
- [ ] **Step 4: Look:** `questions_shots_test.dart` at 100/130/200% on the phone, and wide and mac.
- [ ] **Step 5: Commit**: "Answer Clax’s questions in a native sheet read from the daemon, one overlay at a time". Close the two records separately.

### Task 38: Notices, the draft banner and a deleted app, native

**Files:**
- Modify: `apps/clax_app/lib/ui/app_screen.dart:922-925` (`Notice` → `ClaxToast(place: top)` on compact, `inline` from medium; an `action` shows its label and calls `NoticeAction`; `error` tone uses the `danger` icon), `_onMessage` (`DraftMsg` → a 48 dp `ClaxBanner` under the status bar: "Clax is drafting…" with **Show draft**, or "Showing Clax’s draft" with **Back**; `ShowDraft`/`HideDraft`; `GoneMsg` → `ClaxEmpty` "This app was deleted." with Back to Home, and the app leaves the home's lists)
- Create: `apps/clax_app/test/ui/native_overlays_test.dart`

**Interfaces:**
- Consumes: Tasks 7, 34, 35.

- [ ] **Step 1: Failing tests:** a `Notice('Saved', action: (id: 'n1', label: 'Undo'))` shows a top toast whose Undo sends `NoticeAction('n1')`; `DraftMsg(Drafting('Clax', null))` shows "Clax is drafting…" and Show draft sends `ShowDraft()`; `DraftMsg(Showing(…))` shows Back which sends `HideDraft()`; `DraftMsg(null)` removes it; `GoneMsg` shows "This app was deleted." and Back to Home pops to home.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_native_overlays_test` — FAIL; implement; rerun with `ui_app_screen_test`, `dart_analyze` — PASS.
- [ ] **Step 3: Look:** each, phone and wide, both themes.
- [ ] **Step 4: Commit**: "Draw the shell’s notices, the draft banner and a deleted app natively".

### Task 39: `NativeDialogs` on the Mac (alert and menu)

**Files:**
- Create: `apps/clax_app/macos/Runner/NativeDialogs.swift` (channel `dev.empathic.clax/dialogs`: `menu(items, at)` pops up an `NSMenu` at the point in the key window and answers the chosen ID or null; `alert({style, title, body, buttons, defaultIndex, cancelIndex, armMs?, suppression?})` runs an `NSAlert` as a sheet on the key window and answers `{index, suppressed}`; with `armMs` the default button is disabled from the sheet's first display until then), `apps/clax_app/lib/ui/kit/native_dialogs.dart`, `apps/clax_app/test/ui/kit/native_dialogs_test.dart`
- Modify: `apps/clax_app/macos/Runner/MainFlutterWindow.swift:7-84` (register it beside `installCues`), `apps/clax_app/macos/Runner.xcodeproj/project.pbxproj`, `apps/clax_app/lib/ui/kit/dialog.dart` and `kit/menu.dart` (route to `NativeDialogs` when `hasDesktopWindowChrome`), `apps/clax_app/lib/ui/ask_prompt.dart` (the Mac path: `alert` with `armMs` and the "always" choice as `suppression`)

**Interfaces:**
- Produces:
  ```dart
  abstract final class NativeDialogs {
    static Future<String?> menu(List<ClaxMenuItem> items, Offset at); // `at` in the window's Flutter coordinates
    static Future<({int index, bool suppressed})> alert({required String style /* 'informational'|'warning'|'critical' */, required String title, String? body, required List<String> buttons, int? defaultIndex, int? cancelIndex, int? armMs, String? suppression});
  }
  ```
  The channel is named `dev.empathic.clax/dialogs` because `dev.empathic.clax/native` already carries `snapshotWebView` and `openExternal` (`MainFlutterWindow.swift:21-74`); §4.9's name is taken.
- Consumes: Tasks 7, 36.

- [ ] **Step 1: Read the threads** on `MainFlutterWindow.swift`.
- [ ] **Step 2: Failing tests** (`native_dialogs_test.dart`, a mock channel handler): `menu` sends `{items: [{id, title, key, symbol, destructive, enabled} | {separator: true}], at: {x, y}}`; `alert` sends every argument and decodes `{index, suppressed}`; on macOS `ClaxDialog.show` and `ClaxMenu.show` call the channel and on Android never do (`debugDefaultTargetPlatformOverride`).
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_kit_native_dialogs_test` — FAIL; implement Dart and Swift; rerun — PASS — and `bazelisk build //apps/clax_app:macos_app` — PASS.
- [ ] **Step 4: Look:** in the built app (scratch `CLAX_HOME`), open an app, trigger a capability prompt from the kit example's net call, and capture the window (`screencapture -l<id>`) before and after arming into `target/flutter-app/shots/ask-mac-{0,1}.png`; read them. (Task 52 automates this.)
- [ ] **Step 5: Commit**: "Show menus and alerts on the Mac as NSMenu and NSAlert sheets, with an armed default button for prompts".

### Task 40: App mode end to end with a fake host

**Files:**
- Modify: `web/e2e/app-host.ts:29` (`initHost(page, {native?, platform?})`), `web/e2e/app-mode.spec.ts` (a `describe("native overlays")`)

**Interfaces:**
- Consumes: Task 35.

- [ ] **Step 1: Failing spec:** the fake host declares all four; a kit page calls `claude.use("net")` → the host receives `ask` with `kind: "capability"` and `armMs: 500`, no dialog is in the DOM; the host answers `askAnswer(id, "allow")` and the page's call resolves; a notice reaches the host as `notice` and nothing draws; an agent draft posts `draft`; with no `native`, the web dialog shows (`page.clock` for the arming).
- [ ] **Step 2: Run** `bazelisk test //web:e2e_rest --test_arg=--grep --test_arg='native overlays'` — FAIL; implement the fake host's side; rerun — PASS.
- [ ] **Step 3: Commit**: "Test app mode’s native overlays end to end against a fake host".

---
## Phase 6: The Mac, and the split layout every wide window shares (§3.3, §4, §10.6)

The split layout (`lib/ui/split/`) is driven by `ClaxWidth`, and its look by `ClaxFamily`: Tasks 45–48 and 51 build it once for every window at `ClaxWidth.medium` and up, and Phase 7 gives it its Material form. Only Tasks 42–44, 49, 50's shortcuts and 52 are macOS window chrome (`hasDesktopWindowChrome`).

### Task 41: Spike: further windows on one engine (§4.7)

**Files:**
- Modify (on the branch, kept only if the spike succeeds): `apps/clax_app/macos/Runner/AppDelegate.swift` (`openWindow(kind:)` makes an `NSWindow` with `FlutterViewController(engine: <the main window's engine>, nibName: nil, bundle: nil)`), `apps/clax_app/lib/main.dart` (`runWidget(ClaxViews(…))` rendering a `View` per `PlatformDispatcher.instance.views`, the first the main window, others keyed by an initial route the native side sends on `dev.empathic.clax/windows`)
- Create: `apps/clax_app/lib/ui/mac/windows.dart` (`MacWindows.open(String kind)`, `kind` `settings` or `inbox`)

**Interfaces:**
- Produces (if it works): `abstract final class MacWindows { static Future<void> open(String kind); static Stream<String> get opened; }` and the `ClaxViews` root that later tasks extend; services (`AppServices`, `HomeHost`, the API client, settings) are created once and shared by every view.
- Consumes: phase 5.

- [ ] **Step 1: Read the threads** on `AppDelegate.swift`, `main.dart`, `app.dart`.
- [ ] **Step 2: Build the smallest proof:** ⌘, opens a second window showing `Text('Settings')` under the shared `HomeHost`; the main window keeps working; closing it frees its view. `bazelisk build //apps/clax_app:macos_app`, run it with a scratch `CLAX_HOME`, capture both windows to `target/flutter-app/shots/spike-*.png`, read them.
- [ ] **Step 3: Record the outcome** on `apps/clax_app/lib/ui/mac/windows.dart:1` as a qualifier `alternative` (works: "Further windows are FlutterViewControllers on the one engine; Flutter's RegularWindowController (flag-gated, @internal in 3.47.7) was not used. revisit: when windowing is public in a pinned Flutter") or, if it fails, a `concern` tagged `status:needs-decision` naming what failed, and switch Tasks 49's windows to window sheets.
- [ ] **Step 4: Commit** the working proof (or only the record): "Open further Mac windows as views of the one Flutter engine".

### Task 42: The main window, and Escape from the page

**Files:**
- Modify: `apps/clax_app/macos/Runner/MainFlutterWindow.swift:7-84` (1200×800 default, 720×480 minimum, centred on first launch, `setFrameAutosaveName("ClaxMain")`, `titlebarAppearsTransparent`, `.fullSizeContentView`, `toolbarStyle = .unified`; an `NSVisualEffectView(material: .sidebar)` behind the Flutter view, whose background is clear where the sidebar draws, sized from a `setSidebarWidth` call on `dev.empathic.clax/window`; Reduce Transparency gives the sidebar solid `bg`; a local `NSEvent` monitor for Escape that, while a `WKWebView` is first responder and Dart reported a panel or mode open (`setEscapeWanted(bool)`), sends `escape` to Dart and swallows the event), `apps/clax_app/macos/Runner/Base.lproj/MainMenu.xib:333-335` (window rect 1200×800, style mask with full-size content view)
- Create: `apps/clax_app/lib/ui/mac/window_channel.dart`, `apps/clax_app/test/ui/mac/window_channel_test.dart`
- Modify: `apps/clax_app/lib/ui/app_screen.dart:446-450,576-580` (report `setEscapeWanted` when the panel, comment mode or composer opens or closes; on `escape`, run the same close as the Flutter Escape shortcut at `:1826-1836`)

**Resolves:** 098fa2674e.

**Interfaces:**
- Produces `abstract final class MacWindow { static Future<void> setSidebarWidth(double w); static Future<void> setEscapeWanted(bool on); static Future<void> setTitle(String title, {String? subtitle}); static Stream<void> get escapes; }` over `dev.empathic.clax/window` (no-ops unless `hasDesktopWindowChrome`).
- Consumes: Task 41.

- [ ] **Step 1: Read the threads** on both Swift files and `app_screen.dart`.
- [ ] **Step 2: Failing tests** (mock channel): opening comment mode sends `setEscapeWanted(true)` and closing it `false`; an `escape` from the channel closes the panel; `setTitle('Groceries', subtitle: 'Version 4')` sends both.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_mac_window_channel_test` — FAIL; implement; rerun — PASS — and `bazelisk build //apps/clax_app:macos_app`.
- [ ] **Step 4: Look:** run the app; capture the window: a unified, transparent title bar, the sidebar material where the sidebar will be (a placeholder width of 220 until Task 46).
- [ ] **Step 5: Commit**: "Give the Mac window its size, saved frame, unified title bar and sidebar material, and catch Escape from the page". Close 098fa2674e separately with Task 52's interaction run as evidence, once it exists; until then record the manual capture in the close message.

### Task 43: The native menu bar, Edit and Back

**Files:**
- Modify: `apps/clax_app/macos/Runner/Base.lproj/MainMenu.xib:23-332` (§4.8's menus: Clax, File, Edit, View, App, Window, Help, with key equivalents; Edit's items send `cut:`, `copy:`, `paste:`, `pasteAsPlainText:`, `delete:`, `selectAll:`, `undo:`, `redo:`, `performFindPanelAction:`; Clax's own items send `claxMenuAction:` with their tag), `apps/clax_app/macos/Runner/AppDelegate.swift` (install `MenuBridge` and `EditBridge`), `apps/clax_app/lib/ui/app_menu.dart:100-185` (`ClaxMenuBar` stops building `PlatformMenuBar` when `hasDesktopWindowChrome`; it listens on `dev.empathic.clax/menu` and sets titles and enabled state), `apps/clax_app/lib/app.dart` (`escapeShortcuts`/`_back` route Back ⌘[ through page history then selection; Home ⇧⌘H to the grid), `project.pbxproj`
- Create: `apps/clax_app/macos/Runner/MenuBridge.swift`, `apps/clax_app/macos/Runner/EditBridge.swift`, `apps/clax_app/lib/ui/shortcuts.dart`, `apps/clax_app/test/ui/app_menu_test.dart` (exists; extend), `apps/clax_app/test/ui/shortcuts_test.dart`

**Resolves:** 128c1f7ff7, 32f8f89d40.

**Interfaces:**
- Produces (Dart):
  ```dart
  enum ClaxCommand { newApp, importApp, shareApp, toggleSidebar, comment, toggleThreads, reload, actualSize, zoomIn, zoomOut, back, forward, home, inspect, versions, star, resumeBuild, inbox, settings, deleteApp, openSelected, starred1, starred2, starred3, starred4, starred5, starred6, starred7, starred8, starred9 }
  /// The key for each command: Meta on the Apple family, Control on Material (Chromebooks, Task 56).
  Map<ShortcutActivator, Intent> claxShortcuts(ClaxFamily family);
  final class ClaxCommandIntent extends Intent { const ClaxCommandIntent(this.command); final ClaxCommand command; }
  abstract final class MacMenu { static void setEnabled(ClaxCommand c, bool on); static void setTitle(ClaxCommand c, String title); static Stream<ClaxCommand> get commands; }
  ```
  `EditBridge.swift`: when the Flutter view is first responder, `copy:` and the rest go to Dart over `dev.empathic.clax/edit`, which invokes the focused field's `CopySelectionTextIntent`, `PasteTextIntent`, `SelectAllTextIntent`, `UndoTextIntent`, `RedoTextIntent`; a `WKWebView` first responder gets them natively.
- Consumes: Tasks 41, 42.

- [ ] **Step 1: Read the threads** on `app_menu.dart` (32f8f89d40), `launcher.dart` (128c1f7ff7), `MainMenu.xib`.
- [ ] **Step 2: Failing tests:** `shortcuts_test.dart`: `claxShortcuts(apple)` maps ⌘K to `toggleThreads`, ⇧⌘K to `comment`, ⌘[ to `back`, ⇧⌘H to `home`, ⌘1 to `starred1`; `claxShortcuts(material)` uses Control for each. `app_menu_test.dart`: on macOS no `PlatformMenuBar` is built; a `commands` event of `home` from the mock channel pops to the grid from two screens deep; `importApp` calls `HomeFlows.importApp`; Back with page history sends `GoBack` before popping.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_app_menu_test //apps/clax_app:ui_shortcuts_test` — FAIL; implement; rerun — PASS — and the macOS build.
- [ ] **Step 4: Look:** run the app; open each menu and capture it (`screencapture -R` of the menu's region) into `target/flutter-app/shots/menu-*.png`; read them against §4.8.
- [ ] **Step 5: Commit**: "Draw the Mac’s menu bar natively, with Edit through the responder chain and Clax’s commands over a channel". Close the two records separately.

### Task 44: The toolbar

**Files:**
- Create: `apps/clax_app/macos/Runner/Toolbar.swift` (`MacToolbar`: an `NSToolbar` whose items Flutter declares over `dev.empathic.clax/toolbar`: `setItems([{id, symbol, label, enabled, on?, badge?}])`; `NSSearchToolbarItem` for search; a 28 pt custom item drawing the working state from a `working` flag with its own Core Animation breath; clicks and search text answer as `clicked {id}` and `search {text}`; "Icon and Text" labels), `apps/clax_app/lib/ui/mac/mac_toolbar.dart`, `apps/clax_app/test/ui/mac/mac_toolbar_test.dart`
- Modify: `apps/clax_app/macos/Runner/MainFlutterWindow.swift` (install `MacToolbar`), `project.pbxproj`

**Interfaces:**
- Produces:
  ```dart
  final class ToolbarItemSpec { const ToolbarItemSpec(this.id, {required this.symbol, required this.label, this.enabled = true, this.on = false, this.badge}); final String id, symbol, label; final bool enabled, on; final int? badge; }
  abstract final class MacToolbar { static Future<void> setItems(List<ToolbarItemSpec> items); static Future<void> setWorking(bool working); static Stream<String> get clicks; static Stream<String> get searches; }
  ```
  §4.4's items: `sidebar` (`sidebar.left`), `back`/`forward` (`chevron.left`/`chevron.right`), `working`, `newVersion` (`arrow.down.circle`), `comment` (`text.bubble`, filled when on), `threads` (`list.bullet.rectangle` with the count), `share` (`square.and.arrow.up`, the `NSSharingServicePicker` with the exported `.clax` path Dart passes), `search`.
- Consumes: Task 42.

- [ ] **Step 1: Failing tests** (mock channel): `setItems` sends each spec; a `clicked {id: "threads"}` reaches `clicks`; `share` sends the bundle path from `BundleFlows.share` (`lib/ui/bundle_sheets.dart:1040`) with `toFile` (`app.dart:896-900`).
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:ui_mac_mac_toolbar_test` — FAIL; implement; rerun — PASS — and the macOS build.
- [ ] **Step 3: Commit**: "Add the Mac’s unified toolbar, declared from Flutter, with SF Symbols and a working item".

### Task 45: `HomeLayout`: the arrangement in Dart

**Files:**
- Create: `apps/clax_app/lib/home/home_layout.dart`, `apps/clax_app/test/home/home_layout_test.dart`
- Modify: `apps/clax_app/lib/home/home_host.dart` (owns a `HomeLayout` for the home app's artifact once `homeAppId` is known)

**Interfaces:**
- Produces:
  ```dart
  final class LayoutGroup { const LayoutGroup(this.id, this.name, this.look, this.apps); final String id, name, look; final List<String> apps; }
  final class LayoutSections { const LayoutSections({required this.starred, required this.order, required this.groups}); final List<String> starred; final List<String> order; /* app IDs and 'g:<gid>' */ final Map<String, LayoutGroup> groups; }
  class HomeLayout extends ChangeNotifier {
    HomeLayout({required ClaxApi api, required String homeAid});
    LayoutSections get sections; // reconcile(layout, apps) as home/arrange.js:97 does: unknown apps appended, missing apps dropped, groups kept
    bool get readable; // false when layout/main is missing or unparsable: sections then list apps newest first
    Future<void> load(List<HomeApp> apps);
    Future<void> star(String id); Future<void> unstar(String id); Future<void> moveTo(String id, int index); // each a pinned whole-document write
  }
  ```
  Reads `api.getDoc(homeAid, 'layout/main')`; writes `api.batchDocs(homeAid, [DocWrite(op: 'set', collection: 'layout', docId: 'main', data: next, ifVersion: read.version)])`; a `ClaxApiError` 409 re-reads, re-applies once, and on a second 409 gives up and reloads; every key of the read document the class does not know (`seen`, `opens`, `big`, `bigTiles`, `v` and anything later) is written back unchanged. Never writes when `readable` is false.
- Consumes: phase 5 (only `HomeApp`).

- [ ] **Step 1: Read the threads** on `home_host.dart` and `apps/clax_app/home/arrange.js`, `store.js` (b1d1c9e3).
- [ ] **Step 2: Failing tests** over `FakeApi` (seed `layout/main` with `batchDocs`, versions from `getDoc`): `sections` for `{starred: [b], order: [a, "g:t", c], groups: {t: {name: "Tools", look: "folder", apps: [d]}}}` and apps a–e put e last; `star('a')` writes with the read version and keeps `opens` and an unknown `future: 1`; a write raced by a foreign write (bump the version between read and write) re-reads and lands once, with both changes; a second race gives up without writing; a missing document gives newest-first sections and no write; a document whose `order` is a string is unreadable. Port the cases of `apps/clax_app/home_unit/arrange.test.js` for `reconcile` so the two agree.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:home_home_layout_test` — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Record the waiver** (Decision 2) on `lib/home/home_layout.dart`'s write method: "A page write under its lease (home/store.js:26) can overwrite a pinned Flutter write that landed inside the lease; accepted as b1d1c9e3 accepts the page's own window."
- [ ] **Step 5: Commit**: "Read and write the home screen’s arrangement from Flutter, pinned to the version read".

### Task 46: The split layout and its sidebar (§3.3, §4.2, §4.3)

**Files:**
- Create: `apps/clax_app/lib/ui/split/split_shell.dart` (`SplitShell`), `split/sidebar_model.dart` (`SidebarModel`), `split/sidebar.dart` (`ClaxSidebar`, the Apple family's source list), `split/list_pane.dart` (`ClaxListPane`, the Material family's list, filled in by Task 53), `apps/clax_app/test/ui/split/split_shell_test.dart`, `apps/clax_app/test/ui/split/sidebar_model_test.dart`, `apps/clax_app/test/ui/shots/split_shots_test.dart`
- Modify: `apps/clax_app/lib/app.dart:2163-2256` (`_home()` returns `SplitShell` at `ClaxWidth.medium` and up, the current home below), `:1903-1946` (`_open` selects the app in the shell instead of pushing a route when the shell is showing), `apps/clax_app/lib/ui/mac/window_channel.dart` (title from the selection)

**Interfaces:**
- Produces:
  ```dart
  sealed class SidebarSelection {} final class HomeSelected extends SidebarSelection {} final class AppSelected extends SidebarSelection { AppSelected(this.id); final String id; } final class MakeSelected extends SidebarSelection { MakeSelected(this.id); final String id; }
  sealed class SidebarRow { } // HomeRow, SectionRow(title), AppRow(app, unread), GroupRow(group, expanded, count), MakeRow(make)
  class SidebarModel extends ChangeNotifier { SidebarModel({required HomeHost host, required HomeLayout layout}); List<SidebarRow> get rows; SidebarSelection get selection; void select(SidebarSelection s); void typeSelect(String prefix); void moveSelection(int delta); Future<void> drop(String appId, {required int index, bool toStarred = false}); String filter = ''; }
  class SplitShell extends StatefulWidget { const SplitShell({super.key, required this.model, required this.home, required this.detailFor}); final Widget home; final Widget Function(SidebarSelection) detailFor; }
  ```
  Rows (§4.2): Home; Starred when any; Apps in `layout/main`'s order with groups as disclosure rows; Making when any, with ring and "3/7" and the state as a second line. `SplitShell`: at `ClaxWidth.expanded` and up a persistent sidebar (Apple: `ClaxSidebar`, 220 wide, 180–320, resizable, width remembered in settings; Material: rail plus `ClaxListPane`, Task 53) and the detail; at `medium`, Apple shows the sidebar as a collapsible overlay toggled from the toolbar or ⌃⌘S, Material the rail and one pane (Task 53); the detail is `home` when Home is selected. `ClaxSidebar`'s rows are 28 high with a 20 dp icon (`tile` radius), `body` title and a trailing `ClaxBadge`; selection in the accent with focus, grey without; ↑/↓ and type-select; drag reorders and drops into Starred through `HomeLayout`; right-click opens the app's `ClaxMenu`; `Semantics` exposes an outline (expanded and collapsed group rows); a ⌘F filter from the toolbar's search narrows it. While the daemon is not up 600 ms after launch, the detail shows the working `ClaxMark` (§5.11's Mac case; the sidebar draws from `HomeHost`'s cached lists).
- Consumes: Tasks 42, 44 (title and search, on the Mac), 45.

- [ ] **Step 1: Read the threads** on `app.dart` (cea3964d1b) and the home files.
- [ ] **Step 2: Failing tests:** `sidebar_model_test.dart`: rows for two starred, three apps, a group of two and a make; `typeSelect('gr')` selects Groceries; `moveSelection(1)` skips section heads; `drop('a', index: 0, toStarred: true)` calls `HomeLayout.star` then `moveTo`; a filter of 'tid' leaves only matching rows. `split_shell_test.dart` (sizes set on `tester.view`): at 1200×800 on macOS a sidebar 220 wide and the home grid; selecting an app shows its detail and sets the window title; at 700 on macOS the sidebar is hidden until toggled; at 412 on Android no `SplitShell`; on iOS at 1180 (an iPad) the same sidebar as the Mac (`debugDefaultTargetPlatformOverride = TargetPlatform.iOS`), so nothing in it checks macOS.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:ui_split_sidebar_model_test //apps/clax_app:ui_split_split_shell_test` — FAIL; implement; rerun with `root_app_test`, `dart_analyze` — PASS.
- [ ] **Step 4: Record the alternative** on `docs/superpowers/specs/2026-10-18-professional-ui-design.md:439-472` (§4.3): "The home app rendered in the sidebar was not taken: no source-list selection, type-select, outline semantics, sidebar material or native menus, and a broken home app would break navigation. revisit: when people ask to change the sidebar by commenting".
- [ ] **Step 5: Look:** `split_shots_test.dart` at mac and wide (wide shows the Material placeholder list until Task 53), Home and an app selected, both themes; and the running Mac app's window.
- [ ] **Step 6: Commit**: "Lay wide windows out as a sidebar and detail, driven by size class, with the Mac’s source list fed by HomeHost and layout/main".

### Task 47: The home app in a split window: `chrome().toolbar`

**Files:**
- Modify: `apps/clax_app/lib/home/home_host.dart` (`setChrome` gets `toolbar: true` while `SplitShell` shows, `form: "wide"`), `apps/clax_app/lib/ui/split/split_shell.dart` (sets it), `apps/clax_app/home/home.js:328-344,2273,1877` (with `toolbar` the bar, search field and FAB are not drawn; big tiles only at `form: "phone"`; `chrome` events re-render), `docs/contract.md` (`toolbar`'s meaning), `web/contract/clax-extensions.d.ts`, `apps/clax_app/assets/kit/fake-home.js`, prompt copies
- Test: `apps/clax_app/home/tests/home.test.js` (and the count), `apps/clax_app/test/home/home_host_test.dart`, `//apps/clax_app:prompt_sync`

**Resolves:** H5.

- [ ] **Step 1: Failing tests:** with `chrome` `{toolbar: true, form: "wide"}` there is no `.h-bar`, no `.h-fab` and no big tile; a `chrome` event back to `{toolbar: false, form: "phone"}` brings them back; `home_host_test.dart`: the split shell's presence sets `toolbar: true`.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — plus `//apps/clax_app:home_home_bundle_test`.
- [ ] **Step 3: Look:** the Mac render of Home selected (Task 46's shots): only the grid in the detail pane.
- [ ] **Step 4: Commit**: "Let the home app leave its bar, search and New app to the window when it sits in a split layout".

### Task 48: The threads pane, the composer popover and comment keys (§4.4, §5.3)

**Files:**
- Modify: `apps/clax_app/lib/ui/threads_panel.dart:16` (`AppPanel` as a right pane at `ClaxWidth.expanded` and up: 320 wide, 280–420 resizable, for both families), `apps/clax_app/lib/ui/app_screen.dart:1755-1800,2132-2155` (`_panel(wide)` places it by size class), `:576-580` (⌘K and Escape act on the comment bar too, 3eff33a2), `apps/clax_app/lib/ui/comment_composer.dart:276-518` (Apple family at medium and up: a 360 popover anchored at the pick, Send on Return, ⇧Return a new line), `apps/clax_app/lib/ui/webview_surface.dart` and `MainFlutterWindow.swift:136-215` (a mouse-up the `WKWebView` misses after a drag that left it is reported from Flutter's pointer stream to the shell as `attestPointer` up (`host_protocol.dart:830`), e2ec3a3253)
- Test: `threads_panel_test.dart`, `app_screen_test.dart`, `comment_composer_test.dart`

**Resolves:** 3eff33a2b1, e2ec3a3253 (with Task 52's real drag as evidence).

- [ ] **Step 1: Read the threads** on each file (4c2bfc90's alternative stands for compact).
- [ ] **Step 2: Failing tests:** at 1280 the panel is a 320 right pane beside the page; at 412 it is the lower panel as today; with the comment bar up and the panel closed, ⌘K closes the bar; on macOS at 1200 the composer is a popover 360 wide whose Return sends and ⇧Return inserts a newline; a pointer up outside the surface after a drag that began inside sends one release to the shell.
- [ ] **Step 3: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 4: Look:** an app with threads open and the composer popover, mac and wide, both themes.
- [ ] **Step 5: Commit**: "Show threads as a side pane in wide windows, the composer as a popover on Apple, and let comment keys close the bar". Close the two records separately once Task 52 has run.

### Task 49: The Settings and Inbox windows (§4.5, §4.6)

**Files:**
- Create: `apps/clax_app/lib/ui/settings/settings_tabs.dart` (`SettingsTabs`: General, Claude, Dictation, Sharing, Notifications, Home Screen, Advanced; each a grouped form with right-aligned labels in a 180 column on the Apple family), `apps/clax_app/lib/ui/inbox/inbox_split.dart` (`InboxSplit`: a 360 list with search and an All / Unread / Questions segmented control, and the thread beside it; size-class driven, reused by Task 55), `apps/clax_app/test/ui/settings_tabs_test.dart`, `apps/clax_app/test/ui/inbox_split_test.dart`
- Modify: `apps/clax_app/macos/Runner/AppDelegate.swift` (Settings window 600 wide, height animating per tab, title the tab's name, toolbar tabs with SF Symbols; Inbox window 900×640, remembered; both through Task 41's mechanism), `apps/clax_app/lib/app.dart` (`ClaxViews` routes `settings` and `inbox` views; ⌘, and ⇧⌘I from `MacMenu.commands`; "Open app" in a thread brings the main window forward with that app selected), `apps/clax_app/lib/ui/settings_screen.dart` (its sections become the tabs' bodies, shared with the phone's single column)

**Interfaces:**
- Produces `class SettingsTabs extends StatelessWidget { const SettingsTabs({super.key, required this.tab}); }` and `class InboxSplit extends StatefulWidget { const InboxSplit({super.key, required this.api, required this.stream, required this.onOpenApp}); }`.
- Consumes: Tasks 14, 32, 41, 43.

- [ ] **Step 1: Failing tests:** `settings_tabs_test.dart`: seven tabs in order, Advanced holds Developer tools; labels right-aligned in 180 on macOS; `inbox_split_test.dart`: at 900 the list is 360 and selecting a row shows its thread beside it; at 412 a row pushes the thread.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — and the macOS build.
- [ ] **Step 3: Look:** each Settings tab and the Inbox window, captured from the running app, light and dark (`NSApp.appearance` via `defaults write` is not used; switch the system appearance in the capture step only with Task 52's test channel, or capture light only here and dark in Task 52).
- [ ] **Step 4: Commit**: "Open Settings and the Inbox as their own Mac windows, as tabs and a list-detail".

### Task 50: Hover, cursors, focus and shortcuts (§4.10)

**Files:**
- Modify: `apps/clax_app/lib/ui/split/sidebar.dart`, `lib/ui/launcher.dart` (`_HomeCell`), `lib/ui/inbox_screen.dart` (`InboxRow`), `lib/ui/kit/button.dart` (hover fills with `hover`; the arrow cursor on buttons, rows and tiles, the pointing hand only on links, resize cursors on pane dividers), `lib/ui/kit/family.dart` (`appleAccent(BuildContext)`: `NSColor.controlAccentColor` read once and on change over `dev.empathic.clax/window`, used for focus rings and selection in the Apple family), `lib/ui/split/split_shell.dart` (Space opens the selected app; ⌘⌫ deletes it after a `ClaxDialog`; ⌘1…⌘9 select the first nine starred apps, from `claxShortcuts`), `macos/Runner/MainFlutterWindow.swift` (answers `accentColor` and posts changes)
- Test: `apps/clax_app/test/ui/split/split_shell_test.dart`, create `apps/clax_app/test/ui/hover_test.dart`

- [ ] **Step 1: Failing tests:** a mouse hovering a sidebar row and a launcher cell fills it with `hover`; the cursor over a `ClaxButton` is `SystemMouseCursors.basic` and over a link `click`; ⌘⌫ asks and then calls the delete; ⌘2 selects the second starred app; the focus ring colour is the accent the mock channel reports.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — plus the macOS build.
- [ ] **Step 3: Commit**: "Follow the Mac’s hover, cursor, focus and keyboard conventions in wide windows".

### Task 51: Apple sheets and the making view in the detail pane (§4.9, §5.6)

**Files:**
- Modify: `apps/clax_app/lib/ui/kit/sheet.dart` (the Apple form: slides from under the toolbar band over `ClaxMotion.enter`, 480–640, buttons bottom-right with Return on the default), `apps/clax_app/lib/ui/making_screen.dart` (at `ClaxWidth.medium` and up it opens in the detail pane under the pane's bar or the toolbar, Back on ⌘[; the preview is the page at the pane's width, max 960, not a 448 column), `apps/clax_app/lib/ui/making_preview.dart`, `apps/clax_app/lib/ui/split/split_shell.dart` (`MakeSelected` → the making view)
- Test: `apps/clax_app/test/ui/kit/surfaces_test.dart`, `making_screen_test.dart`, `making_screen_shots_test.dart`

- [ ] **Step 1: Failing tests:** on macOS at 1200 `ClaxSheet.show` places its body under the 52 pt band and its default button rightmost; Return triggers it; selecting a make in the split shell shows the making view in the detail with a preview wider than 448 at 1200.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS.
- [ ] **Step 3: Look:** the making view and a bundle sheet, mac and wide.
- [ ] **Step 4: Commit**: "Slide Apple sheets from under the toolbar, and show making in the detail pane at full width".

### Task 52: The macOS chrome test (§9)

**Files:**
- Create: `apps/clax_app/integration_test/mac_chrome_test.dart`, `apps/clax_app/integration_test/goldens/main_menu.txt`, `apps/clax_app/macos/Runner/TestChannel.swift` (inside `#if DEBUG || PROFILE`: channel `dev.empathic.clax/test` with `captureWindow(path, window)` (ScreenCaptureKit's `SCScreenshotManager.captureImage` with `SCContentFilter(desktopIndependentWindow:)` on macOS 14+, `CGWindowListCreateImage` on 12–13), `setAppearance("aqua"|"darkAqua")`, `describeMenu()`, `openMenu(title)` and `captureMenu(path)` (`NSMenu.popUp(positioning:at:in:)` on a run-loop timer, capture the region, `cancelTracking()`))
- Modify: `apps/clax_app/macos/Runner/MainFlutterWindow.swift:78-81` (install it beside `installTestInput`), `project.pbxproj`, `apps/clax_app/integration_test/rig.dart` (helpers `captureWindow`, `setAppearance`, `describeMenu`), `docs/usage.md:551-568` (the first run asks for Screen Recording permission for the test host)

**Resolves (as evidence for):** c0375dfc82, e2ec3a3253, 098fa2674e, 3eff33a2b1 (Tasks 42, 48), and the menu's keys (32f8f89d40's "unverified on a real Mac").

- [ ] **Step 1: Write the test:** drive the app (the rig's `Rig.create`) to Home selected, an app selected with the toolbar, comment mode with the composer popover, the threads pane, each Settings tab, the Inbox window, the permission alert before and after arming (an injected `armMs` large enough to capture before), a sidebar context menu, and each top-level menu open; write `target/flutter-app/mac/<state>-<light|dark>.png` for each (both appearances through `setAppearance`); compare `describeMenu()` with `goldens/main_menu.txt` (titles, key equivalents, enabled state); make a real mouse pick in the `WKWebView` through `dev.empathic.clax/test-input` (`rig.dart:47,388`), a drag whose release lands outside the page, ⌘C and ⌘V in a page field and a Flutter field, and Escape with the page focused.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:integration_mac_chrome --test_output=streamed` (manual, in the background, logged; the first run asks for Screen Recording). Expected: PASS, and the golden matches.
- [ ] **Step 3: Look** at every PNG it wrote, light and dark, against §4.
- [ ] **Step 4: Commit**: "Check the Mac’s window chrome, toolbar, menus and real interactions in an integration test". Close c0375dfc82 and the records Tasks 42 and 48 left open, in a separate commit with this run as evidence.

**Phase 6 ends** when `mac_chrome_test` passes and its renders have been read. The Mac parts of bc952edafa and a22c402ec8 run on a release build at this point (§10.8), recorded in those threads, not closed by this plan.

---

## Phase 7: Android wide (§3.2, §10.7)

### Task 53: The Material form of the split layout: rail and panes

**Files:**
- Modify: `apps/clax_app/lib/ui/split/list_pane.dart` (`ClaxListPane`: 360 wide; Search, Starred, Apps with groups as expandable rows, Making; the same `SidebarModel`), `apps/clax_app/lib/ui/split/split_shell.dart` (Material: a `NavigationRail` of 80 with the New app FAB at the top (mic and "New", `accent-tint`), Home, Inbox with its badge, Settings; at `expanded` the rail, the list pane and the detail; at `medium` the rail and one pane, Home the grid), `apps/clax_app/lib/app.dart` (Inbox and Settings as rail destinations in the shell rather than pushed routes when it shows)
- Test: `apps/clax_app/test/ui/split/split_shell_test.dart`, `split_shots_test.dart`

**Interfaces:**
- Consumes: Tasks 46, 47, 49 (`InboxSplit`, `SettingsTabs` bodies).

- [ ] **Step 1: Failing tests:** on Android at 1280×800 a rail of 80, a list pane of 360 and the detail; at 700 the rail and the grid; the rail's Inbox badge shows the unread count and hides at 0; New app opens the new-app sheet.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — plus `dart_analyze`.
- [ ] **Step 3: Look:** the wide renders, Home and an app selected, both themes.
- [ ] **Step 4: Commit**: "Give Android tablets, foldables and Chromebooks a navigation rail and list-detail panes".

### Task 54: An app in a pane (§3.2, §5.2)

**Files:**
- Modify: `apps/clax_app/lib/ui/app_screen.dart` (at `ClaxWidth.medium` and up in the Material family: a 64 dp top app bar with back, title, Comment, Threads with its count, Share, and ⋮ (Inspect, Versions, Star); no pill; toasts `ClaxToast(place: inline)`; the build and change chips' content in a side sheet, never over the page's header (2634bc3a); threads a standard side sheet of 360 pushing the page at `large`, a modal side sheet below it), `apps/clax_app/lib/ui/working_chip.dart` (`BuildChip` and `WorkingChip`'s expanded forms render inside the side sheet)
- Test: `app_screen_test.dart`, `app_shots_test.dart`

- [ ] **Step 1: Failing tests:** at 1280 on Android an app shows the top app bar's six actions and no `ClaxPill`; a notice is an inline bar under it; opening the change shows a side sheet, and nothing overlaps the top 64 dp of the page; at 1300 the threads side sheet pushes the page (the page's width shrinks by 360); at 1000 it overlays.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS.
- [ ] **Step 3: Look:** an app in the pane with threads and with a change, both themes.
- [ ] **Step 4: Commit**: "Show an app in a wide pane under a top app bar, with threads and changes in side sheets".

### Task 55: Inbox and Settings on wide Android

**Files:**
- Modify: `apps/clax_app/lib/ui/inbox/inbox_split.dart` (the Material family's list-detail, 360 list), `apps/clax_app/lib/ui/settings_screen.dart` (one column, max 640, centred), `apps/clax_app/lib/ui/split/split_shell.dart` (the rail's Inbox and Settings show them)
- Test: `inbox_split_test.dart`, `settings_test.dart`, `inbox_shots_test.dart`, `settings_shots_test.dart`

- [ ] **Step 1: Failing tests:** on Android at 1280 the inbox is a 360 list and the thread beside it; Settings is 640 wide and centred.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS.
- [ ] **Step 3: Look** at both, wide, both themes.
- [ ] **Step 4: Commit**: "Lay out the inbox as list-detail and Settings as a centred column on wide Android".

### Task 56: Foldables, keyboards and mice

**Files:**
- Modify: `apps/clax_app/lib/ui/split/split_shell.dart` (with a hinge in `MediaQuery.displayFeaturesOf(context)` of type `hinge` or `fold` that is vertical, the split between the list and the detail sits on it), `apps/clax_app/lib/app.dart` (`Shortcuts(shortcuts: claxShortcuts(ClaxFamily.of(context)))` on Material too, so Ctrl works on Chromebooks), `apps/clax_app/lib/ui/launcher.dart`, `lib/ui/split/list_pane.dart`, `lib/ui/inbox_screen.dart` (a secondary click opens `ClaxMenu` at the pointer), `apps/clax_app/tool/apk-check.sh` (checks MainActivity's `configChanges` still includes `screenSize|smallestScreenSize|screenLayout|orientation|density` (`AndroidManifest.xml:36`), so a resize never restarts the activity)
- Test: `split_shell_test.dart`, `shortcuts_test.dart`, `hover_test.dart`

- [ ] **Step 1: Failing tests:** a `DisplayFeature(bounds: Rect.fromLTWH(640, 0, 0, 800), type: DisplayFeatureType.hinge, state: DisplayFeatureState.postureFlat)` at 1280 puts the detail's left edge at 640; Ctrl+K toggles threads on Android; a secondary tap on a tile opens the menu at the pointer; `apk-check.sh` fails on a manifest without `screenSize`.
- [ ] **Step 2: Run** — FAIL; implement; rerun — PASS — and `//apps/clax_app:apk_test` (background).
- [ ] **Step 3: Commit**: "Split panes on a foldable’s hinge, and take Ctrl shortcuts, right clicks and hover on Chromebooks".

**Phase 7 ends** with the wide renders of Tasks 53–56 read. Then `just check` (background) and `"$Q" exec diff main`, reported.

---

## Phase 8: Device verification (§10.8)

Not in this plan's execution: it needs the phone and runs separately, in §10.8's order (T1's records, the home tiles' feel session, the Comment tab against the real gesture zone, the status band on real pages, predictive back). Phases 1–7 do not wait on it; the concerns Tasks 10 and 27 record (`phase:8`) list what it must look at.

## Ready for iOS

Clax ships next on iPhone and iPad, with the Apple family's look and the iPad in the split layout; the iOS spec covers the in-process daemon, the Bazel iOS build and TestFlight, which this plan does not touch. The iOS work builds on:

- **Task 1** (tokens): `type.<role>.apple` gains an `ios` entry; `ClaxType.of` already falls back from iOS to the macOS sizes.
- **Task 5** (`ClaxFamily`, `ClaxWidth`, `claxTheme(…, TargetPlatform.iOS)`): the Apple family is chosen for iOS with no change.
- **Tasks 6, 7, 8** (components and the mark): every component's Apple form is the iOS form's starting point; `ClaxSheet`'s compact form on Apple becomes an iOS sheet with detents.
- **Tasks 45, 46, 48, 49, 51** (`HomeLayout`, `SplitShell` and `SidebarModel`, the threads pane, `InboxSplit`, sheets and making in the pane): size-class driven, so the iPad gets them at `medium` and up (the iOS test in Task 46 pins that nothing checks macOS).
- **Tasks 34–38** (native overlays): the overlay protocol and `OverlayGate` are platform-free; `Init.platform` gains `"ios"`.
- **Task 43's `claxShortcuts(apple)`**: hardware-keyboard shortcuts on the iPad.
- **Not reused**: Tasks 42, 44, 49's windows, 50's Mac accent channel and 52 (`hasDesktopWindowChrome`: NSToolbar, the menu bar, `NSAlert` sheets, extra windows).

## Out of scope: browser-only

The app session covers only what a person sees in the Clax app. These spec items are the browser shell's and get no tasks here:

- The shell's own theming in a desktop browser: reading tokens into `web/shell/src/theme.css`, its contrast comments, `accent-color`, hover states (§1.1's shell output, §2's web list, S14).
- The browser shell's agent naming (§7.2's browser part and the spec's open question, now moot), and 65e399a8ed.
- People, hosted and live seats (18933d0d47, T10).
- The gallery (its haiku footer is kept as it is), the browser threads panel and `threads.ts`'s `anchorLabel` (S2's browser part), the browser composer's clip (S7), browser-only notices and the notice banner (S1), zero counts in `PhoneTabs.svelte`/`TopbarIsland.svelte` (S4), the connection pill (S8), the phone versions sheet (S9), quotes in the browser (S16's shell part), deleted and unknown pages in the browser (S3, S18), `userText`, Svelte markup in `check-copy.py`, and the browser dialog's "Allow in a moment…" line (S5's browser part).
- Browser-only records: 2ae15dfc4b, e5e0475f1a, ef1f5e8974.
