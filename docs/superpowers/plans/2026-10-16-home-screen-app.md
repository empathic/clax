# Home Screen as a Clax App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clax's home screen becomes a Clax app (a web artifact improved by commenting), with Flutter keeping every core system and a native fallback.

**Architecture:** A new `home` capability is relayed on the existing device-cap path (bridge → shell → `ClaxHost` → Flutter `DeviceHost`-style handler). Flutter's `HomeHost` owns the app list, makes, build progress and inbox count once, and serves both the home app (gated on its recorded artifact ID) and the fallback (today's `Launcher`). The home app ships as a bundle in the app's assets and installs on first run through the bundle installer.

**Tech Stack:** Flutter/Dart (`apps/clax_app`), TypeScript shell and bridge (`web/shell`, `web/bridge`, `web/contract`), the app kit (Preact + htm, `apps/clax_app/assets/kit`), vitest, Playwright, flutter_test.

**Spec:** `docs/superpowers/specs/2026-10-16-home-screen-app-design.md`

## Global Constraints

- The home app's artifact ID is stored by Flutter in the app settings directory beside `device-grants.json` as `home.json` `{"homeAppId": "<id>", "builtin": false, "keptRelease": null}`. No daemon migration.
- Declaring `home` grants nothing; Flutter answers `home` calls only when the calling `AppScreen`'s artifact ID equals `homeAppId`. Every other caller gets `{error: "refused"}` and `available: false`.
- The home app is left out of every list `home.apps()` returns and out of the fallback grid.
- The fallback home screen shows when there is no `homeAppId`, the install failed, the home app hit "This app stopped loading", or no `ready` within 10 s of a cold start (injected clock in tests).
- Copy, verbatim: banner "Your home screen couldn't load"; buttons "Undo last change", "Reset"; Settings section "Home screen" with "Open", "Undo last change", "Reset to Clax's home screen", "Use the built-in home screen".
- Tests are deterministic: no sleeps standing in for conditions, injected clocks for the 10 s rule.
- AGENTS.md rules apply: no `unsafe`, unsigned commits (`git -c commit.gpgsign=false commit`), never push, never `~/.clax` or port 7480, qualifier records via `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec …`.
- UI is verified from renders looked at with the Read tool, phone and desktop, light and dark, before it is called done.
- Web bundle budgets (`node web/scripts/bundle-size.mjs`) keep passing; `home` code in the shell lives in the lazy app-mode part, not the eager bridge.

## Review Focus

1. **Another app declaring `home`** — must see it unavailable and every call refused, including after the person opens it right after the home app (screen reuse). Test in Task 1.
2. **A home app that loads but is broken** (throws, renders blank, never sends `ready`) — the 10 s rule shows the fallback with Undo/Reset; Undo then restores and the home app is shown again. Test in Task 4.
3. **A make or build finishing while the home app is covered** (person is inside another app) — on return the grid shows the new state without a reload. Test in Task 1 (events buffered to latest state) and Task 3 (re-subscribe on resume).
4. **First run offline / daemon slow** — install from the asset bundle needs no network; until it lands, the fallback shows, then the home app takes over without a flash of empty grid. Test in Task 3.
5. **A tap on the cold-start snapshot before `ready`** — opens the app under the tap once `ready` lands, or does nothing if no tile was there; never opens the wrong app when the grid changed. Test in Task 5.

---

### Task 1: `HomeHost` and the `home` capability (Flutter + shell + bridge)

**Files:**
- Create: `apps/clax_app/lib/home/home_host.dart` — owns `gallery`/`inbox` subscriptions, `MakeService`, `BuildLink`; exposes state + change stream; answers calls.
- Create: `apps/clax_app/lib/home/home_settings.dart` — reads/writes `home.json`.
- Modify: `apps/clax_app/lib/ui/launcher.dart:168-327` — move subscription/refetch logic into `HomeHost`; `Launcher` consumes `HomeHost`.
- Modify: the device-call dispatch in `apps/clax_app/lib/ui/app_screen.dart` (`device` message handling) to route `cap == "home"` to `HomeHost.call(aid, method, args)`.
- Modify: `web/shell/src/caps/registry.ts`, `web/shell/src/caps/availability.ts`, `web/shell/src/caps/device-link.ts` — register `home` as a device-relayed capability, available only in app mode.
- Modify: `web/bridge/src/capabilities.ts` (`CAPABILITY_METHODS`), `web/contract/clax-extensions.d.ts` — `clax.home` API.
- Test: `apps/clax_app/test/home/home_host_test.dart`, `web/shell/src/caps/home.test.ts`.

**Interfaces:**
- Produces (Dart):
```dart
class HomeApp { final String id, title; final DateTime updatedAt; final Map<String,Object?> icon; final ({int done,int total})? build; }
class HomeMake { final String id, request; final String state; // making|offline|waiting|interrupted|failed
  final ({int done,int total})? progress; }
class HomeState { final List<HomeApp> apps; final List<HomeMake> makes; final int unread; final bool needsKey; }
abstract class HomeActions { // implemented by app.dart's routing
  void open(String id); void openMake(String id); void newApp(); void menu(String id, Rect anchor);
  void openInbox(); void openSettings(); void importApp(); }
class HomeHost extends ChangeNotifier {
  HomeHost({required ClaxApi api, required ClaxStream stream, required MakeService makes, required BuildLink builds, required HomeSettings settings, required HomeActions actions});
  HomeState get state;
  Future<Object?> call(String callerAid, String method, Map<String,Object?> args); // throws HomeRefused when callerAid != settings.homeAppId
  bool availableTo(String aid);
}
class HomeSettings { String? homeAppId; bool builtin; String? keptRelease; Future<void> save(); static Future<HomeSettings> load(Directory dir); }
```
- Produces (page, `clax.home`): `apps()`, `makes()`, `unread()`, `needsKey()`, `open(id)`, `openMake(id)`, `newApp()`, `menu(id, {x,y,w,h})`, `openInbox()`, `openSettings()`, `importApp()`, `on(name, fn)` for `apps|makes|unread|key`. Icon in `apps()` is `{url}` (daemon-served cached icon), `{emoji}`, `{word}` or `{monogram, hue}`.
- Events go host → shell as the existing `device` event path (`deviceEvent {cap:"home", name, data}`); add it to `HostMessage`/`ClaxHost` in `web/shell/src/app/host.ts` and `host_protocol.dart` if the path has no events yet.

- [ ] **Step 1: Write failing Dart tests** in `home_host_test.dart` with fakes for api/stream/makes/builds:
  - `apps()` from caller == homeAppId returns gallery newest first, home app excluded.
  - caller != homeAppId → `HomeRefused`; `availableTo(other)` is false; still false after the home app called first.
  - a `version` event on `gallery` refetches that app and notifies once; `artifact_deleted` removes it.
  - a make finishing while nobody listens: `state.makes` reflects the latest state when read later.
  - `open`, `newApp`, `menu` call the matching `HomeActions` method with the args.
- [ ] **Step 2:** Run `cd apps/clax_app && flutter test test/home/home_host_test.dart` — expect compile failure.
- [ ] **Step 3:** Implement `HomeSettings`, `HomeHost`, moving the logic from `launcher.dart:168-327`; make `Launcher` take a `HomeHost` and render from `state` (its tests in `test/ui/launcher_test.dart` must still pass unchanged in behaviour).
- [ ] **Step 4:** Route `home` device calls in `app_screen.dart` to `HomeHost.call`, answering `deviceResult {error:"refused"}` on `HomeRefused`.
- [ ] **Step 5: Shell/bridge failing tests** in `home.test.ts`: `home` unavailable outside app mode; available in app mode only when the host reports it for this document; calls relay as `device {cap:"home"}`; `deviceEvent` for `home` reaches `clax.home.on` listeners.
- [ ] **Step 6:** Implement in registry/availability/device-link, `CAPABILITY_METHODS`, `clax-extensions.d.ts`; keep it in the lazy app-mode part. Run `cd web && npx vitest run shell/src/caps bridge/src` and `node scripts/bundle-size.mjs`.
- [ ] **Step 7:** Run `flutter test` for `test/home`, `test/ui/launcher_test.dart`, `test/ui/app_screen*`; commit: "Add HomeHost and the home capability, answered only for the recorded home app".

### Task 2: The home app at parity with today's launcher

**Files:**
- Create: `apps/clax_app/home/` — the home app's source: `index.html`, `home.js` (Preact + htm from the kit), `home.css`, `tests/home.test.js`, `manifest.json` declaring `{"capabilities": {"home": {}, "db": {}}}`.
- Create: `scripts/build-home-bundle.sh` — packs `apps/clax_app/home/` into `apps/clax_app/assets/home/home.clax` with the existing bundle writer (via a small Dart tool `apps/clax_app/tool/pack_home.dart` using `bundle/bundle_writer.dart`), signed with the dev/install key path the exporter uses. Works from any cwd; dry-run default, `--i-know-what-i-am-doing` to write; `just home-bundle` passes the flag.
- Modify: `apps/clax_app/pubspec.yaml:37` — add `assets/home/`.
- Modify: kit fake host for tests: `apps/clax_app/assets/kit/` gets `fake-home.js` (sample apps/makes) so the agent's tests and the inspector can run a home app.
- Test: `apps/clax_app/home/tests/home.test.js` (runs under the kit test runner the agent uses), Playwright `web/e2e/home-app.spec.ts` with a fake host.

**Interfaces:**
- Consumes: `clax.home` from Task 1.
- Produces: `assets/home/home.clax` (manifest title "Home", content hash recorded in `assets/home/release.json` `{"release": "<semver>", "hash": "<content hash>"}`).

- [ ] **Step 1:** Write `home.test.js` against `fake-home.js`: grid lists apps newest first with icon kinds; making tiles first with their state labels (Making…, Offline, Waiting…, Interrupted, Couldn't make it) and the "3/7" ring; tap → `open(id)`; long press (500 ms pointer hold, or contextmenu) → `menu(id, rect)`; New app button → `newApp()`; Inbox shows the unread badge and calls `openInbox()`; key banner when `needsKey()`; empty state "Say what you want…"; with `clax.home` unavailable (a desktop browser, or a recipient's copy) the page shows "This is a Clax home screen. Open it in the Clax app." and no grid.
- [ ] **Step 2:** Implement the page with `kit.css` tokens, matching today's layout: cells ≤ 112 px, 60 px icons, four to a row on a phone, top bar "Clax" with Inbox/Settings/More (Import app…), extended New app button with the mic icon bottom right. Live updates through `clax.home.on`.
- [ ] **Step 3:** Run the page tests and the Playwright spec; render phone/desktop light/dark to `target/flutter-app/home/` and look at them next to `target/flutter-app/x4/launcher-phone-light.png`.
- [ ] **Step 4:** Write `build-home-bundle.sh` + `tool/pack_home.dart`; produce `home.clax` and `release.json`; add to pubspec; commit source, script and generated asset: "Add the home app and its shipped bundle".

### Task 3: First-run install and home mode

**Files:**
- Create: `apps/clax_app/lib/home/home_install.dart` — on daemon Up: if `homeAppId` is null or its artifact is missing, install `assets/home/home.clax` with `bundle/installer.dart`, record the ID; idempotent and safe to re-run after a crash mid-install (match by lineage before installing again).
- Modify: `apps/clax_app/lib/app.dart:1810-1834` — the home route: `HomeRoute` shows `AppScreen(aid: homeAppId, mode: AppMode.home)` when the home app is installed and not `builtin`, else the fallback `Launcher`.
- Modify: `apps/clax_app/lib/ui/app_screen.dart` — `AppMode.home`: back at the page's root lets the system leave Clax (`PopScope canPop: true` at root); no "to launcher" step; pushes for inbox/settings/inspector/making pop back to it.
- Test: `apps/clax_app/test/home/home_install_test.dart`, `apps/clax_app/test/home/home_route_test.dart`.

**Interfaces:**
- Consumes: `HomeSettings`, `HomeHost` (Task 1), `assets/home/home.clax` (Task 2), `Installer.install(Bundle, …)` from `bundle/installer.dart`.
- Produces: `Future<String> ensureHomeApp(HomeSettings, Installer, ClaxApi)`; `enum AppMode { app, home }`.

- [ ] **Step 1:** Failing tests: first run installs once and records the ID; a second call is a no-op; a recorded ID whose artifact was deleted reinstalls; an install interrupted after publish but before the ID is saved adopts the existing artifact by lineage instead of installing twice; offline (fake api with no network) still installs (local daemon only).
- [ ] **Step 2:** Implement `home_install.dart`; call it on daemon Up in `app.dart` (next to `inspector.warm`).
- [ ] **Step 3:** Failing route tests: no ID → fallback `Launcher`; ID → `AppScreen` in home mode; ID arriving while fallback is shown swaps to the home app only after its first `ready` (no empty-grid flash); back at root in home mode does not push/pop to a launcher; resuming the app re-reads `HomeHost.state` so a make finished while covered shows.
- [ ] **Step 4:** Implement `AppMode.home` and `HomeRoute`. Run `flutter test test/home test/ui/app_screen* test/ui/launcher_test.dart`; commit: "Install the home app on first run and show it as the home screen".

### Task 4: Fallback, Undo, Reset and Settings › Home screen

**Files:**
- Create: `apps/clax_app/lib/home/home_recovery.dart` — `undoLast(api, homeAppId)` (restore the version before the latest via `restoreVersion`), `reset(installer, api, homeAppId)` (publish the shipped bundle's files as a new version of the same artifact, data and threads kept).
- Modify: `apps/clax_app/lib/ui/launcher.dart` — `Launcher(banner: HomeBanner?)` shows "Your home screen couldn't load" with "Undo last change" and "Reset".
- Modify: `HomeRoute` (Task 3) — show the fallback with the banner on "This app stopped loading" or on no `ready` within 10 s of a cold start (injected `Clock`).
- Modify: `apps/clax_app/lib/ui/settings_screen.dart` — section "Home screen": "Open" (home app in comment mode), "Undo last change", "Reset to Clax's home screen", "Use the built-in home screen" (toggle, saved as `builtin`).
- Test: `apps/clax_app/test/home/home_recovery_test.dart`, additions to `home_route_test.dart`, `test/ui/settings_test.dart`.

- [ ] **Step 1:** Failing tests: 10 s without `ready` (fake clock advanced) → fallback with banner; `ready` at 9.9 s → no fallback; "stopped loading" → fallback; Undo calls `restoreVersion(n-1)` and the route goes back to the home app; Reset publishes the shipped files as version n+1 with documents untouched; "Use the built-in home screen" shows the fallback without banner and survives restart; Settings › Open opens the home app with comment mode on.
- [ ] **Step 2:** Implement; render the banner and Settings section (phone, light/dark) to `target/flutter-app/home/` and look at them.
- [ ] **Step 3:** Run targeted tests; commit: "Fall back to the built-in home screen, with Undo and Reset, when the home app fails".

### Task 5: Snapshot cover, warm WebView and the `home-usable` budget

**Files:**
- Create: `apps/clax_app/lib/home/home_snapshot.dart` — saves `{png, tiles: [{id, rect}], at}` in the app cache dir when the home screen is covered, backgrounded or updated; loads it on cold start.
- Modify: `apps/clax_app/lib/ui/webview_surface.dart` — expose `Future<Uint8List?> capture()` (WebView screenshot API on Android/macOS); the page reports tile rects through a `home.tiles` call after each render.
- Modify: `HomeRoute` — show the snapshot image under the WebView until `ready`; queue one tap; on `ready`, map it through the *recorded* rects and call `open(id)` only if that ID is still in `HomeHost.state.apps`.
- Modify: `apps/clax_app/lib/agent/timings.dart` — new `home-usable` timing (launch → home app first `ready`) and `home-cover` (launch → snapshot shown); log both.
- Modify: warm the home WebView on daemon Up (as `inspector.warm`, `inspector_surface.dart:313`).
- Test: `apps/clax_app/test/home/home_snapshot_test.dart`.

- [ ] **Step 1:** Failing tests: snapshot round-trips; cold start shows the image in the first frame; a queued tap on a recorded tile opens that app at `ready`; a tap outside tiles does nothing; a tap on a tile whose app was deleted does nothing; a corrupt snapshot file is ignored.
- [ ] **Step 2:** Implement; measure on the macOS app (`just app-macos` build) and record `home-usable`/`home-cover` numbers in the commit message; add the phone measurement step to `docs/usage.md`.
- [ ] **Step 3:** Run targeted tests; commit: "Show the last home screen at once on a cold start and time home-usable".

### Task 6: Shipped updates, agent prompt and kit, docs

**Files:**
- Create: `apps/clax_app/lib/home/home_update.dart` — at startup compare `assets/home/release.json` with the installed home app via `bundle/trust.dart`: unmodified → update silently; modified → `UpdateSheet` ("Keep mine (stop updates)" / "Update (replaces your changes)"); "Keep mine" saves `keptRelease` so it asks once per release.
- Modify: `apps/clax_app/assets/prompt/` (and run `scripts/sync-app-prompt.sh`) — document `clax.home`: it is the launcher; keep open/newApp/menu/inbox reachable; never hide apps the person didn't ask to hide.
- Modify: `docs/contract.md` (the `home` capability), `docs/usage.md` (home screen, recovery, `just home-bundle`).
- Test: `apps/clax_app/test/home/home_update_test.dart`.

- [ ] **Step 1:** Failing tests: same release → nothing; new release + unmodified → silent update; new release + modified → sheet; Keep mine → not asked again for that release, asked again for the next; Update → replaced, data kept.
- [ ] **Step 2:** Implement; update prompt and docs; run `scripts/sync-app-prompt.sh` and its check.
- [ ] **Step 3:** Run `flutter test`, `flutter analyze`, vitest, the home Playwright spec, bundle size; commit: "Update the shipped home app silently or ask when it was changed; document the home capability for the agent".

### Task 7: End-to-end check on the device

- [ ] Build and install with `just app-phone`; on first launch the home app installs and shows; comment "sort my apps alphabetically" on the home screen and see the change land with New version ready; break it on purpose (Settings › Home screen › Open, comment "make the page blank") and confirm the fallback appears with Undo; Undo restores. Record results and `home-usable` on the phone in `progress.md`.
