# The home screen as a Clax app

Date: 2026-10-16. Branch: `flutter-app`. Status: approved by the owner, 2026-10-16.

## Goal

Clax's home screen is a Clax app. It is built, shown and improved exactly
like every other app: you change it by commenting on it, it has versions you
can step back through, and it can be shared. It holds one permission no other
app can hold: to list, open and start your apps and to watch them being
built.

Flutter is the fast, grounded UI around everything. It keeps the threading
and comment UI, the inbox, settings and the API key, the making and build
views, the inspector, share and import, voice capture, the chrome around
every app (pills, back, toasts, New version ready) and recovery.

## What moves and what stays

| Moves into the home app | Stays in Flutter |
|---|---|
| The grid of apps and making tiles, their order and layout | Voice capture and the New app sheet (the home app asks for it) |
| Tile states: Making…, Offline, Waiting…, Interrupted, Couldn't make it, the build ring "3/7" | The making screen and build view (opened by Flutter when a make starts or a making tile is tapped) |
| The empty state, load error and key banner | Settings, the API key, the inbox and its threads |
| The Inbox, Settings and Import buttons (the home app draws them; Flutter opens the screens) | The long-press menu: Open, Inspect, Share app, Resume build (the home app asks for it) |
| Anything the person adds by commenting: folders, sorting, widgets, a greeting | The inspector, bundles, share and import sheets |
| | The comment UI on the home app itself, the pills, back and toasts |
| | Recovery: the fallback home screen, Undo and Reset |

Today's `apps/clax_app/lib/ui/launcher.dart` stays in the tree as the
**fallback home screen** (§6). Nothing it does is lost.

## 1. The home app

- A normal artifact, published from a bundle shipped in the app's assets
  (`apps/clax_app/assets/home/home.clax`, built from `apps/clax_app/home/`
  source by a script, like `assets/kit/`). Installed on first run through
  the existing bundle installer (`bundle/installer.dart`), so it has
  lineage, versions and a seed like any installed app.
- Its artifact ID is recorded by Flutter in the app's settings file beside
  `device-grants.json` (`homeAppId`). No daemon migration.
- It is left out of its own grid and out of every list the home capability
  returns. Settings › Home screen reaches it: Open in comment mode,
  Inspect, Versions, Undo last change, Reset to Clax's home screen.
- It declares a `home` capability in its manifest. **Declaring grants
  nothing**: unknown capability names are stored as given
  (`crates/clax-core/src/capabilities.rs:20`), so the name is not the gate.
  Flutter answers `home` calls only when the calling screen's artifact ID
  equals `homeAppId`. Any other app that declares `home` sees the
  capability as unavailable and every call refused.

## 2. The `home` capability

Carried on the existing device-cap path: the page calls through the bridge,
the shell relays `device {id, cap: "home", method, args}` to the host and
the host answers `deviceResult` (`web/shell/src/caps/device-link.ts`,
`web/shell/src/app/host.ts`). The shell's per-document nonce stops a page
posting host messages itself. Available only in app mode
(`served.device`), so the home app in a desktop browser falls back to a
plain page that says to open Clax.

Page API (`clax.home`, typed in `web/contract/clax-extensions.d.ts`):

| Call | Answer |
|---|---|
| `apps()` | `[{id, title, updatedAt, icon: {url} \| {emoji} \| {word} \| {monogram, hue}, build: null \| {done, total}}]`, newest first, the home app left out |
| `makes()` | `[{id, request, state: "making" \| "offline" \| "waiting" \| "interrupted" \| "failed", progress: null \| {done, total}}]` |
| `unread()` | the inbox's unread count |
| `open(id)` | Flutter opens the app (the same route as a tile tap today) |
| `openMake(id)` | Flutter opens the making screen or build view, or asks "Make it again?" for an interrupted make |
| `newApp()` | Flutter shows the voice New app sheet; the make then appears in `makes()` |
| `menu(id, rect)` | Flutter shows the long-press menu anchored at `rect` |
| `openInbox()`, `openSettings()`, `importApp()` | Flutter opens that screen or the system picker |
| `needsKey()` | whether the API key is missing |

Events (`clax.home.on(name, fn)`): `apps`, `makes`, `unread`, `key`. Flutter
already has every source: the `gallery` and `inbox` stream topics, the
`MakeService` notifier and `BuildLink` (`launcher.dart:168-327`). A new
`HomeHost` in Flutter owns those subscriptions once, for the fallback home
screen and the home app alike.

Icons: `apps()` hands the page a URL to the cached icon (`FileIconStore`,
`app_icon.dart`), served by the daemon under the home app's origin, so the
home app needs no cross-app read grant.

## 3. Showing it

- The home route is an `AppScreen` for `homeAppId` in **home mode**:
  - back at its root leaves Clax (Android) instead of popping to a launcher;
  - no "leave to launcher" step; the inbox, settings, inspector and making
    screens push over it and pop back to it;
  - comment mode, the threads panel, pills, toasts and New version ready
    work exactly as on any app. Changing the home screen is commenting on
    it.
- Opening an app from home pushes its `AppScreen` above the home route.

## 4. Fast first paint

- **Cached snapshot.** Whenever the home app is covered, backgrounded or
  updated, Flutter captures its WebView as an image
  in the app's cache directory, together with the app list it
  was showing. A cold start shows that image at once and swaps to the live
  page on its first `ready`. A tap on the image before `ready` is queued and
  replayed as `open(id)` when it lands on a tile's recorded rectangle.
- **Warm WebView.** The home app's WebView is created as soon as the daemon
  is up, before the first frame the person sees, as `inspector.warm` does
  for the agent's inspector (`inspector_surface.dart:313`).
- **Budget.** New measured gate `home-usable`: app launch → the home app's
  first `ready`, with the snapshot shown within one frame of launch. The
  target is set from the first measurement on the owner's phone.

## 5. Updates to the home app

Clax releases ship a new `home.clax`. At startup Flutter compares it with
the installed home app using the bundle trust rules (`bundle/trust.dart`):

- **Unmodified** (content hash, title and capabilities match a shipped
  version): update silently.
- **Modified by the person**: the existing `UpdateSheet` asks: "Keep mine
  (stop updates)" or "Update (replaces your changes)". "Keep mine" is
  remembered per shipped version, so the sheet asks once per release.

## 6. Recovery

- **The fallback home screen** is today's Flutter launcher, kept working.
  Flutter shows it whenever the home app:
  - has no `homeAppId` yet, or its install failed;
  - fails to load (`AppScreen`'s "This app stopped loading" state);
  - has not sent `ready` within 10 s of a cold start.
  It carries a banner: "Your home screen couldn't load" with **Undo last
  change** and **Reset**.
- **Undo last change** restores the version before the latest
  (`restoreVersion`, as the Versions tab's Undo does).
- **Reset** reinstalls the shipped `home.clax` as a new version of the same
  artifact, keeping its data and threads.
- Settings › Home screen has Undo, Reset and **Use the built-in home
  screen**, which keeps the fallback as home until switched back.

## 7. The agent

- The agent's prompt (`assets/prompt/`, kept in step by
  `scripts/sync-app-prompt.sh`) documents `clax.home` and its rules: the
  home app is a launcher, keep `open`, `newApp`, `menu` and the inbox
  reachable, never hide apps the person did not ask to hide.
- The kit gets a fake `home` host so the agent's tests and the inspector
  run the home app with sample apps and makes.
- A comment on the home app is a normal change: draft, tests, landed,
  New version ready.

## 8. Sharing

The home app can be shared like any app. A recipient installs it as an
ordinary app with no `home` permission. Using a received app as the home
screen is out of scope for now.

## 9. Testing

- Dart: `HomeHost` answers only `homeAppId`; every call refused for another
  app; events follow the stream, `MakeService` and `BuildLink`; the
  fallback shows on each failure trigger; Undo and Reset; update
  silent vs asked.
- Shell: vitest for the `home` relay and availability; Playwright app-mode
  spec with a fake host: the home app lists, opens, starts a make and opens
  the menu.
- Renders of the home app and the fallback banner at phone and desktop
  sizes, light and dark, looked at before claiming the UI works.
- `home-usable` measured on the phone.

## 10. Phases

1. `HomeHost`, the `home` capability on both sides, the gate on
   `homeAppId`.
2. The home app at parity with today's launcher; first-run install from
   `home.clax`; home mode in `AppScreen`.
3. Fallback, Undo, Reset, Settings › Home screen.
4. Snapshot, warm WebView, `home-usable` gate.
5. Shipped updates, agent prompt and kit.

## Decisions taken in this spec

- The home app's ID lives in Flutter's settings, not the daemon, so no
  migration is needed.
- The home app is left out of its own grid.
- A modified home app asks once per shipped release.
- The fallback triggers after 10 s without `ready` on a cold start.
- Using a received app as the home screen is deferred.
