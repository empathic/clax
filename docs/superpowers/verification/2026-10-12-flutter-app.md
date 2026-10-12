# Clax app: what the build verified, and what it did not (2026-10-12)

Branch `flutter-app`, checked on a Mac (Apple silicon, macOS 26) with Flutter
3.47.7, Android SDK platform 36, NDK 28.2.13676358 and JDK 17. No Android
device or emulator was attached at any point. No Claude API key exists on
this machine.

Read this as a list of claims, each with what backs it. Where something was
only read or unit-tested, it says so.

## 1. Try it yourself

### On your Android phone (Android 10 or later, arm64)

Turn on developer options and USB debugging, plug the phone in, then from
the checkout:

```
export PATH="$HOME/Library/Android/sdk/platform-tools:$PATH"
adb devices        # the phone must be listed as "device", not "unauthorized"
just app-apk       # or use the APK already built, below
adb install -r apps/clax_app/build/app/outputs/flutter-apk/app-release.apk
```

Open Clax, tap Settings (the launcher shows a banner while no key is set),
paste a Claude API key, and tap New. This would be the first run of the
app on any Android device, and the first call of `MessagesApiBackend` to
the real API (section 4): expect it to be where problems show up first. The APK is signed with the Android
debug key (`CN=Android Debug`, certificate SHA-256
`9874f8473ebc181beb90c6af8b4d29dfac87a97bcde62da0880a71938d28ce85`), as
`android/app/build.gradle.kts` configures the release build. That is enough
to install it on your own phone. It cannot go to a store, and a later APK
signed with another key will not install over it without an uninstall
first.

Use `just app-apk`, not `flutter run --release`, to build the APK you
install. Only the recipe keeps the 32-bit ABI out (record c2eff94c).

### On this Mac

The release build is sandboxed, so it offers only the Claude API with a key:

```
just app-macos
open apps/clax_app/build/macos/Build/Products/Release/clax_app.app
```

It is signed ad hoc on this machine (`Signature=adhoc`, no team), so it
runs on the Mac that built it. Its data lives in
`~/Library/Containers/dev.empathic.clax/Data/Library/Application Support/dev.empathic.clax/`
(`clax-home/` is the daemon's home). It never uses `~/.clax` or port 7480.

To use your Claude Code login instead of a key, run a debug build, which
runs without the App Sandbox (open decision D-5):

```
just app-clax macos
cd apps/clax_app && flutter run -d macos
```

Then choose Settings → "Claude Code on this Mac".

## 2. Release artifacts and their checks

The builds ran with `just app-clax` (both platforms, release), then
`just app-apk` and `just app-macos`. The log is
`target/flutter-app/v1/builds.log`.

### APK

The APK is `apps/clax_app/build/app/outputs/flutter-apk/app-release.apk`:
41,320,603 bytes, SHA-256 `1cd676afbfb431485ed62d2c046e09532eeee16a2837459c7b2745c6e1bf2104`.
These checks ran on it:

- **Native libraries** (`unzip -l`): `lib/arm64-v8a/` and `lib/x86_64/`,
  each with `libapp.so`, `libclax.so`, `libdartjni.so` and
  `libflutter.so`. There is no `lib/armeabi-v7a/`.
- **Manifest** (`aapt2 dump badging`, build-tools 36.0.0):
  - package `dev.empathic.clax`, label `Clax`;
  - `minSdkVersion:'29'`, `targetSdkVersion:'36'`;
  - permissions `INTERNET`, `RECORD_AUDIO`, and
    `dev.empathic.clax.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` (added by
    AndroidX);
  - `native-code: 'arm64-v8a' 'x86_64'`.
- **Manifest tree** (`aapt2 dump xmltree`): `extractNativeLibs=true`, and
  a `networkSecurityConfig` is set.
- **libclax.so** (`file`): an aarch64 ELF and an x86-64 ELF, both PIE with
  interpreter `/system/bin/linker64`. Both embed the web UI: 97 references
  to `_clax/shell` each.
- **Signature** (`apksigner verify --print-certs`): the Android Debug
  certificate.

The first release build had a gap. Built with only
`--target-platform android-arm64,android-x64`, the APK still declared
`native-code: 'arm64-v8a' 'armeabi-v7a' 'x86_64'`, because it carried
`lib/armeabi-v7a/libdartjni.so` from the `jni` plugin. Flutter's Gradle
plugin (`FlutterPlugin.kt`, `configureAbiWithoutSplits`) replaces the app's
`abiFilters` with all three ABIs. A 32-bit-only phone would have installed
that APK and failed at start, since it had no `libflutter.so` or
`libclax.so` for that ABI.

`just app-apk` now also passes `-P disable-abi-filtering=true` (4c2191b2).
Record a4af3134 is closed with this evidence. Builds outside the recipe
still carry the 32-bit ABI (c2eff94c).

### macOS app

The app is `apps/clax_app/build/macos/Build/Products/Release/clax_app.app`
(81.1 MB). These checks ran on it:

- **Embedded clax**: `codesign -d --entitlements -` on
  `Contents/Resources/clax` shows `com.apple.security.app-sandbox` and
  `com.apple.security.inherit`.
- **The app's entitlements**: `app-sandbox`, `network.client`,
  `network.server`, `device.audio-input` and `get-task-allow`. Xcode adds
  `get-task-allow` to a build signed to run locally.
- **Signature**: `codesign --verify --deep --strict` passes.

Launched as a child of the shell (PID 33721), the app worked:

- It wrote its `daemon.json`. The embedded `clax` ran as PID 33741, a
  child of 33721, with `--port 57053 serve --foreground`, bound to
  127.0.0.1.
- `GET /healthz` answered 200 with `{"pid":33741,…}`.
- `osascript -e 'tell application id "dev.empathic.clax" to quit'` returned
  "User canceled (-128)". This is how the app's quit handler defers the
  terminate while it stops the daemon. Both PIDs then exited, and
  `/healthz` stopped answering.

Two things in this launch were not what the plan asked for:

- **The scratch home was ignored.** The app was started with
  `CFFIXED_USER_HOME` set to a scratch directory under `target/`, but the
  sandbox ignored it. The daemon's home was the app's own container
  (above), which was empty before this run and now holds that run's
  `clax-home`. It is not `~/.clax`.
- **Nobody looked at the launcher.** This session has no screen-recording
  permission, so the check is the process and `/healthz`, not a screenshot.

## 3. What ran end to end, and by what

### The macOS integration test (F10)

`apps/clax_app/integration_test/full_loop_test.dart`, the test "make,
change, undo and recover on the embedded daemon", ran again for this
document. The command was `scripts/build-app-clax.sh macos` followed by
`flutter test integration_test -d macos`. The result: **PASS**, 1 test,
42 s of test time, 92 s with the build (`target/flutter-app/v1/f10.log`).

It runs the real release `clax` from the app bundle, the real HTTP client,
supervisor, worker and WKWebView, with a scripted agent (no model). The app
around them is a debug build, which runs without the App Sandbox (D-5), so
this test says nothing about the sandboxed release app. Its clicks are
NSEvents the test synthesizes, and the test attests each press to the
shell itself (c0375dfc):

1. New app → Make → the app opens on its first `ready`, and its db
   document is in the store.
2. It waits past the shell's 30 s hidden threshold. WKWebView reports the
   document hidden from load.
3. A two-finger swipe left opens the panel in comment mode. A synthesized
   click on the heading picks it, and the composer opens. The comment is
   typed through the host and sent with the peek's Send. The change lands
   in place, with data and scroll kept.
4. Versions › Undo last change restores version 1 as version 3, in place.
5. The daemon is killed by its PID while a change is held in the agent.
   The supervisor restarts it, and the change completes exactly once.
6. A second app declares a read of the first. Opening it asks the owner.
   A synthesized click on Allow grants the read, and the data shows.

The two "feedback poll failed" lines in the log come from step 5's
deliberate daemon kill.

After the pass, the debug `clax_app` process was still running, reparented
to launchd, with no daemon. I stopped it by its PID (record 44c5dca6).

The test attests the press itself. It does not prove that a real pointer's
arrival on the frame counts in a WKWebView (c0375dfc).

### Live model runs (L1)

L1 used `ClaudeCliBackend` (`claude -p`, Haiku 5.5) in a debug macOS build
on this machine. It spent 6 calls in total, with the results in
`target/flutter-app/l1/RESULTS.md`:

| Measure | Value |
|---|---|
| Direct `claude -p` run (the CLI checklist, record 1641a6ef) | 10.8 s |
| say → usable, run log | 30,996 ms |
| say → usable, dashboard reading the run log | 17,952 ms |
| long-press → landed (Send 47 s after load) | 4,846 ms |

What the runs showed:

- **Run log.** The generated app worked in Chromium against the same
  daemon, and its data persisted.
- **Change.** The change landed in place in the app, with the data kept.
- **Dashboard.** It declared `db.read` of the run log. The consent dialog
  armed in the app, a synthesized NSEvent click on Allow (its press
  attested through `claxHost` by the test) granted the read (in
  `grants.json`), and the dashboard showed the weekly total, as read in
  Chromium.

Not every step was live:

- Several steps were **replayed** from recorded model output and spent no
  call. Their timings (460 ms, 438 ms) are not model numbers and are left
  out of the table.
- The page text was read in Chromium against the same daemon, because the
  in-app frame is cross-origin to the shell.
- No screenshot of the app window was possible (no screen-recording
  permission).
- Each live measure is one run. The change (call 4) and the dashboard
  (call 6) were made on top of replayed earlier steps, and every pick and
  click was synthesized, never a person's mouse or finger.

L1 found two blockers, 4ae44372 and 9e96bca7. Both were fixed and
re-verified live before this document.

### Playwright (in the web e2e gate, Chromium)

The branch adds these specs, each run in both serving modes:

- `web/e2e/app-mode.spec.ts`:
  - `app=1` hides the chrome and posts `ready`;
  - the page cannot reach `claxHost`;
  - a late host still gets `ready`;
  - a browser view has no host;
  - a long-press picks, and Send goes to the app session;
  - a version lands in place with documents and scroll kept, and a restore
    brings the earlier version back;
  - a version lands after the hidden threshold;
  - a permission dialog arms with no animation frames.
- `web/e2e/inter-app.spec.ts`:
  - a tap that reads another app asks once;
  - after Allow, the data shows, and a reload reads it without asking;
  - a share in the other app;
  - consent with no animation frames.
- `web/e2e/long-press.spec.ts`:
  - a long-press on a heading opens the composer and selects nothing;
  - a long-press on a link picks it without following it.
- `web/e2e/db-artifact.spec.ts`:
  - a read is refused until the owner allows it;
  - Allow grants the read, and the view follows the other app;
  - an earlier grant or a share is read without asking;
  - a non-owner is never asked.

Touch is emulated in Chromium here. That is not a real Android WebView.

### Daemon routes

- `GET /api/artifacts/<id>/docs:collections` was called on a scratch daemon
  (`CLAX_HOME` under `target/`, a free port, stopped by its PID). It
  answered 200 `{"collections":[{"count":1,"path":"boards/b1/columns"},{"count":2,"path":"runs"}]}`,
  and 401 without the token. Record 4720080d is closed.
- The Rust tests in `cargo nextest` cover the grant, reader and restore
  routes.

## 4. What was only read or unit-tested, and never run

- **Any Android device or emulator.**
  - The APK was built and inspected, never installed or started.
  - Unverified on Android:
    - the two-finger swipe;
    - long-press in the Android WebView: whether a click follows the
      release, the no-click fallback, and attestation by the Flutter
      Listener (8cc89df6, 86741946);
    - on-device speech-to-text (`SpeechToTextVoice` has no automated test,
      ef50a675);
    - `*.localhost` in the Android WebView;
    - starting `libclax.so` from `nativeLibraryDir`;
    - phantom-process kills of the daemon and the supervisor's restart.
- **The WebView renderer crash on Android is not handled** (1ee688df).
  macOS is covered. On Android, it needs native code, so the app process
  dies, and the supervisor restarts the daemon on the next launch.
- **Real-mouse pick on macOS.** The integration test and L1 attest the
  press through `claxHost`. Whether a person's real mouse press reaches the
  shell as a finger arrival in WKWebView was not checked (c0375dfc). The
  ledger notes that macOS clicks never reach Flutter's Listener, so the
  mouse is not attested.
- **`MessagesApiBackend` against the real Claude API.** There is no key
  here. It is tested only against a fake Messages API server. Its
  structured-output request shape (D-1) and the `max_tokens` 16000 cap
  (2ab7d464) have never met the real API.
- **The release app's API-key path.** The key in the login keychain and a
  model call from the sandbox were not exercised.
- **The `/api/token` exposure on Android** (spec §8, D-3). It is reasoned
  about, not demonstrated with a second app.
- **Dictation on macOS.** It was not exercised by any test with a
  microphone.

## 5. Quality gates

`scripts/quality_gates.sh` ran once (log `target/flutter-app/gates.log`).
The load average was 7 to 14, with other agents' work on the machine. The
wall clock was 273.8 s, and the run exited 1 on one perf gate.

| Gate | Result | Time |
|---|---|---|
| web npm ci / web build / web unit | ok | 1.8 / 4.1 / 18.9 s |
| no unsafe code, web bundle size, justfile, no hard links, pipefail pipes | ok | ≤ 2.7 s each |
| playwright browser, web lint, cargo fmt --check | ok | 1.6 / 1.8 / 1.6 s |
| pi npm ci, pi extension | ok | 6.8 / 28.6 s |
| dev scripts, build clax, release installer, release scripts | ok | 7.9 / 8.6 / 13.8 / 21.9 s |
| tool hook gate, plugins, extension signing, stable bin | ok | 1.0 / 23.9 / 5.4 / 1.6 s |
| build app clax, plugin wrapper, comment loop | ok | 0.8 / 22.6 / 3.6 s |
| release build | ok | 38.3 s |
| web typecheck | ok | 37.7 s |
| **app (flutter analyze + test)** | ok | 34.9 s |
| build tests, cargo clippy, clippy (no test features) | ok | 42.8 / 21.6 / 18.4 s |
| cargo nextest | ok | 60.2 s |
| web e2e (Playwright) | ok | 83.5 s |
| daemon latency (perf) | ok | 26.0 s |
| realtime clients (perf) | ok | 20.3 s |
| **time to usable (perf)** | **FAIL** | 27.3 s |

Time to usable failed on the subdomain run's comment-ready figure. It
measured 95, 104 and 102 ms against a limit of 86 ms (three tries). The
sandbox run passed, and `app ready` (43 to 45 ms against 75) passed
everywhere. The subdomain cold ready latency was also over its limit in
two of the three tries (42 and 44 ms against 38). `web/perf/budget.json`
sets `enforceTargets`, so that measure is gated too, and the comment-ready
failure stops the test before it is checked: making comment ready pass
alone would not make this gate pass. No record covers the cold-ready
overrun.

Rerun alone (`cd web && CLAX_PERF_QUICK=1 npm run perf`, load average
10 to 12, `target/flutter-app/v1/perf-rerun.log`), it failed the same way.
Comment ready measured 103, 104 and 106 ms against scaled limits of 94 to
102 ms; cold ready latency was 43 ms against 41 in two tries; the sandbox
run passed.

The comment-ready part is the known item f585f63e, which waits on the
owner. That record was opened on 2026-10-10 for another branch, and its
title says the overrun never happened on main. Its reply 1e79ad8b traces
the comment-ready path for that other branch, not this one. For this
branch the evidence is 30446f51: in two interleaved runs each, at load
average 5 to 7, the web UI from before this branch's web work
(`8a2ff314^`) measured 97 and 95 ms and the current one 91 and 92 ms,
both over the 86 ms limit. That is two runs, not a trace, and nobody ran
the gate on main itself for this document.

`backfill_on_perf_seed_under_3s`, the other known load-sensitive test,
passed in this run's `cargo nextest`.

## 6. Open decisions for the owner

These are the `status:needs-decision` threads this branch opened or
depends on.

**Spec §12** (`docs/superpowers/specs/2026-10-12-flutter-app-design.md`):

- **D-1** (8966525a): the Messages API backend uses structured output, not
  one forced tool. The spec asked for a forced tool, but forcing a tool
  returns 400 on Opus 5.5 and Sonnet 5.5. Recommended: keep structured
  output. It has never run against the real API.
- **D-2** (92ee4c3f): the default model is `claude-haiku-5-5` at effort
  `low`, chosen for speed; apps may be worse than Sonnet's. Recommended:
  keep it until someone compares live runs with Sonnet 5.5.
- **D-3** (51f0d067): other Android apps can reach the embedded daemon and
  fetch its token through `/api/token`. Any app on the phone would then
  have full owner access to every Clax app and its data. Recommended:
  accept it on your own phone only, and close it before anyone else
  installs the app.
- **D-4** (c1f27009): there is no automatic read grant when the agent
  builds an app that reads another app, so you are asked once even though
  you requested it. Recommended: keep asking unless it proves annoying.
- **D-5** (f998fa52): macOS debug builds run without the App Sandbox so
  that `claude -p` can be started; the release build keeps the sandbox and
  needs an API key. Recommended: keep.

**Server-side:**

- **d359fbc3**: an app session's feedback is acknowledged at hand-over and
  never resent, so a comment in flight when the app or daemon dies would
  be lost. You decide whether the app's own recovery is enough or the
  server should resend.
  - The app recovers on start: it re-runs threads that are sent to the
    agent, acknowledged, and have no agent reply after the last viewer
    comment (74211b60).
  - The server stays unchanged.
- **d20ea53f**: an owner grant outlives a re-publish that drops `db.read`.
  It applies again without a prompt when the declaration returns, even if
  the agent has rewritten the reader into a different app. You decide
  whether a grant belongs to the pair of apps (now) or should be dropped
  when the declaration is.

**Gates:**

- **f585f63e**: time to usable's subdomain comment ready (section 5): the
  gate fails, so a merge needs either a fix, a raised budget, or your
  acceptance of a failing gate. Cold ready latency also fails (section 5)
  and has no record.

**Settled:**

- **74cdfc55** is closed. The spec settles it: long-press → landed starts
  at Send (§6.2).

**Not tagged needs-decision, but needing a call:**

- **ad59c22f**: `--tools ""` leaves Claude Code's server-side advisor tool
  enabled. L1's debug log showed it enabled, never called. A run could
  call a larger model.

Main also has older needs-decision threads that this branch did not open:

- e63a6a28;
- 0bcf4f58;
- 094670e6;
- caca9840;
- ef1f5e89;
- 4d1b4269;
- 2ae15dfc.

## 7. Known risks

**Android:**

- Everything in section 4.
- **The renderer crash** (1ee688df).
- **A `--debug` Android clax serves no web UI** (688a1a4e). Release builds
  embed it, as checked above.
- **Only `just app-apk` drops armeabi-v7a** (c2eff94c).

**Agent:**

- **Recovery is client-side** (d359fbc3, 03d18a78). It re-runs stranded
  threads of every session, and judges a landed change by timestamps.
- **A daemon failure mid-change cancels the change** (e6695aa0). The model
  run is paid again after Restart.
- **A daemon restart reloads the open app** (db08c124, dc83a752). An
  unsent composer draft is lost, and the page reopens at the top.
- **A slow first daemon start counts as a failure** (9fbb9e03). The 15 s
  health limit ignores `starting.json`.
- **A change can edit only the version's text files** (d5d5b180).
- **Output is capped far below what the result check allows** (2ab7d464).
  `max_tokens` is 16000.

**Cross-app reads:**

- **No retry backoff after a failed refetch** (aee18f03).
- **A per-view subscription cap of 64** (987f6cee).
- **Ring replay is not re-checked against the grant** (69724d39).

**Host channel:**

- **`claxHost` reaches any top-level document** (8de86523). That includes
  an artifact page loaded at the top level.
- **Attestation trusted by construction** (55f28ea7, a waiver).

**Migration 24** (014aa41c): it is numbered against main at 5da3c861,
whose last migration is 23. It must be renumbered if another branch lands
a migration first.

## 8. Qualifier diff against main

`qualifier diff main` is reckoned from the merge-base 5da3c86. The full
output is in `target/flutter-app/v1/qual-diff.txt`.

**Added (118 records still active** when `qualifier diff main` was rerun
after 5b4ba6c8; the saved file, from before 44c5dca6, has 117):

| Kind | Count |
|---|---|
| comment | 64 |
| concern | 37 |
| alternative | 14 |
| suggestion | 2 |
| waiver | 1 |

There are no open blockers. `qualifier threads --kind blocker` lists none.

**Closed on the branch:** across the `.qual` files, the branch wrote 17
blockers, 127 concerns, 39 suggestions and 16 alternatives, and 142
`resolve` records closed most of them on the branch. No record that
already existed on main was resolved.

**Drifted: 8**, all pre-existing records from another session, on code
this branch changed:

- 3f206f41: `crates/clax-server/src/routes/mod.rs`;
- 094670e6: `crates/clax-server/src/stream.rs`;
- 39562e11: `scripts/quality_gates.sh`;
- aa1344c2: `web/bridge/src/bridge.ts`;
- 0239b9e0 and 87de9a64: `web/shell/src/stream.ts`;
- 7bec3684 and 76a00f1b: `web/shell/src/view/artifact-controller.ts`.

None was written by this branch's session, so none was re-recorded here.
Their author or the owner should re-anchor them before the merge.
