# A professional Clax on every platform

Date: 2026-10-18. Branch: `flutter-app`. Status: draft for the owner's review.
Builds on `2026-10-16-home-screen-app-design.md` and
`2026-10-16-launcher-arrange-design.md`.

**Scope of this round (2026-10-18).** Implementation covers the Clax app
only (`apps/clax_app` on Android and macOS): the app, the home app, the
starter kit, and the shell's app-mode pieces that run inside the app's
WebView. Browser-only items (the shell's own theming, browser agent naming,
which makes Open question 1 moot, People and hosted features, the gallery
and the browser's panels, composer and notices) are not planned now; the
plan's "Out of scope: browser-only" lists them. Clax ships on iOS next: the
shared components form a Material family and an Apple family, and the
wide layouts follow the window's size class, so the iPad reuses the Mac's
sidebar and detail (plan, "Ready for iOS").

## Goal

Clax looks like one finished product on the Android phone, on Android
tablets, foldables and Chromebooks, on the Mac, and in the browser. It
looks like its own product, too. Today it is three visual systems: Flutter's
slate-blue Material 3, the home app's hand-copied Material hexes, and the
web shell's warm brand. The kit adds a fourth palette. The Mac app is an
Android app in a window called `clax_app` with the Flutter icon.

After this work:

- one brand (the shell's warm neutrals, red-orange for people, green for
  Clax) comes from one token file into Flutter, the home app, the shell and
  the kit, and a check keeps all four in step;
- the Mac follows Apple's HIG: a sidebar and one window, a unified toolbar,
  a native menu bar and context menus, a Settings window;
- Android follows Material 3 on every size: edge to edge, predictive back,
  48 dp targets, and a navigation rail with panes on wide screens;
- what reads as a prototype is gone (raw errors, session names, developer
  views, the Flutter icon). The haikus and easter eggs stay and get a
  deliberate place.

The inputs were three audits of `flutter-app` at `e8900563a`:
- the Flutter app, screen by screen (PNGs in `target/flutter-app/ui-audit/`);
- the home app, the shell and app mode (PNGs in
  `target/flutter-app/ui-audit-web/`);
- the open qualifier records on UI paths.

Audit finding codes (H1, S1, A1, X1) refer to the web audit's tables. Record
IDs are qualifier IDs.

## Owner's decisions

1. **macOS is native on the desktop.** Follow Apple's HIG on the Mac: SF type,
   a unified toolbar, native context menus, a separate Settings window (⌘,),
   and no floating action button.
2. **The Mac has a sidebar and one window**, like Mail or Notes. The sidebar
   lists Starred, the apps, groups and makes. The selected app opens beside
   it under a unified toolbar: back/forward, title, Comment, Threads, Share.
   The home grid shows when nothing is selected. Inbox and Settings are
   separate windows. §4.3 settles how this fits with the home screen being
   a Clax app.
3. **The brand is the web shell's brand everywhere**: warm neutrals,
   red-orange for people, green for the agent (`web/shell/src/theme.css`).
   One token source feeds Flutter's theme on both platforms, the home app's
   CSS, the shell and the starter kit (`apps/clax_app/assets/kit/kit.css`).
   A generated file and a check keep them in step.
4. **Overlays are drawn natively in app mode.** Flutter draws the agent's
   questions, the permission prompt, notices and the draft banner as real
   sheets and banners; the web asks for them over the host link. The
   browser keeps the web versions.
5. **The prototype is cleaned up.**
   - The developer views (Recent timings, raw JSON, MIME types, capability
     names, "Coming" rows) go behind a Developer switch in Settings.
   - The agent is called "Clax" everywhere: never a session name like
     app-2 or claude-2, and never the project folder.
   - A failed change shows one short line. Cost, files and notes go under
     "Details". No raw errors (`Bad state:`, `net::ERR_…`).
   - **The haikus and the whimsy stay.** "Don't drop the haikus. Whimsy is
     important." The haikus and easter eggs are kept, and whimsy gets a
     deliberate, polished place in the design language (§7.4).
6. **The app icon and splash use the Clax mark**,
   `web/shell/public/_clax/mark.svg`: an orange arc, an ink dot and a
   green arc, the arcs facing the dot (`mark-dot.svg` adds a notification
   dot). It becomes:
   - an Android adaptive icon with a monochrome layer;
   - a macOS icon on Apple's grid;
   - a splash mark.
   The app is named "Clax" everywhere: Dock, Finder, window title,
   About and Quit.
7. **Android tablets, foldables and Chromebooks are in scope.** They get the
   Mac's sidebar-and-detail structure, drawn in Material as a navigation
   rail and panes.
8. **Comment entry on Android.** The notification's Comment and Threads
   stay. The edge handle moves out of the system back-gesture zone and
   becomes a clear, labelled tab.

## 1. Design tokens

### 1.1 One source, four outputs

The source is `design/tokens.json` at the repository root: plain JSON, with
light and dark values and per-platform overrides. `scripts/sync-tokens.py`
writes four files from it. It follows `scripts/sync-app-prompt.sh`: it works
from any directory, and with `--check` it writes nothing and exits 1 with a
diff.

| Output | Form | Read by |
|---|---|---|
| `web/shell/src/tokens.css` | `:root` custom properties, both dark blocks (`prefers-color-scheme` under `:root:not([data-theme="light"])`, and `[data-theme="dark"]`) | `theme.css` imports it first. Its own `:root` blocks (lines 18–60 today) go. |
| `apps/clax_app/home/tokens.css` | the same properties | `home.css`. Its `--h-*` Material hexes go; the `--h-*` names become aliases of the brand tokens for one release, then go. |
| `apps/clax_app/assets/kit/kit.css`, between `/* tokens:begin */` and `/* tokens:end */` | the `--k-*` neutrals, type, radii and motion, each `color-mix()` preceded by a plain fallback | resolves 2f7a336048 |
| `apps/clax_app/lib/ui/tokens.g.dart` | `abstract final class ClaxTokens` with `light`/`dark` `ClaxPalette` constants, `ClaxType`, `ClaxSpace`, `ClaxRadius`, `ClaxElevation` and `ClaxMotion` | `lib/ui/theme.dart` |

The script also copies `web/shell/src/view/haiku.json` to
`apps/clax_app/assets/haiku.json`, so the app and the shell share one list
of haikus (§7.4).

**The check.** There is a Bazel test, `//design:tokens_sync`, which runs
`sync-tokens.py --check`, and `just check` runs it too.
- It fails when an output differs from what the source would write.
- It computes the WCAG 2.x contrast of every pair listed under
  `"contrast"` in the source and fails below the stated minimum: 4.5 for
  text and 3.0 for UI boundaries.
- This replaces the hand-kept ratio comments in `theme.css`.

The generated files are committed. A comment at the top of each says it is
generated and names the command that writes it.

### 1.2 Colour

The values are the shell's, kept exactly. New tokens are marked **new**.
Hex values are given as light / dark.

| Token | Light | Dark | Use |
|---|---|---|---|
| `bg` | #f7f7f5 | #141413 | window and page background |
| `card` | #ffffff | #1c1c1a | cards, list groups, fields |
| `raised` | #ffffff | #232321 | sheets, dialogs, popovers |
| `overlay` **new** | #ffffff | #2a2a27 | menus (dark separates from `raised`) |
| `hover` | #f2f2ef | #2a2a27 | hover and pressed fill |
| `menu-hover`, `menu-press` | #f2f2ef, #e6e5e1 | #3a3a36, #46453f | a menu item's hover and press, on `overlay` |
| `fg` | #1c1b19 | #ecebe7 | text; the primary button's fill ("ink") |
| `muted` | #5f5d58 | #a9a69f | secondary text |
| `border` | #e6e5e1 | #2c2c29 | hairlines, decoration |
| `border-hover` | #d6d4ce | #3a3a36 | hovered hairline |
| `border-strong` | #85827b | #7c7972 | boundaries that must be seen (fields, checkboxes) |
| `agent` = `accent` | #2f6f2a | #7cc36f | Clax: avatars, sends, progress, switches, checks |
| `agent-ink` | #2f6f2a | #7cc36f | Clax's text |
| `accent-hover` | #285f24 | #8fd082 | |
| `on-accent` | #ffffff | #141413 | |
| `accent-tint` | #e8f1e6 | #1f2a1d | Clax's containers (FAB, tonal buttons) |
| `you` | #e0532f | #ff7a57 | people: pins, comment mode, unread dots, badges |
| `you-ink` | #b5401f | #ff7a57 | people's text in colour |
| `on-you` | #141413 | #141413 | |
| `comment-hl` | #fbe9e3 | #2e1e19 | people's containers, the comment quote |
| `pink` / `selection-bg` | #f6d3c7 | (as light) | text selection |
| `selection-fg` | #1c1b19 | (as light) | selected text on the web (`::selection`) |
| `selection-tint` | #f6d3c7 | rgba(246,211,199,.28) | Flutter's selection, under text that keeps its colour |
| `danger` | #b3261e | #ff8a80 | errors: icons, text, rules, destructive actions |
| `on-danger` | #ffffff | #141413 | text on a `danger` fill |
| `danger-tint` | #fbeceb | #3a1f1c | small error containers only (§9) |
| `primary-hover` | #3a3935 | #d6d4ce | the ink button's hover |
| `focus` | = `agent` | = `agent` | focus ring: web, Android. macOS uses the system accent (§4.10) |
| `scrim` | rgba(20,20,15,.40) | rgba(0,0,0,.60) | behind modal sheets |
| `pin`, `on-pin`, `pin-ring` | #e0532f, #141413, #ffffff | (same) | over pages, whose colours Clax does not set |

**Flutter's `ColorScheme`** is built from these by hand, not `fromSeed`, and
`claxSeed` goes:

| ColorScheme | Light | Dark |
|---|---|---|
| primary / onPrimary | agent / on-accent | agent / on-accent |
| primaryContainer / onPrimaryContainer | accent-tint / agent-ink | accent-tint / agent-ink |
| secondary / onSecondary | fg / bg | fg / bg |
| tertiary / onTertiary | you / on-you | you / on-you |
| tertiaryContainer / onTertiaryContainer | comment-hl / you-ink | comment-hl / you-ink |
| error / onError | danger / on-danger | danger / on-danger |
| errorContainer / onErrorContainer | danger-tint / danger | danger-tint / danger |
| surface / onSurface / onSurfaceVariant | bg / fg / muted | bg / fg / muted |
| surfaceContainerLowest … Highest **new** | #ffffff, #fbfbf9, #f2f2ef, #ecebe7, #e6e5e1 | #0f0f0e, #1c1c1a, #232321, #2a2a27, #32322f |
| outline / outlineVariant | border-strong / border | border-strong / border |
| inverseSurface / onInverseSurface / inversePrimary | fg / bg / #7cc36f | fg / bg / #2f6f2a |
| surfaceTint | transparent | transparent |

The primary *button* is ink, as in the shell: `filledButtonTheme` sets
`fg` on `bg` with `primary-hover` on hover. Green marks Clax and the
controls (switches, radios, checkboxes, progress). Red-orange marks people.

### 1.3 Type

The brand's type is the system sans, so nothing is downloaded:
- Roboto, the system default, on Android;
- SF Pro (`.AppleSystemUIFont`) on macOS;
- `--font` in the browser.

Mono is `ui-monospace` on the web, `.AppleSystemUIFontMonospaced` then
`Menlo` on macOS, and `monospace` on Android. Every use goes through
`ClaxType.mono`, so the bare `fontFamily: 'monospace'` in
`version_page.dart:522,577,690,756` goes.

Sizes are in px, dp and pt; each cell is size/line-height and weight.

| Role | Web and Android | macOS (HIG) | Used for |
|---|---|---|---|
| `large-title` | 28/34 600 | 26/32 700 | the splash name, empty-state titles on wide screens |
| `title-1` | 22/28 600 | 22/26 600 | screen titles (Android top app bar 22) |
| `title-2` | 17/24 600 | 17/22 600 | sheet and dialog titles, empty-state titles |
| `title-3` | 15/20 600 | 15/20 600 | section heads, card titles |
| `headline` | 14/20 600 | 13/16 700 | list-row titles that lead |
| `body` | 14/20 400 | 13/17 400 | default text (the shell's body) |
| `body-l` | 16/24 400 | 13/17 400 | Android reading text: thread bubbles, settings rows, fields |
| `callout` | 13/18 400 | 12/15 400 | metadata lines |
| `caption` | 12/16 500 | 11/14 400 | tile labels, badges, timestamps |
| `mono` | 12.5/18 400 | 12/16 400 | code, IDs, counts in the shell |
| `haiku` **new** | serif italic 14/22 400, `muted` | New York italic 13/20 | haikus only (§7.4) |

Letter-spacing is 0 throughout. The Material scale is mapped onto these roles
in `claxTextTheme(platform)`:
- `displaySmall` → `large-title`;
- `headlineSmall`, `titleLarge` → `title-1`;
- `titleMedium` → `title-2`;
- `titleSmall` → `title-3`;
- `bodyLarge` → `body-l`;
- `bodyMedium` → `body`;
- `bodySmall`, `labelSmall` → `caption`.

### 1.4 Spacing, sizes and layout

- **Space scale** (4-point): 0, 2, 4, 8, 12, 16, 20, 24, 32, 48, 64.
- **Gutters:** 16 on phones, 24 on wide Android and in Mac panes.
- **Content widths:** reading 720, forms 640, grids 960, sheets and
  dialogs 560 (Android) or 480–640 (Mac sheets).
- **Breakpoints** (Material's window size classes): compact < 600,
  medium 600–839, expanded 840–1199, large ≥ 1200. These replace the
  scattered `bundleDialogBreakpoint`, `changePanelBreakpoint`,
  `questionDialogBreakpoint` and `wide = maxWidth >= 720` with
  `ClaxBreakpoints.of(context)`.
- **Targets:** 48 dp on Android and with a coarse pointer on the web. On
  macOS, AppKit's regular control size, with nothing smaller than 24 pt
  to click.
- **Pane widths:**
  - Mac sidebar 220 pt (180–320, resizable, remembered);
  - Mac threads pane 320 pt (280–420);
  - Android list pane 360 dp;
  - Android rail 80 dp.

### 1.5 Radii

| Token | Value | Used for |
|---|---|---|
| `radius-xs` | 4 | key caps, small tags |
| `radius-sm` | 8 | buttons and fields (web, Android) |
| `radius` | 10 | cards, list groups, menus, Mac sheets' inner cards |
| `radius-lg` | 16 | Android FAB, banners, bubbles (one 4 corner at the speaker) |
| `radius-xl` | 28 | Android bottom sheets (top corners), dialogs, search bar |
| `radius-full` | 999 | pills, badges, avatars |
| `radius-control-mac` | 6 | Mac push buttons, fields and popups drawn by Flutter |
| `tile` | 25% of the icon's size | app icons in grids |

These seven replace the thirteen values in use now (2 to 28).

### 1.6 Elevation

| Level | Light | Dark | Used for |
|---|---|---|---|
| 0 | none | none | page content |
| 1 | `0 1px 2px rgba(20,20,15,.06), 0 4px 16px rgba(20,20,15,.05)` | `0 1px 2px rgba(0,0,0,.4)` + 1 px `border` | cards |
| 2 **new** | `0 2px 6px rgba(20,20,15,.08), 0 8px 24px rgba(20,20,15,.10)` | `0 2px 6px rgba(0,0,0,.45), 0 8px 24px rgba(0,0,0,.5)` + 1 px `border-hover` | menus, popovers, chips over pages, toasts |
| 3 | `0 2px 6px rgba(20,20,15,.06), 0 16px 40px rgba(20,20,15,.12)` | `0 2px 6px rgba(0,0,0,.4), 0 16px 40px rgba(0,0,0,.5)` + 1 px `border-hover` | sheets, dialogs |

In dark mode a raised surface is always one surface step lighter *and*
outlined, since shadows do not show on dark. This fixes H9 (menus at
#1e1f25 on #121318). In Flutter, `ClaxElevation.of(level)` returns a
`BoxDecoration`. Material's `elevation` is used only where Material draws
the shadow itself; `surfaceTintColor` stays transparent.

### 1.7 Motion

| Token | Value | Used for |
|---|---|---|
| `t-fast` | 120 ms, standard (.4,0,.2,1) | hover, press, colour (the shell's `--t`) |
| `t` | 200 ms, standard | small moves, fades |
| `t-enter` | 300 ms, decelerate (.05,.7,.1,1) | sheets, panes, banners arriving |
| `t-exit` | 200 ms, accelerate (.3,0,.8,.15) | leaving |
| `spring-tile` | the constants in `home/physics.js` | home tiles (unchanged) |
| `mark-breath` **new** | 1600 ms loop, ease-in-out | the working mark (§7.4) |

With reduced motion, every duration above 120 ms becomes a 120 ms
cross-fade: `prefers-reduced-motion` on the web,
`MediaQuery.disableAnimations` in Flutter, and Reduce Motion on the Mac.
Springs, the toss and the mark's breath stop.

## 2. Shared components

Each lives once under `apps/clax_app/lib/ui/kit/` and adapts to its
platform and size. Screens use these, never a bare Material widget with
local styling.

| Widget | Android | macOS |
|---|---|---|
| `ClaxScaffold` | edge-to-edge `Scaffold` with an `AnnotatedRegion<SystemUiOverlayStyle>` from the surface under the status bar | content view inside the main window or a Settings or Inbox window, toolbar items declared through `MacToolbar` |
| `ClaxButton` (`.primary`, `.tonal`, `.plain`, `.destructive`) | Filled ink, FilledTonal on `accent-tint`, Text, Text in `danger`; 40 dp high in a 48 dp target | push buttons: default (system accent), regular, borderless; 24 pt; Return triggers the default |
| `ClaxTextField` | outlined, `radius-sm`, 1 px `border-strong`, 2 px `focus` when focused, filled `card`; the label floats into the outline's gap | bordered rounded field, `radius-control-mac`, 13 pt, the system focus ring |
| `ClaxSwitch`, `ClaxCheckbox`, `ClaxRadio` | Material, coloured from the scheme (green) | the Mac switch, checkbox and radio look, in the system accent |
| `ClaxSheet.show` | compact: modal bottom sheet (`radius-xl`, drag handle, no X); medium+: centred dialog, max 560 | a window-attached sheet that slides from under the toolbar, 480–640 pt, buttons bottom-right (Cancel, then the default) |
| `ClaxDialog.show` | M3 `AlertDialog`, actions right-aligned; full-width stacked when a label does not fit | `NSAlert` as a window sheet, through `NativeDialogs` (§4.9) |
| `ClaxMenu.show(items, anchor)` | anchored `MenuAnchor` popup | native `NSMenu` popped up at the pointer (§4.9) |
| `ClaxBanner` | `radius-lg` card on `card` with a leading icon; actions inline when wide, below when narrow | an inline bar under the toolbar |
| `ClaxToast` | floating `SnackBar`, 16 dp margins, max 560 wide | an inline bar under the toolbar, which slides in and dismisses itself after 4 s |
| `ClaxEmpty` | centred column, max 360: icon or mark, `title-2`, `body` in `muted` with balanced lines, then actions | same, centred in the pane |
| `ClaxBadge` | `you` fill, `on-you` text, caption; hidden at 0; "99+" above 99 | same |
| `ClaxMark` | the mark as a `CustomPainter` from the same path data as `mark.svg`, with the states in §7.4 | same |

`claxTheme(brightness, platform)` in `lib/ui/theme.dart` sets the following
from the tokens:
- `textTheme`, `filledButtonTheme`, `inputDecorationTheme` (outlined;
  this fixes the label defect at `theme.dart:22-29`), `snackBarTheme`
  (floating, max 560), `dialogTheme`, `listTileTheme` (one 16 dp inset),
  `chipTheme`, `menuTheme`, `tooltipTheme` and `badgeTheme`;
- `pageTransitionsTheme`, with `PredictiveBackPageTransitionsBuilder` on
  Android;
- `visualDensity`: standard on Android, compact on macOS.

The web gets the same set in CSS:
- `accent-color: var(--accent)` on `:root` in the shell, home and kit, so
  checkboxes and radios are themed (S14);
- `::selection` from tokens in home (H6);
- 44 px buttons with a coarse pointer (X3).

## 3. Per-platform structure

### 3.1 Android phone (compact)

Unchanged in shape: the home app is the home screen, an app opens full
screen with no Clax chrome over it, and Inbox, Settings, the inspector and
making push over it. What changes:

- **Edge to edge.** `SystemChrome.setEnabledSystemUIMode(edgeToEdge)` with
  transparent bars. Every screen pads by `MediaQuery.padding`, and the
  status-bar style follows the surface under it (`ClaxScaffold`).
- **Over an app**, the status-bar band is painted in the page's own top
  colour, and the navigation-bar band in its bottom colour. The shell
  reports both in the `chrome` message (§6.2), and icon brightness follows
  their luminance. The `pillStrip` band goes, and the page always has the
  full height. This removes both bands in `app-working-pill` and fixes
  c8cac2b3ff, since the snapshot and the live page now share one frame.
- **Predictive back.**
  - `android:enableOnBackInvokedCallback="true"` in `AndroidManifest.xml`.
  - The predictive builder on every route.
  - `AppScreen` uses `PopScope(canPop: !history.back)` and turns a back
    gesture inside the page into `GoBack`.
- **Long-press menus** are anchored popups (`ClaxMenu`), not bottom sheets.

### 3.2 Android wide (medium and up): tablets, foldables, Chromebooks

The same structure as the Mac, in Material.

```
┌──┬────────────────┬──────────────────────────────────────┐
│⊕ │ ⌕ Search apps  │ ← Groceries        💬 Comment  ☰ 3  ⤴ ⋮│
│⌂ │ Starred        │                                      │
│✉3│  ▣ Runs        │            the app's page            │
│⚙ │ Apps           │                                      │
│  │  ▣ Groceries ● │                                      │
│  │  ▸ Tools (3)   │                                      │
│  │ Making         │                                      │
│  │  ◌ A tide table│                                      │
└──┴────────────────┴──────────────────────────────────────┘
 rail   list pane 360           detail pane
```

- **Navigation rail** (80 dp): the New app FAB at the top (a mic and
  "New", `accent-tint`), then Home, Inbox (with its badge) and Settings.
- **Expanded (≥ 840)**, the list-detail canonical layout:
  - The list pane has the same model as the Mac sidebar (§4.2): Search,
    Starred, Apps (groups as expandable rows), Making.
  - The detail pane shows the selected app under a 64 dp top app bar:
    back, title, Comment, Threads with a count, Share, and ⋮ (Inspect,
    Versions, Star).
  - With nothing selected, the detail pane shows the home grid.
- **Medium (600–839):** the rail and one pane. Home is the grid, and an app
  opens in the pane with the same top app bar.
- **Panels.**
  - Threads is a standard side sheet (360 dp) that pushes the page at
    large width, and a modal side sheet below it.
  - Inbox is a rail destination with its own list-detail.
  - Settings is one column, max 640, centred.
- **Foldables.** With a hinge in `MediaQuery.displayFeatures`, the split
  between panes sits on the hinge.
- **Chromebooks and keyboards.**
  - The Mac's shortcuts (§4.10) work with Ctrl.
  - A right click opens `ClaxMenu`.
  - Hover states show (§4.10).
  - Resizing never restarts the activity; the template's `configChanges`
    are kept and checked.

### 3.3 macOS: a sidebar and one window

```
┌ ● ● ●  ⌂ ‹ ›   Groceries · Version 4          ◌  💬  ☰3  ⤴  ⌕ ┐
├────────────────┬─────────────────────────────────┬────────────┤
│ ⌂ Home         │                                 │ Threads    │
│ STARRED        │                                 │ ▸ Open (3) │
│  ▣ Runs        │        the app's page           │ ▸ Resolved │
│ APPS           │                                 │            │
│  ▣ Groceries   │                                 │            │
│  ▸ Tools       │                                 │            │
│ MAKING         │                                 │            │
│  ◌ A tide table│                                 │            │
└────────────────┴─────────────────────────────────┴────────────┘
   sidebar 220      detail (home grid when Home is selected)   pane 320
```

§4 has the details.

## 4. The Mac

### 4.1 Window

- **The main window.**
  - Default 1200×800, minimum 720×480, centred on first launch.
  - Its frame is saved with `setFrameAutosaveName("ClaxMain")`.
  - `titlebarAppearsTransparent`, `.fullSizeContentView`, and
    `toolbarStyle = .unified`.
  - `MainMenu.xib:333-335` and `MainFlutterWindow.swift` change with it.
- **Name and copyright.**
  - `PRODUCT_NAME = Clax` (`macos/Runner/Configs/AppInfo.xcconfig:8`).
  - `app_name = "Clax"` (`apps/clax_app/BUILD.bazel:350`).
  - `PRODUCT_COPYRIGHT = Copyright © 2026 Empathic. All rights reserved.`
- **The window title** is the selected app's title, with "Version 4"
  as its subtitle; with Home selected it is "Clax". There is no in-app title
  under it, so the title shows once.

### 4.2 Sidebar

| Section | Rows | Shown |
|---|---|---|
| (top) | **Home** (house symbol): selects the home grid | always |
| Starred | starred apps, in the person's order | when any are starred |
| Apps | apps and groups in `layout/main`'s order; a group is a disclosure row (folder symbol, name, count) | always |
| Making | makes in progress, with a ring and "3/7"; Offline, Waiting, Interrupted and "Couldn't make it" as a second line | when any exist |

- **Rows** are 28 pt high, with a 20 pt app icon (`tile` radius), the
  title in `body` (13 pt), and an unread-thread count as a trailing
  `ClaxBadge` (never 0).
- **The background** is the sidebar material: an `NSVisualEffectView`
  (`.sidebar`) behind a transparent region of the Flutter view.
- **Selection.** The selected row uses the system accent when the sidebar
  has focus and grey when it does not.
- **Keyboard.** ↑/↓ move the selection, and typing selects by title.
- **Drag** reorders and stars (by dropping into Starred), writing
  `layout/main`.
- **A right click** opens the app's menu (§4.9).
- **The sidebar hides** with ⌃⌘S or View › Hide Sidebar.

### 4.3 Where the sidebar comes from

**Recommendation: the sidebar is native Flutter, fed by `HomeHost`. The
home app keeps the grid in the detail pane.**

- `HomeHost` (`lib/home/home_host.dart`) already owns `apps`, `makes`,
  `unread` and `key`. A new `HomeLayout` beside it reads the home app's
  `layout/main` document through the daemon, as the owner. It does the
  same reconciliation as `home/arrange.js`, so the sidebar shows the same
  Starred, order and groups as the grid.
- Drags in the sidebar write `layout/main` whole, pinned to the version
  read. On a conflict they re-read and retry once, as the home app does,
  and keys `HomeLayout` does not know are written back unchanged.
- If `layout/main` is missing or unreadable, the sidebar lists Apps newest
  first. "Use the built-in home screen" (Settings › Home Screen) changes
  the grid, never the sidebar.
- The grid keeps everything the person changed by commenting on the home
  app.

Why native:
- A sidebar is window chrome, like the toolbar. It needs source-list
  selection, type-select, VoiceOver's outline semantics, the sidebar
  material and native context menus. A WebView gives none of these.
- It must keep working when the home app fails, is being updated or is
  being changed by the agent. Navigation that a comment on the home app
  could break would make the window unusable.
- It costs no second always-alive WebView and no cold-start wait, and it
  draws on the first frame from `HomeHost`'s cached lists.

What is given up: a comment on the home app cannot reshape the sidebar. The
arrangement still reaches it through `layout/main`, so stars, order and
groups made in the grid show there. The rejected path (the home app
rendered in the sidebar) is recorded as an alternative on this section,
with `revisit:` when people ask to change the sidebar by commenting.

The `home` capability gains `chrome()`, which answers:
- `{platform: "android" | "macos", form: "phone" | "wide", toolbar: boolean}`;
- an event `chrome` when the answer changes.

With `toolbar: true` the home app draws no top bar, no search field and no
FAB, because the window's toolbar and the rail have them. Big tiles show
only at `form: "phone"`. This settles H5 and the audit's question about big
tiles on desktop. The contract, its types, the agent prompt copies and the
kit's fake host change with it.

### 4.4 The toolbar

The toolbar is a native `NSToolbar`, built by `MacToolbar` in
`macos/Runner/Toolbar.swift`. Flutter declares the items and their state
over the channel `dev.empathic.clax/toolbar`, and the toolbar sends back
which item was clicked. Every item has an SF Symbol and a label for
"Icon and Text" mode.

| Item | Symbol | Action | Key |
|---|---|---|---|
| Toggle sidebar | `sidebar.left` | system | ⌃⌘S |
| Back / Forward (segmented) | `chevron.left` / `chevron.right` | page history first, then the app selection | ⌘[ / ⌘] |
| Title and subtitle | (window title) | | |
| Working | `ClaxMark`, breathing, in a 28 pt item | opens the change popover (the change sheet's wide form) | |
| New Version | `arrow.down.circle` | shown when one is ready; loads it | ⌘R |
| Comment | `text.bubble` (on: filled, `you`-tinted) | comment mode | ⇧⌘K |
| Threads | `list.bullet.rectangle` + count | shows or hides the threads pane | ⌘K |
| Share | `square.and.arrow.up` | `NSSharingServicePicker` with the exported `.clax` | |
| Search | `NSSearchToolbarItem` | filters the sidebar, and the grid when Home is selected | ⌘F |

With Home selected, Comment and Threads act on the home app, as on the
phone.

### 4.5 Settings window (⌘,)

- A separate window, 600 pt wide, whose height animates per tab.
- Its toolbar tabs (SF Symbols) are:
  - General (`gearshape`);
  - Claude (`key`): the API key and the model;
  - Dictation (`mic`);
  - Sharing (`person.2`);
  - Notifications (`bell`): the system setting's state and Open System
    Settings;
  - Home Screen (`house`);
  - Advanced (`wrench.and.screwdriver`): the Developer switch, and Developer
    tools when it is on.
- Each tab is a grouped form: right-aligned labels in a 180 pt column and
  controls beside them.
- The window title is the tab's name.

### 4.6 Inbox window

- Window › Inbox (⇧⌘I) opens a separate window, 900×640 by default and
  remembered.
- It is a list-detail: the inbox list is 360 pt with a toolbar search and
  an All / Unread / Questions segmented control, and the thread view sits
  beside it.
- "Open app" in a thread brings the main window forward with that app
  selected.

### 4.7 One engine, several windows

Settings and Inbox are further views of the one Flutter engine, through
Flutter's multi-window support at the pinned SDK (3.47). They share
`HomeHost`, the API client and settings. Phase 6 opens with a spike to
confirm it. The fallback is a second `FlutterViewController` on the same
engine, driven from `AppDelegate.swift`.

### 4.8 Menu bar

The menu bar is native, in `MainMenu.xib`, with the standard selectors.
Clax's own items reach Dart over `dev.empathic.clax/menu`, and their
enabled state and titles are set from Dart. `PlatformMenuBar`
(`lib/ui/app_menu.dart:132-190`) is removed on macOS: it replaces the whole
menu and cannot carry Edit's responder-chain items.

| Menu | Items (key equivalent) |
|---|---|
| Clax | About Clax · Settings… (⌘,) · Services · Hide Clax (⌘H) · Hide Others (⌥⌘H) · Show All · Quit Clax (⌘Q) |
| File | New App… (⌘N) · Import App… (⌘O) (resolves 128c1f7ff7) · Share App… · Close Window (⌘W) |
| Edit | Undo (⌘Z) · Redo (⇧⌘Z) · Cut (⌘X) · Copy (⌘C) · Paste (⌘V) · Paste and Match Style (⌥⇧⌘V) · Delete · Select All (⌘A) · Find › Find… (⌘F) · (the system adds Writing Tools, AutoFill, Start Dictation and Emoji & Symbols) |
| View | Show/Hide Sidebar (⌃⌘S) · Comment (⇧⌘K) · Show/Hide Threads (⌘K) · Reload App (⌘R) · Actual Size (⌘0) · Zoom In (⌘+) · Zoom Out (⌘−) · Enter Full Screen (⌃⌘F) |
| App | Back (⌘[) · Forward (⌘]) · Home (⇧⌘H) · Inspect (⌥⌘I) · Versions… · Star / Unstar · Resume Build |
| Window | Minimize (⌘M) · Zoom · Inbox (⇧⌘I) · Bring All to Front · (window list) |
| Help | Clax Help (opens the docs) · (the system's search field) |

**Edit in WebViews and in Flutter.**
- Edit's items send `cut:`, `copy:`, `paste:`, `selectAll:`, `undo:` and
  `redo:` down the responder chain.
- When the first responder is a `WKWebView`, the page gets them natively.
- When it is the Flutter view, `EditBridge.swift` forwards the action to
  Dart, which invokes the focused field's intent (`CopySelectionTextIntent`
  and the rest).
- An integration test checks ⌘C and ⌘V in a page field and in a Flutter
  field.

**Escape and Back.**
- Escape while a `WKWebView` has focus is caught by a local `NSEvent`
  monitor in `MainFlutterWindow.swift` and sent to Dart when a Clax panel or
  mode is open (098fa2674e).
- Back (⌘[) steps the page's history and then the selection. Home (⇧⌘H)
  goes straight to the grid (32f8f89d40).

### 4.9 Context menus, dialogs and alerts

- **`NativeDialogs`** (`macos/Runner/NativeDialogs.swift`, channel
  `dev.empathic.clax/native`) offers two calls:
  - `menu(items, at)`: items are `{id, title, key?, symbol?, destructive?,
    enabled?}` or a separator. It pops up an `NSMenu` at the point and
    answers the chosen ID or null.
  - `alert({style, title, body, buttons, defaultIndex, cancelIndex,
    armMs?, suppression?})`: an `NSAlert` run as a sheet on the window. It
    answers `{index, suppressed}`. With `armMs`, the default button is
    disabled for that long from the sheet's first display.
- **Users.**
  - Sidebar rows and grid tiles: the home app's `menu(id, rect, items)`
    comes here, with its rect turned into window coordinates.
  - Inbox and thread rows, and the threads pane.
  - Every `ClaxDialog`.

### 4.10 Hover, cursor and keyboard

- **Hover** fills rows, tiles and plain buttons with `hover`. Home tiles
  also lift (scale 1.03, level 1) on a fine pointer, which fixes H3.
- **Cursors** follow the Mac:
  - the arrow over buttons, rows and tiles;
  - the pointing hand only over links;
  - the I-beam over text;
  - resize cursors over the pane dividers.
  The shell's app mode sets `cursor: default` on controls when the host
  says `platform: "macos"`.
- **Focus.** Full keyboard navigation (Tab, ⇧Tab, Space, Return, arrows in
  lists), with the system focus ring in the system accent.
- **Shortcuts.** Beyond the menus:
  - Space in the sidebar opens the selected app;
  - ⌘⌫ deletes an app, with a confirmation alert;
  - ⌘1…⌘9 select the first nine starred apps.
- **The comment bar.** The bar's shortcuts work while the bar is up
  (3eff33a2's scope).

## 5. Surfaces

Each subsection ends with the records it resolves.

### 5.1 Launcher and home

The home app (`apps/clax_app/home/`) and the fallback launcher
(`lib/ui/launcher.dart`) look the same.

**Look.**
- **Tokens and icons.** Both read the tokens, and both use **Material
  Symbols Rounded** (Apache 2.0, the same glyphs as Flutter's `Icons.*_rounded`).
  `home.js` drops Lucide. On the Mac the home app's own chrome is hidden
  (§4.3), so its glyphs barely show there.
- **Top bar** (phone): a 56 dp bar with `ClaxMark` at 24 dp and "Clax" in
  `title-1`. Its actions are search, inbox with a `ClaxBadge` and settings,
  and the ⋮ menu. The focus ring has 8 px of padding so it is never
  clipped (H8).
- **The New app FAB** is an extended FAB on `accent-tint` with
  `agent-ink`, a mic and "New app". It shows on the phone only; on wide
  Android it is in the rail, and on the Mac it is File › New App and a
  toolbar search.

**Tiles and labels.**
- **Tiles:**
  - the icon is 60 dp on the phone and 64 on wide, with `tile` radius;
  - cells are 112 wide;
  - the grid is centred, with max width 960 (H2).
- **Labels** use `caption` and break only between words, at most two lines
  (H1):
  - CSS: `overflow-wrap: normal; word-break: normal; hyphens: auto`, with
    the page's `lang` set.
  - If one word is wider than the cell, the label is that word alone on
    one line with an ellipsis. `tileLabel()` in `home/grid.js` decides
    this, and `TileLabel` does in Flutter.
- **A make in progress** has its request on line 1 (one line) and its
  state on line 2 in its colour: "Making…", "Offline", "Waiting…",
  "Interrupted", or "Couldn't make it" in `danger`. This fixes H4 and the
  audit's "A tide table…".
  - The icon is the breathing `ClaxMark` on `accent-tint`, with the ring
    drawn in `agent` on `border`.
  - A build on an existing app keeps that app's icon at full strength,
    with a 3 dp `agent` ring around it and a "2/5" `ClaxBadge` at the
    corner. There is no 82% scrim and no ghost monogram.

**Banners, menus and states.**
- **Banners:** one `ClaxBanner` layout for the key banner and the
  recovery banner.
  - Recovery uses `home_rounded` and "Your home screen didn't load", on
    `card` with a `danger` icon, not an `errorContainer` slab.
  - Banners stay within the content width.
- **Long-press** opens an anchored `ClaxMenu`; on the Mac, a native menu.
  The bottom-sheet menu and its misaligned "Recipes" title go.
- **Empty:** `ClaxEmpty` with the mark, "Say what you need, and Clax makes
  it.", `text-wrap: balance` and the New app button (H10).
- **Load error:**
  - "Couldn't show your apps", with "Clax's service didn't answer." and
    Try again.
  - It offers no New app and uses no cloud icon: everything is local
    (H11).
- **Folders:** the rename field selects all on open and starts scrolled to
  the start (H6). The panel is `raised` with level 2; the scrim is 40% on
  light and 60% on dark.
- **Snackbars** in home are full width less 16 px margins, max 560 (H12).
  The home app learns tap or click from `chrome()` ("Tap again to open" or
  "Click again to open").
- **Badges** cap at "99+" (H7).
- **First run:** the fallback grid hides any tile whose ID is the home
  app's, even before `homeAppId` is saved: `HomeHost` knows the ID being
  installed (6833bf1756).

Resolves: eababe2045, 6833bf1756, c8cac2b3ff (with §3.1); and the web
audit's H1–H12.

### 5.2 App chrome

**Android phone.** The page has the full screen. Clax's controls over it:

- **The Clax pill** replaces the three 24 dp pills and the 32 dp strip.
  - It is one floating pill at the top right, 8 dp under the status bar:
    32 dp to see and a 48 dp target. Its surface is `fg` with `bg` text,
    at level 2.
  - It shows only while something is going on:
    - the breathing mark and "Working";
    - "Building 2/5";
    - "New version", with Show;
    - "Location", with its symbol and a label, not a bare arrow.
  - After 3 s it folds to a 32 dp mark-only circle. A tap opens the change
    sheet, or the location details.
  - Nothing pushes the page down.
- **Toasts over an app** ("New version ready", "Sent to Clax") show at the
  top, under the status bar, as `ClaxToast` cards 16 dp from the edges. At
  the bottom they cover the page's own navigation. A Restore or Undo
  suppresses "New version" until its own reload lands (93b32b9ec2).
- **The first-run hint** is one line with one action, shown at the top and
  dismissible:
  - "Turn on notifications to comment from the shade." with
    **Turn on** (2521833ce5);
  - or, once they are refused, "Use the Comment tab on the right to
    comment."
- **Loading:** the app's icon and title centred, and after 400 ms the
  breathing mark under them.
- **The load failure:** `ClaxEmpty` with the app's icon:
  - "This app didn't load", then "Clax's service didn't answer.", then
    Try again and Back to Home;
  - `net::ERR_*` appears only under Details, with Developer on.
- **The status bar band and edge to edge** are as in §3.1.

**Android wide and Mac.**
- The pane's top app bar (Android) or the toolbar (Mac) carries back,
  title, the working item, New Version, Comment, Threads and Share. There
  is no pill.
- Toasts are inline bars under the bar or toolbar, never a 1440-wide
  SnackBar.
- The expanded build and change chips no longer cover the page's header
  (2634bc3a's remaining case). Their content lives in the toolbar popover
  (Mac) or in a side sheet (Android wide).

### 5.3 Comment mode

- **One colour system.** Comment mode is people's, so it is `you`
  red-orange throughout:
  - the page frame and pins (the web);
  - the comment bar's top rule;
  - the composer quote's left rule;
  - the toolbar's Comment button when it is on.
  This fixes A2: `comment_bar.dart:97,163` and
  `comment_composer.dart:239-242` change from `scheme.primary` to
  `tertiary`.
- **The comment bar** (phone and Android wide):
  - "Tap anything to comment on it" on Android, "Click anything to comment
    on it" on the Mac;
  - one row: the instruction, then **Whole app**, then Threads with its
    count, then Voice and Close;
  - all are 48 dp targets, with icon buttons at 24 dp glyphs.
  Below 360 dp, Threads shows only its icon and count.
- **The composer:**
  - The field is `ClaxTextField`, so the 1 px black outline on
    `surfaceContainerLowest` goes.
  - The quote's caption reads "Your pick" rather than "Picked".
  - A composer the page opened (`byPage`) is labelled "From this app" with
    the app's icon, never as Clax's (5e0be2eadd).
  - On the Mac it is a popover anchored at the pick, 360 pt wide, with
    Send on Return and ⇧Return for a new line.
- **A slow page:** when comment mode or `use()` waits for the page, the
  shell posts `pageWait` (§6.2). The bar says "Waiting for the page…"
  (0e9f60d93e).
- **The Android Comment tab** (owner's decision 8) replaces the
  5×56 dp grey bar:
  - It is a labelled tab, "Comment" with `ClaxMark` at 16 dp: 44 dp high
    in a 48 dp target, `fg` fill with `bg` text, level 2.
  - Its right edge sits at `MediaQuery.systemGestureInsets.right + 4 dp`
    from the screen edge, so it is outside the back-gesture zone at every
    gesture-sensitivity setting.
  - It sits at 62% of the height, and a long-press and vertical drag moves
    it; the position is remembered.
  - It is tap-only. A tap opens a small menu: Comment, Threads (n), and
    the change in progress when there is one.
  - It shows when Clax's ongoing notification cannot show (notifications
    off). Settings › Notifications has "Show the Comment tab" to show it
    always.
  - The notification's Comment and Threads actions stay as they are.
- **The Mac:** the stuck area rectangle after a drag (e2ec3a3253) is
  fixed by reporting mouse releases from Flutter to the shell when the
  `WKWebView` misses them. The macOS integration test makes a real mouse
  pick (c0375dfc82).

Resolves: 5e0be2eadd, 0e9f60d93e, e2ec3a3253, c0375dfc82; A2.

### 5.4 Inbox and threads

- **Inbox (phone):**
  - A top app bar titled "Inbox", without the "· 11" count.
  - An M3 `SearchBar` (56 dp, `radius-xl`).
  - Filter chips: All, Unread, Questions, Changes.
- **Inbox rows:**
  - a 40 dp app icon (never a kind glyph over the app's colour);
  - the title in `headline`;
  - one line of summary that names the kind in words ("Clax asked: …",
    "Version 4 is ready");
  - the time trailing.
  Unread is a bold title and an 8 dp `you` dot, with no row tint.
- **Wide:** the inbox is list-detail: 360 dp on Android, and the Inbox
  window on the Mac (§4.6).
- **The thread page:**
  - Its title is the app's name, with a subtitle naming the anchor and
    the version, such as "On the p95 chart · Version 3", or "Whole app ·
    Version 1" as a subtitle, not a headline.
  - The run switcher reads "‹ Run 2 of 3 ›", with two icon buttons.
  - The conversation's width is at most 720, centred, and so is the reply
    bar.
  - "Waiting for connection" offers **Try now** (tonal) and Cancel
    (plain).
- **A failed change** shows once, as Clax's reply. The separate banner
  above it goes.
  - The reply is a neutral `card` bubble with a 3 dp `danger` left rule and
    a `danger` icon.
  - It has **one short line** from `userMessage` (§7.3), such as "This app
    may only reach api.weather.example, so it can't reach strava.com."
  - It offers Retry, and **Details**, a disclosure.
  - Details holds the cost ("$2.10 of $6"), the files changed with their
    lines, and Clax's notes rendered as Markdown, so bullets are bullets.
  - Raw error text appears only with Developer on, at the end of Details.
  - Retry is hidden when the change has spent its limit (45f2d35d25).
  - No `errorContainer` fill anywhere in a thread.
- **The threads panel:**
  - Open threads come first. A collapsed "Resolved (n)" section follows,
    with its rows in `muted`.
  - The meta line is short: "3 comments · not sent".
  - "Send to Clax" is an icon button with a tooltip in the 360 dp panel,
    so titles keep their width.
  - A wide clip gets its own line, cropped to 16:9 at the panel's width
    (ca36e76845).
- **Thread titles never show a CSS selector** (S2):
  - The bridge records `name` at pick time: the element's accessible
    name, else its text (first 60 characters), else its role and nearest
    heading ("Chart under 'p95 by four-hour window'").
  - `anchorLabel` (`web/shell/src/threads.ts:224-235`) uses the quote,
    then `name`, then the kind word ("Table", "Chart", "Image", "Area on
    the page").
  - An old anchor with no name reads "A spot on the page", shown with its
    clip.
  - Quotes are curly in both the shell and home (S16).

Resolves: ca36e76845, 45f2d35d25; S2, S16.

### 5.5 Settings

**Phone and Android wide.** One column, max 640 and centred on wide
screens. Every row starts at one 16 dp inset; there are no leading icons
except the sharing key's fingerprint, and those rows line up too.
- The app bar's title is large and collapses on scroll. While content is
  under it, it shows a 1 px `border` line.
- Sections:
  1. **Claude:**
     - the API key (`ClaxTextField`, so the label sits in the outline's
       gap);
     - Save. Saving warns that changes in progress will stop.
     - **Model**, a menu row: Fast, Balanced or Best, with the model's
       name in the menu.
  2. **Dictation:** the two switches.
  3. **Sharing:** your name, and your sharing key.
  4. **Notifications** (Android): whether they are on, and **Open
     settings** (`openAppSettings`, `MainActivity.kt:70`). Also "Show the
     Comment tab" (da2ee5a94f).
  5. **Home screen:** Open in comment mode, Versions, Undo last change,
     Reset, and Use the built-in home screen.
  6. **About:**
     - Clax's version and the home screen's version;
     - Licences (`showLicensePage`);
     - a haiku, a new one each visit (§7.4).
  7. **Developer:** a switch, **Developer tools**, off by default. On, it
     shows:
     - Recent timings, with every name labelled; `send-first-step` gets
       its label (15fafe1f35);
     - the inspector's Files, Code and Data tabs and raw JSON;
     - MIME types, capability names and the Coming rows;
     - raw error text under Details.

**Mac:** the Settings window, §4.5.

Resolves: da2ee5a94f, 15fafe1f35.

### 5.6 Making and the build view

- **The title.**
  - It is the request in curly quotes, with the ellipsis always inside
    them: “A tide table for…” (`making_screen.dart:152,160`).
  - It is left-aligned in `title-1` and wraps to at most two lines.
- **Progress copy** is plain: "Getting started", "Writing the page",
  "Checking it works", "Waiting for you". File names show only with
  Developer on.
- The "Live" badge's dot sits inside the badge.
- `MaterialTapTargetSize.shrinkWrap` goes (`making_screen.dart:807`).
- **Android phone:** a full-screen route with a close (X) in its app bar,
  rather than "Back to apps" as a text button.
- **Wide and Mac:**
  - The making view opens in the detail pane, under the pane's bar or the
    toolbar, with Back on ⌘[.
  - The preview is the page at the pane's width, max 960, not a 448 phone
    column.
- **Build copy:**
  - "Building: Start and stop a session".
  - A feature the person added says "Added by you" in `caption`, not a
    purple "you" chip.
  - A failure reads "Couldn't add it to the plan." with Details. This is
    the `'$e'` at `build_plan.dart:442`.
- **The build sheet** is a `ClaxSheet`, which is a window sheet on the Mac.
- **Haiku:** after 8 s of waiting, a haiku shows under the progress line
  (§7.4).
- **"Make it again?"** reads: "This wasn't finished." with **Make it
  again** and Cancel. It no longer says "Clax closed".
- **A new app stopped at a cap** is titled from the request's first words,
  not "New app" (9b71118ca5).
- **A new app whose starting documents were refused** opens with a
  `ClaxBanner`: "Some of its starting data couldn't be added." with
  Details (3cb5573fc9).

Resolves: 9b71118ca5, 3cb5573fc9.

### 5.7 Inspector and version page

- **The title shows once.** The app bar says "Inspect", and the hero shows
  the app's icon and title.
- **Tabs:** Overview, Versions, Permissions and Plan. Files, Code and Data
  show only with Developer on.
- **Tabs load on their own.** Each tab loads its own files, and one
  failed file shows Try again in that tab alone (6eb839c54f).
- **Overview** speaks plainly. "Capabilities: db, net, notify" becomes:
  - "Saves its own data";
  - "Uses the internet: api.weather.example";
  - "Sends notifications".
- **Permissions** has no "Coming" rows (Developer shows them). Clax's icon
  is `ClaxMark`, not `smart_toy`.
- **Data,** with Developer on, shows documents as key/value tables, with
  the raw JSON behind a toggle.
- **The version page:**
  - It shows no note heading when there is none ("No note" goes).
  - A changed file whose size did not change reads "Changed".
  - Diffs use `ClaxType.mono`.
  - The diff colours come from the tokens (`agent` and `danger` tints), not
    `version_page.dart:609`'s seed.

Resolves: 6eb839c54f.

### 5.8 Sheets and dialogs

Rules:
- One primary action per surface.
- Button labels are at most three words, and the body explains the
  consequence.
- Destructive actions are `ClaxButton.destructive` and are never the
  default.
- A sheet has a drag handle *or* a close button, never both.

**Sharing, importing and updating:**
- **Update sheets:** the body says what each choice does. Two buttons sit
  side by side, "Keep mine" and **Update**; on a phone too narrow for
  both, they stack full-width.
- **Export:**
  - **Share** (an app without your data) or **Share a copy with my
    data**.
  - "Include my data" no longer sits above both (c1dc9d72cc).
  - Three buttons never wrap onto two rows.
- **The update progress sheet** has Cancel (874d8a6753).
- **Bundle checks:** "Tests" becomes "Checks", and Details is a
  disclosure, not a text button beside Keep current.

**Consent:**
- **Read consent:**
  - The title is one line: "Let Clax read Groceries?"
  - The actions are "Don't allow" and **Allow once**.
  - "Always for this app" is a checkbox on Android, and the alert's
    suppression checkbox on the Mac.
  - It says "Clax", never "the agent".
  - A run in the background that waits on it shows "Clax needs your OK"
    in its notification (e2b8fca37b).
- **Location consent:** Precise and Approximate are a radio choice in the
  body, and the actions are "Don't allow" and **Allow**.

**Change sheet:**
- Stop is `ClaxButton.destructive` (plain) and Open thread is tonal, both
  40 dp.
- The phone has a handle and no X.
- "No errors" goes; it shows errors only when there are some.

Resolves: c1dc9d72cc, 874d8a6753, e2b8fca37b.

### 5.9 Overlays drawn natively

In app mode Flutter draws these four. The shell draws its web versions only
in a browser, or when the host has not declared them (§6.1).

| Overlay | Android | Mac |
|---|---|---|
| **Permission prompt** | M3 `AlertDialog` route above the WebView. "Don't allow" has focus; **Allow** is disabled for `armMs` from the dialog's first frame, with "Allow in a moment…" under it until it arms. | `NSAlert` sheet through `NativeDialogs.alert`, with `armMs` and the default button disabled until it arms |
| **Clax's questions** | `QuestionsSheet`: a bottom sheet (`radius-xl`, handle) titled "Clax has a question", one native `QuestionCard` per question; centred dialog on medium+ | a window sheet, 560 pt |
| **Notices** | `ClaxToast` at the top over an app | an inline bar under the toolbar |
| **The draft banner** | a 48 dp `ClaxBanner` under the status bar: "Clax is drafting…" with **Show draft**; when shown, "Showing Clax's draft" with **Back** | a bar under the toolbar, same copy |

**The permission prompt.**
- Allow counts only on a tap or click whose press began after arming. A
  Return or Space that went down after arming also counts, because a page
  cannot synthesise native input or draw over a native route.
- This gives the browser's keyboard-less guard
  (`PromptDialog.svelte`) a keyboard path in the app. 2ae15dfc4b stays
  open for the browser.
- Escape or a tap outside answers `dismiss`, never `deny` (44415b8019).

**Questions.**
- `QuestionCard` (`lib/ui/questions/question_card.dart`) draws:
  - single and multiple choice, as rows with label and detail;
  - "Other…" with a field;
  - Skip, and **Answer**.
- It always says "Clax"; neither "Answer app-2" nor "in tmp" appears.
- Cards come from the daemon's question routes (`ClaxApi`) for the app's
  artifact, and answers go back the same way. The shell learns of them from
  its stream as now.
- The sheet replaces `question_sheet.dart`'s whole `/inbox` page
  (6a28a0de6a).
- Only one overlay shows at a time. The questions sheet waits while the
  composer is open and while a permission prompt is up (8fa8b99217).

Resolves: 6a28a0de6a, 8fa8b99217, 44415b8019; A1, A3, A4, A5, A6. The prompt
also resolves the web audit's S5 in the app; the browser's dialog gets the
same "Allow in a moment…" line.

### 5.10 Errors and empty states

- **Every empty and error state is a `ClaxEmpty`**, centred, text included:
  - `error_screen.dart:21-40` left-aligns its text under a centred icon;
    that goes.
  - States that wait use the mark; other states use a Material Symbol or SF Symbol.
- **The start error screen:**
  - It reads "Clax couldn't start" and "It stopped while starting. Try
    again, or copy the details for a report."
  - Its actions are **Try again** and Copy details.
  - The log sits under a Details disclosure in `mono`, scrolling
    sideways, with a fade at the edge.
  - "clax stopped 5 times…" and "The last lines clax wrote" go.
- **The browser shell:**
  - **A deleted artifact** (S3) shows the mark, "This page was deleted.",
    and **Back to gallery**. In app mode the shell posts `gone`, and Flutter
    shows "This app was deleted." with Back to Home and drops it from the
    sidebar.
  - **An unknown URL** (S18) shows "Nothing here" with Back to gallery,
    rather than showing the gallery silently.
  - **The notice banner** (S1) is `left: 16px; right: 16px;
    margin-inline: auto; max-width: 560px`, top 12 px. In app mode it is
    native (§5.9). The connection pill and the gesture hint stack, never
    overlap (S8).
  - **Badges at zero** hide everywhere, including "Threads 0" in
    `PhoneTabs.svelte:9` and `TopbarIsland.svelte:65` (S4).
  - **The phone versions sheet** gets a heading ("Versions") and Close, and
    its "Addressed" numbers sort ascending (S9).
  - **The composer's clip** is at most 160 px high on a phone, never
    upscaled, on `card` in dark mode (S7).
- **Raw errors never reach the screen.** `userMessage(Object)` in Dart
  and `userText(unknown)` in the shell map known failures to one line
  (§7.3). Anything unknown reads "Something went wrong. Try again." The
  raw text goes to the log, and to Details with Developer on.
  - This covers `make_service.dart:146`, `build_plan.dart:442` and
    `bundle_sheets.dart:1046,1100`.
  - A check fails on a `Text('$e')`, `'$e'` or `${e}` interpolation in
    `lib/ui/` and `lib/home/`: `scripts/check-copy.py`, run by `just check`.

### 5.11 Splash and icon

**The mark.** `mark.svg` is a 22×16 viewBox:
- an orange arc (`you`) at the left, opening left;
- an ink dot (`fg`) at the centre;
- a green arc (`agent`) at the right, opening right.
The path data is copied once into `design/mark.json` (arcs, dot, stroke
2.6, round caps). The icon builder, `ClaxMark` and the splash drawable all
read it.

**Android icon** (`android/app/src/main/res/`):
- `mipmap-anydpi-v26/ic_launcher.xml` and `ic_launcher_round.xml`, each an
  `<adaptive-icon>` with three layers:
  - the background, `@color/ic_launcher_bg` (#f7f7f5);
  - the foreground, `drawable/ic_launcher_fg.xml` (vector);
  - the monochrome, `drawable/ic_launcher_mono.xml`.
- On the 108 dp canvas the mark is 52 dp wide (38 dp high), centred, so
  its corners stay inside the 66 dp safe circle.
- The monochrome layer is the same three shapes in one colour; Android
  tints it for themed icons.
- Legacy `mipmap-{m,h,xh,xxh,xxxh}dpi/ic_launcher.png` (48–192 px) and the
  512 px store icon are rendered from the same source.

**macOS icon:**
- `AppIcon.appiconset` holds 16–1024 at @1x and @2x on Apple's grid: a
  1024 canvas with an 824 rounded-rectangle body (radius 185, continuous
  corners).
- The body is filled #ffffff to #f2f2ef, top to bottom, with Apple's
  standard drop shadow. The mark is 560 px wide, centred.
- For macOS 26 there is also an Icon Composer `AppIcon.icon`:
  - layers for the background, each arc and the dot;
  - variants for light, dark (body #232321, arcs in the dark tokens, dot
    #ecebe7) and tinted.
  The deployment target (12.0) keeps the `.appiconset` for older systems.

**Building the icons.** `scripts/build-app-icons.sh` renders every size
from `design/mark.json` with `rsvg-convert`, and the outputs are committed.
`//design:icons_sync` checks that `apps/clax_app/icons/SOURCE.sha256`
matches the source's hash, so a changed mark without new icons fails.
Pixels are not compared, so the renderer's version cannot flake it.

**Splash.**
- **Android:**
  - `Theme.Clax.Starting` has parent `Theme.SplashScreen`, from androidx
    `core-splashscreen`, for API 29–30.
  - Its `windowSplashScreenBackground` is `bg`, light and dark through
    `values-night`.
  - Its `windowSplashScreenAnimatedIcon` is the foreground vector on the
    splash's 288 dp canvas: the mark 132 dp wide, inside the 192 dp
    circle.
  - `postSplashScreenTheme` is `Theme.Clax` (DayNight, transparent bars).
  - The template's `Theme.Light.NoTitleBar` and white `launch_background`
    go.
- **Flutter's first frame:**
  - `ClaxSplash` (`lib/app.dart:2445-2475`) draws the same mark at the same
    size and place, so there is no jump.
  - After 600 ms, if the daemon is still starting, the mark breathes and
    "Starting…" fades in under it.
  - Roboto's "Clax" word goes.
- **The Mac** has no splash. The window opens with the sidebar from
  `HomeHost`'s cache. If the daemon takes over 600 ms, the detail pane
  shows the breathing mark.

**The name.** "Clax" everywhere: `android:label` (already), `PRODUCT_NAME`,
`app_name`, the About panel, Hide Clax and Quit Clax, and the window title.

## 6. Host messages

### 6.1 Declaring native overlays

`claxHost.init` (`Init`, `host_protocol.dart:598`) gains `native`: a list
of the overlays the host draws itself, with any of `"ask"`, `"questions"`,
`"notice"` and `"draft"`. It also gains `platform`: `"android"` or
`"macos"`. An overlay the host leaves out keeps its web version. So does an
older app, which sends no `native`.

### 6.2 Messages (shell → host)

Each carries the per-load `nonce`, as every message does
(`web/shell/src/app/host.ts:56`). Dart gets one sealed class each in
`lib/ui/host_protocol.dart`.

| Message | Fields | Sent when |
|---|---|---|
| `ask` (`AskMsg`) | `id`, `kind: "capability" \| "download" \| "crossRead" \| "sample" \| "comments"`, `title`, `body`, `allow`, `deny`, `always?` (the label for an "always" choice), `armMs` (= `ALLOW_DELAY_MS`, 500) | a prompt reaches the head of the shell's `promptQueue` and `ask` is native |
| `askCancel` | `id` | the view unloads or the page goes away before an answer |
| `questions` (`QuestionsMsg`, extended) | `open`, `showing`, `ids: string[]`, `raise: boolean` | as now, plus `raise: true` when the shell would raise its sheet |
| `notice` (`Notice`, extended) | `text`, `tone: "info" \| "error"`, `action?: {id, label}` | as now; with `notice` native the shell draws nothing |
| `draft` (`DraftMsg`) | `state: null \| {kind: "drafting", by, note?} \| {kind: "showing", by, base, detail}` | the draft banner's text would change |
| `chrome` (`ChromeMsg`) | `top`, `bottom` (`#rrggbb`) | first paint, and whenever the page's top or bottom background changes (sampled at the bridge from the elements at the viewport's edges) |
| `gone` (`GoneMsg`) | none | the artifact was deleted |
| `pageWait` (`PageWaitMsg`) | `waiting: boolean` | comment mode or `use()` starts or stops waiting on a slow page |

`by` is the agent's display name (§7.2), which is "Clax" for the app's own
agent.

### 6.3 Calls (host → shell)

| Call | Arguments | Effect |
|---|---|---|
| `askAnswer` (`AskAnswer`) | `id`, `"allow" \| "deny" \| "always" \| "dismiss"` | resolves the queued prompt; `always` is `allow` plus the grant the web dialog's "always" stores |
| `noticeAction` (`NoticeAction`) | `id` | runs the notice's action |
| `showDraft` / `hideDraft` | `by?` | as the web banner's Show draft and Back |

With `questions` native, the host never calls `OpenQuestions` or
`CloseQuestions`. Both stay for browsers and older hosts.

### 6.4 Tests

- vitest:
  - with each overlay declared, the shell draws nothing and posts the
    message;
  - without it, it draws the web version;
  - `askAnswer` resolves the right queued prompt;
  - a late answer after `askCancel` is ignored.
- Dart: parsing every new message, and refusing bad shapes with a
  `TypeError`, as `DeviceRequest.fromJson` does.
- Widget tests: the native prompt's arming with an injected clock (no
  sleeps), dismiss versus deny, and one overlay at a time.
- Playwright app-mode: a fake host declares all four and answers an ask.

## 7. Copy and voice

### 7.1 Voice

- Plain, warm and short. Speak to "you"; Clax is named in the third person
  ("Clax is making it").
- Sentence case. No period on a label; a period on a sentence.
- Curly apostrophes and quotes (’ “ ”) and the ellipsis character (…) in
  all UI text. `scripts/check-copy.py` fails on `n't`, `'s`, `'re`, `'ll`
  and `...` in Dart string literals under `lib/ui/` and `lib/home/`, and in
  Svelte markup under `web/shell/src/`.
- Errors take the form *what happened. What you can do.*, in at most two
  sentences, with no codes, type names or stack text.

### 7.2 Names and glossary

| Say | Not |
|---|---|
| Clax | the agent, app-2, claude-2, codex, a harness name, the project folder |
| app (in the app); page (in the browser shell) | artifact (in the app) |
| change, "Clax is making a change" | run, turn, session |
| version 4 | v4 (in prose; "v4" stays in tight badges) |
| checks | tests |
| your pick | picked |
| uses the internet, saves its own data, sends notifications | net, db, notify, capabilities |
| Clax's service | the daemon, clax (lowercase) |
| Offline | (kept under a tile, per 7b568e1991's recommendation; the build view and screen readers say "Waiting for connection") |

**Display names.**
- The app registers its worker's session with the display name "Clax"
  (`lib/agent/worker.dart`).
- The shell's agent naming (`view/working-model.ts`, `agentNames`) shows
  a session's display name when it has one. It never shows its harness
  label with a counter ("app-2"), and never its project folder ("in tmp").
- In app mode every agent the person sees is Clax. For agents in the
  browser shell, see Open questions.

### 7.3 Before and after

| Where | Before | After |
|---|---|---|
| make failure | Bad state: The Claude API said: overloaded (529)… | Claude is busy right now. Try again in a minute. |
| app load | This app stopped loading · net::ERR_CONNECTION_REFUSED | This app didn't load. Clax's service didn't answer. |
| start | clax stopped 5 times within a minute; last: exited with code 1 | Clax couldn't start. |
| launcher | Couldn't load your apps (cloud icon) | Couldn't show your apps |
| make again | Clax closed before it finished | This wasn't finished. |
| making | Writing index.html · waiting for you | Writing the page · Waiting for you |
| build | Building Start and stop a session | Building: Start and stop a session |
| questions | Answer app-2 · app-2 in tmp | Answer · Clax |
| draft | app-3 is drafting… | Clax is drafting… |
| hint | Notifications are off for Clax, so tap the handle on the right edge… | Turn on notifications to comment from the shade. **Turn on** |
| comment bar (Mac) | Tap anything to comment on it | Click anything to comment on it |
| Keep going (Mac) | Tap Keep going… (`failure.dart:63`) | Click Keep going… |
| shell | Threads 0 | Threads |
| version page | No note · 26 B to 26 B | (nothing) · Changed |

`userMessage` maps, at least:
- `SocketException` and `ClientException` → "Clax's service didn't
  answer.";
- `TimeoutException` → "That took too long. Try again.";
- the API's overloaded and rate-limit errors → "Claude is busy right
  now…";
- an invalid key → "Your Claude API key wasn't accepted. Check it in
  Settings.";
- an `AgentError` → its own message, which is already plain.

### 7.4 Whimsy

Whimsy is part of the design language. It lives where the person waits or
rests: never where they must decide, are warned, or where something failed.
At most one whimsical element shows on a screen. It is quiet: the `haiku`
style, muted, small. Its motion is never more than the mark's breath, and
it is off in comment mode and under reduced motion.

**The haikus.**
- One list, `web/shell/src/view/haiku.json`, copied to the app by
  `sync-tokens.py`. `pickHaiku` picks one: a new one per visit, or seeded
  where a place wants it stable.
- **Kept as they are:** the gallery footer ("clax haiku · a new one each
  visit") and the working strip, both in the browser.
- **New, in the app:**
  - the making screen, after 8 s of waiting, under the progress line;
  - the empty inbox: "All caught up." and a haiku;
  - Settings › About and the Mac's About panel, as its credits.
- **Screen readers** read a haiku only when it is focused. It is never a
  live region, and it carries `aria-label="Haiku: …"`.

**The mark as a character.** The dot is the conversation between the
person (the orange arc) and Clax (the green arc). `ClaxMark` and a CSS twin,
`.clax-mark` in `tokens.css`'s companion `mark.css`, have five states:

| State | Motion (`mark-breath`) | Shown in |
|---|---|---|
| resting | still | splash, empty states, the top bar |
| working | the dot drifts toward the green arc and back, 1.6 s | the Clax pill, the toolbar's working item, making tiles, loading |
| sent | the dot slides to the green arc once (300 ms) | after Send to Clax |
| landed | the green arc nods (scale 1.08 and back, 300 ms) | New version ready |
| asking | the dot drifts toward the orange arc | Clax has a question |

This replaces the 14 dp spinner that read as a stray glyph in stills
(`working_chip.dart:134-138`). Screen readers hear the state's words
("Clax is working").

**Easter eggs.** They are invisible until found, harmless, and never in the
way of a task.
- **Rally of 10** (`view/rally.ts`) stays in the browser. The app gets a
  twin: the first time an app reaches version 10, its "New version ready"
  toast reads "Version 10. A rally!" and the dot bounces between the arcs
  ten times. It shows once per app, remembered in the app's settings.
- **Haiku of the day:** a long-press (Android) or Option-click (Mac) on
  the About haiku shows another.
- Home's springs and toss stay as they are.

### 7.5 Tap and click

`ClaxVerbs.of(context)` gives `tap` ("Tap" / "Click"), `longPress`
("Long-press" / "Right-click") and `swipe` ("Swipe" / "Drag"), chosen by
platform and pointer:
- on a Chromebook with a mouse, "Click";
- on a tablet by touch, "Tap".

The shell uses `(pointer: coarse)`, and the home app uses `chrome()`. Every
"Tap" in `lib/` goes through `ClaxVerbs`. Today those are in
`comment_bar.dart:53`, `failure.dart:63`, `app_screen.dart:2016` and the
home app's "Tap again to open".

## 8. Accessibility

- **Contrast:** the token check (§1.1) guards every listed pair: text 4.5,
  UI boundaries 3.0. It adds the pairs the audit flagged:
  - the "Added by you" caption;
  - `muted` timestamps on `hover`;
  - the Comment tab on page colours, which is now an opaque `fg` fill, so
    3:1 holds on any page.
- **Targets:** 48 dp on Android everywhere (the pills, comment bar, handle
  and chips), and 44 px with a coarse pointer in the shell and home.
- **Text size:** Android font scale up to 200% keeps every screen usable.
  The renders include 130% and 200% for the launcher, a thread, Settings
  and the questions sheet. Tile labels stay at two lines and ellipsize.
- **Screen readers:**
  - every control is labelled;
  - the Clax pill and the toolbar's working item announce their state;
  - the sidebar is an outline (`Semantics` with expand/collapse);
  - thread titles read their anchor names, never selectors.
- **Keyboard:** full keyboard use on the Mac and Chromebooks (§4.10). In
  app mode the native permission prompt is usable from the keyboard
  (§5.9).
- **Reduced motion and transparency:** §1.7. On the Mac, Reduce
  Transparency gives the sidebar a solid `bg`, and Increase Contrast
  swaps `border` for `border-strong`. The generator emits a `contrast-more`
  set, also used for the web's `prefers-contrast: more`.

## 9. Testing

**Renders.** Every surface in §5 is rendered and looked at in three forms
and two themes:
- phone: 412×915 dp at 2.625×, Android, Roboto;
- Android wide: 1280×800 dp at 2×, Android;
- Mac: 1200×800 pt at 2×, macOS, SF Pro.

They are rendered in light and dark, plus the text-scale renders in §8.

- **Flutter:** the audit rig (`apps/clax_app/target/ui_audit/audit.dart`)
  moves to `apps/clax_app/test/ui/shots/rig.dart`, with the three forms.
  The status-bar and gesture-bar stand-ins move with it, and the Mac form
  draws a stand-in toolbar band. The `*_shots_test.dart` files use it.
  They write PNGs to `target/flutter-app/shots/<surface>-<form>-<theme>.png`,
  and nothing compares pixels.
- **Web:** the audit's Playwright spec (`zz-ui-audit.spec.ts`) becomes
  `web/e2e/shots.spec.ts`, in a `shots` project left out of the default
  runs. It renders the shell, home and app mode at phone, Android wide
  (1280×800, touch) and desktop, in light and dark.
- **Looking:** each phase ends with the changed surfaces looked at in all
  six renders before it is called done.

**The macOS window chrome and the menu bar, for real.** `flutter_test` can
draw neither. A macOS integration test,
`apps/clax_app/integration_test/mac_chrome_test.dart`, does both. It is
tagged `manual` like the other macOS integration tests (0c0ad1bbb7) and run
by `just app-integration`.

- **The test channel.** A test-only channel, `dev.empathic.clax/test`, is
  compiled in debug and profile builds only (`#if DEBUG || PROFILE` in
  `MainFlutterWindow.swift`). It offers:
  - `captureWindow(path)`: on macOS 14 and later, ScreenCaptureKit's
    `SCScreenshotManager.captureImage` with an
    `SCContentFilter(desktopIndependentWindow:)` for the main, Settings or
    Inbox window, which includes the title bar, toolbar and traffic lights.
    On macOS 12–13 it uses `CGWindowListCreateImage`.
  - `setAppearance("aqua" | "darkAqua")`, through `NSApp.appearance`.
  - `describeMenu()`: the main menu tree as text (titles, key equivalents,
    enabled state).
  - `openMenu(title)` and `captureMenu(path)`: they open a top-level menu
    with `NSMenu.popUp(positioning:at:in:)` on a run-loop timer, capture
    the display region under it, then `cancelTracking()`.
- **What it does:**
  - drives the app to the states below;
  - writes `target/flutter-app/mac/<state>-<light|dark>.png`;
  - compares `describeMenu()` with the committed text golden
    `integration_test/goldens/main_menu.txt`, so the menu's items and keys
    are checked on every run and the pixels are looked at.
- **States:**
  - Home selected;
  - an app selected, with the toolbar's items;
  - comment mode, with the composer popover;
  - the threads pane;
  - each Settings tab;
  - the Inbox window;
  - the permission alert sheet, before and after arming;
  - a sidebar context menu;
  - each top-level menu open.
- **The real interactions:** it also makes a real mouse pick in the
  `WKWebView` (c0375dfc82), a drag whose release lands outside the page
  (e2ec3a3253), ⌘C and ⌘V in a page field and in a Flutter field, and
  Escape with the page focused (098fa2674e).
- **Permission:** the first run asks for Screen Recording permission for
  the test host. `docs/usage.md` notes it.

**Other checks.** These are in `just check`:
- `//design:tokens_sync`;
- `//design:icons_sync`;
- `scripts/check-copy.py`;
- the existing widget, vitest and Playwright suites, with the new tests in
  §6.4 and §4.3 (`HomeLayout` reconciliation and pinned writes, with
  injected versions).

## 10. Phases

Each phase ships something a person can see. Tokens come first.

1. **Brand tokens.**
   - `design/tokens.json`, `sync-tokens.py` and both checks.
   - The shell, home (a home bundle release) and kit read the generated
     files.
   - `claxTheme` is rebuilt from the tokens, which brings the outlined
     fields (the label defect), the themed checkboxes, floating snackbars
     and the dark-mode separation.
   - On the web: `accent-color`, the notice banner's width, zero-count
     badges and hover states.
   - *Visible:* one warm brand on every surface.
2. **Identity.**
   - The icons (adaptive, monochrome, Mac `.appiconset` and `.icon`).
   - The splash, `ClaxSplash` and `ClaxMark` with its states.
   - The name "Clax", the copyright and About.
   - *Visible:* Clax's mark in the Dock and launcher and at launch.
3. **Clean-up and copy.**
   - The Developer switch, the "Clax" naming, `userMessage`, `userText`
     and `check-copy.py`.
   - The failure card with Details, tap and click, and thread titles from
     names.
   - The deleted and missing pages, tile labels, `ClaxEmpty` everywhere,
     and the haiku placements.
   - *Visible:* no prototype text left.
4. **The Android phone.**
   - Edge to edge, with the band in the page's colour (`chrome`).
   - Predictive back, the Clax pill, top toasts and the Comment tab.
   - Anchored menus, 48 dp targets, Settings › Notifications, and the
     inbox and thread layouts.
5. **Native overlays.**
   - The host messages (§6) on both sides.
   - The prompt, `QuestionsSheet` with `QuestionCard`, the notice and the
     draft banner, in Flutter and through `NativeDialogs.alert` on the
     Mac.
6. **The Mac.**
   - It opens with the multi-window spike.
   - The window, the native menu bar with the Edit bridge and Escape, and
     the toolbar.
   - The sidebar with `HomeLayout`, the home app's `chrome()`, and the
     threads pane.
   - The Settings and Inbox windows, context menus, hover, cursors and
     shortcuts.
   - Mac sheets, and `mac_chrome_test.dart`.
7. **Android wide.**
   - The rail, the list and detail panes, and pane top app bars.
   - Side sheets, the hinge, and keyboard and mouse on Chromebooks.
8. **Device verification. This needs the phone, and runs separately.**
   - Theme T1, in the audit's order:
     - install and open: bc952edafa, 18cb7cff96;
     - a change lands: 1e942268f0, c1401f5967, 104ee7477a;
     - picks: 8cc89df609, 86741946a5, 552b0eb007;
     - the keyboard: e2d39b88dc;
     - cold start: 44801f5ac0;
     - then 56d5a66f4f, 5249ca0f6c, 7219cb2700, ef50a675ad, 96c8a30c5e,
       9f0b8dce57, c3f4359f0f, a22c402ec8, 7c4650a4a1, c92f75e6d3 and
       295a31f61e.
   - Also:
     - the home tiles' feel session: 99ef2afb95, with 15dd68df37 and
       b0a0d9e6a3 decided in it;
     - the Comment tab against the real gesture zone;
     - the status band's colour on real pages;
     - predictive back.
   - The Mac parts of bc952edafa and a22c402ec8 run at phase 6's end on a
     release build.
   - Phases 1–7 do not wait on this phase. Records it confirms are closed
     with the device's evidence.

## 11. Qualifier records

The sections above list what each resolves. Records are closed only with
the evidence of the commit that fixes them (AGENTS.md); every other close is
proposed to the owner.

**Out of scope here, and why:**

| Records | Why |
|---|---|
| 9d4ddacf0e, 553cb43495 | wait on the Apple Development certificate (owner action), not on design |
| bc48d38298, 7219cb2700 (the macOS half) | file-type intake belongs to the bundles spec |
| 1ee688df3a | needs a native `WebViewClient` for renderer crashes; the state it shows is §5.2's load failure |
| a1991d5fe9, 664cefd459 | location behaviour, not presentation |
| 695570708c, 9fbb9e0347, 287cd716ee, aee18f03f1, 4db301fde0, 0a2664b07c, e2bb80a719, 2e2c5abc5f | recovery and retry mechanics; the copy they show follows §7 when they are fixed |
| 16065304e8, a2e0574e66, c102424456, e941822a7c, dc83a75205 (T8) | state preservation and history, not visual |
| 37c67e6ee4, 42c815abd8, 34d387dba6 | performance and build-preview logic |
| 744fb01ddb, 5be7a53639 | `status:deferred` by their own records |
| c2697bc489 | a load trade-off the record leaves to the owner |
| d126063d9a | code highlighting, now behind Developer tools; low priority |
| 18933d0d47 and T10 (94ab5cdaf4 … 3561332cb0) | browser-only People, hosted and live-seat features; this spec rethemes them through tokens but does not change their behaviour |
| the (d) proposed closes | untouched; still the owner's to approve |

**Owner decisions** this spec touches but does not take:
- 2ae15dfc4b: the keyboard path in the browser. The app gets one through
  the native prompt.
- 65e399a8ed: whose agent; this ties into the open question below.
- 7b568e1991: "Offline"; the glossary follows its recommendation.
- e5e0475f1a and ef1f5e8974: browser shell.

The rest of the (c) list is not UI.

## Decisions taken in this draft (the owner may override)

1. The sidebar is native Flutter fed by `HomeHost` plus a new `HomeLayout`
   reading `layout/main`. The home app keeps the grid (§4.3).
2. The token source is `design/tokens.json`. The generated files are
   committed and checked by `//design:tokens_sync`, and contrast is
   checked from the same file.
3. The kit's neutrals, type, radii and motion follow the brand. Its default
   `--k-accent` becomes `agent` green, and apps still override it. Apps made
   earlier keep the kit copy they were built with until their next change.
4. Material's primary colour is Clax's green, but the primary *button* is
   ink, as in the shell. Red-orange is people's: comment mode, pins,
   unread dots and badges.
5. macOS window chrome, menus, context menus and alerts are AppKit.
   Content is Flutter with our own Mac-styled components, not a third-party
   macOS widget package. Focus rings and selection use the system accent on
   the Mac.
6. Settings and Inbox on the Mac are further views of one engine, with a
   second view controller on the same engine as the fallback.
7. The phone's three pills and their strip become one floating Clax pill
   that folds to the mark after 3 s. Toasts over an app show at the top.
8. The Android Comment tab is tap-only, sits outside
   `systemGestureInsets`, shows when notifications are off, and can be
   turned on always.
9. Big tiles show only at phone size.
10. The home app uses Material Symbols Rounded; Lucide goes from home (the
    shell keeps its own icons).
11. Fields are outlined (the shell's style) on Android too, not M3 filled.
12. The questions sheet reads and answers through the daemon from Flutter.
    The shell sends only counts, IDs and `raise`.
13. In the app, the native permission prompt accepts a key press that began
    after arming. Dismissing answers `dismiss`.
14. New whimsy: haikus on the making screen, in the empty inbox and in
    About; the mark's five states; the app's Rally of 10.
15. Icons are rendered by script and committed. Their check compares source
    hashes, not pixels.
16. The copyright line reads "Empathic".

## Open questions

1. **Names of other agents in the browser shell.** In the app every agent
   is Clax. In the browser, Claude Code and Codex sessions also comment,
   publish and draft, sometimes several at once, and some belong to other
   people.
   - Should each of these also be called "Clax"?
   - Or should each go by its product ("Claude", "Codex"), with
     "· <person>" for other people's (65e399a8ed's recommendation), and
     when two of yours are live, "Claude, since 10:42"?

   The draft takes the second. This blocks only phase 3's shell naming;
   the app's naming does not wait on it.
