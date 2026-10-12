# Clax app: design

Date: 2026-10-12. Branch `flutter-app`. Status: approved decisions O1–O10
(below) are the owner's; everything else here is this design's.

## 1. Goal and experience

Say a thing and get an app; improve it fluidly while using it. A malleable
OS at your fingertips.

- **Say → usable.** On the launcher, the person says or types "a tracker
  for my runs with a weekly total". Seconds later the app is open, full
  screen, working, and keeps what they enter.
- **Long-press → landed change.** While using it, a two-finger swipe left
  opens the threads panel and turns comment mode on. The person holds a
  finger on what bugs them, lets go, says "make this a bar chart", and taps
  Send. The change lands in place: the app reloads to the new version at the
  same page and scroll, with every document they entered still there.
- **Change is safe.** Every version is kept. The versions list restores any
  earlier one as a new version in one tap, data untouched.
- **Apps share data.** An app may read another app's documents ("show my
  run totals on my dashboard"), with the owner's consent.

Two numbers are defended, measured by the app itself on every run and
reported by the live runs (§11): **say→usable** (submit on the launcher to
the new app's first frame load) and **long-press→landed** (Send in the
composer to the new version's frame load).

Everything runs on the device. Only the LLM call leaves it.

### Owner decisions (binding)

- O1 A standalone Flutter app, not a PWA.
- O2 Apps are HTML in a WebView under the existing page contract
  (`window.claude.use(...)`, db, assets, …) and the existing bridge.
- O3 The agent is the Claude API, called in the app by a Dart loop; the
  owner's API key lives in platform secure storage.
- O4 A two-finger swipe left opens the threads overlay; while it is open,
  a long-press on the page starts the normal Clax comment flow.
- O5 `clax` runs embedded in the app on loopback with its own `CLAX_HOME`.
- O6 Android first, plus macOS desktop for testing and use.
- O7 On-device speech-to-text for "New" and for comments, in v1.
- O8 v1 scope: launcher → generated app; full screen; swipe → overlay;
  long-press → comment; Send → change lands with data kept; version undo;
  one app reads another app's db. Later: widgets, background work,
  notifications.
- O9 The agent sits behind a backend interface with two implementations
  returning the same structured edit result: `MessagesApiBackend` (the
  phone, the owner's key) and `ClaudeCliBackend` (desktop: `claude -p
  --output-format json --json-schema <schema>` with tools disabled).
- O10 Feature work is tested deterministically (fake backends, fake
  Messages API server, scripted results). No test and no gate calls a live
  model. Live `claude -p` runs happen once, after all feature work.

## 2. Architecture

```
┌──────────────────────── Flutter app (apps/clax_app) ────────────────────────┐
│ Launcher ─ New (text + mic) ─ App screen ─ Threads/Versions panel ─ Settings │
│      │                          │  WebView /a/<id>?app=1                     │
│      │                          │    shell (app mode) ─ frame ─ bridge       │
│      │                          │    window.claxHost ⇄ JavaScriptChannel     │
│  AgentLoop ── AgentBackend ── MessagesApiBackend | ClaudeCliBackend          │
│      │            (same EditResult)                                          │
│  ClaxApi (HTTP, bearer token) ───────────────┐                              │
│  DaemonSupervisor ── child process ──────────┼─► clax serve --foreground    │
└──────────────────────────────────────────────┼──── 127.0.0.1:<port> ────────┘
                                               └─ CLAX_HOME in app storage
```

- The daemon is today's `clax`, release build, unchanged in shape: a child
  process the app supervises (§3.1).
- The UI is Flutter around one WebView showing the existing Svelte shell in
  a new chromeless **app mode** (§3.4). The shell keeps doing everything it
  does today (capabilities, consent prompts, the composer, clips); Flutter
  adds the launcher, gestures, panels, voice and the agent.
- The agent is Dart (§3.6). It is an agent session of the daemon with the
  new harness name `app`, so comments reach it through the existing
  feedback route, and its replies, resolves, working marks and publishes are
  the existing agent routes.
- Server additions are small (§4): the `app` harness, an atomic version
  restore, and the cross-app `db` read grant.

## 3. Components

### 3.1 DaemonSupervisor (Dart, `lib/daemon/`)

Starts, watches and restarts the embedded daemon.

```dart
abstract interface class ProcessStarter {
  Future<RunningProcess> start(String exe, List<String> args,
      {required Map<String, String> environment, required String workingDirectory});
}
class DaemonEndpoint { final Uri base; final String token; final int pid; }
class DaemonSupervisor {
  DaemonSupervisor({required ExeLocator exe, required String home,
      required ProcessStarter starter, required PortPicker ports,
      required HealthProbe health, Clock clock = const SystemClock()});
  Stream<DaemonState> get states;        // starting, up(endpoint), restarting(n), failed(reason, logTail)
  Future<DaemonEndpoint> ensureUp();     // idempotent; restarts a dead daemon
  Future<void> stop();                   // SIGTERM, then SIGKILL after 3 s
}
```

- **Executable.** macOS: `<bundle>/Contents/Resources/clax`, found from
  `Platform.resolvedExecutable`. Android: `<nativeLibraryDir>/libclax.so`,
  from the Kotlin MethodChannel `dev.empathic.clax/native` method
  `nativeLibraryDir`.
- **Home.** `CLAX_HOME = <application support dir>/clax-home`, from
  `path_provider` (inside the macOS container, Android app-private).
- **Port.** The last port used (kept in `<support>/daemon-port`) if it
  binds, else a free one (`ServerSocket.bind(loopbackIPv4, 0)`, then
  closed). Keeping the port keeps the WebView's origin and cookies.
- **Environment.** A clean one: `CLAX_HOME`, `HOME` (the support dir),
  `TMPDIR` (the cache dir), `PATH=/usr/bin:/bin` (macOS) or
  `/system/bin` (Android), `RUST_LOG=warn`. Nothing else is inherited, so a
  `codex` on the developer's `PATH` never starts a push.
- **Command.** `<exe> --port <P> serve --foreground`.
- **Stale daemon.** Before starting, a `daemon.json` whose `pid` is alive is
  stopped (SIGTERM, then SIGKILL after 3 s), since no other process uses
  this home.
- **Up.** `GET /healthz` every 50 ms until 200 (15 s limit, injected
  clock), then `daemon.json` is read again for `token` and `port`.
- **Death.** The child's exit is watched. An exit the supervisor did not ask
  for restarts it after 0.2, 0.5, 1, 2, 5 s (5 s from then on); five exits
  within 60 s is `failed`, shown as an error screen with **Restart** and the
  last 50 lines of the child's stderr (kept in memory).
- **Resume.** On app resume (`AppLifecycleState.resumed`) the supervisor
  probes `/healthz` and restarts a dead daemon (Android 12+'s
  phantom-process killer may kill it in the background). SQLite's WAL
  tolerates the kill.
- **Exit.** On app detach the daemon gets SIGTERM.

### 3.2 Native packaging

- **macOS.** A Run Script build phase copies
  `apps/clax_app/build-native/macos/clax` into
  `$BUILT_PRODUCTS_DIR/$CONTENTS_FOLDER_PATH/Resources/clax` by writing a
  temporary file next to it and renaming it into place (never a copy over a
  binary that may be running), then signs it. In Release it is signed with
  `macos/Runner/ClaxHelper.entitlements` (`app-sandbox`, `inherit`), so it
  inherits the sandboxed app's sandbox. In Debug and Profile it is signed
  without entitlements: the Runner there is not sandboxed, and a helper
  carrying `inherit` under an unsandboxed parent is killed at launch.
  Entitlements:
  - `Release.entitlements`: `app-sandbox`, `network.server`,
    `network.client`, `device.audio-input`. No keychain access group: it
    is a restricted entitlement that an ad hoc signed build cannot carry,
    so `flutter_secure_storage` keeps the API key in the login keychain
    (`MacOsOptions(usesDataProtectionKeychain: false)`), which needs none.
  - `DebugProfile.entitlements`: the same without `app-sandbox`, so the
    desktop build can start `claude` for `ClaudeCliBackend` (§3.6).
  - `Info.plist`: `NSMicrophoneUsageDescription`,
    `NSSpeechRecognitionUsageDescription`.
- **Android.** `android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libclax.so`
  (git-ignored), `useLegacyPackaging = true` (so the `.so` is extracted and
  executable), `minSdk = 29`. Manifest: `INTERNET`, `RECORD_AUDIO`, a
  `<queries>` entry for `android.speech.RecognitionService`, and a network
  security config permitting cleartext to `127.0.0.1` and `localhost`
  including subdomains (the WebView loads `http://<id>.localhost:<port>`).
- **Builds.** `scripts/build-app-clax.sh [android|macos|all] [--debug]`
  cross-compiles `clax` (`aarch64-linux-android` and `x86_64-linux-android`
  with the NDK's `*-linux-android29-clang` linkers; the host target for
  macOS), strips it, and places each binary at its path above by renaming a
  temporary file into place. `just app-clax`, `just app-apk`, `just
  app-macos`, `just app-test`, `just app-integration` wrap it and Flutter.
  A release build of `clax` embeds `web/dist`, so the web UI is built first.

### 3.3 ClaxApi (Dart, `lib/clax/`)

A typed client over the daemon's HTTP API, bearer token on every request,
`X-Clax-Session` on agent calls. One abstract interface with a real
implementation over `dart:io` `HttpClient` and a fake for widget tests.

```dart
abstract interface class ClaxApi {
  Future<List<ArtifactSummary>> listArtifacts();
  Future<ArtifactDetail> getArtifact(String aid);                 // {artifact, versions}
  Future<Map<String, String>> readFiles(String aid, int n);       // path -> text (text files only)
  Future<PublishResult> createArtifact(PublishRequest r);
  Future<PublishResult> publish(String aid, PublishRequest r);    // if_version required
  Future<PublishResult> restoreVersion(String aid, int n, {required int ifVersion});
  Future<Session> registerAppSession({required String installId, required int pid});
  Future<FeedbackBatch> pollFeedback(String sid, {Duration wait});
  Future<void> ackFeedback(String sid, List<String> feedbackIds);
  Future<void> replyAsAgent(String aid, String tid, String body);
  Future<void> resolveAsAgent(String aid, String tid);
  Future<void> setWorking(String aid, {required bool on, String? note});
  Future<Uint8List?> clip(String aid, String tid);
  Future<List<Doc>> listDocs(String aid, String collection, {int limit});
  Future<List<CollectionInfo>> collections(String aid);
  Future<void> batchDocs(String aid, List<DocWrite> writes);
  Future<Doc?> getDoc(String aid, String path);
  Future<List<ThreadView>> threads(String aid);
  Stream<ClaxEvent> events({List<String>? artifacts, List<String>? types}); // GET /api/events, resumes with Last-Event-ID
}
```

### 3.4 Shell app mode (TypeScript, `web/shell/src/app/`)

`/a/<id>?app=1` (also `/a/<id>/v/<n>?app=1`) is the artifact view without
chrome, driven by its host.

- `artifact-main.ts` reads `app=1`, sets `html.app`, passes `app: true` in
  the view props, and imports `./app/host` lazily, so the artifact bundle
  grows by the import only.
- CSS under `html.app` hides the top bar, the sidebar, the phone tabs and
  the working strip; the frame fills the viewport. Pins show only while
  comment mode is on. The composer is a sheet docked at the bottom.
- In app mode the composer's primary button is **Send**: it posts the
  thread and sends it to the agent (`sendToAgent` with the view's `sendTo`),
  in one press. The composer, its anchor, its screenshot and its wait for
  the screenshot are today's.
- **Auto-follow.** In app mode, when a newer version arrives and the view
  is not pinned to a version, the view loads it at once (or, while a
  composer is open, once it closes), on the same page and fragment, and
  asks the bridge to restore the last scroll position the bridge reported:
  the first welcome of the new page carries `restoreScroll: <y>` (§3.5).
- **Protocol.** The header of `web/shell/src/app/host.ts` is the source of
  truth for the channel protocol, and `docs/contract.md` ("The Clax app")
  documents it. The host is installed only in the top-level document at
  `/a/<id>[/v/<n>][/<file>]` with `app=1`. The WebView injects `ClaxHost`
  into every frame, the page's included, so the app makes a fresh nonce
  per load of the shell document and calls `claxHost.init({nonce})`; the
  shell posts nothing before it, every message after it carries the nonce,
  and the app drops any message without it. The app lets the top level
  navigate only to that shell path on the daemon's origin.
- **`window.claxHost`** (Flutter → page, by `runJavaScript`):

  | Method | Effect |
  |---|---|
  | `enterCommentMode()` / `exitCommentMode()` | Turns comment mode on or off (`toggleComment` semantics, no-op when already so). |
  | `loadVersion(n \| null)` | Shows version `n`, or the latest for `null` (`chooseVersion`). |
  | `reloadLatest()` | Loads the latest version, keeping page, fragment and scroll. |
  | `setDraftText(text)` | Replaces the open composer's text (dictation). No-op without a composer. |
  | `submitDraft()` | Presses Send on the open composer (waits for its screenshot as Send does). |
  | `cancelDraft()` | Closes the composer. |
  | `openThread(id)` | Selects the thread and scrolls the page to it. |
  | `init({nonce})` | Starts the channel with this load's nonce (first valid call only). |
  | `resend(id)` | Sends an open, unsent thread again after `sendFailed`. |
  | `attestPointer({pointer, phase, x, y})` | Attests a real finger press or release on the WebView (§3.5, the gesture on Android). |

- **Messages to the host** go through the webview_flutter JavaScriptChannel
  `ClaxHost` (`window.ClaxHost.postMessage(JSON)`); a page without it (a
  browser) drops them.

  | `type` | Fields | When |
  |---|---|---|
  | `ready` | `artifact`, `version`, `latest`, `file` | Each frame load. |
  | `state` | `commenting`, `draft: {label, text, clip: "pending"\|"ready"\|"none", byPage?} \| null`, `busy` | Any change of those. `label` names the anchor in words. |
  | `threads` | `threads: [{id, status, summary, label, sent, comments, version}]` | Any change of the view's threads. |
  | `sent` | `thread` | After Send posted and sent a thread. |
  | `sendFailed` | `thread`, `text` | The send failed; the thread stays posted and unsent. |
  | `landed` | `version` | The page of a version auto-follow loaded has greeted. |
  | `notice` | `text` | A shell notice (post failed, …). |

### 3.5 Bridge additions (`web/bridge/src/`)

- **Touch long-press in comment mode** (`long-press.ts`, used by
  `comment-mode.ts`): one touch held 450 ms within 10 px of its start
  outlines the target under the finger (the same target rules as the
  pointer, including oversized-element text targeting) and, on release,
  picks it exactly as a click would (`clax:pick-start` and the clip). The
  pick happens on release because the browser grants user activation on
  `touchend`, not while a finger rests, and the shell takes a pick only with
  the frame's activation. Moving beyond the slop, a second finger, or
  `touchcancel` abandons it. A short tap keeps today's behaviour (the
  synthesized click picks). The click the browser sends after a
  long-press's release picks too; when none comes within 400 ms (Android
  may send none), comment mode moves focus into the page and picks what is
  under the finger itself. The timer is injected for tests.
- **The gesture on Android.** A finger on the cross-origin frame sends the
  shell nothing, so the app attests each real press and release it sees
  over the WebView (`claxHost.attestPointer`); a press on the frame itself,
  and its release, count as the finger's arrival on the frame for 1 s
  (`hostArrival` in `web/shell/src/caps/gesture.ts`).
- **Selection and callout guard.** While a finger is held in comment mode,
  `contextmenu` and `selectstart` are cancelled, so a long-press neither
  selects text nor opens a menu; while comment mode is on, the document
  gets only `-webkit-touch-callout: none`. `user-select: none` for the
  whole mode would break mouse drag-select comments, Option+drag widening
  and right-click menus on desktop.
- **Scroll report.** After the shell's welcome with `app: true`, the bridge
  posts `clax:scroll {file, y}` at once and then at most every 250 ms while
  the page scrolls. The welcome itself carries the restore: the first
  welcome of a newer version's page has `restoreScroll: <y>`, and the page
  scrolls there once its document is tall enough, or after 2 s as far as
  it reaches. There is no `clax:restore-scroll` message.

### 3.6 Agent (Dart, `lib/agent/`)

```dart
/// What every backend returns: one structured edit.
class EditResult {
  final String reply;                 // to the person: what changed, in a sentence or two
  final String? title;                // required for a new app
  final List<FileWrite> files;        // whole-file writes: create or replace
  final List<TextEdit> edits;         // exact replacements in existing text files
  final List<String> removed;         // paths to delete
  final List<DbOp> db;                // set | update | delete, data as a JSON string
  final Map<String, Object?>? capabilities; // full declaration, null keeps the current one
  final bool resolve;                 // the comments are done
}
abstract interface class AgentBackend {
  String get name;                                    // "messages-api" | "claude-cli" | "scripted"
  Future<EditResult> run(AgentRequest request);       // throws AgentError
}
```

- **The schema** (`edit_schema.json`, one file used by both backends) is
  strict-structured-output compatible: every object has
  `additionalProperties: false` and lists all its properties as required;
  optional values are `anyOf [..., null]`; document data travels as a JSON
  string (`data_json`), since free-form objects are not allowed; the
  capability declaration travels as `capabilities_json`.
- **`MessagesApiBackend`** posts to `https://api.anthropic.com/v1/messages`
  (base URL injectable for the fake server) with `x-api-key`,
  `anthropic-version: 2023-06-01`, `max_tokens: 16000`, the system prompt as
  one cached block (`cache_control: ephemeral`), and
  `output_config: {effort: "low", format: {type: "json_schema", schema}}`.
  It reads the first text block as JSON. `stop_reason` `refusal` or
  `max_tokens` is an `AgentError`. The clip, when there is one, goes as an
  `image` block before the text. Raw HTTP is used because there is no
  official Dart SDK. Forced `tool_choice` (`{"type": "tool"}`) is refused
  with a 400 by `claude-opus-5-5` and `claude-sonnet-5-5`, so the "one
  forced tool" shape of O9 is carried as structured output with the same
  schema (§12, D-1).
- **`ClaudeCliBackend`** (desktop, offered only when the app is not
  sandboxed) runs `claude -p --output-format json --json-schema
  <schema> --tools "" --system-prompt-file <file> --model <model>
  --effort low --no-session-persistence --strict-mcp-config --safe-mode`
  with the request on stdin, from a scratch directory per run, with a
  300 s timeout, and with Claude Code's session variables and
  `ANTHROPIC_API_KEY` dropped from its environment. `--safe-mode` keeps
  the person's CLAUDE.md, memory, skills, plugins, hooks and MCP servers
  out of the run while their login still works (`--bare` would not read
  the login). It reads the
  structured result from the JSON's `structured_output`, else parses
  `result` as JSON. The executable is found on `PATH` and at
  `~/.local/bin/claude`, overridable in settings. It sends no image: with
  tools off it has no way to read the clip; the anchor and quote go as text.
- **Models.** Default `claude-haiku-5-5` at effort `low`, for both new apps
  and changes, for speed (§12, D-2). Settings offer `claude-sonnet-5-5` and
  `claude-opus-5-5`.
- **The prompt** teaches the page contract: the system prompt is built from
  `assets/prompt/system.md` (how to build an app in Clax: one
  self-contained `index.html` unless more files help, persist with
  `window.claude.use("db")`, handle `null`, phone first, the token and dark
  mode rules), plus verbatim copies of `docs/contract.md`'s "Page contract"
  and "Runtime capabilities" sections and the served `db.d.ts` and
  `clax-extensions.d.ts`, all kept in sync by `scripts/sync-app-prompt.sh`
  (its `--check` runs in the app's gate lane). The user message carries the
  request, the current files (for a change), the comments with their anchor
  lines, and the person's other apps (§3.8).
- **AgentLoop** applies an `EditResult` over `ClaxApi`:
  - **New app.** Validates that `files` holds `index.html` and `title` is
    set, then `createArtifact` with `X-Clax-Session` (so the app session
    owns and watches it), then the `db` operations, and returns the ID.
  - **Change.** On each feedback batch for an artifact: `setWorking(on)`,
    read the current version and its files, run the backend, apply `edits`
    to the files (each `old_str` must occur exactly once), add `files`,
    drop `removed`, `publish` with `if_version` and `addresses` (the
    threads), apply the `db` operations (`docs:batch`, `if_version` from a
    read of each existing document), reply on every thread as the agent,
    resolve them when `resolve`, `ackFeedback`, `setWorking(off)`.
  - **Retries.** An edit whose `old_str` is missing or ambiguous, or a
    result that fails validation, is retried once with the failure named
    and full files asked for. A publish conflict (409) re-reads the latest
    version and runs the change again once. Anything else, or a second
    failure, replies on the threads with `Couldn't make that change: <reason>`
    and leaves them open.
  - **Feedback worker.** One long-poll at a time on
    `/api/sessions/<sid>/feedback?tier=wait&wait=50`, items grouped by
    artifact, one artifact's change at a time, a second artifact's batch
    queued behind it.
- **The app session.** `POST /api/sessions` with `{"harness": "app",
  "harness_session_id": <install ID>, "pid": <app PID>}` (install ID: a
  random UUID kept in the support dir). A restarted daemon or app reuses the
  live row by the install ID. The worker re-registers on `unknown_session`.

### 3.7 Flutter UI (`lib/ui/`)

- **Launcher.** A grid of the owner's apps (`listArtifacts`, live from
  `events(types: version, artifact_deleted)`), newest first, each a tile
  with the title and icon word. A **New** button opens the New sheet: a
  text field and a mic button; submitting shows "Making <first words>…"
  with elapsed seconds and Cancel, then opens the app.
- **App screen.** A full-screen `AppSurface` over the WebView at
  `<base>/a/<id>?app=1`. `AppSurface` is an interface (`WebViewAppSurface`,
  `FakeAppSurface` for tests) with `load(Uri)`, `call(HostCall)` and
  `Stream<HostMessage> messages`.
- **Gestures.** Android: a `Listener` (translucent, outside the gesture
  arena) over the WebView recognizes two pointers that both move left more
  than 80 dp with `|dy| < |dx| / 2` within 600 ms; it opens the panel. A
  two-finger swipe right, or the system back gesture, closes it. A thin
  edge handle on the right also opens it with a tap, as a fallback. macOS:
  ⌘K and the **App → Comments** menu item toggle it; Escape closes it.
- **Panel.** A Flutter panel: on a narrow screen a bottom sheet over the
  lower 45% (the page stays visible above it and can be long-pressed), on a
  wide one a 360 px side panel. Tabs **Threads** (from the host's `threads`
  messages; tap selects via `openThread`) and **Versions** (from
  `getArtifact`; tap previews via `loadVersion(n)`, **Restore** calls
  `restoreVersion`, then `reloadLatest`). Opening the panel calls
  `enterCommentMode`; closing it calls `exitCommentMode`.
- **Composer and voice.** The composer stays the shell's (§5). When the
  host reports a draft, the Flutter panel shrinks to a peek, and a floating
  mic button appears above the composer; with **Dictate comments** on (the
  default) dictation starts at once. Partial results go to `setDraftText`.
  A **Working…** chip shows from `sent` until `landed` (or the agent's
  failure reply).
- **Settings.** API key (masked, stored with `flutter_secure_storage`,
  never logged, never given to the daemon or the WebView), backend (Claude
  CLI offered only on an unsandboxed macOS build), model, Dictate comments,
  CLI path, and the last ten measured timings.
- **Voice.** `VoiceInput` interface; `SpeechToTextVoice` (the
  `speech_to_text` plugin, `onDevice: true` where the platform offers it,
  falling back to the platform recognizer) and `FakeVoice` for tests.
- **Timings.** `Timings` records `say→usable` and `long-press→landed` with
  an injected clock to `<support>/timings.jsonl` and shows them in
  Settings.

### 3.8 Inter-app data

- **Declaring.** App A declares `{"db": {"read": ["<B ID>", …]}}`; B may
  declare `{"db": {"share": {"read": ["<A ID>", …]}}}`. Both are validated by
  `ReadDecl` (`crates/clax-core/src/db.rs`; at most 16 IDs each, valid
  artifact IDs, not itself, `db.share` holding only `read`).
- **Consent.** A may read B's documents when A declares the read, B
  declares `db` (else 403 `not_declared`, the token included), and either
  B shares with A or the owner granted it. Grants are rows of
  `db_read_grants(reader_id, source_id, granted_at)` (migration 24), made
  by `PUT /api/artifacts/<A>/db-grants/<B>` and removed by `DELETE` (token),
  listed by `GET /api/artifacts/<A>/db-grants`. Deleting either artifact
  removes its rows.
- **Reading.** The db read routes of B (`GET …/docs`, `GET …/docs/<path>`)
  accept `X-Clax-Reader: <A ID>`: the caller's level is that of a reader,
  read-only, judged by B's rules at `view` with no viewer; without consent
  it is 403 `not_granted` (code also used by the shell). Writes with the
  header are 403 `read_only`, and the header on any other route
  (`docs:collections` included) is 403 `not_granted`, before any handler
  runs. Private `data/users/<id>/` documents are never readable this way.
  Following changes: `GET /api/events?reader=<A>`, or on `/api/stream` the
  topic `docs:<B>?reader=<A>` (never `docs:<B>`, whose events reach a
  stream at its own level); each `doc` event is checked against consent as
  it is sent.
- **Shell and bridge.** `db.artifact(id)` in the page returns a read-only
  handle with `doc(path).get()`, `collection(path).get()/query()/onSnapshot()`.
  The shell checks A's declaration, asks the owner once when there is
  neither a share nor a grant ("Let <A> read <B>'s data?" Allow / Don't
  allow; Allow makes the grant), and serves reads and subscriptions over
  B's routes with the reader header. Types go in `clax-extensions.d.ts`.
- **Collections.** `GET /api/artifacts/<aid>/docs:collections` (token)
  answers `{collections: [{path, count}]}`, for the agent.
- **The agent knows the other apps.** Each request carries up to 20 of the
  owner's other apps that declare `db`: ID, title, collections with counts,
  and up to two sample documents per collection, 4 KiB per app and 16 KiB
  in all. The prompt says how to declare `db.read` and read with
  `db.artifact(id)`.

## 4. Server additions

| Change | Where | Contract |
|---|---|---|
| Harness `app` | `routes/sessions.rs` `HARNESSES`; `push_info` (`tier: "wait"`, available); feedback tier mapping (`store/feedback.rs`); display name "Clax app" (`toolpath/mod.rs`) | Registration accepts `app`; comments sent to an `app` session wait for its feedback poll. |
| Version restore | `POST /api/artifacts/<aid>/versions:restore` `{"version": n, "if_version": cur}` | Publishes version `n`'s files as a new version (`label: "Restored v<n>"`, `note: "Restored version <n>"`), atomically, in one store transaction, writing each of `n`'s files again into the new version's directory with its bytes, type and hash (the store keeps a directory per version, so each restore adds `n`'s size on disk); carried-forward files not in `n` are removed; capabilities and documents are untouched. 409 `conflict` as publish; 404 for an unknown `n`. Emits `version`; the audit's `version.publish` adds `restored_from: n`, and its `carried` names only files unchanged from the previous version. |
| `db.read` / `db.share` | `db.rs`, `capabilities.rs`, `store/docs.rs`, `db_caller.rs`, `routes/docs.rs`, migration 24 | §3.8. |
| `docs:collections` | `routes/docs.rs` | §3.8. |

The restore route is `versions:restore` with the version in the body
rather than `versions/<n>:restore`: the router (matchit 0.8.4) cannot match
a parameter with a static suffix in one segment.

## 5. The composer lives in the shell

The comment composer stays the shell's own, inside the WebView, and
Flutter feeds it (dictation via `setDraftText`, Send via `submitDraft`).

- A pick is accepted only with user activation in the frame (the composer
  tier of "The viewer's gesture" in `docs/contract.md`). The long-press
  release gives it; nothing Flutter does (`runJavaScript`) can, so the pick
  must start in the page whichever side renders the composer.
- The clip PNG is rendered by the bridge only after the shell's composer
  textarea has focus (`clax:composer-ready`), and posted with the thread
  as a multipart part. A Flutter composer would need the PNG (up to 5 MiB)
  carried over the JavaScript channel and a second implementation of the
  wait-for-screenshot, the anchor and the post; keeping the shell's
  composer reuses all of it, tested.
- The cost: the soft keyboard and the shell's textarea are WebView UI, not
  Flutter's. Dictation avoids the keyboard; a typed comment uses the
  WebView's.

## 6. Data flow

### 6.1 Say → usable

1. Launcher: New, the person says or types the request (dictation fills the
   field). Submit starts the say→usable timer.
2. `AgentLoop.newApp(text)`: the prompt with the other apps' summaries;
   `backend.run`; validate; `createArtifact` (session header, title,
   capabilities, files); `db` operations.
3. The app screen opens `/a/<id>?app=1`; the host's first `ready` for that
   artifact stops the timer.

### 6.2 Long-press → landed change

1. Two-finger swipe left: the panel opens, `enterCommentMode()`.
2. Hold on an element, release: the bridge picks; the shell opens its
   composer with the anchor and the screenshot; the host posts `state`
   with a draft. Flutter shrinks the panel and starts dictation.
3. Partial words go to `setDraftText`. The person taps Send (the shell's or
   Flutter's, which calls `submitDraft`): the thread is created and sent to
   the app session; `sent` starts the long-press→landed timer and the
   Working chip.
4. The worker's feedback poll returns the comment; the loop marks working,
   reads files, runs the backend, publishes with `addresses`, replies,
   resolves, acks, ends working.
5. The shell sees the new version and auto-follows; `landed` stops the
   timer. The page is at the same file, fragment and scroll; its documents
   are where they were.

### 6.3 Undo

Versions tab → Restore on version `n` → `restoreVersion(aid, n,
ifVersion: latest)` → the shell auto-follows the new version → `landed`.

## 7. Errors

| Failure | Behaviour |
|---|---|
| Daemon exits | Restart with backoff (§3.1); the WebView reloads at the new endpoint once up; the worker re-registers on `unknown_session`. Five exits in 60 s: error screen with Restart and stderr tail. |
| Daemon killed in the background (Android) | Resume probe restarts it before the screen shows; the WebView reloads. |
| No API key | New and Send are refused with "Add your Claude API key in Settings", linking there. |
| Backend error (HTTP, timeout, refusal, max_tokens, invalid result) | New: the sheet shows the reason and keeps the text, with Retry. Change: one retry where §3.6 says so, then a reply on the threads, which stay open. |
| Publish conflict | Re-read and run the change again once (§3.6). |
| WebView process gone (`onWebResourceError` on the main frame, Android renderer crash) | Reload the surface; a second failure in 10 s shows a Reload button. |
| Grant refused | The page gets `not_granted` from `db.artifact(id)` calls. |
| Voice unavailable or denied | The mic button is disabled with the reason; typing works. |

## 8. Security

- The daemon binds `127.0.0.1` only. The API key is stored with
  `flutter_secure_storage` (Keychain on macOS, Keystore-backed storage on
  Android), read only by `MessagesApiBackend`, never written to logs, the
  daemon, `CLAX_HOME` or the WebView.
- Agent output is untrusted: file paths go through the daemon's path
  validation; `edits` only touch files of the version; db writes go
  through the daemon at owner level (the agent is the owner's).
- Comment text and documents are data in the prompt, quoted as JSON
  strings, with the note that they are the person's words, not
  instructions.
- **Loopback on Android is shared by every app on the device.** Another
  app can reach the port: unauthenticated reads (artifacts, content, `view`
  level documents) and, because `/api/token` answers any loopback request
  with a local `Host` and a forgeable `Sec-Fetch-Site`, the token. v1
  accepts this on the owner's own phone; the fix (a daemon option that
  turns `/api/token` off, with the token handed to the WebView by its host)
  is §12, D-3.
- Cross-app reads are read-only, opt-in per pair, and never include
  private per-viewer documents.

## 9. Testing

Deterministic only; no test or gate calls a model.

| Layer | Tests |
|---|---|
| Rust | Integration tests for the `app` harness, `versions:restore` (atomicity, conflict, unknown version, blobs reused, documents untouched), migration 24, `db.read`/`db.share` validation, grants, the reader header (allowed, not granted, read-only, private documents, stream filtering), `docs:collections`. |
| Web unit (vitest) | Bridge long-press (timer injected: hold, slop, second finger, cancel, release picks, tap unchanged), selection guard on/off, scroll report throttle and restore; shell app mode (class, lazy host, every `claxHost` method, every host message, Send = post + send, auto-follow keeps file/fragment and asks restore), `db.artifact` (declaration check, consent prompt, reader header, subscription filtering). |
| Web e2e (Playwright) | `?app=1` with a stubbed `window.ClaxHost`: touch long-press picks (touch emulation) and Send sends; a published version auto-follows with data kept and scroll restored; restore; one page reads another's documents after consent. |
| Flutter unit | Supervisor with a fake `ProcessStarter` and clock (start, health wait, stale PID, restart backoff, failure after five, resume); supervisor against the real debug `clax` (`CLAX_TEST_BIN`); `ClaxApi` against a scripted in-process HTTP server and against the real debug daemon; edit schema/validation; `AgentLoop` with `ScriptedBackend` and `FakeClaxApi` (new, change, edits, retries, conflict, failure reply, db ops, other-apps summary limits); `MessagesApiBackend` against an in-process fake Messages API (request shape, cache block, image block, refusal, max_tokens, HTTP errors, key never logged); `ClaudeCliBackend` against `test/fixtures/fake-claude.sh` (arguments, stdin, `structured_output`, `result` fallback, non-zero exit, timeout). |
| Flutter widget | Launcher, New sheet (voice via `FakeVoice`), settings (backend list per platform), app screen with `FakeAppSurface` (swipe recognizer, panel, peek on draft, dictation to `setDraftText`, Working chip, versions restore), error screen. |
| macOS integration | `integration_test/full_loop_test.dart` on the macOS build: the real embedded release daemon, `ScriptedBackend`: New → app loads → a thread created and sent over the API (the long-press itself is covered by Playwright) → the change lands with a document kept → Restore → the earlier version is back. |
| Live (once, last) | Three runs on the macOS build with `ClaudeCliBackend`; the timings recorded. |

The app lane in `scripts/quality_gates.sh` runs `flutter analyze` and
`flutter test` (unit and widget) and the prompt sync check, and is skipped
with `app: skipped (flutter not found)` when Flutter is missing. The
integration test builds the macOS app and is not in the gates (`just
app-integration`).

## 10. Repository layout

```
apps/clax_app/                     Flutter project (package clax_app, org dev.empathic)
  lib/main.dart, lib/app.dart      entry, AppRoot composition
  lib/daemon/                      supervisor, exe locator, ports, health
  lib/clax/                        ClaxApi, models, SSE
  lib/agent/                       EditResult, schema, backends, prompt, loop, worker
  lib/ui/                          launcher, new sheet, app screen, panel, settings, error
  lib/platform/                    voice, secure settings, native channel, timings
  assets/prompt/                   system.md + synced contract copies
  test/, integration_test/
  tool/check.sh                    analyze + test + prompt check (the gate lane runs it)
  build-native/                    git-ignored: the macOS clax
scripts/build-app-clax.sh          cross-compile and place clax
scripts/sync-app-prompt.sh         keep assets/prompt in step with docs/contract.md and web/contract
web/shell/src/app/                 host.ts, app.css
web/bridge/src/long-press.ts
```

## 11. Verification

`docs/superpowers/verification/2026-10-12-flutter-app.md` records what was
exercised end to end and by what: the macOS integration test, the live
`claude -p` runs with their timings, the release APK's contents
(`libclax.so` per ABI, manifest), and what was not exercised: no Android
device or emulator ran the APK, so the Android gestures, the WebView on
Android, speech on Android and the phantom-process restart are unverified.

## 12. Open questions for the owner

Recorded as `status:needs-decision` qualifier records on this file; each
has the conservative choice this design takes.

- D-1 Structured output instead of a forced tool on the Messages API
  (§3.6).
- D-2 Default model `claude-haiku-5-5` at effort `low` (§3.6).
- D-3 The `/api/token` exposure to other apps on Android (§8).
- D-4 No auto-grant: an app the agent made to read another app still asks
  the owner on first read (§3.8).
- D-5 macOS debug builds run without the App Sandbox so `claude -p` can be
  started (§3.2).

## 13. Out of scope

Widgets, background work and notifications (O8); iOS; agent questions
(`ask`) in the app; LAN viewers of the embedded daemon; streaming a
generated app as it is written; image input for `ClaudeCliBackend`;
multiple people on one install; publishing the app to a store.
