# Arranging the Home Screen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The home app Clax ships (`apps/clax_app/home/`) lets the person star apps, drag them into an order that sticks, gather them into groups shown as folders or piles, find any app by search, and grows the apps they open most into 2×2 tiles, with spring physics, released as home bundles 1.1.0 to 1.5.0.

**Architecture:** The arrangement is one document, `layout/main`, in the home app's own `db`, read and written whole under a short lease. Pure modules beside `home.js` hold every rule (`arrange.js` the document, `grid.js` the cells, `physics.js` the springs and toss, `drag.js` the press, `search.js` the ranking) and are tested under vitest; `home.js` draws them with Preact and moves tiles with a FLIP animator (`motion.js`) on an injected clock. The long-press menu gains the page's own items through `home.menu(id, rect, {items})`, carried by the shell (`web/shell/src/caps/home.ts`) to `HomeHost.call` and `HomeFlows.menu`, which now resolves the picked item's ID.

**Tech Stack:** Preact + htm from the kit (`apps/clax_app/assets/kit/preact.js`), plain ES modules, vitest 3.2 (jsdom) through `web/`, Playwright on the pinned Chromium, the inspect view's test harness (`tests/home.test.js`), Flutter/Dart (`flutter_test`), Bazel (`bazelisk test`, `bazelisk run //apps/clax_app:home_bundle`).

**Spec:** `docs/superpowers/specs/2026-10-16-launcher-arrange-design.md` (builds on `docs/superpowers/specs/2026-10-16-home-screen-app-design.md`).

## Global Constraints

- Commits are unsigned: `git -c commit.gpgsign=false commit …`. Never push. Never file, close or comment on issues or pull requests.
- Never run anything against `~/.clax` or the daemon on port 7480. Playwright's fixtures start their own daemons; a manual run uses `just dev` (`~/.clax-dev`) or a scratch `CLAX_HOME` on a free port, stopped by its PID.
- Never wait on processes with `pgrep -f`/`pkill -f`; use the PID you started or an exit file, with a timeout.
- Tests are deterministic: no sleeps standing in for a condition, no wall-clock budgets. Every timing rule (the 400 ms hold, the 600 ms hover, the springs, the toss, the 14-day window, the lease retry) runs on an injected clock: `manualClock()` from `apps/clax_app/home/physics.js` in vitest and the harness, `page.clock` in Playwright.
- Write "ID" in prose, comments and commit messages; `id` only as a literal symbol in code.
- Doc comments and commit messages describe the contract or the change, never this plan, the conversation or a rating of quality.
- Qualifier runs through `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec <args>` (written `$Q` below: `Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"`). Before editing a file, `"$Q" exec threads <path>`. More than two records go in one `"$Q" exec record --stdin` batch written to a file outside the repository, dry-run first. Read `"$Q" exec agents` and each topic it lists before a session's first record.
- No `unsafe`, no hard links, anywhere.
- Builds and tests go through Bazel (`docs/bazel.md`, "The Clax app"); a native twin is given where Bazel writes outside the checkout (renders). Anything longer than a couple of minutes runs in the background, logged under the worktree's `target/`, waited on in bounded stretches.
- Each phase ships one home bundle release: phase 1 `"version": 8, "release": "1.1.0"`, phase 2 `9`/`1.2.0`, phase 3 `10`/`1.3.0`, phase 4 `11`/`1.4.0`, phase 5 `12`/`1.5.0`, in `apps/clax_app/home/manifest.json`, packed with `bazelisk run //apps/clax_app:home_bundle -- --i-know-what-i-am-doing` (the key is `~/.config/clax/home-bundle/key`; if packing stops on the key, stop and report). `//apps/clax_app:home_home_bundle_test` fails between a phase's first home source change and its pack step, by design ("home.clax holds the current home/ sources"); every other target passes at every commit.
- `layout/main` stays readable by every shipped release from 1.1.0 on: add fields, never change the meaning of one; every write keeps the fields and `order` entries the writer does not know (spec §9).
- The page's copy, verbatim: section headings "Starred" and "All apps"; menu items "Star", "Unstar", "Remove from group"; group menu "Rename", "Show as pile", "Show as folder", "Ungroup"; the tile mark "New"; the ⋮ item "Big tiles for apps you use"; a new group's name is the common icon word, capitalised, else "Group"; the search field's label "Search apps".
- UI work is done only once its renders (phone 412×915 and desktop 1280×800, light and dark) are looked at with the Read tool and match the rest of the home screen.
- The inspect harness test count in `web/e2e/home-app.spec.ts` (`expect(tests.passed).toBe(…)`) is the number of `test(` calls in `apps/clax_app/home/tests/home.test.js` (`grep -c "^test(" apps/clax_app/home/tests/home.test.js`); every task that adds harness tests updates it.

## Decisions this plan takes where the spec is silent or the platform differs

The owner may override any of these; each is the smallest reading that keeps the spec's intent.

1. **Pinned writes (spec §2).** A page's `db` has no version-pinned write (`web/contract/0.2.61/db.d.ts`: `set` is last-writer-wins, no precondition). Each change runs under a 2 s lease on `layout/main` (`acquire`), reads the body, applies the change and writes it whole; a busy lease is retried when the document changes or after 1 s, up to 10 times. That is "re-read, re-apply, write again" without a version.
2. **Opens in the last 14 days (spec §2, §6).** `{n, last}` cannot say how many opens fell in the last 14 days, so each `opens` entry also keeps `days: {"YYYY-MM-DD": count}` (UTC days, the last 14 kept). Counting starts in 1.1.0 so that 1.5.0 has history.
3. **Where the pure functions live (spec §4, §10).** In modules beside `home.js` (`arrange.js`, `grid.js`, `physics.js`, `drag.js`, `motion.js`, `store.js`, `search.js`): `home.js` imports `./kit/preact.js`, which exists only in the bundle, so vitest could not import it. They ship in the bundle like `home.js`.
4. **Where the vitest files live.** In `apps/clax_app/home_unit/`, outside `home/`: the updater runs every `tests/*.test.js` of a home release in the inspect harness (`apps/clax_app/lib/bundle/updater.dart:276-279`) and `pack_home.dart` ships every file under `home/`.
5. **Forward compatibility from 1.1.0.** 1.1.0 already understands `groups` in the document (it shows a group's apps in the group's place) so that stepping back from 1.3.0 never re-files grouped apps as new.
6. **The first load.** With no `layout/main` yet, the page writes one at once with every listed app in `seen`, so nothing reads "New" on the first update to 1.1.0.
7. **Headings.** "Starred" and "All apps" show only when something is starred: an unarranged home screen looks as it does today. While a drag is on, an empty Starred drop zone shows at the top.
8. **Makes** stay first under All apps and are not arrangeable.
9. **Menu items.** An item's ID is 1 to 64 characters and unique in the call; a label is trimmed, 1 to 32 characters (code points), no control characters.
10. **Star / Unstar from the menu.** Star puts the app last in Starred (out of any group); Unstar puts it first in All apps.
11. **Back during search.** Opening search pushes a history entry (`#search`), so the app's back (`apps/clax_app/lib/ui/app_screen.dart:774-779`) closes search; Escape, and Backspace in an empty field, close it too.
12. **Layout read timeout.** If `layout/main` has not answered 2 s after the apps arrived, the grid draws newest first (nothing written) and rearranges when the read answers, so `shown()` is never held past Clax's 10 s.

## Review Focus

1. **The first update to 1.1.0 over a home screen with apps** — no app may read "New", and the order must stay newest first; a new app made after that load must read "New". Test: Task 2 (`reconcile` with `first`), Task 3 (harness "the first load writes every listed app as seen").
2. **Stepping back a release** (Undo last change, or Keep mine, after a newer home wrote `layout/main`) — the older page must keep `groups`, `g:` entries, `bigTiles` and any unknown field when it writes, and must not re-file grouped apps as new. Test: Task 2 (`normalize` and `reconcile` keep unknown fields and groups), Task 3 (harness "a layout from a later release keeps its groups and unknown fields through a star").
3. **Two writes close together, or a write from another device between read and write** (star, then a quick drag; the home screen open on the Mac and the phone) — no change may be lost. Test: Task 2 (`store.change` queues; a foreign `put` between two changes; a busy lease retried).
4. **A scroll that starts on a tile** — a finger moving more than 8 px within 400 ms scrolls the page, never lifts or drags, and a tap still opens. Test: Task 6 (press controller), Task 7 (harness "a touch that moves before the hold scrolls and lifts nothing").
5. **Dragging to Starred with nothing starred** — the person must have somewhere to drop. Test: Task 7 (harness "a drag with nothing starred shows the Starred drop zone, and a drop there stars").

---

## Phase 1 — `layout/main`, Starred, New marks, menu items (release 1.1.0)

Tasks 1 and 2 touch disjoint files and can run in parallel. Task 3 needs both. Task 4 touches only the prompt files and can run in parallel with Task 3.

### Task 1: Menu items through the shell, `HomeHost` and `HomeFlows`, and the contract

**Files:**
- Modify: `apps/clax_app/lib/home/home_host.dart:100-117` (`HomeActions.menu`), `:358-415` (`call`, its `menu` case at `:393-394`), add `_items`
- Modify: `apps/clax_app/lib/ui/launcher.dart:343` (the built-in launcher's long press), `:1184-1300` (`HomeFlows._menu`), `:1304-1309` (`HomeFlows.menu`)
- Modify: `web/shell/src/caps/home.ts:1-60` (doc comment, `argsFor`'s `menu` case)
- Modify: `web/contract/clax-extensions.d.ts:305-370` (`ClaxExtensions.Home`)
- Modify: `docs/contract.md:4786` (the `menu` row) and `:4796-4798` (rejections)
- Regenerate: `apps/clax_app/assets/prompt/clax-extensions.d.ts` (`scripts/sync-app-prompt.sh`)
- Modify (fakes implementing `HomeActions`): `apps/clax_app/test/home/home_host_test.dart:27-43`, `apps/clax_app/test/home/home_route_test.dart:52-61`, `apps/clax_app/test/home/home_snapshot_test.dart:43-52`, `apps/clax_app/test/home/app_screen_home_test.dart:19-28`
- Create: `apps/clax_app/test/home/home_menu_test.dart` (Bazel target `//apps/clax_app:home_home_menu_test`, made by the `tests_home` suite's glob)
- Test: `web/shell/src/caps/home.test.ts`

**Interfaces:**
- Produces (Dart, `lib/home/home_host.dart`):
  ```dart
  typedef HomeMenuItem = ({String id, String label});
  abstract class HomeActions { Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items); /* others unchanged */ }
  class HomeHost { static const maxMenuItems = 4; static const maxMenuLabel = 32; }
  ```
  `HomeHost.call(home, 'menu', {'id', 'rect', 'items'?})` resolves the picked item's ID or `null`; `items` absent means none.
- Produces (page, `ClaxExtensions.Home.Home`): `menu(id: string, rect: Rect, options?: { items?: MenuItem[] }): Promise<string | null>`, `interface MenuItem { id: string; label: string }`. The shell sends `{id, rect}` with no items and `{id, rect, items}` with some.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Read the open threads on the files**

```bash
Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"
for p in apps/clax_app/lib/home/home_host.dart apps/clax_app/lib/ui/launcher.dart web/shell/src/caps/home.ts web/contract/clax-extensions.d.ts docs/contract.md; do "$Q" exec threads "$p"; done
```

Expected: `home_host.dart` a891774f (icon URLs, unrelated), `launcher.dart` 128c1f7f and 7b568e19 (unrelated), `home.ts` 8c0474a5 (events after the first call, unrelated). None blocks this task.

- [ ] **Step 2: Write the failing shell tests** in `web/shell/src/caps/home.test.ts`, beside the existing `menu` cases (`:76-92`), using the file's own `host`/`sent` helpers:

```ts
  it("menu forwards the page's items, and sends {id, rect} alone without them", async () => {
    void host.handle({ type: "clax:call", id: "m1", ns: "home", method: "menu", args: ["a1", { x: 1, y: 2, w: 3, h: 4 }, { items: [{ id: "star", label: " Star " }, { id: "rm", label: "Remove from group" }] }] });
    void host.handle({ type: "clax:call", id: "m2", ns: "home", method: "menu", args: ["a1", { x: 1, y: 2, w: 3, h: 4 }] });
    void host.handle({ type: "clax:call", id: "m3", ns: "home", method: "menu", args: ["a1", { x: 1, y: 2, w: 3, h: 4 }, {}] });
    await vi.waitFor(() => expect(sent.filter(s => s.method === "menu")).toHaveLength(3));
    const menus = sent.filter(s => s.method === "menu");
    expect(menus[0].args).toEqual([{ id: "a1", rect: { x: 1, y: 2, w: 3, h: 4 }, items: [{ id: "star", label: "Star" }, { id: "rm", label: "Remove from group" }] }]);
    expect(menus[1].args).toEqual([{ id: "a1", rect: { x: 1, y: 2, w: 3, h: 4 } }]);
    expect(menus[2].args).toEqual([{ id: "a1", rect: { x: 1, y: 2, w: 3, h: 4 } }]);
  });

  it("menu refuses bad items with bad_request and sends nothing", async () => {
    const rect = { x: 1, y: 2, w: 3, h: 4 };
    const five = Array.from({ length: 5 }, (_, i) => ({ id: `i${i}`, label: "x" }));
    for (const options of [
      { items: five },
      { items: [{ id: "a", label: "x".repeat(33) }] },
      { items: [{ id: "a", label: "   " }] },
      { items: [{ id: "", label: "Star" }] },
      { items: [{ id: "a".repeat(65), label: "Star" }] },
      { items: [{ id: "a", label: "Star" }, { id: "a", label: "Unstar" }] },
      { items: [{ id: "a", label: "Line\nbreak" }] },
      { items: [{ id: 3, label: "Star" }] },
      { items: "star" },
      "star",
    ]) {
      const before = sent.length;
      const r = await host.handle({ type: "clax:call", id: `b${before}`, ns: "home", method: "menu", args: ["a1", rect, options] });
      expect(r, JSON.stringify(options)).toMatchObject({ error: { code: "bad_request" } });
      expect(sent.length).toBe(before);
    }
  });
```

(Match the result shape the file's existing bad-rect test at `:92` asserts; if it reads the rejection from a posted `clax:result`, read it the same way.) A 32-character label passes: add `{ items: [{ id: "a", label: "x".repeat(32) }] }` to the first test's accepted cases if the file's style prefers one assertion per case.

- [ ] **Step 3: Run them to verify they fail**

Run: `pnpm --dir web exec vitest run shell/src/caps/home.test.ts`
Expected: FAIL: the first test's `args` lack `items`; the second resolves instead of rejecting.

- [ ] **Step 4: Implement the shell side** in `web/shell/src/caps/home.ts`. Add to the header comment: "`menu(id, rect, {items})` as `{id, rect, items}` (at most MAX_MENU_ITEMS, labels trimmed, at most MAX_LABEL characters), and as `{id, rect}` without items; it resolves the picked item's ID, or null." Then:

```ts
/** The most items a page adds to a menu, and the longest label. */
export const MAX_MENU_ITEMS = 4;
export const MAX_LABEL = 32;

type MenuItem = { id: string; label: string };

const badItems = () => new CapError("bad_request", `home.menu takes up to ${MAX_MENU_ITEMS} items {id, label}: IDs of 1 to 64 characters, each once, and labels of 1 to ${MAX_LABEL} characters of plain text`);

const plain = (s: string) => [...s].every(ch => { const c = ch.codePointAt(0)!; return c >= 0x20 && c !== 0x7f; });

/** The items `menu`'s options add, or undefined for none. */
function itemsOf(options: unknown): MenuItem[] | undefined {
  if (options === undefined || options === null) return undefined;
  if (typeof options !== "object") throw badItems();
  const list = (options as { items?: unknown }).items;
  if (list === undefined) return undefined;
  if (!Array.isArray(list) || list.length > MAX_MENU_ITEMS) throw badItems();
  const seen = new Set<string>();
  return list.map(i => {
    const e = (i ?? null) as { id?: unknown; label?: unknown } | null;
    const label = e && typeof e.label === "string" ? e.label.trim() : "";
    if (!e || typeof e.id !== "string" || e.id.length === 0 || e.id.length > 64 || seen.has(e.id)) throw badItems();
    if (!label || [...label].length > MAX_LABEL || !plain(label)) throw badItems();
    seen.add(e.id);
    return { id: e.id, label };
  });
}
```

and the `menu` case of `argsFor`:

```ts
    case "menu": {
      const rect = rectOf(args[1]);
      if (!rect) throw new CapError("bad_request", "home.menu takes an app ID and the tile's rect {x, y, w, h}");
      const items = itemsOf(args[2]);
      return [items ? { id: id(method, args[0]), rect, items } : { id: id(method, args[0]), rect }];
    }
```

- [ ] **Step 5: Run the shell tests to verify they pass**

Run: `pnpm --dir web exec vitest run shell/src/caps/home.test.ts`
Expected: PASS, every test in the file.

- [ ] **Step 6: Write the failing Dart tests.** In `apps/clax_app/test/home/home_host_test.dart`, change `_Actions`:

```dart
class _Actions implements HomeActions {
  final calls = <(String, List<Object?>)>[];

  /// What the next `menu` resolves with.
  String? menuAnswer;
  // … open, openMake, newApp, openInbox, openSettings, importApp unchanged …
  @override
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) async {
    calls.add(('menu', [id, anchor, items]));
    return menuAnswer;
  }
}
```

Update the existing expectation in 'open, newApp and menu call the matching action' (`:253-269`) to `['menu', aid, const Rect.fromLTWH(1, 2, 3, 4), const <HomeMenuItem>[]]`, and add:

```dart
  test("menu passes the page's items and resolves the picked item's ID", () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    actions.menuAnswer = 'star';
    final picked = await host.call(home, 'menu', {
      'id': aid,
      'rect': {'x': 1, 'y': 2, 'w': 3, 'h': 4},
      'items': [
        {'id': 'star', 'label': ' Star '},
        {'id': 'rm', 'label': 'Remove from group'},
      ],
    });
    expect(picked, 'star');
    expect(actions.calls.single.$2[2], [
      (id: 'star', label: 'Star'),
      (id: 'rm', label: 'Remove from group'),
    ]);
    actions.menuAnswer = null;
    expect(
      await host.call(home, 'menu', {
        'id': aid,
        'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
      }),
      isNull,
    );
  });

  test('menu refuses bad items as a bad request and shows nothing', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    final rect = {'x': 0, 'y': 0, 'w': 1, 'h': 1};
    for (final items in <Object?>[
      [for (var i = 0; i < 5; i++) {'id': 'i$i', 'label': 'x'}],
      [{'id': 'a', 'label': 'x' * 33}],
      [{'id': 'a', 'label': '  '}],
      [{'id': '', 'label': 'Star'}],
      [{'id': 'a' * 65, 'label': 'Star'}],
      [{'id': 'a', 'label': 'Star'}, {'id': 'a', 'label': 'Unstar'}],
      [{'id': 'a', 'label': 'Line\nbreak'}],
      [{'id': 3, 'label': 'Star'}],
      'star',
    ]) {
      await expectLater(
        host.call(home, 'menu', {'id': aid, 'rect': rect, 'items': items}),
        throwsA(isA<HomeBadRequest>()),
        reason: '$items',
      );
    }
    expect(actions.calls, isEmpty);
  });

  test('while actions are held, menu resolves null without showing', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    host.holdActions = () => true;
    actions.menuAnswer = 'star';
    expect(
      await host.call(home, 'menu', {
        'id': aid,
        'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
        'items': [{'id': 'star', 'label': 'Star'}],
      }),
      isNull,
    );
    expect(actions.calls, isEmpty);
  });
```

Give the other three fakes the new signature: `Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) async { /* their current body */ return null; }` (`home_route_test.dart` keeps `menus.add((id, anchor))`).

Create `apps/clax_app/test/home/home_menu_test.dart`, a widget test of `HomeFlows.menu`, built the way `home_host_test.dart:62-97` builds a host:

```dart
import 'dart:async';
import 'dart:ui' show Rect;

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

void main() {
  late FakeClaxApi api;
  late HomeHost host;
  late HomeFlows flows;
  late String app;
  late List<String> opened;
  final nav = GlobalKey<NavigatorState>();

  Future<void> pump(WidgetTester tester) async {
    api = FakeClaxApi();
    opened = [];
    final home = api.seedArtifact('Home', _page);
    app = api.seedArtifact('Runs', _page);
    flows = HomeFlows(
      contextOf: () => nav.currentContext,
      api: api,
      openApp: opened.add,
      voice: FakeVoice.unsupported('No speech here'),
    );
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: MakeService((r, c, s) => Completer<String>().future),
      builds: null,
      settings: HomeSettings(homeAppId: home),
      actions: flows,
    );
    flows.host = host;
    addTearDown(host.dispose);
    await tester.pumpWidget(MaterialApp(navigatorKey: nav, home: const SizedBox()));
    await tester.runAsync(host.load);
    await tester.pump();
  }

  testWidgets("the page's items come first, above a divider, and a pick resolves its ID", (tester) async {
    await pump(tester);
    final picked = flows.menu(app, Rect.zero, const [(id: 'star', label: 'Star'), (id: 'rm', label: 'Remove from group')]);
    await tester.pumpAndSettle();
    final star = tester.getTopLeft(find.text('Star'));
    final open = tester.getTopLeft(find.text('Open'));
    expect(star.dy, lessThan(open.dy));
    expect(find.byType(Divider), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('home-item-star')));
    await tester.pumpAndSettle();
    expect(await picked, 'star');
    expect(opened, isEmpty);
  });

  testWidgets("Clax's own item acts and resolves null; a dismissal resolves null", (tester) async {
    await pump(tester);
    var picked = flows.menu(app, Rect.zero, const [(id: 'star', label: 'Star')]);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(await picked, isNull);
    expect(opened, [app]);
    picked = flows.menu(app, Rect.zero, const []);
    await tester.pumpAndSettle();
    expect(find.byType(Divider), findsNothing);
    await tester.tapAt(const Offset(10, 10));
    await tester.pumpAndSettle();
    expect(await picked, isNull);
  });
}
```

(If `HomeFlows.mounted` needs the navigator's context to be inside a `Scaffold`, give `home:` a `Scaffold`; if `host.load` is not the loader's name, use the one `home_host_test.dart` calls at `:103`.)

- [ ] **Step 7: Run the Dart tests to verify they fail**

Run: `bazelisk test //apps/clax_app:home_home_host_test //apps/clax_app:home_home_menu_test --test_output=errors`
Expected: FAIL to compile: `HomeMenuItem` is undefined, `menu` overrides a `void` method.

- [ ] **Step 8: Implement the Dart side.** In `home_host.dart`, above `HomeActions`:

```dart
/// An item a home app adds to an app's menu: [id] is what `menu` resolves
/// with when the person picks it, [label] what the menu shows.
typedef HomeMenuItem = ({String id, String label});
```

Replace `HomeActions.menu`:

```dart
  /// Shows app [id]'s long-press menu, anchored at [anchor]: [items] (the
  /// home app's own) first, above a divider, then Clax's. Resolves with
  /// the picked item's ID, or null when the person picked one of Clax's
  /// own items or dismissed the menu.
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items);
```

In `HomeHost`, the `menu` case returns the action's answer (the `holdActions` check above the switch already returns null):

```dart
      case 'menu':
        return actions.menu(
          _appId(args),
          _rect(args['rect']),
          _items(args['items']),
        );
```

and add beside `_tilesOf`:

```dart
  /// The most items a home app may add to a menu, and the longest label.
  static const maxMenuItems = 4;
  static const maxMenuLabel = 32;

  static List<HomeMenuItem> _items(Object? v) {
    if (v == null) return const [];
    const bad = HomeBadRequest(
      'menu takes up to $maxMenuItems items {id, label}: IDs of 1 to 64 '
      'characters, each once, and labels of 1 to $maxMenuLabel characters '
      'of plain text',
    );
    if (v is! List || v.length > maxMenuItems) throw bad;
    final seen = <String>{};
    return [
      for (final i in v)
        switch (i) {
          {'id': final String id, 'label': final String label}
              when id.isNotEmpty &&
                  id.length <= 64 &&
                  _plain(label.trim()) &&
                  seen.add(id) =>
            (id: id, label: label.trim()),
          _ => throw bad,
        },
    ];
  }

  static bool _plain(String s) =>
      s.isNotEmpty &&
      s.runes.length <= maxMenuLabel &&
      !s.runes.any((r) => r < 0x20 || r == 0x7f);
```

Update the class doc of `HomeHost.call` ("`menu` resolves the picked item's ID") and the `_opens` comment if it describes `menu`'s answer.

In `launcher.dart`: the built-in launcher's long press (`:343`) becomes `onLongPress: () => unawaited(_actions.menu(apps[i].id, Rect.zero, const []))`. `HomeFlows.menu`:

```dart
  @override
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) async {
    if (!mounted) return null;
    for (final a in host.summaries ?? const <ArtifactSummary>[]) {
      if (a.id == id) return _menu(a, items);
    }
    return null;
  }
```

`_menu(ArtifactSummary app, List<HomeMenuItem> items)` returns `Future<String?>`; inside the sheet's `Column`, after the title `Padding` and before Open:

```dart
            for (final i in items)
              ListTile(
                key: ValueKey('home-item-${i.id}'),
                title: Text(i.label),
                onTap: () => Navigator.of(context).pop('item:${i.id}'),
              ),
            if (items.isNotEmpty) const Divider(),
```

and right after the sheet closes (before `final c = contextOf();`):

```dart
    if (choice != null && choice.startsWith('item:')) return choice.substring(5);
```

Every other path of `_menu` returns `null` (an early `return;` becomes `return null;`). Update `HomeFlows`'s class doc ("The menu of an app: the home app's items, then Open; Inspect …").

- [ ] **Step 9: Run the Dart tests and analysis**

Run: `bazelisk test //apps/clax_app:home_home_host_test //apps/clax_app:home_home_menu_test //apps/clax_app:home_home_route_test //apps/clax_app:home_home_snapshot_test //apps/clax_app:home_app_screen_home_test //apps/clax_app:dart_analyze --test_output=errors`
Expected: PASS.

- [ ] **Step 10: The contract and the types.** In `web/contract/clax-extensions.d.ts`, inside `namespace Home`, before `interface Home`:

```ts
    /** An item a home app adds to an app's menu. */
    interface MenuItem {
      /** What `menu` resolves with when the person picks it: 1 to 64 characters, each item its own. */
      id: string;
      /** What the menu shows: plain text, 1 to 32 characters. */
      label: string;
    }
```

and replace the `menu` member:

```ts
      /** Shows app `id`'s menu anchored at `rect`: the page's `items` (at
       * most 4) first, above a divider, then Clax's own (Open, Inspect,
       * Share app, Resume build). Resolves with the picked item's `id`, or
       * `null` when the person picked one of Clax's own or dismissed it.
       * Bad items reject `bad_request`. */
      menu(id: string, rect: Rect, options?: { items?: MenuItem[] }): Promise<string | null>;
```

Add "bad menu items" to the namespace comment's `bad_request` list. In `docs/contract.md`, the `menu` row (`:4786`) becomes:

```markdown
| `menu(id, {x, y, w, h}, {items})` | Shows app `id`'s menu anchored at that rectangle, in CSS pixels from the viewport's top left: the page's `items` (at most 4 `{id, label}`, labels plain text of at most 32 characters) first, above a divider, then Clax's own (Open, Inspect, Share app, Resume build). Resolves with the picked item's `id`, or `null` for one of Clax's own or a dismissal. `items` may be left out. |
```

and the rejections sentence (`:4796-4798`) names "malformed rectangle or menu items". Then:

```bash
scripts/sync-app-prompt.sh
scripts/sync-app-prompt.sh --check
bash scripts/test-plugins.sh
```

Expected: the sync writes `apps/clax_app/assets/prompt/clax-extensions.d.ts`; the check exits 0; `test-plugins.sh` passes (this task does not touch the "Runtime capabilities" section the four skill copies share; if a later edit does, copy it word for word into `plugins/claude-code/skills/clax/SKILL.md`, `plugins/clax/skills/clax/SKILL.md`, `plugins/pi/skills/clax/SKILL.md`, `plugins/clax-grok/skills/clax/SKILL.md`).

- [ ] **Step 11: Run the remaining checks**

Run: `bazelisk test //web:unit //web:typecheck //web:lint //web:bundle_size //apps/clax_app:prompt_sync //apps/clax_app:agent_prompt_test --test_output=errors`
Expected: PASS. (The prompt test's target is `agent_prompt_test` if the file is `test/agent/prompt_test.dart`.)

- [ ] **Step 12: Commit**

```bash
git add apps/clax_app/lib/home/home_host.dart apps/clax_app/lib/ui/launcher.dart apps/clax_app/test/home web/shell/src/caps/home.ts web/shell/src/caps/home.test.ts web/contract/clax-extensions.d.ts docs/contract.md apps/clax_app/assets/prompt/clax-extensions.d.ts
git -c commit.gpgsign=false commit -m "Let the home app add its own items to an app's menu, and have menu resolve the picked item's ID"
```

### Task 2: The arrangement document, its store, and the kit's fake `db`

**Files:**
- Create: `apps/clax_app/home/arrange.js` — the `layout/main` rules (pure)
- Create: `apps/clax_app/home/store.js` — reading and writing it through `db`
- Create: `apps/clax_app/home/physics.js` — only `realClock` and `manualClock` in this task (Task 5 adds the motion)
- Modify: `apps/clax_app/assets/kit/fake-home.js` — `fakeDb()`, and `menu`'s items and answer (`home.menuPick`)
- Modify: `apps/clax_app/assets/kit/README.md:18` (the `fake-home.js` row)
- Create: `apps/clax_app/home_unit/arrange.test.js`, `apps/clax_app/home_unit/store.test.js`, `apps/clax_app/home_unit/clock.test.js`
- Modify: `apps/clax_app/BUILD.bazel:25-32` (a `home_unit` `js_library` beside `home`)
- Modify: `web/vitest.config.ts` (include `../apps/clax_app/home_unit/**/*.test.js`; `server.fs.allow` the repository)
- Modify: `web/BUILD.bazel:181-185` (`UNIT_DATA` gains `//apps/clax_app:home`, `//apps/clax_app:home_unit`, `//apps/clax_app:kit`)

**Interfaces:**
- Produces (`arrange.js`):
  ```js
  export const LAYOUT_PATH = 'layout/main';
  export const OPEN_DAYS = 14;
  export const groupKey = (gid) => `g:${gid}`;
  export const isGroupKey = (key) => boolean;
  // Layout = {v, starred: string[], order: string[] /* app IDs and 'g:<gid>' */, groups: {[gid]: {name, look: 'folder'|'pile', apps: string[]}}, seen: string[], opens: {[id]: {n, last, days: {[yyyy-mm-dd]: number}}}, ...unknown}
  export function normalize(body: object|null): Layout;
  export function reconcile(layout: object|null, apps: App[], opts?: {first?: boolean}): Layout;
  export function sections(layout: Layout, apps: App[]): {starred: Entry[], all: Entry[]};   // Entry = {key, app} | {key, group: {id, name, look}, apps: App[]}
  export function moveTo(layout, key: string, section: 'starred'|'all', index: number): Layout;
  export function star(layout, id): Layout;    // last in Starred, out of any group
  export function unstar(layout, id): Layout;  // first in All apps
  export function opened(layout, id, now: Date): Layout;
  export function isNew(layout, id): boolean;
  export function groupOf(layout, id): string|null;
  ```
- Produces (`store.js`): `layoutStore(db|null, {clock, holder?}) → {load(): Promise<object|null>, watch(fn: (body|null) => void): () => void, change(op: (body|null) => object): Promise<object>}`.
- Produces (`physics.js`): `realClock`, `manualClock(startMs = 0, dateMs = Date.UTC(2026, 9, 16, 12))`, each `{now(): ms, date(): Date, after(ms, fn): cancel, frame(fn(now)): cancel}`, and `manualClock`'s `advance(ms)`.
- Produces (kit `fake-home.js`): `fakeDb(docs = {}, {persist?, now?}) → {doc(path) → {id, path, get, set, delete, onSnapshot, acquire}, writes: {path, data}[], put(path, data), hold(path, holder, ms)}`; `fakeHome().menu(id, rect, {items}?)` validates items as the real one does, records the call, and resolves `home.menuPick` (a value, or a function of the call's arguments), `null` by default.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Wire vitest to the home's unit tests.** In `apps/clax_app/BUILD.bazel`, after the `home` `js_library`:

```python
js_library(
    name = "home_unit",
    srcs = glob(
        ["home_unit/**"],
        exclude = ["**/.qual"],
    ),
    visibility = ["//web:__pkg__"],
)
```

In `web/BUILD.bazel`, `UNIT_DATA` becomes:

```python
UNIT_DATA = [
    ":files",
    ":node_modules",
    "//apps/clax_app:home",
    "//apps/clax_app:home_unit",
    "//apps/clax_app:kit",
    "//crates/clax-core:toolpath_vectors_bin",
]
```

with its comment ("What the unit tests read outside web/: … and the home app's modules, its unit tests and the kit, apps/clax_app/home_unit"). In `web/vitest.config.ts`:

```ts
export default defineConfig({
  plugins: [svelte(), svelteTesting()],
  define: { __CLAX_TEST_CLOCK__: "true", __CLAX_EXT_TEST__: "false" },
  resolve: { alias: { "clax-bridge-parts": fileURLToPath(new URL("./bridge/src/parts-static.ts", import.meta.url)) } },
  // The home app's unit tests (apps/clax_app/home_unit) import its modules and the kit from outside web/.
  server: { fs: { allow: [fileURLToPath(new URL("..", import.meta.url))] } },
  test: { environment: "jsdom", setupFiles: ["shell/test-setup.ts"], include: ["bridge/test/**/*.test.ts", "extension/src/**/*.test.ts", "shell/src/**/*.test.ts", "scripts/**/*.test.ts", "../apps/clax_app/home_unit/**/*.test.js"] },
});
```

(keeping the existing comments). The home's unit tests import `test`/`expect` from `'vitest'`; without `server.fs.allow` vite refuses to load files outside `web/` ("Failed to load url … Does the file exist?").

- [ ] **Step 2: Write the failing clock test** `apps/clax_app/home_unit/clock.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { manualClock } from '../home/physics.js';

describe('manualClock', () => {
  test('runs timers in time order and a frame every 16 ms, only when advanced', () => {
    const c = manualClock(0, Date.UTC(2026, 9, 16, 12));
    const seen = [];
    c.after(40, () => seen.push(['t40', c.now()]));
    c.after(10, () => seen.push(['t10', c.now()]));
    const stop = c.after(20, () => seen.push(['never']));
    stop();
    const frame = (t) => { seen.push(['f', t]); if (t < 48) c.frame(frame); };
    c.frame(frame);
    expect(seen).toEqual([]);
    c.advance(50);
    expect(seen).toEqual([['t10', 10], ['f', 16], ['f', 32], ['t40', 40], ['f', 48]]);
    expect(c.now()).toBe(50);
    expect(c.date().toISOString()).toBe('2026-10-16T12:00:00.050Z');
  });
});
```

- [ ] **Step 3: Write the failing arrangement tests** `apps/clax_app/home_unit/arrange.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { normalize, reconcile, sections, moveTo, star, unstar, opened, isNew, groupOf, groupKey } from '../home/arrange.js';

const app = (id, hoursAgo = 0, icon = {}) => ({ id, title: id.toUpperCase(), updatedAt: new Date(Date.UTC(2026, 9, 16, 12) - hoursAgo * 3600e3).toISOString(), build: null, icon });
const A = app('a', 1), B = app('b', 2), C = app('c', 3), D = app('d', 4);

describe('normalize', () => {
  test('fills what is missing and keeps fields it does not know', () => {
    expect(normalize(null)).toEqual({ v: 1, starred: [], order: [], groups: {}, seen: [], opens: {} });
    const later = { v: 1, starred: ['a'], order: ['b', 'g:x', 'zzz'], groups: { x: { name: 'Tools', look: 'pile', apps: ['c', 'd'], colour: 'teal' } }, seen: ['a'], opens: {}, bigTiles: false, wallpaper: { kind: 'dots' } };
    const n = normalize(later);
    expect(n.bigTiles).toBe(false);
    expect(n.wallpaper).toEqual({ kind: 'dots' });
    expect(n.groups.x).toEqual({ name: 'Tools', look: 'pile', apps: ['c', 'd'], colour: 'teal' });
  });

  test('drops malformed entries and duplicates', () => {
    const n = normalize({ starred: ['a', 'a', 3, ''], order: 'b', groups: { x: { apps: ['c'], look: 'cube' }, '': {} }, opens: { a: { n: -1, days: { '2026-10-16': 2, nope: 1 } } } });
    expect(n.starred).toEqual(['a']);
    expect(n.order).toEqual([]);
    expect(n.groups).toEqual({ x: { name: 'Group', look: 'folder', apps: ['c'] } });
    expect(n.opens.a).toEqual({ n: 0, last: null, days: { '2026-10-16': 2 } });
  });
});

describe('reconcile', () => {
  test('a first load lists the apps newest first, every one seen', () => {
    const l = reconcile(null, [B, A, C], { first: true });
    expect(l.order).toEqual(['a', 'b', 'c']);
    expect(l.seen.sort()).toEqual(['a', 'b', 'c']);
    expect(['a', 'b', 'c'].some((id) => isNew(l, id))).toBe(false);
  });

  test('a listed app in no place goes first, unseen; an app no longer listed goes everywhere', () => {
    const stored = { starred: ['a', 'gone'], order: ['b', 'gone', 'c'], seen: ['a', 'b', 'c', 'gone'], opens: { gone: { n: 3, last: null, days: {} } } };
    const l = reconcile(stored, [D, A, B, C]);
    expect(l.starred).toEqual(['a']);
    expect(l.order).toEqual(['d', 'b', 'c']);
    expect(isNew(l, 'd')).toBe(true);
    expect(l.seen).not.toContain('gone');
    expect(l.opens.gone).toBeUndefined();
  });

  test('an app in two places keeps Starred, then its group, then All apps', () => {
    const l = reconcile({ starred: ['a'], order: ['a', 'g:x', 'b', 'c'], groups: { x: { name: 'X', apps: ['a', 'b', 'c', 'd'] } }, seen: ['a', 'b', 'c', 'd'] }, [A, B, C, D]);
    expect(l.starred).toEqual(['a']);
    expect(l.groups.x.apps).toEqual(['b', 'c', 'd']);
    expect(l.order).toEqual(['g:x']);
  });

  test('a group left with one app gives way to it in place; one left with none goes', () => {
    const stored = { order: ['a', 'g:x', 'g:y', 'b'], groups: { x: { name: 'X', apps: ['c', 'gone'] }, y: { name: 'Y', apps: ['gone2'] } }, seen: ['a', 'b', 'c'] };
    const l = reconcile(stored, [A, B, C]);
    expect(l.order).toEqual(['a', 'c', 'b']);
    expect(l.groups).toEqual({});
  });

  test('a group no entry names is put at the end of All apps; a g: entry with no group goes', () => {
    const l = reconcile({ order: ['a', 'g:missing'], groups: { x: { name: 'X', apps: ['b', 'c'] } }, seen: ['a', 'b', 'c'] }, [A, B, C]);
    expect(l.order).toEqual(['a', 'g:x']);
  });

  test('keeps fields and groups an earlier release does not show', () => {
    const later = { v: 1, order: ['g:x', 'a'], groups: { x: { name: 'Tools', look: 'pile', apps: ['b', 'c'] } }, seen: ['a', 'b', 'c'], bigTiles: false, wallpaper: 'dots' };
    const l = reconcile(later, [A, B, C]);
    expect(l.order).toEqual(['g:x', 'a']);
    expect(l.groups.x.look).toBe('pile');
    expect(l.bigTiles).toBe(false);
    expect(l.wallpaper).toBe('dots');
    expect(isNew(l, 'b')).toBe(false);
  });
});

describe('sections', () => {
  test('gives Starred and All apps, a group as one entry with its apps', () => {
    const l = reconcile({ starred: ['d'], order: ['a', 'g:x'], groups: { x: { name: 'Tools', look: 'folder', apps: ['b', 'c'] } }, seen: ['a', 'b', 'c', 'd'] }, [A, B, C, D]);
    const s = sections(l, [A, B, C, D]);
    expect(s.starred.map((e) => e.key)).toEqual(['d']);
    expect(s.all.map((e) => e.key)).toEqual(['a', 'g:x']);
    expect(s.all[1].group).toEqual({ id: 'x', name: 'Tools', look: 'folder' });
    expect(s.all[1].apps.map((a) => a.id)).toEqual(['b', 'c']);
  });
});

describe('moving and starring', () => {
  const base = () => reconcile({ starred: ['a'], order: ['b', 'g:x', 'c'], groups: { x: { name: 'X', apps: ['d', 'e'] } }, seen: ['a', 'b', 'c', 'd', 'e'] }, [A, B, C, D, app('e')]);

  test('moveTo moves within and between sections, clamped', () => {
    expect(moveTo(base(), 'c', 'all', 0).order).toEqual(['c', 'b', 'g:x']);
    expect(moveTo(base(), 'b', 'all', 99).order).toEqual(['g:x', 'c', 'b']);
    const s = moveTo(base(), 'c', 'starred', 0);
    expect(s.starred).toEqual(['c', 'a']);
    expect(s.order).toEqual(['b', 'g:x']);
    expect(moveTo(base(), 'a', 'all', 1).order).toEqual(['b', 'a', 'g:x', 'c']);
  });

  test('a group moves like an app but is never starred', () => {
    expect(moveTo(base(), groupKey('x'), 'all', 0).order).toEqual(['g:x', 'b', 'c']);
    expect(moveTo(base(), groupKey('x'), 'starred', 0)).toEqual(base());
  });

  test('star puts an app last in Starred, out of its group; unstar puts it first in All apps', () => {
    const s = star(base(), 'd');
    expect(s.starred).toEqual(['a', 'd']);
    expect(s.groups.x.apps).toEqual(['e']);
    expect(groupOf(s, 'd')).toBeNull();
    const u = unstar(base(), 'a');
    expect(u.starred).toEqual([]);
    expect(u.order).toEqual(['a', 'b', 'g:x', 'c']);
    expect(groupOf(base(), 'e')).toBe('x');
  });

  test('changes return new layouts and leave the old one as it was', () => {
    const l = base();
    const copy = JSON.parse(JSON.stringify(l));
    star(l, 'b'); unstar(l, 'a'); moveTo(l, 'c', 'all', 0); opened(l, 'b', new Date());
    expect(l).toEqual(copy);
  });
});

describe('opened', () => {
  test('marks the app seen and counts the open per UTC day, keeping the last 14 days', () => {
    let l = reconcile({ order: ['a'], seen: [], opens: { a: { n: 4, last: '2026-09-01T00:00:00.000Z', days: { '2026-09-01': 4, '2026-10-03': 1 } } } }, [A]);
    expect(isNew(l, 'a')).toBe(true);
    l = opened(l, 'a', new Date('2026-10-16T12:00:00Z'));
    l = opened(l, 'a', new Date('2026-10-16T13:00:00Z'));
    expect(isNew(l, 'a')).toBe(false);
    expect(l.opens.a).toEqual({ n: 6, last: '2026-10-16T13:00:00.000Z', days: { '2026-10-03': 1, '2026-10-16': 2 } });
  });
});
```

- [ ] **Step 4: Write the failing store tests** `apps/clax_app/home_unit/store.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { layoutStore } from '../home/store.js';
import { LAYOUT_PATH } from '../home/arrange.js';
import { manualClock } from '../home/physics.js';
import { fakeDb } from '../assets/kit/fake-home.js';

const tick = () => new Promise((r) => setTimeout(r, 0));

describe('layoutStore', () => {
  test('loads null with no document, then each change writes the whole body', async () => {
    const db = fakeDb();
    const s = layoutStore(db, { clock: manualClock() });
    expect(await s.load()).toBeNull();
    await s.change((b) => ({ ...(b ?? {}), starred: ['a'] }));
    expect(await s.load()).toEqual({ starred: ['a'] });
    expect(db.writes).toEqual([{ path: LAYOUT_PATH, data: { starred: ['a'] } }]);
  });

  test('a change that changes nothing writes nothing', async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { starred: ['a'] } });
    const s = layoutStore(db, { clock: manualClock() });
    await s.change((b) => ({ ...b }));
    expect(db.writes).toEqual([]);
  });

  test('changes go one at a time, each over the body the one before wrote', async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { order: [] } });
    const s = layoutStore(db, { clock: manualClock() });
    await Promise.all([
      s.change((b) => ({ ...b, order: [...b.order, 'a'] })),
      s.change((b) => ({ ...b, order: [...b.order, 'b'] })),
    ]);
    expect((await s.load()).order).toEqual(['a', 'b']);
  });

  test("another device's write between two changes is kept", async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { starred: [], order: ['a', 'b'] } });
    const s = layoutStore(db, { clock: manualClock() });
    await s.change((b) => ({ ...b, starred: ['a'] }));
    db.put(LAYOUT_PATH, { ...(await s.load()), order: ['b', 'a'] });
    await s.change((b) => ({ ...b, starred: [...b.starred, 'b'] }));
    expect(await s.load()).toEqual({ starred: ['a', 'b'], order: ['b', 'a'] });
  });

  test('a busy lease is tried again when the document changes or after 1 s', async () => {
    const clock = manualClock();
    const db = fakeDb({ [LAYOUT_PATH]: { n: 0 } }, { now: () => clock.now() });
    db.hold(LAYOUT_PATH, 'the-mac', 3000);
    const s = layoutStore(db, { clock, holder: 'the-phone' });
    const stop = s.watch(() => {});
    let done = false;
    const p = s.change((b) => ({ ...b, n: b.n + 1 })).then(() => { done = true; });
    await tick();
    clock.advance(1000); await tick();
    expect(done).toBe(false);
    db.put(LAYOUT_PATH, { n: 10 });
    await tick();
    expect(done).toBe(false);
    // Each retry registers its next wait only after the one before ran, so
    // the clock moves a second at a time until the lease ends at 3 s.
    for (let i = 0; i < 4 && !done; i++) { clock.advance(1000); await tick(); }
    await p;
    expect(await s.load()).toEqual({ n: 11 });
    stop();
  });

  test('gives up after 10 busy tries and rejects', async () => {
    const clock = manualClock();
    const db = fakeDb({}, { now: () => clock.now() });
    db.hold(LAYOUT_PATH, 'the-mac', 60_000);
    const s = layoutStore(db, { clock, holder: 'the-phone' });
    const p = s.change(() => ({ n: 1 }));
    let failed = null;
    p.catch((e) => { failed = e; });
    for (let i = 0; i < 12; i++) { await tick(); clock.advance(1000); }
    await tick();
    expect(failed?.code).toBe('busy');
    expect(db.writes).toEqual([]);
  });

  test('without db it keeps the layout in memory', async () => {
    const s = layoutStore(null, { clock: manualClock() });
    expect(await s.load()).toBeNull();
    await s.change(() => ({ starred: ['a'] }));
    expect(await s.load()).toEqual({ starred: ['a'] });
    expect(typeof s.watch(() => {})).toBe('function');
  });
});

describe('fakeDb', () => {
  test('keeps documents in sessionStorage under `persist`', async () => {
    sessionStorage.clear();
    await fakeDb({}, { persist: 'k' }).doc('layout/main').set({ a: 1 });
    expect((await fakeDb({}, { persist: 'k' }).doc('layout/main').get()).data()).toEqual({ a: 1 });
  });

  test('a snapshot listener hears the current state, then each write', async () => {
    const db = fakeDb({ 'layout/main': { a: 1 } });
    const heard = [];
    const stop = db.doc('layout/main').onSnapshot((s) => heard.push(s.data()));
    await tick();
    db.put('layout/main', { a: 2 });
    stop();
    db.put('layout/main', { a: 3 });
    expect(heard).toEqual([{ a: 1 }, { a: 2 }]);
  });
});
```

- [ ] **Step 5: Run them to verify they fail**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: FAIL: `../home/arrange.js`, `../home/store.js`, `../home/physics.js` do not exist; `fakeDb` is not exported.

- [ ] **Step 6: Write `apps/clax_app/home/physics.js`** (the clocks only; Task 5 adds the rest):

```js
// The home screen's time: the page's clock, and one for tests that moves
// only when told. Every timing rule of the page (the hold, the hover, the
// springs, the toss, the store's retries) runs on one of these.

/** The page's clock: `now()` in ms (monotonic), `date()` the wall time,
 * `after(ms, fn)` and `frame(fn)` (fn gets `now()`), each returning a
 * function that cancels it. */
export const realClock = Object.freeze({
  now: () => performance.now(),
  date: () => new Date(),
  after(ms, fn) {
    const t = setTimeout(fn, ms);
    return () => clearTimeout(t);
  },
  frame(fn) {
    const r = requestAnimationFrame(() => fn(performance.now()));
    return () => cancelAnimationFrame(r);
  },
});

/** A clock that moves only on `advance(ms)`, which runs the timers that
 * fall due in time order and a frame every 16 ms. `date()` starts at
 * `dateMs` and moves with it. */
export function manualClock(startMs = 0, dateMs = Date.UTC(2026, 9, 16, 12)) {
  let t = startMs;
  let seq = 0;
  const timers = new Map();
  let frames = new Map();
  let nextFrame = startMs + 16;
  return {
    now: () => t,
    date: () => new Date(dateMs + (t - startMs)),
    after(ms, fn) {
      const id = ++seq;
      timers.set(id, { at: t + Math.max(0, ms), fn });
      return () => timers.delete(id);
    },
    frame(fn) {
      const id = ++seq;
      frames.set(id, fn);
      return () => frames.delete(id);
    },
    advance(ms) {
      const end = t + ms;
      for (;;) {
        let due = null;
        for (const [id, x] of timers) if (x.at <= end && (!due || x.at < due[1].at)) due = [id, x];
        const at = Math.min(due ? due[1].at : Infinity, nextFrame);
        if (at > end) break;
        t = at;
        if (due && due[1].at === at) {
          timers.delete(due[0]);
          due[1].fn();
          continue;
        }
        nextFrame += 16;
        const run = frames;
        frames = new Map();
        for (const fn of run.values()) fn(t);
      }
      t = end;
    },
  };
}
```

(Timers due at the same instant as a frame run first; insertion order breaks ties among timers since `Map` iterates in insertion order and `<` keeps the first.)

- [ ] **Step 7: Write `apps/clax_app/home/arrange.js`**:

```js
// The home screen's arrangement: the `layout/main` document in the home
// app's own documents, and the rules over it. Nothing here touches the
// page or the store. Every function returns a new layout and keeps the
// fields it does not know, so a layout a later release wrote survives an
// earlier one's write (add fields, never change the meaning of one).
//
//   {v: 1, starred: [appId], order: [appId | "g:<groupId>"],
//    groups: {groupId: {name, look: "folder" | "pile", apps: [appId]}},
//    seen: [appId], opens: {appId: {n, last, days: {"YYYY-MM-DD": n}}}}

export const LAYOUT_PATH = 'layout/main';
/** The days `opens[id].days` keeps, today included. */
export const OPEN_DAYS = 14;

const GROUP = 'g:';
export const groupKey = (gid) => GROUP + gid;
export const isGroupKey = (key) => typeof key === 'string' && key.startsWith(GROUP);
const gidOf = (key) => key.slice(GROUP.length);
const isId = (v) => typeof v === 'string' && v.length > 0 && v.length <= 64;
const isKey = (v) => isId(v) || (isGroupKey(v) && isId(gidOf(v)));
const ids = (v, ok = isId) => (Array.isArray(v) ? [...new Set(v.filter(ok))] : []);
const obj = (v) => (v && typeof v === 'object' && !Array.isArray(v) ? v : null);
const DAY = /^\d{4}-\d{2}-\d{2}$/;
export const dayOf = (date) => date.toISOString().slice(0, 10);
const daysBefore = (date, n) => dayOf(new Date(date.getTime() - n * 86_400_000));

/** `body` (the stored document, or null) as a layout: every known field
 * there and well formed, every unknown field as it was. */
export function normalize(body) {
  const b = obj(body) ?? {};
  const groups = {};
  for (const [gid, g] of Object.entries(obj(b.groups) ?? {})) {
    if (!isId(gid) || !obj(g)) continue;
    groups[gid] = { ...g, name: typeof g.name === 'string' && g.name.trim() ? g.name : 'Group', look: g.look === 'pile' ? 'pile' : 'folder', apps: ids(g.apps) };
  }
  const opens = {};
  for (const [id, o] of Object.entries(obj(b.opens) ?? {})) {
    if (!isId(id) || !obj(o)) continue;
    const days = {};
    for (const [d, n] of Object.entries(obj(o.days) ?? {})) if (DAY.test(d) && Number.isInteger(n) && n > 0) days[d] = n;
    opens[id] = { ...o, n: Number.isInteger(o.n) && o.n > 0 ? o.n : 0, last: typeof o.last === 'string' ? o.last : null, days };
  }
  return { ...b, v: Number.isInteger(b.v) && b.v >= 1 ? b.v : 1, starred: ids(b.starred), order: ids(b.order, isKey), groups, seen: ids(b.seen), opens };
}

const newestFirst = (apps) => [...apps].sort((a, b) => String(b.updatedAt || '').localeCompare(String(a.updatedAt || '')));

/** `layout` made to match `apps` (the `home` capability's list): an ID no
 * longer listed goes from everywhere; an app in two places keeps the
 * first of Starred, its group, All apps; a group left with one app gives
 * way to that app in its place, one left with none goes, and one no entry
 * names goes last in All apps; a listed app in no place goes first in All
 * apps, newest first. `first` (nothing stored yet) marks every listed app
 * seen, so nothing reads as new on the first load. */
export function reconcile(layout, apps, { first = false } = {}) {
  const l = normalize(layout);
  const listed = new Set(apps.map((a) => a.id));
  const placed = new Set();
  const take = (id) => {
    if (!listed.has(id) || placed.has(id)) return false;
    placed.add(id);
    return true;
  };
  const starred = l.starred.filter(take);
  const members = {};
  for (const [gid, g] of Object.entries(l.groups)) members[gid] = g.apps.filter(take);
  const order = [];
  const named = new Set();
  const placeGroup = (gid) => {
    named.add(gid);
    const m = members[gid];
    if (m.length >= 2) order.push(groupKey(gid));
    else if (m.length === 1) order.push(m[0]);
  };
  for (const key of l.order) {
    if (isGroupKey(key)) {
      const gid = gidOf(key);
      if (members[gid] && !named.has(gid)) placeGroup(gid);
    } else if (take(key)) order.push(key);
  }
  for (const gid of Object.keys(members)) if (!named.has(gid)) placeGroup(gid);
  const groups = {};
  for (const [gid, g] of Object.entries(l.groups)) if (members[gid].length >= 2) groups[gid] = { ...g, apps: members[gid] };
  const fresh = newestFirst(apps.filter((a) => !placed.has(a.id))).map((a) => a.id);
  const seen = first ? [...new Set([...l.seen.filter((id) => listed.has(id)), ...apps.map((a) => a.id)])] : l.seen.filter((id) => listed.has(id));
  const opens = Object.fromEntries(Object.entries(l.opens).filter(([id]) => listed.has(id)));
  return { ...l, starred, order: [...fresh, ...order], groups, seen, opens };
}

/** What the page shows: Starred's entries and All apps', in order, each
 * `{key, app}` or, for a group, `{key, group: {id, name, look}, apps}`. */
export function sections(layout, apps) {
  const byId = new Map(apps.map((a) => [a.id, a]));
  const appEntry = (id) => (byId.has(id) ? { key: id, app: byId.get(id) } : null);
  return {
    starred: layout.starred.map(appEntry).filter(Boolean),
    all: layout.order.map((key) => {
      if (!isGroupKey(key)) return appEntry(key);
      const gid = gidOf(key);
      const g = layout.groups[gid];
      const list = g ? g.apps.map((id) => byId.get(id)).filter(Boolean) : [];
      return list.length ? { key, group: { id: gid, name: g.name, look: g.look }, apps: list } : null;
    }).filter(Boolean),
  };
}

/** The group app `id` is in, or null. */
export function groupOf(layout, id) {
  for (const [gid, g] of Object.entries(normalize(layout).groups)) if (g.apps.includes(id)) return gid;
  return null;
}

/** `key` (an app ID or a group's key) moved to `section` ("starred" or
 * "all") at `index` among its entries, clamped, out of wherever it was;
 * a group it leaves keeps its other apps. A group never goes to Starred:
 * that move changes nothing. */
export function moveTo(layout, key, section, index) {
  const l = normalize(layout);
  if (isGroupKey(key) && section === 'starred') return l;
  const out = (list) => list.filter((k) => k !== key);
  const groups = Object.fromEntries(Object.entries(l.groups).map(([gid, g]) => [gid, { ...g, apps: out(g.apps) }]));
  const into = (list) => {
    const i = Math.max(0, Math.min(Number.isFinite(index) ? Math.trunc(index) : list.length, list.length));
    return [...list.slice(0, i), key, ...list.slice(i)];
  };
  const starred = section === 'starred' ? into(out(l.starred)) : out(l.starred);
  const order = section === 'all' ? into(out(l.order)) : out(l.order);
  return { ...l, starred, order, groups };
}

/** App `id` last in Starred, out of any group. */
export const star = (layout, id) => moveTo(layout, id, 'starred', Infinity);
/** App `id` first in All apps. */
export const unstar = (layout, id) => moveTo(layout, id, 'all', 0);

/** App `id` opened from the home screen at `now`: seen, and counted (`n`
 * all told, `last`, and `days`, the opens per UTC day of the last
 * OPEN_DAYS days). */
export function opened(layout, id, now) {
  const l = normalize(layout);
  const o = l.opens[id] ?? { n: 0, last: null, days: {} };
  const since = daysBefore(now, OPEN_DAYS - 1);
  const today = dayOf(now);
  const days = Object.fromEntries(Object.entries(o.days).filter(([d]) => d >= since));
  days[today] = (days[today] ?? 0) + 1;
  return {
    ...l,
    seen: l.seen.includes(id) ? l.seen : [...l.seen, id],
    opens: { ...l.opens, [id]: { ...o, n: o.n + 1, last: now.toISOString(), days } },
  };
}

/** Whether app `id` has not been opened from the home screen yet. */
export const isNew = (layout, id) => !layout.seen.includes(id);
```

Note: the test "a group left with one app gives way to it" expects `['a', 'c', 'b']`: `c` takes `g:x`'s place and `g:y` (empty) goes.

- [ ] **Step 8: Write `apps/clax_app/home/store.js`**:

```js
// The arrangement in the home app's own documents (`db`), read and written
// whole at layout/main. A change is a function of the stored body; changes
// go one at a time, each under a short lease on the document (`acquire`),
// so the body a change reads is the one it replaces even with the home
// screen open on another device. A busy lease is asked again when the
// document changes or after RETRY_MS, at most TRIES times, then the change
// rejects `{code: "busy"}`. Without `db` (a plain browser) the layout
// lives in memory for the page's life.
import { LAYOUT_PATH } from './arrange.js';

const LEASE_MS = 2000;
const RETRY_MS = 1000;
const TRIES = 10;

const same = (a, b) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

export function layoutStore(db, { clock, holder = `home-${Math.random().toString(36).slice(2, 10)}` } = {}) {
  const ref = db ? db.doc(LAYOUT_PATH) : null;
  let memory = null;
  let queue = Promise.resolve();
  const wakers = new Set();
  const read = async () => {
    const s = await ref.get();
    return s.exists ? s.data() : null;
  };
  const lease = async () => {
    for (let i = 0; i < TRIES; i++) {
      const r = await ref.acquire({ holder, ttlMs: LEASE_MS });
      if (r.acquired) return;
      await new Promise((resolve) => {
        const wake = () => { cancel(); wakers.delete(wake); resolve(); };
        const cancel = clock.after(RETRY_MS, wake);
        wakers.add(wake);
      });
    }
    throw Object.assign(new Error('the arrangement is being changed elsewhere'), { code: 'busy' });
  };
  return {
    load: () => (ref ? read() : Promise.resolve(memory)),
    watch(fn) {
      if (!ref) return () => {};
      return ref.onSnapshot((s) => {
        for (const wake of [...wakers]) wake();
        fn(s.exists ? s.data() : null);
      }, () => {});
    },
    change(op) {
      const run = async () => {
        if (!ref) {
          memory = op(memory);
          return memory;
        }
        await lease();
        const current = await read();
        const next = op(current);
        if (!same(next, current)) await ref.set(next);
        return next;
      };
      const p = queue.then(run, run);
      queue = p.catch(() => {});
      return p;
    },
  };
}
```

- [ ] **Step 9: Extend the kit's fake** `apps/clax_app/assets/kit/fake-home.js`. Extend the header comment: "`menu(id, rect, {items})` checks the items as the app does (at most 4 `{id, label}`, labels of 1 to 32 characters; `bad_request` otherwise), keeps the call, and resolves `home.menuPick`: an item's ID, `null` (the default: Clax's own item or a dismissal), or a function of the call's arguments. `fakeDb(docs, {persist, now})` stands in for `db`'s documents (below)." Replace `menu: act('menu')` with a `menu` built after the object exists:

```js
const MENU_ITEMS = 4;
const MENU_LABEL = 32;

/** Rejects as the app does when `options.items` is not up to MENU_ITEMS
 * `{id, label}` with IDs of 1 to 64 characters, each once, and labels of
 * 1 to MENU_LABEL characters of plain text. */
function checkItems(options) {
  if (options == null) return;
  const bad = () => Promise.reject({ code: 'bad_request', message: `menu takes up to ${MENU_ITEMS} items {id, label}` });
  if (typeof options !== 'object') return bad();
  const list = options.items;
  if (list === undefined) return;
  if (!Array.isArray(list) || list.length > MENU_ITEMS) return bad();
  const seen = new Set();
  for (const i of list) {
    const label = typeof i?.label === 'string' ? i.label.trim() : '';
    const plain = [...label].every((ch) => ch.codePointAt(0) >= 0x20 && ch.codePointAt(0) !== 0x7f);
    if (typeof i?.id !== 'string' || !i.id || i.id.length > 64 || seen.has(i.id) || !label || [...label].length > MENU_LABEL || !plain) return bad();
    seen.add(i.id);
  }
}
```

and in `fakeHome`, build the object as `const home = { … }`, with:

```js
    menuPick: null,
    async menu(...args) {
      const refused = checkItems(args[2]);
      if (refused) return refused;
      calls.push({ method: 'menu', args: copy(args) });
      const pick = home.menuPick;
      return (typeof pick === 'function' ? pick(...args) : pick) ?? null;
    },
```

then `return home;`. Add `fakeDb`, after `fakeHome`:

```js
/** A stand-in for the `db` capability's documents, for a home app's tests
 * and renders: `doc(path)` with `get`, `set`, `delete`, `onSnapshot` and
 * `acquire` (a lease other holders wait out; `now` gives its time). `docs`
 * seeds it as {path: body}; `persist` names a sessionStorage key that
 * keeps the documents across reloads of the page. `writes` lists each
 * `set` as {path, data}; `put(path, data)` writes as another device would
 * (listeners hear it); `hold(path, holder, ms)` gives another holder the
 * lease. */
export function fakeDb(docs = {}, { persist, now = () => Date.now() } = {}) {
  const kept = () => {
    try {
      const s = persist ? sessionStorage.getItem(persist) : null;
      return s ? JSON.parse(s) : null;
    } catch { return null; }
  };
  const store = new Map(Object.entries(kept() ?? copy(docs)));
  const leases = new Map();
  const listeners = new Map();
  const writes = [];
  const save = () => {
    try { if (persist) sessionStorage.setItem(persist, JSON.stringify(Object.fromEntries(store))); } catch { /* storage refused: memory only */ }
  };
  const snap = (path) => {
    const d = store.get(path);
    return { id: path.split('/').pop(), exists: d !== undefined, data: () => (d === undefined ? undefined : copy(d)), metadata: { fromCache: false, hasPendingWrites: false } };
  };
  const tell = (path) => { for (const fn of [...(listeners.get(path) ?? [])]) fn(snap(path)); };
  const write = (path, data) => { store.set(path, copy(data)); save(); tell(path); };
  return {
    doc(path) {
      const parts = String(path).split('/');
      if (parts.length % 2 || parts.some((p) => !p)) throw new TypeError(`not a document path: ${path}`);
      return {
        id: parts.at(-1),
        path,
        get: async () => snap(path),
        set: async (data) => { writes.push({ path, data: copy(data) }); write(path, data); },
        delete: async () => { store.delete(path); save(); tell(path); },
        onSnapshot(next) {
          const set = listeners.get(path) ?? new Set();
          listeners.set(path, set);
          set.add(next);
          queueMicrotask(() => { if (set.has(next)) next(snap(path)); });
          return () => set.delete(next);
        },
        async acquire({ holder, ttlMs = 30000 }) {
          const t = now();
          const held = leases.get(path);
          if (held && held.until > t && held.holder !== holder) return { acquired: false, expiresAt: new Date(held.until).toISOString() };
          const until = t + Math.min(600000, Math.max(1000, ttlMs || 30000));
          leases.set(path, { holder, until });
          return { acquired: true, version: 1, expiresAt: new Date(until).toISOString(), holder };
        },
      };
    },
    writes,
    put: (path, data) => write(path, data),
    hold: (path, holder, ms) => leases.set(path, { holder, until: now() + ms }),
  };
}
```

Update the kit README row: "`fake-home.js` | `fakeHome(state)`, a stand-in for the home screen's `home` capability with sample apps and makes, and `fakeDb(docs)`, one for `db`'s documents, for a home app's tests and the agent's renders (`add_kit` part `home`) | Clax".

- [ ] **Step 10: Run the unit tests to verify they pass**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: PASS, 3 files.

Then under Bazel, which runs the copy of the declared files:

Run: `bazelisk test //web:unit //web:lint //web:typecheck --test_output=errors`
Expected: PASS. If `//web:unit` reports no `home_unit` files, the copy places `apps/clax_app` elsewhere than `../apps` from `web/`: read `bazel/js/npm_script.mjs` and fix the include, never the test.

- [ ] **Step 11: Check the kit drift report and the add_kit part still load**

Run: `bazelisk test //apps/clax_app:home_home_bundle_test //apps/clax_app:agent_tools_kit_tool_test --test_output=errors`
Expected: PASS (the kit drift check reports `fake-home.js` without failing; the kit tool's test target may be named for its file under `test/agent/tools/`: use `bazelisk query 'tests(//apps/clax_app:all)' | grep kit` to find it).

- [ ] **Step 12: Commit**

```bash
git add apps/clax_app/home/arrange.js apps/clax_app/home/store.js apps/clax_app/home/physics.js apps/clax_app/home_unit apps/clax_app/assets/kit/fake-home.js apps/clax_app/assets/kit/README.md apps/clax_app/BUILD.bazel web/BUILD.bazel web/vitest.config.ts
git -c commit.gpgsign=false commit -m "Add the home screen's arrangement document, its store over db, and the kit's fake db"
```

(This commit changes `home/`, so `//apps/clax_app:home_home_bundle_test` fails until Task 3's pack step.)

### Task 3: Starred, All apps, New marks and the menu's Star in the home app; release 1.1.0

**Files:**
- Modify: `apps/clax_app/home/home.js` — `mount(root, home, opts)`, `start()` gets `db`, `useLayout`, sections, New marks, menu items, open counting, `act` resolving the call's value
- Modify: `apps/clax_app/home/home.css` — section headings, the New mark
- Modify: `apps/clax_app/home/tests/home.test.js` — every test mounts with `fakeDb()`; the menu expectations gain items; the new tests below
- Modify: `web/e2e/home-app.spec.ts` — `serve()` gives `db`; the menu expectations; the harness count; star, New, reload; the `starred` render
- Modify: `apps/clax_app/home/manifest.json` — `"version": 8`, `"release": "1.1.0"`
- Regenerate: `apps/clax_app/assets/home/home.clax`, `apps/clax_app/assets/home/release.json`

**Interfaces:**
- Consumes: Task 1's `menu(id, rect, {items})` → `string|null`; Task 2's `arrange.js`, `layoutStore`, `realClock`/`manualClock`, `fakeDb`, `fakeHome().menuPick`.
- Produces (`home.js`): `mount(root, home, {db = null, clock = realClock} = {})`; `start(root)` passes `db: await claude.use('db')`. DOM: `.h-section[data-section="starred"|"all"]`, `.h-heading`, each app tile `.h-tile[data-id]` with `data-new` when new and `aria-label` ending ", new"; group entries (from a later release's document) show their apps inline at the group's place in this release. `act(method, args, what)` resolves the call's value, or `undefined` when it failed.

- [ ] **Step 1: Read the open threads**

```bash
Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"
for p in apps/clax_app/home/home.js apps/clax_app/home/home.css apps/clax_app/home/tests/home.test.js web/e2e/home-app.spec.ts; do "$Q" exec threads "$p"; done
```

Expected: 162d9491 (glyph style) and 4065cba5 (useTopic ordering has no test) on `home.js`; neither blocks this task.

- [ ] **Step 2: Write the failing harness tests.** In `apps/clax_app/home/tests/home.test.js`, import `fakeDb` and `manualClock`, and make `show` mount over a fresh store:

```js
import { mount } from '../home.js';
import { fakeHome, fakeDb, SAMPLE } from '../kit/fake-home.js';
import { manualClock } from '../physics.js';

const LAYOUT = 'layout/main';
let db;
async function show(state, { layout, clock } = {}) {
  const home = fakeHome(state);
  db = fakeDb(layout ? { [LAYOUT]: layout } : {});
  mount(root(), home, { db, clock });
  await ui.waitFor(() => document.querySelector('.h-grid, .h-message'));
  return home;
}
const section = (name) => [...document.querySelectorAll(`.h-section[data-section="${name}"] .h-tile[data-id]`)].map((t) => t.dataset.id);
const stored = async () => (await db.doc(LAYOUT).get()).data();
```

Every existing test that calls `mount(root(), …)` directly passes `{ db: fakeDb() }` too, so no test touches the draft's real documents. The two menu expectations in 'a context menu asks for the menu at the tile…' become `args: ['app-habits', rect, { items: [{ id: 'star', label: 'Star' }] }]` (check `first.args[2]` likewise). Add:

```js
test('the first load writes every listed app as seen, so none reads New; an app made after reads New', async () => {
  const home = await show({ makes: [] });
  await wait(async () => (await stored())?.seen?.length === SAMPLE.apps.length);
  expect(document.querySelectorAll('.h-tile[data-new]').length).toBe(0);
  expect(labels()).toEqual(SAMPLE.apps.map((a) => a.title));
  home.set({ apps: [{ id: 'app-tide', title: 'Tide table', updatedAt: '2026-10-17T00:00:00Z', build: null, icon: { monogram: 'TT', hue: 200 } }, ...SAMPLE.apps] });
  await wait(() => ui.exists('[data-id="app-tide"][data-new]'));
  expect(labels()[0]).toBe('Tide table');
  expect(document.querySelector('[data-id="app-tide"]').getAttribute('aria-label')).toBe('Tide table, new');
  expect(ui.text('[data-id="app-tide"] .h-new')).toBe('New');
});

test('opening a new app from the home screen clears its mark and counts the open', async () => {
  const seen = SAMPLE.apps.map((a) => a.id).filter((id) => id !== 'app-notes');
  const clock = manualClock(0, Date.UTC(2026, 9, 16, 12));
  await show({ makes: [] }, { layout: { order: [], seen }, clock });
  await wait(() => ui.exists('[data-id="app-notes"][data-new]'));
  await ui.click('[data-id="app-notes"]');
  await wait(() => !ui.exists('[data-id="app-notes"][data-new]'));
  const l = await stored();
  expect(l.seen).toContain('app-notes');
  expect(l.opens['app-notes']).toEqual({ n: 1, last: '2026-10-16T12:00:00.000Z', days: { '2026-10-16': 1 } });
});

test('Star from the menu moves the app to Starred; Unstar puts it first under All apps', async () => {
  const home = await show({ makes: [] });
  home.menuPick = 'star';
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => section('starred').length === 1);
  expect(section('starred')).toEqual(['app-plants']);
  expect(section('all')).not.toContain('app-plants');
  expect([...document.querySelectorAll('.h-heading')].map((h) => h.textContent)).toEqual(['Starred', 'All apps']);
  expect((await stored()).starred).toEqual(['app-plants']);
  home.menuPick = (id, rect, opts) => opts.items[0].id;
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => section('starred').length === 0);
  expect(home.calls.at(-1).args[2]).toEqual({ items: [{ id: 'unstar', label: 'Unstar' }] });
  expect(section('all')[0]).toBe('app-plants');
  expect(ui.exists('.h-heading')).toBe(false);
});

test("Clax's own menu item or a dismissal changes nothing", async () => {
  const home = await show({ makes: [] });
  await wait(async () => !!(await stored()));
  const before = db.writes.length;
  home.menuPick = null;
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => home.calls.length === 1);
  await new Promise((r) => setTimeout(r, 0));
  expect(db.writes.length).toBe(before);
  expect(section('starred')).toEqual([]);
});

test('follows the arrangement written from another device', async () => {
  await show({ makes: [] });
  await wait(async () => !!(await stored()));
  db.put(LAYOUT, { ...(await stored()), starred: ['app-mood', 'app-run'] });
  await wait(() => section('starred').join() === 'app-mood,app-run');
});

test('a layout from a later release keeps its groups and unknown fields through a star', async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const later = { v: 1, starred: [], order: ['app-run', 'g:g1', 'app-habits', 'app-recipes', 'app-budget', 'app-mood'], groups: { g1: { name: 'Lists', look: 'pile', apps: ['app-groceries', 'app-notes', 'app-plants'] } }, seen: ids, opens: {}, bigTiles: false, wallpaper: { kind: 'dots' } };
  const home = await show({ makes: [] }, { layout: later });
  await wait(() => section('all').length === SAMPLE.apps.length);
  expect(section('all')).toEqual(['app-run', 'app-groceries', 'app-notes', 'app-plants', 'app-habits', 'app-recipes', 'app-budget', 'app-mood']);
  expect(document.querySelectorAll('.h-tile[data-new]').length).toBe(0);
  home.menuPick = 'star';
  document.querySelector('[data-id="app-mood"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(async () => (await stored()).starred.includes('app-mood'));
  const l = await stored();
  expect(l.groups.g1).toEqual(later.groups.g1);
  expect(l.order).toContain('g:g1');
  expect(l.bigTiles).toBe(false);
  expect(l.wallpaper).toEqual({ kind: 'dots' });
});

test('a layout read that does not answer within 2 s draws the grid newest first and writes nothing', async () => {
  const clock = manualClock();
  const home = fakeHome({ makes: [] });
  let listed = false;
  const apps = home.apps;
  home.apps = async () => { const v = await apps(); listed = true; return v; };
  const stuck = { doc: () => ({ get: () => new Promise(() => {}), onSnapshot: () => () => {}, acquire: () => new Promise(() => {}), set: () => new Promise(() => {}) }) };
  mount(root(), home, { db: stuck, clock });
  await wait(() => listed);
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  expect(home.shownCount).toBe(0);
  // The 2 s wait starts once the apps are in; move the clock until it ends.
  await wait(() => { clock.advance(500); return home.shownCount === 1; });
  expect(clock.now()).toBeGreaterThanOrEqual(2000);
  expect(labels()).toEqual(SAMPLE.apps.map((a) => a.title));
});
```

- [ ] **Step 3: Run the harness to verify the new tests fail**

The harness runs in the browser through the inspect view. Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: FAIL: "tests/home.test.js passes over the fake home capability" lists the new tests as failed (no `.h-section`, no `data-new`, `menu` called without items).

- [ ] **Step 4: Implement in `home.js`.** Import the modules:

```js
import { reconcile, normalize, sections, star, unstar, opened, isNew } from './arrange.js';
import { layoutStore } from './store.js';
import { realClock } from './physics.js';
```

`act` resolves the call's value (the `then` returns `v`; the failure branch returns `undefined`):

```js
    return p.then((v) => { setNotice(null); return v; }, (e) => { /* unchanged */ return undefined; });
```

The layout hook:

```js
/** How long the first read of the arrangement may take before the grid
 * draws without it. */
const LAYOUT_MS = 2000;

/** The arrangement over `apps`: null until the apps and the stored layout
 * (or LAYOUT_MS without an answer) are in. `change(op)` applies `op` to
 * the stored layout made to match the apps and writes the result; with no
 * layout stored yet the first load writes one with every listed app seen.
 * With an empty app list the stored layout is changed as it is, so a list
 * that is briefly empty never drops an ID. */
function useLayout(store, apps, clock) {
  const [body, setBody] = useState(undefined);
  const [late, setLate] = useState(false);
  const wrote = useRef(false);
  useEffect(() => {
    let live = true;
    store.load().then((b) => { if (live) setBody(b ?? null); }, () => { if (live) setBody(null); });
    const stop = store.watch((b) => { if (live) setBody(b ?? null); });
    return () => { live = false; stop(); };
  }, [store]);
  const listed = Array.isArray(apps);
  useEffect(() => {
    if (!listed || body !== undefined) return undefined;
    return clock.after(LAYOUT_MS, () => setLate(true));
  }, [listed, body === undefined]);
  const fit = useCallback((b, first) => (apps.length ? reconcile(b ?? {}, apps, { first }) : normalize(b)), [apps]);
  const change = useCallback((op) => store.change((b) => {
    const next = op(fit(b, b == null));
    return apps.length ? reconcile(next, apps) : next;
  }).then((b) => { setBody(b); return b; }, () => undefined), [store, fit, apps]);
  useEffect(() => {
    if (!listed || body !== null || wrote.current || !apps.length) return;
    wrote.current = true;
    change((l) => l);
  }, [listed, body, change]);
  const layout = useMemo(() => {
    if (!listed) return null;
    if (body === undefined) return late ? reconcile({}, apps, { first: true }) : null;
    return reconcile(body ?? {}, apps, { first: body === null });
  }, [listed, apps, body, late]);
  return [layout, change];
}
```

(`useMemo` comes from `./kit/preact.js`, which exports it.) The menu's items and the open count:

```js
/** What the home app adds to app `id`'s menu. */
const menuItems = (layout, id) => (layout.starred.includes(id) ? [{ id: 'unstar', label: 'Unstar' }] : [{ id: 'star', label: 'Star' }]);
```

In `Home`: `const store = useMemo(() => layoutStore(db, { clock }), [db, clock]);`, `const [layout, change] = useLayout(store, apps, clock);` and the per-app actions handed to tiles:

```js
  const openApp = useCallback((app) => act('open', [app.id], `open ${app.title}`).then((v) => {
    if (v !== undefined) change((l) => opened(l, app.id, clock.date()));
  }), [act, change, clock]);
  const menuOf = useCallback((app, rect) => act('menu', [app.id, rect, { items: menuItems(layout, app.id) }], `show ${app.title}'s menu`).then((picked) => {
    if (picked === 'star') change((l) => star(l, app.id));
    else if (picked === 'unstar') change((l) => unstar(l, app.id));
  }), [act, change, layout]);
```

(`act` resolves `null` from the fake's `open`, so a successful open is `v !== undefined`.) `drawn` becomes `(Array.isArray(apps) && layout !== null) || !!appsError`. `AppTile` takes `{ app, fresh, onOpen, onMenu }`; `usePress(() => onOpen(app), (el) => onMenu(app, faceRect(el)))`; it sets `data-new` when `fresh`, appends ", new" to `aria-label`, and draws `${fresh && html`<span class="h-new">New</span>`}` inside `.h-icon`. The grid becomes sections, each with its own `.h-grid` sharing the measured column count:

```js
function Sections({ layout, apps, makes, cols, onOpen, onMenu, act }) {
  const s = sections(layout, apps);
  const tile = (app) => html`<${AppTile} key=${`a:${app.id}`} app=${app} fresh=${isNew(layout, app.id)} onOpen=${onOpen} onMenu=${onMenu} />`;
  const headed = s.starred.length > 0;
  return html`
    ${headed && html`<section class="h-section" data-section="starred" aria-label="Starred">
      <h2 class="h-heading">Starred</h2>
      <div class="h-grid" style=${`--cols:${cols}`}>${s.starred.map((e) => tile(e.app))}</div>
    </section>`}
    <section class="h-section" data-section="all" aria-label="All apps">
      ${headed && html`<h2 class="h-heading">All apps</h2>`}
      <div class="h-grid" style=${`--cols:${cols}`}>
        ${makes.map((m) => html`<${MakeTile} key=${`m:${m.id}`} make=${m} act=${act} />`)}
        ${s.all.flatMap((e) => (e.app ? [tile(e.app)] : e.apps.map(tile)))}
      </div>
    </section>`;
}
```

`Grid` keeps measuring the wrap's width and renders `<${Sections} …/>` inside `.h-grid-wrap`; the `newestFirst(apps)` call goes (the layout's order replaces it). `mount` and `start`:

```js
/** Shows the home screen in `root` over `home` (the `home` capability, or
 * null when it is unavailable) and `db` (the page's documents, where the
 * arrangement is kept; null keeps it in memory), with `clock` timing the
 * hold, the motion and the reads. */
export function mount(root, home, { db = null, clock = realClock } = {}) {
  render(html`<${Home} key=${++mounts} home=${home ?? null} db=${db} clock=${clock} />`, root);
}

export async function start(root = document.getElementById('app')) {
  let home = null;
  let db = null;
  try { home = (await window.claude?.use?.('home')) ?? null; } catch { home = null; }
  try { db = home ? ((await window.claude?.use?.('db')) ?? null) : null; } catch { db = null; }
  mount(root, home, { db });
}
```

Update `usePress`'s timer to `clock.after` (pass `clock` into `AppTile`) so tests may drive it; HOLD_MS stays 500 until Task 7.

- [ ] **Step 5: Styles** in `home.css`, after "The grid.":

```css
.h-section + .h-section { margin-top: 8px; }
.h-heading {
  margin: 8px 4px 4px; color: var(--h-on-variant);
  font: 500 14px/20px var(--k-font); letter-spacing: 0.1px;
}
.h-new {
  position: absolute; top: -6px; left: -6px; z-index: 1;
  height: 18px; padding: 0 6px; border-radius: 9px;
  background: var(--h-primary); color: var(--h-surface);
  font: 500 11px/18px var(--k-font); letter-spacing: 0.5px;
}
```

- [ ] **Step 6: The browser tests.** In `web/e2e/home-app.spec.ts`: the init script in `serve()` gives `db` beside `home`:

```ts
      w.claude = {
        use: async (name: string) => {
          const { fakeHome, fakeDb } = await import(`${location.origin}/kit/fake-home.js`);
          if (name === "db") return (w.fakeDb ??= fakeDb((state as { docs?: Record<string, unknown> }).docs ?? {}, { persist: "clax-home-db" }));
          if (name !== "home") return null;
          const home = fakeHome(state);
          w.fakeHome = home;
          return home;
        },
      };
```

The menu expectation in "lists, opens, starts a make and opens the menu" gains `{ items: [{ id: "star", label: "Star" }] }` as its third argument; the app-mode test's `expect(menu.args)` becomes `[{ id: "app-groceries", rect: { … }, items: [{ id: "star", label: "Star" }] }]`; `expect(tests.passed).toBe(…)` takes the new count. Add to "over a fake host":

```ts
  test("stars from the menu, marks a new app, and keeps the arrangement across a reload", async ({ page }) => {
    const seen = ["app-run", "app-groceries", "app-habits", "app-recipes", "app-budget", "app-plants", "app-mood"];
    await serve(page, { makes: [], docs: { "layout/main": { v: 1, starred: [], order: [], seen } } });
    await expect(page.locator('[data-id="app-notes"][data-new]')).toHaveCount(1);
    await page.evaluate(() => { (window as unknown as { fakeHome: { menuPick: string } }).fakeHome.menuPick = "star"; });
    await page.locator('[data-id="app-plants"]').click({ button: "right" });
    await expect(page.locator('.h-section[data-section="starred"] [data-id="app-plants"]')).toHaveCount(1);
    await page.reload();
    await expect(page.locator('.h-section[data-section="starred"] [data-id="app-plants"]')).toHaveCount(1);
    await expect(page.locator('[data-id="app-notes"][data-new]')).toHaveCount(1);
    await page.locator('[data-id="app-notes"]').tap();
    await expect(page.locator('[data-id="app-notes"][data-new]')).toHaveCount(0);
    await page.reload();
    await expect(page.locator(".h-tile[data-id]")).toHaveCount(8);
    await expect(page.locator("[data-new]")).toHaveCount(0);
  });
```

In "renders", after the `home-states` screenshot, a starred render:

```ts
          await page.evaluate(async () => {
            const { mount } = await import(`${location.origin}/home.js`);
            const { fakeHome, fakeDb, SAMPLE } = await import(`${location.origin}/kit/fake-home.js`);
            const ids = SAMPLE.apps.map((a: { id: string }) => a.id);
            const db = fakeDb({ "layout/main": { v: 1, starred: ["app-run", "app-notes"], order: ids.filter((id: string) => !["app-run", "app-notes", "app-mood"].includes(id)), seen: ids.filter((id: string) => id !== "app-mood") } });
            mount(document.getElementById("app"), fakeHome({ makes: [] }), { db });
          });
          await expect(page.locator(".h-heading")).toHaveCount(2);
          await page.evaluate(() => Promise.all([...document.images].map(i => i.decode().catch(() => {}))));
          await page.screenshot({ path: join(shots, `home-starred-${size}-${scheme}.png`), animations: "disabled" });
```

- [ ] **Step 7: Run the home's tests and the browser tests**

Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: PASS: the harness reports every test passed and none skipped; the fake-host tests and the app-mode test pass.

- [ ] **Step 8: Look at the renders.** Write them into the worktree (Bazel's copy writes them elsewhere):

```bash
bazelisk build //crates/clax-cli:clax
cd web && CLAX_TEST_BIN="$(../scripts/stable-bin.sh ../bazel-bin/crates/clax-cli/clax ../target/clax-bin/debug)" pnpm exec playwright test --project rest e2e/home-app.spec.ts -g "renders" > ../target/home-renders.log 2>&1; echo $?
```

Expected: `0`. Read each of `target/flutter-app/home/home-starred-{phone,desktop}-{light,dark}.png` and `home-{phone,desktop}-{light,dark}.png` with the Read tool. Check: the headings sit left-aligned with the tiles' icons and read as secondary text in both themes; the New mark sits on the icon's top-left corner, legible in light and dark; the unarranged home (`home-phone-light.png`) has no headings and looks as before. Fix and rerun until they do.

- [ ] **Step 9: Pack release 1.1.0.** In `apps/clax_app/home/manifest.json` set `"version": 8` and `"release": "1.1.0"`. Then:

```bash
./scripts/build-home-bundle.sh
bazelisk run //apps/clax_app:home_bundle -- --i-know-what-i-am-doing
```

Expected: the dry run says it would write `home.clax` (Home v8, release 1.1.0, … files) and `release.json`; the run writes both.

- [ ] **Step 10: Run the app's home tests**

Run: `bazelisk test //apps/clax_app:tests_home //apps/clax_app:dart_analyze --test_output=errors`
Expected: PASS, `home_home_bundle_test` included. (If `tests_home` is not a test suite name Bazel knows, run `bazelisk test //apps/clax_app:all --test_tag_filters=-manual --test_output=errors` filtered with `bazelisk query 'tests(//apps/clax_app:all)' | grep '^//apps/clax_app:home_'`.)

- [ ] **Step 11: Commit**

```bash
git add apps/clax_app/home apps/clax_app/assets/home web/e2e/home-app.spec.ts
git -c commit.gpgsign=false commit -m "Show Starred above All apps, mark new apps until opened, and star from the menu; home release 1.1.0"
```

### Task 4: The agent prompt's home section knows the arrangement document

**Files:**
- Modify: `apps/clax_app/assets/prompt/system.md:282-313` ("## The home screen")
- Modify: `apps/clax_app/assets/prompt/single-call.md:78-91` ("## The home screen")
- Test: `apps/clax_app/test/agent/prompt_test.dart`

**Interfaces:**
- Consumes: the document shape of Task 2 (`arrange.js` header comment).
- Produces: nothing code uses. Tasks 9, 10 and 14 add the fields their phases ship.

- [ ] **Step 1: Write the failing test** in `apps/clax_app/test/agent/prompt_test.dart`, in the group that reads `system.md` (`:292-320`):

```dart
    test("the home screen section gives the arrangement's document and its rule", () async {
      for (final f in ['assets/prompt/system.md', 'assets/prompt/single-call.md']) {
        final md = await rootBundle.loadString(f);
        final home = md.substring(md.indexOf('## The home screen'));
        expect(home, contains('`layout/main`'), reason: f);
        expect(home, contains('add fields, never change the meaning of one'), reason: f);
        expect(home, contains('menu(id, {x, y, w, h}, {items})'), reason: f);
      }
    });
```

- [ ] **Step 2: Run it to verify it fails**

Run: `bazelisk test //apps/clax_app:agent_prompt_test --test_output=errors`
Expected: FAIL: `layout/main` not found.

- [ ] **Step 3: Write the section.** In `system.md`, the actions bullet names `menu(id, {x, y, w, h}, {items})` ("up to 4 `{id, label}` items of the page's own, shown first; it resolves the picked item's `id`, or `null`"), and the bullet "Never hide, drop or reorder away an app…" becomes:

```markdown
- Never hide, drop or reorder away an app the person did not ask to hide;
  a new app they make must show.
- The shipped home app keeps the person's arrangement in one document of
  its own, `layout/main`:
  `{v: 1, starred: [appId], order: [appId | "g:<groupId>"], groups: {groupId: {name, look: "folder" | "pile", apps: [appId]}}, seen: [appId], opens: {appId: {n, last, days: {"YYYY-MM-DD": n}}}}`.
  `starred` is the Starred section in order; `order` is All apps; `seen`
  lists the apps opened from the home screen (the rest show "New");
  `opens` counts those opens. Keep it readable by the shipped home app:
  add fields, never change the meaning of one, and keep every field you do
  not use when you write it, so stepping back a version never loses the
  arrangement. Show any app the document does not place.
```

In `single-call.md`, the last bullet's "keep the order or groups they asked for in `db`" becomes "keep the arrangement in `layout/main` (`{v: 1, starred, order, groups, seen, opens}`) readable by the shipped home app: add fields, never change the meaning of one", and its actions list names `menu(id, {x, y, w, h}, {items})`.

- [ ] **Step 4: Run the prompt tests**

Run: `bazelisk test //apps/clax_app:agent_prompt_test //apps/clax_app:prompt_sync --test_output=errors`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/clax_app/assets/prompt/system.md apps/clax_app/assets/prompt/single-call.md apps/clax_app/test/agent/prompt_test.dart
git -c commit.gpgsign=false commit -m "Tell the agent the home app's arrangement document and how to keep it readable"
```

---

## Phase 2 — Hold, drag, snap, physics and toss (release 1.2.0)

Task 5 and Task 6 touch disjoint files; Task 6 uses Task 5's exports, so run Task 6 after Task 5 (or in parallel against the Interfaces blocks, with its tests run once Task 5 lands). Task 7 needs both.

### Task 5: Springs, toss and the grid's cells

**Files:**
- Modify: `apps/clax_app/home/physics.js` — `SPRING`, `springStep`, `atRest`, `TOSS_TAU`, `TOSS_MIN`, `tossStep`, `tossRest`, `tossDone`, `SPEED_MS`, `velocityOf`
- Create: `apps/clax_app/home/grid.js` — `pack`, `geometry`, `cellBox`, `slotAt`
- Create: `apps/clax_app/home_unit/physics.test.js`, `apps/clax_app/home_unit/grid.test.js`

**Interfaces:**
- Produces (`physics.js`):
  ```js
  export const SPRING = { k: 380, c: 26 };        // unit mass; one overshoot of about 3%
  export function springStep(s: {x, v}, target: number, dt: seconds): {x, v};
  export function atRest(s, target): boolean;     // |x - target| < 0.5 and |v| < 5
  export const TOSS_TAU = 0.325;                  // seconds; rest = x + v·τ
  export const TOSS_MIN = 600;                    // px/s: a release faster than this is a toss
  export function tossStep(p: {x, y, vx, vy}, dt): {x, y, vx, vy};
  export function tossRest(p): {x, y};
  export function tossDone(p): boolean;           // speed under 20 px/s
  export const SPEED_MS = 80;
  export function velocityOf(samples: {t, x, y}[]): {vx, vy};
  ```
- Produces (`grid.js`):
  ```js
  export function pack(items: {key, span?: 1|2}[], cols: number): {cells: {key, col, row, span}[], rows: number};
  export function geometry(width, cols, opts?: {gapX = 4, gapY = 8, cellH = 116}): {cols, cellW, cellH, gapX, gapY};
  export function cellBox(cell, g): {x, y, w, h};   // grid-relative CSS px
  export function slotAt(cells, g, x, y): number;   // index of the cell whose centre is nearest (x, y); -1 for no cells
  ```

- [ ] **Step 1: Write the failing tests** `apps/clax_app/home_unit/physics.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { springStep, atRest, tossStep, tossRest, tossDone, velocityOf, TOSS_TAU } from '../home/physics.js';

describe('springStep', () => {
  test('carries a tile to its cell with one small overshoot, at rest within 500 ms', () => {
    let s = { x: 100, v: 0 };
    let low = 0; let crossings = 0; let side = 1; let restAt = null;
    for (let f = 1; f <= 60; f++) {
      s = springStep(s, 0, 1 / 60);
      low = Math.min(low, s.x);
      if (Math.abs(s.x) > 0.5 && Math.sign(s.x) !== side) { crossings++; side = Math.sign(s.x); }
      if (restAt === null && atRest(s, 0)) restAt = f;
    }
    expect(crossings).toBe(1);
    expect(-low).toBeGreaterThan(1);
    expect(-low).toBeLessThan(10);
    expect(restAt).toBeLessThanOrEqual(30);
  });

  test('one long step equals the same time in short steps, and never blows up', () => {
    const a = springStep({ x: 100, v: 0 }, 0, 0.5);
    let b = { x: 100, v: 0 };
    for (let i = 0; i < 15; i++) b = springStep(b, 0, 1 / 30);
    expect(a.x).toBeCloseTo(b.x, 6);
    expect(Math.abs(springStep({ x: 100, v: 0 }, 0, 5).x)).toBeLessThan(1);
  });
});

describe('toss', () => {
  test('decays with the release speed and comes to rest v·τ on', () => {
    let p = { x: 0, y: 0, vx: 2000, vy: -1000 };
    expect(tossRest(p)).toEqual({ x: 2000 * TOSS_TAU, y: -1000 * TOSS_TAU });
    for (let f = 0; f < 180 && !tossDone(p); f++) p = tossStep(p, 1 / 60);
    expect(tossDone(p)).toBe(true);
    expect(Math.abs(p.x - 2000 * TOSS_TAU)).toBeLessThan(10);
    expect(Math.abs(p.y + 1000 * TOSS_TAU)).toBeLessThan(10);
  });
});

describe('velocityOf', () => {
  test('takes the speed over the last 80 ms only', () => {
    expect(velocityOf([])).toEqual({ vx: 0, vy: 0 });
    expect(velocityOf([{ t: 0, x: 0, y: 0 }])).toEqual({ vx: 0, vy: 0 });
    const samples = [{ t: 0, x: 0, y: 0 }, { t: 100, x: 0, y: 0 }, { t: 140, x: 40, y: 0 }, { t: 180, x: 80, y: -20 }];
    expect(velocityOf(samples)).toEqual({ vx: 1000, vy: -250 });
  });
});
```

`apps/clax_app/home_unit/grid.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { pack, geometry, cellBox, slotAt } from '../home/grid.js';

const at = (r) => r.cells.map((c) => `${c.key}@${c.col},${c.row}${c.span === 2 ? '×2' : ''}`);

describe('pack', () => {
  test('1×1 items fill rows in order', () => {
    expect(at(pack(['a', 'b', 'c', 'd', 'e'].map((key) => ({ key })), 4))).toEqual(['a@0,0', 'b@1,0', 'c@2,0', 'd@3,0', 'e@0,1']);
  });

  test('a 2×2 takes the first block at or after its place; the next 1×1 items fill the gaps it leaves', () => {
    const r = pack([{ key: 'a' }, { key: 'B', span: 2 }, { key: 'c' }, { key: 'd' }, { key: 'e' }, { key: 'f' }], 4);
    expect(at(r)).toEqual(['a@0,0', 'B@1,0×2', 'c@3,0', 'd@0,1', 'e@3,1', 'f@0,2']);
    expect(r.rows).toBe(3);
  });

  test('a 2×2 that does not fit at the row end starts the next row; earlier items keep their places', () => {
    expect(at(pack([{ key: 'a' }, { key: 'b' }, { key: 'c' }, { key: 'D', span: 2 }, { key: 'e' }], 4))).toEqual(['a@0,0', 'b@1,0', 'c@2,0', 'D@0,1×2', 'e@3,0']);
  });

  test('one column holds a 2×2 as a 1×1', () => {
    expect(at(pack([{ key: 'A', span: 2 }, { key: 'b' }], 1))).toEqual(['A@0,0', 'b@0,1']);
  });
});

describe('geometry and slotAt', () => {
  const g = geometry(4 * 100 + 3 * 4, 4);
  test('cells are 100 wide, 116 high, 4 and 8 apart; a 2×2 spans the gap', () => {
    expect(g.cellW).toBe(100);
    expect(cellBox({ col: 1, row: 1, span: 1 }, g)).toEqual({ x: 104, y: 124, w: 100, h: 116 });
    expect(cellBox({ col: 1, row: 0, span: 2 }, g)).toEqual({ x: 104, y: 0, w: 204, h: 240 });
  });

  test('a point lands on the nearest cell', () => {
    const { cells } = pack(['a', 'b', 'c', 'd', 'e'].map((key) => ({ key })), 4);
    expect(slotAt(cells, g, 10, 10)).toBe(0);
    expect(slotAt(cells, g, 260, 60)).toBe(2);
    expect(slotAt(cells, g, 50, 900)).toBe(4);
    expect(slotAt([], g, 0, 0)).toBe(-1);
  });
});
```

Note the third pack test: `D` cannot fit at `(3,0)` (one column left), so it takes `(0,1)`; `e`, a 1×1, fills the first free cell, `(3,0)`.

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: FAIL: `springStep`, `grid.js` missing.

- [ ] **Step 3: Implement.** Append to `physics.js`:

```js
/** A tile's spring, per axis, unit mass: it arrives in about 300 ms and
 * overshoots once, by about 3%. */
export const SPRING = Object.freeze({ k: 380, c: 26 });
const MAX_DT = 1 / 30;

/** `s` ({x, v}) after `dt` seconds of the spring toward `target`. A step
 * longer than 1/30 s is taken in parts, so a slow frame never throws a
 * tile off. */
export function springStep(s, target, dt, { k, c } = SPRING) {
  let { x, v } = s;
  for (let left = dt; left > 1e-9; left -= MAX_DT) {
    const h = Math.min(left, MAX_DT);
    v += (-k * (x - target) - c * v) * h;
    x += v * h;
  }
  return { x, v };
}

/** Whether a spring is close enough to `target`, slowly enough, to stop. */
export const atRest = (s, target) => Math.abs(s.x - target) < 0.5 && Math.abs(s.v) < 5;

/** A tossed tile's decay time: released at v px/s it comes to rest
 * v × TOSS_TAU px on. */
export const TOSS_TAU = 0.325;
/** The release speed, in px/s, above which a release is a toss. */
export const TOSS_MIN = 600;

/** `p` ({x, y, vx, vy}) after `dt` seconds of a toss. */
export function tossStep(p, dt) {
  const f = Math.exp(-dt / TOSS_TAU);
  const g = TOSS_TAU * (1 - f);
  return { x: p.x + p.vx * g, y: p.y + p.vy * g, vx: p.vx * f, vy: p.vy * f };
}
/** Where a toss from `p` comes to rest. */
export const tossRest = (p) => ({ x: p.x + p.vx * TOSS_TAU, y: p.y + p.vy * TOSS_TAU });
/** Whether a toss is slow enough to end. */
export const tossDone = (p) => Math.hypot(p.vx, p.vy) < 20;

/** The pointer samples a release's speed is taken over. */
export const SPEED_MS = 80;

/** The pointer's velocity in px/s over the samples ({t ms, x, y}) of the
 * last SPEED_MS; zero for fewer than two. */
export function velocityOf(samples) {
  if (samples.length < 2) return { vx: 0, vy: 0 };
  const last = samples[samples.length - 1];
  const first = samples.find((s) => last.t - s.t <= SPEED_MS) ?? last;
  const dt = (last.t - first.t) / 1000;
  return dt > 0 ? { vx: (last.x - first.x) / dt, vy: (last.y - first.y) / dt } : { vx: 0, vy: 0 };
}
```

Create `grid.js`:

```js
// The home screen's cells: where each entry goes in a grid of a given
// width, and which cell a point is over.

/** Where `items` ({key, span}: 1, or 2 for a 2×2) go in a grid `cols`
 * wide, keeping their order as nearly as the cells allow: a 1×1 takes the
 * first free cell, so it fills a gap a 2×2 left before it; a 2×2 takes
 * the first free block at or after the place the item before it took. */
export function pack(items, cols) {
  const used = new Set();
  const free = (c, r, n) => {
    if (c + n > cols) return false;
    for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) if (used.has(`${c + i},${r + j}`)) return false;
    return true;
  };
  const cells = [];
  let cursor = 0;
  let rows = 0;
  for (const it of items) {
    const n = Math.min(it.span === 2 ? 2 : 1, cols);
    let i = n === 1 ? 0 : cursor;
    while (!free(i % cols, Math.floor(i / cols), n)) i++;
    const col = i % cols;
    const row = Math.floor(i / cols);
    for (let a = 0; a < n; a++) for (let b = 0; b < n; b++) used.add(`${col + a},${row + b}`);
    cells.push({ key: it.key, col, row, span: n });
    cursor = Math.max(cursor, i);
    rows = Math.max(rows, row + n);
  }
  return { cells, rows };
}

/** The measures of a grid `width` CSS px wide in `cols` columns. */
export function geometry(width, cols, { gapX = 4, gapY = 8, cellH = 116 } = {}) {
  return { cols, cellW: (width - gapX * (cols - 1)) / cols, cellH, gapX, gapY };
}

/** `cell`'s box, from the grid's top left. */
export function cellBox(cell, g) {
  return {
    x: cell.col * (g.cellW + g.gapX),
    y: cell.row * (g.cellH + g.gapY),
    w: cell.span * g.cellW + (cell.span - 1) * g.gapX,
    h: cell.span * g.cellH + (cell.span - 1) * g.gapY,
  };
}

/** The index of the cell whose centre is nearest (x, y), from the grid's
 * top left; -1 with no cells. */
export function slotAt(cells, g, x, y) {
  let best = -1;
  let dist = Infinity;
  cells.forEach((cell, i) => {
    const b = cellBox(cell, g);
    const d = Math.hypot(b.x + b.w / 2 - x, b.y + b.h / 2 - y);
    if (d < dist) { dist = d; best = i; }
  });
  return best;
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/clax_app/home/physics.js apps/clax_app/home/grid.js apps/clax_app/home_unit/physics.test.js apps/clax_app/home_unit/grid.test.js
git -c commit.gpgsign=false commit -m "Add the home screen's springs, toss and grid packing"
```

### Task 6: The press controller and the tile animator

**Files:**
- Create: `apps/clax_app/home/drag.js` — `HOLD_MS`, `SLOP_PX`, `createPress`
- Create: `apps/clax_app/home/motion.js` — `createMotion`
- Create: `apps/clax_app/home_unit/drag.test.js`, `apps/clax_app/home_unit/motion.test.js`

**Interfaces:**
- Consumes: Task 5's `springStep`, `atRest`, `tossStep`, `tossDone`, `SPEED_MS`; Task 2's `manualClock`.
- Produces (`drag.js`):
  ```js
  export const HOLD_MS = 400, SLOP_PX = 8;
  // handlers: {lift(), unlift(), menu(), dragStart(p), dragMove(p), dragEnd(p, samples), dragCancel()}; p = {x, y, t}
  export function createPress(handlers, clock): {
    readonly phase: 'idle'|'down'|'lifted'|'drag',
    down(e), move(e), up(e), cancel(e),   // e: {pointerId, pointerType, button, clientX, clientY}
    contextMenu(): boolean,               // true: a press is under way (ignore it); false: open the menu at once
    takeClick(): boolean,                 // false once after a menu or a drag (swallow that click), else true
  };
  ```
- Produces (`motion.js`):
  ```js
  export function createMotion(clock, {reduced = false, onSettle = () => {}} = {}): {
    from(key, el, dx, dy),       // FLIP: el is drawn at its new place; start it dx, dy away, springing to 0
    hold(key, el, dx, dy),       // a dragged tile: put it at dx, dy from its place now, no spring
    let(key),                    // release a held tile: it springs from where it is to 0
    bump(key, el, vx, vy),       // give a tile a push (px/s) it springs back from
    readonly moving: boolean,
    stop(),
  };
  ```
  `el.style.transform` is `translate(dxpx, dypx)` while moving and `''` at rest; `onSettle()` runs once each time every tile comes to rest. With `reduced`, `from` and `let` put tiles at rest at once and `bump` does nothing.

- [ ] **Step 1: Write the failing tests** `apps/clax_app/home_unit/drag.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { createPress, HOLD_MS } from '../home/drag.js';
import { manualClock } from '../home/physics.js';

function rig() {
  const clock = manualClock();
  const log = [];
  const h = (name) => (...a) => log.push([name, ...a.map((x) => (x && typeof x === 'object' && 'x' in x ? `${x.x},${x.y}` : x))]);
  const p = createPress({ lift: h('lift'), unlift: h('unlift'), menu: h('menu'), dragStart: h('dragStart'), dragMove: h('dragMove'), dragEnd: (q, s) => log.push(['dragEnd', `${q.x},${q.y}`, s.length]), dragCancel: h('dragCancel') }, clock);
  const ev = (x, y, type = 'touch', button = 0) => ({ pointerId: 1, pointerType: type, button, clientX: x, clientY: y });
  return { clock, log, p, ev };
}

describe('createPress', () => {
  test('a quick touch is a tap: nothing but the click', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(100); p.up(ev(10, 10));
    expect(log).toEqual([]);
    expect(p.takeClick()).toBe(true);
  });

  test('held 400 ms still the tile lifts; released without moving it asks for the menu and swallows the click', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(HOLD_MS - 1);
    expect(log).toEqual([]);
    clock.advance(1);
    expect(log).toEqual([['lift']]);
    p.move(ev(14, 14)); p.up(ev(14, 14));
    expect(log).toEqual([['lift'], ['menu']]);
    expect(p.takeClick()).toBe(false);
    expect(p.takeClick()).toBe(true);
  });

  test('lifted, a move past 8 px drags, and the release ends the drag with the recent samples', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(HOLD_MS);
    clock.advance(16); p.move(ev(30, 10));
    clock.advance(16); p.move(ev(50, 10));
    p.up(ev(50, 10));
    expect(log).toEqual([['lift'], ['dragStart', '30,10'], ['dragMove', '30,10'], ['dragMove', '50,10'], ['dragEnd', '50,10', 2]]);
    expect(p.takeClick()).toBe(false);
  });

  test('a touch that moves 8 px before the hold lets go: the page scrolls, nothing lifts', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(100); p.move(ev(10, 30));
    clock.advance(1000); p.up(ev(10, 30));
    expect(log).toEqual([]);
    expect(p.phase).toBe('idle');
  });

  test('a mouse drags at once on moving 8 px, with no hold', () => {
    const { p, ev, log } = rig();
    p.down(ev(10, 10, 'mouse')); p.move(ev(15, 10, 'mouse'));
    expect(log).toEqual([]);
    p.move(ev(30, 10, 'mouse'));
    expect(log[0]).toEqual(['dragStart', '30,10']);
  });

  test("a mouse's other buttons press nothing; a context menu outside a press opens at once, during one is ignored", () => {
    const { p, ev, log } = rig();
    p.down(ev(10, 10, 'mouse', 2));
    expect(p.phase).toBe('idle');
    expect(p.contextMenu()).toBe(false);
    p.down(ev(10, 10));
    expect(p.contextMenu()).toBe(true);
    expect(log).toEqual([]);
  });

  test('a cancel ends a lift or a drag', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(HOLD_MS); p.cancel(ev(10, 10));
    p.down(ev(10, 10)); clock.advance(HOLD_MS); p.move(ev(40, 10)); p.cancel(ev(40, 10));
    expect(log.map((l) => l[0])).toEqual(['lift', 'unlift', 'lift', 'dragStart', 'dragMove', 'dragCancel']);
  });
});
```

`apps/clax_app/home_unit/motion.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { createMotion } from '../home/motion.js';
import { manualClock } from '../home/physics.js';

const el = () => ({ style: { transform: '' } });

describe('createMotion', () => {
  test('a moved tile springs from where it was to its new place, then settles once', () => {
    const clock = manualClock();
    let settled = 0;
    const m = createMotion(clock, { onSettle: () => settled++ });
    const a = el();
    m.from('a', a, 100, 0);
    expect(a.style.transform).toBe('translate(100px, 0px)');
    expect(m.moving).toBe(true);
    clock.advance(1000);
    expect(a.style.transform).toBe('');
    expect(m.moving).toBe(false);
    expect(settled).toBe(1);
  });

  test('a held tile stays where it is put, and springs home once let go', () => {
    const clock = manualClock();
    const m = createMotion(clock);
    const a = el();
    m.hold('a', a, 40, 30);
    clock.advance(500);
    expect(a.style.transform).toBe('translate(40px, 30px)');
    m.let('a');
    clock.advance(1000);
    expect(a.style.transform).toBe('');
  });

  test('a bump pushes a tile aside and it settles back', () => {
    const clock = manualClock();
    const m = createMotion(clock);
    const a = el();
    m.bump('a', a, 800, 0);
    clock.advance(48);
    expect(a.style.transform).toMatch(/^translate\([1-9]/);
    clock.advance(1000);
    expect(a.style.transform).toBe('');
  });

  test('with reduced motion nothing springs', () => {
    const clock = manualClock();
    let settled = 0;
    const m = createMotion(clock, { reduced: true, onSettle: () => settled++ });
    const a = el();
    m.from('a', a, 100, 0);
    m.bump('a', a, 800, 0);
    expect(a.style.transform).toBe('');
    expect(m.moving).toBe(false);
    expect(settled).toBe(1);
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm --dir web exec vitest run home_unit/drag home_unit/motion`
Expected: FAIL: modules missing.

- [ ] **Step 3: Implement `drag.js`**:

```js
// A tile's press, as one state machine over pointer events and a clock.
// A tap is the click that follows. Held HOLD_MS still, a touch lifts the
// tile; a lifted press that moves more than SLOP_PX drags it, one released
// without moving asks for the menu. A mouse drags once it moves SLOP_PX
// with its main button down, with no hold. A touch that moves SLOP_PX
// before the hold is the page scrolling: the press lets go. The click
// after a menu or a drag is swallowed.
import { SPEED_MS } from './physics.js';

export const HOLD_MS = 400;
export const SLOP_PX = 8;

export function createPress(h, clock) {
  let s = null;
  let swallow = false;
  const end = () => { s?.cancel?.(); s = null; };
  const at = (e) => ({ x: e.clientX, y: e.clientY, t: clock.now() });
  return {
    get phase() { return s?.phase ?? 'idle'; },
    down(e) {
      if (e.pointerType === 'mouse' && e.button !== 0) return;
      end();
      swallow = false;
      s = { id: e.pointerId, mouse: e.pointerType === 'mouse', x0: e.clientX, y0: e.clientY, phase: 'down', samples: [] };
      if (!s.mouse) {
        const mine = s;
        s.cancel = clock.after(HOLD_MS, () => {
          if (s !== mine || s.phase !== 'down') return;
          s.phase = 'lifted';
          h.lift();
        });
      }
    },
    move(e) {
      if (!s || e.pointerId !== s.id) return;
      const p = at(e);
      const far = Math.hypot(p.x - s.x0, p.y - s.y0) > SLOP_PX;
      if (s.phase === 'down' && far && !s.mouse) { end(); return; }
      if ((s.phase === 'down' || s.phase === 'lifted') && far) {
        s.cancel?.();
        s.phase = 'drag';
        h.dragStart(p);
      }
      if (s.phase !== 'drag') return;
      s.samples.push(p);
      while (s.samples.length > 1 && p.t - s.samples[0].t > SPEED_MS) s.samples.shift();
      h.dragMove(p);
    },
    up(e) {
      if (!s || e.pointerId !== s.id) return;
      const was = s;
      end();
      if (was.phase === 'drag') { swallow = true; h.dragEnd(at(e), was.samples); }
      else if (was.phase === 'lifted') { swallow = true; h.menu(); }
    },
    cancel(e) {
      if (!s || (e && e.pointerId !== s.id)) return;
      const was = s;
      end();
      if (was.phase === 'drag') h.dragCancel();
      else if (was.phase === 'lifted') h.unlift();
    },
    contextMenu() { return s !== null; },
    takeClick() {
      if (!swallow) return true;
      swallow = false;
      return false;
    },
  };
}
```

`motion.js`:

```js
// The tiles' motion: each moving tile has a spring per axis that carries
// its offset (a transform from where the page lays it out) to zero, on one
// frame loop over the page's clock. A dragged tile is held where the
// pointer puts it. `onSettle` runs each time every tile has come to rest.
// With reduced motion, tiles go straight to their places.
import { springStep, atRest } from './physics.js';

export function createMotion(clock, { reduced = false, onSettle = () => {} } = {}) {
  const tiles = new Map();
  let stopFrame = null;
  let last = 0;
  const draw = (t) => {
    t.el.style.transform = t.x.x === 0 && t.y.x === 0 && !t.held ? '' : `translate(${Math.round(t.x.x * 10) / 10}px, ${Math.round(t.y.x * 10) / 10}px)`;
  };
  const settle = () => { if (tiles.size === 0) onSettle(); };
  const loop = (now) => {
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;
    for (const [key, t] of tiles) {
      if (t.held) continue;
      t.x = springStep(t.x, 0, dt);
      t.y = springStep(t.y, 0, dt);
      if (atRest(t.x, 0) && atRest(t.y, 0)) { t.x = { x: 0, v: 0 }; t.y = { x: 0, v: 0 }; draw(t); tiles.delete(key); } else draw(t);
    }
    stopFrame = null;
    if ([...tiles.values()].some((t) => !t.held)) run();
    else settle();
  };
  const run = () => {
    if (stopFrame) return;
    if (!last) last = clock.now();
    stopFrame = clock.frame(loop);
  };
  const put = (key, el, x, vx, y, vy, held = false) => {
    const t = { el, x: { x, v: vx }, y: { x: y, v: vy }, held };
    tiles.set(key, t);
    draw(t);
    if (!held) { last = clock.now(); run(); }
  };
  return {
    from(key, el, dx, dy) {
      if (reduced) { tiles.delete(key); el.style.transform = ''; settle(); return; }
      const t = tiles.get(key);
      put(key, el, dx, t?.x.v ?? 0, dy, t?.y.v ?? 0);
    },
    hold(key, el, dx, dy) { put(key, el, dx, 0, dy, 0, true); },
    let(key) {
      const t = tiles.get(key);
      if (!t) return;
      if (reduced) { tiles.delete(key); t.el.style.transform = ''; settle(); return; }
      t.held = false;
      last = clock.now();
      run();
    },
    bump(key, el, vx, vy) {
      if (reduced) return;
      const t = tiles.get(key);
      put(key, el, t?.x.x ?? 0, (t?.x.v ?? 0) + vx, t?.y.x ?? 0, (t?.y.v ?? 0) + vy);
    },
    get moving() { return tiles.size > 0; },
    stop() { stopFrame?.(); stopFrame = null; tiles.clear(); },
  };
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/clax_app/home/drag.js apps/clax_app/home/motion.js apps/clax_app/home_unit/drag.test.js apps/clax_app/home_unit/motion.test.js
git -c commit.gpgsign=false commit -m "Add the home tile's press controller and its spring animator"
```

### Task 7: Dragging in the home app; release 1.2.0

**Files:**
- Modify: `apps/clax_app/home/home.js` — tiles placed by `pack`; FLIP on every layout change; the press from `drag.js`; drag across sections; the empty Starred drop zone; edge scrolling; toss; Alt+arrows; reduced motion; `useTiles` waits for rest
- Modify: `apps/clax_app/home/home.css` — the lifted tile, the dragged tile, the drop zone
- Modify: `apps/clax_app/home/tests/home.test.js`, `web/e2e/home-app.spec.ts`
- Modify: `apps/clax_app/home/manifest.json` (`9`, `"1.2.0"`); regenerate `assets/home/home.clax`, `assets/home/release.json`

**Interfaces:**
- Consumes: Task 5 (`pack`, `geometry`, `cellBox`, `slotAt`, `velocityOf`, `tossRest`, `TOSS_MIN`), Task 6 (`createPress`, `createMotion`, `HOLD_MS`), Task 2 (`moveTo`).
- Produces (DOM): each grid entry is placed by `style="grid-column: <col+1> / span <span>; grid-row: <row+1> / span <span>"`; a lifted tile has `data-held`; the dragged tile `data-dragging`; while a drag is on `.h-page[data-dragging]`, and with nothing starred `.h-section[data-section="starred"].h-drop` ("Drop here to star") shows. `home.tiles` is reported only once `motion.moving` is false.

- [ ] **Step 1: Write the failing harness tests** in `tests/home.test.js` (all with a `manualClock`, events dispatched on the tile, positions from the tiles' rects):

```js
const centre = (id) => { const r = document.querySelector(`[data-id="${id}"]`).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; };
const pointer = (type, { x, y }, kind = 'touch', id = 9) => new PointerEvent(type, { bubbles: true, cancelable: true, pointerId: id, isPrimary: true, pointerType: kind, button: 0, clientX: x, clientY: y });
/** Drags tile `from` to `to`: held (touch) or not (mouse), moved in
 * `steps` over `ms`, then still for `pause` ms before the release (a drop;
 * `pause: 0` releases at speed, a toss), then the springs run out. */
async function drag(clock, from, to, { kind = 'touch', steps = 6, ms = 160, hold = true, pause = 100 } = {}) {
  const el = document.querySelector(`[data-id="${from}"]`);
  const a = centre(from);
  el.dispatchEvent(pointer('pointerdown', a, kind));
  if (hold) clock.advance(400);
  for (let i = 1; i <= steps; i++) {
    clock.advance(ms / steps);
    el.dispatchEvent(pointer('pointermove', { x: a.x + (to.x - a.x) * i / steps, y: a.y + (to.y - a.y) * i / steps }, kind));
  }
  if (pause) {
    clock.advance(pause);
    el.dispatchEvent(pointer('pointermove', to, kind));
  }
  el.dispatchEvent(pointer('pointerup', to, kind));
  clock.advance(1500);
}

test('hold and drag a tile onto another place in All apps: it lands there and the order sticks', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  await drag(clock, 'app-run', centre('app-budget'));
  await wait(async () => (await stored()).order.indexOf('app-run') === 4);
  expect(section('all').indexOf('app-run')).toBe(4);
  expect(document.querySelector('[data-id="app-run"]').style.transform).toBe('');
});

test('a hold released without moving still asks for the menu, and nothing moves', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock });
  const el = document.querySelector('[data-id="app-run"]');
  el.dispatchEvent(pointer('pointerdown', centre('app-run')));
  clock.advance(400);
  expect('held' in el.dataset).toBe(true);
  el.dispatchEvent(pointer('pointerup', centre('app-run')));
  await wait(() => home.calls.length === 1);
  expect(home.calls[0].method).toBe('menu');
});

test('a touch that moves before the hold scrolls and lifts nothing; a tap still opens', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock });
  const el = document.querySelector('[data-id="app-run"]');
  const a = centre('app-run');
  el.dispatchEvent(pointer('pointerdown', a));
  clock.advance(100);
  el.dispatchEvent(pointer('pointermove', { x: a.x, y: a.y - 40 }));
  clock.advance(1000);
  expect('held' in el.dataset).toBe(false);
  el.dispatchEvent(pointer('pointerup', { x: a.x, y: a.y - 40 }));
  expect(home.calls).toEqual([]);
  await ui.click('[data-id="app-run"]');
  await wait(() => home.calls.length === 1);
  expect(home.calls[0].method).toBe('open');
});

test('a drag with nothing starred shows the Starred drop zone, and a drop there stars', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  expect(ui.exists('.h-drop')).toBe(false);
  const el = document.querySelector('[data-id="app-plants"]');
  const a = centre('app-plants');
  el.dispatchEvent(pointer('pointerdown', a));
  clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x, y: a.y - 20 }));
  await wait(() => ui.exists('.h-section[data-section="starred"].h-drop'));
  expect(ui.text('.h-drop')).toContain('Drop here to star');
  const z = document.querySelector('.h-drop').getBoundingClientRect();
  el.dispatchEvent(pointer('pointermove', { x: z.x + 40, y: z.y + z.height / 2 }));
  el.dispatchEvent(pointer('pointerup', { x: z.x + 40, y: z.y + z.height / 2 }));
  clock.advance(1500);
  await wait(() => section('starred').join() === 'app-plants');
  expect(ui.exists('.h-drop')).toBe(false);
});

test('dragging out of Starred into All apps unstars', async () => {
  const clock = manualClock();
  const ids = SAMPLE.apps.map((a) => a.id);
  await show({ makes: [] }, { clock, layout: { starred: ['app-run'], order: ids.slice(1), seen: ids } });
  await wait(() => section('starred').length === 1);
  await drag(clock, 'app-run', centre('app-mood'));
  await wait(() => section('starred').length === 0);
  expect(section('all')).toContain('app-run');
});

test('a mouse drags at once, without the hold', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  await drag(clock, 'app-notes', centre('app-run'), { kind: 'mouse', hold: false });
  await wait(() => section('all')[0] === 'app-notes');
});

test('a toss carries the tile on with its speed and it snaps to the nearest cell', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  const a = centre('app-run');
  const b = centre('app-groceries');
  const step = b.x - a.x;
  // About a cell in 60 ms, released at speed: it comes to rest about two cells further on.
  await drag(clock, 'app-run', { x: a.x + step, y: a.y }, { steps: 3, ms: 60, pause: 0 });
  await wait(async () => (await stored()).order.indexOf('app-run') >= 2);
});

test('Alt+arrows move a focused tile one cell', async () => {
  await show({ makes: [] });
  await wait(async () => !!(await stored()));
  const el = document.querySelector('[data-id="app-run"]');
  el.focus();
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true }));
  await wait(() => section('all')[1] === 'app-run');
  expect(document.activeElement.dataset.id).toBe('app-run');
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', altKey: true, bubbles: true }));
  await wait(() => section('all')[0] === 'app-run');
});

test('tiles are reported once they come to rest, not while they move', async () => {
  const clock = manualClock();
  const home = fakeHome({ makes: [] });
  const reports = [];
  home.tiles = async (list) => { reports.push(list); };
  db = fakeDb();
  mount(root(), home, { db, clock });
  await wait(() => reports.length === 1);
  const el = document.querySelector('[data-id="app-run"]');
  const a = centre('app-run');
  el.dispatchEvent(pointer('pointerdown', a));
  clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 120, y: a.y }));
  clock.advance(200);
  await new Promise((r) => setTimeout(r, 120));
  expect(reports.length).toBe(1);
  el.dispatchEvent(pointer('pointerup', { x: a.x + 120, y: a.y }));
  clock.advance(1500);
  await wait(() => reports.length === 2);
  const run = reports[1].find((t) => t.id === 'app-run');
  const r = document.querySelector('[data-id="app-run"]').getBoundingClientRect();
  expect(run.rect).toEqual({ x: r.x, y: r.y, w: r.width, h: r.height });
});
```

(`useTiles`' 50 ms debounce stays on a real timer, so the "not while they move" check waits a real 120 ms once for a report that must not come. That wait proves an absence, not a condition, and is the only one; keep it.) Update the existing 'a long press arms' test: the press now arms at 400 ms; drive it with a `manualClock` (`clock.advance(400)`) instead of waiting on `data-held`.

- [ ] **Step 2: Run them to verify they fail**

Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: FAIL: the harness lists the new tests failing (tiles do not move, no `.h-drop`).

- [ ] **Step 3: Implement in `home.js`.** The parts, in order:

1. **Placement.** `Sections` builds, per section, `items = entries.map((e) => ({ key: e.key, span: 1 }))` (makes first in All apps as `m:<id>`), `const { cells } = pack(items, cols)`, and gives each tile `style=${`grid-column:${c.col + 1} / span ${c.span};grid-row:${c.row + 1} / span ${c.span}`}`. `.h-grid` keeps `display: grid` with `grid-auto-rows: 116px`.
2. **FLIP.** A `useLayoutEffect` in `Sections`, after every render, reads each `.h-tile[data-key]`'s `offsetLeft`/`offsetTop` relative to the page (`getBoundingClientRect()` less the tile's current transform), compares with the map kept from the previous render, and calls `motion.from(key, el, oldX - newX, oldY - newY)` for each tile that moved and is not the dragged one. One `motion` per `Home`: `useMemo(() => createMotion(clock, { reduced, onSettle: report }), [clock])`, where `reduced = matchMedia('(prefers-reduced-motion: reduce)').matches` (re-read on its `change` event) and `report` is `useTiles`' report.
3. **The press.** `AppTile` takes a `press` object built by `Home` per tile key with `createPress(handlers, clock)`; its element gets `onPointerDown/Move/Up/Cancel`, `onContextMenu` (`if (!press.contextMenu()) onMenu(...)`, always `preventDefault`), and `onClick` (`if (press.takeClick()) onOpen(app)`). `lift` sets `data-held` and calls `el.setPointerCapture(pointerId)`; `unlift` removes it. While a press is lifted or dragging, a document `touchmove` listener added with `{ passive: false }` calls `preventDefault()` so the page does not scroll; it is removed on `dragEnd`/`dragCancel`/`menu`/`unlift`.
4. **Dragging.** `dragStart(p)` records the tile's centre and the pointer offset, sets `drag = { key, from: section }` in state (rendering `.h-page[data-dragging]`, `data-dragging` on the tile, and with no stars the `.h-drop` Starred zone), and `motion.hold(key, el, 0, 0)`. `dragMove(p)` calls `motion.hold(key, el, p.x - x0, p.y - y0)`, finds the section under the pointer (the Starred section or `.h-drop` by rect, else All apps), computes that grid's `geometry(width, cols)` and `slotAt(cells, g, p.x - gridLeft, p.y - gridTop)`, and when the slot or section changed sets a provisional layout `moveTo(layout, key, section, slot)` in state, so the neighbours re-pack and FLIP aside. Near an edge (pointer within 48 px of the viewport's top below the bar, or of its bottom) it scrolls by `speed × dt` each frame on `clock.frame`, `speed` growing to 720 px/s at the edge.
5. **Dropping.** `dragEnd(p, samples)`: `const v = velocityOf(samples)`; if `Math.hypot(v.vx, v.vy) > TOSS_MIN` the drop point is `tossRest({ x: p.x, y: p.y, vx: v.vx, vy: v.vy })` clamped to the grid, and each tile whose cell centre the tossed tile's path passes within half a cell of gets `motion.bump(key, el, v.vx * 0.15, v.vy * 0.15)`; the slot is taken at the drop point; `change((l) => moveTo(l, key, section, slot))`; `motion.let(key)` lets the tile spring from where it was released to its cell. `dragCancel` drops the provisional layout and lets the tile spring home. With reduced motion there is no toss: the drop point is the release point.
6. **Keyboard.** `onKeyDown` on a tile: with `altKey`, `ArrowLeft`/`ArrowRight` move it one entry, `ArrowUp`/`ArrowDown` one row (`cols` entries), within its section, clamped; `change((l) => moveTo(l, key, section, index ± n))`, `preventDefault()`, and focus stays on the tile (it keeps its `key`, so Preact keeps the element).
7. **Tiles.** `useTiles(home, drawn)` reports from `report()` only when `!motion.moving`; `onSettle` calls `report()`.

Raise `HOLD_MS`/`SLOP_PX` to `drag.js`'s (400, 8) by removing `usePress` and its constants from `home.js`. Pointer events on the dragged tile only are handled; a second pointer is ignored (`createPress` checks `pointerId`).

- [ ] **Step 4: Styles** in `home.css`:

```css
.h-grid { grid-auto-rows: 116px; }
.h-tile { position: relative; will-change: transform; }
.h-tile[data-held] .h-icon > * { transform: scale(1.08); }
.h-tile[data-held] .h-face, .h-tile[data-dragging] .h-face { box-shadow: var(--h-fab-shadow); }
.h-tile[data-dragging] { z-index: 5; pointer-events: none; }
.h-page[data-dragging] .h-tile:not([data-dragging]) { cursor: grabbing; }
.h-drop {
  margin: 8px 4px; height: 116px; border-radius: 20px;
  border: 2px dashed color-mix(in srgb, var(--h-primary) 60%, transparent);
  display: grid; place-items: center; color: var(--h-on-variant);
}
@media (prefers-reduced-motion: reduce) { .h-icon > * { transition: none; } }
```

(replacing the old `[data-held] … scale(0.92)` rule: a lifted tile grows, as the spec's "scales up, shadow"). The `.h-drop` element reads "Drop here to star".

- [ ] **Step 5: The browser tests.** In `web/e2e/home-app.spec.ts`, "over a fake host":

```ts
  test("a touch hold then drag reorders, and a mouse drag to Starred stars", async ({ page }) => {
    await page.clock.install();
    await serve(page, { makes: [] });
    await expect(page.locator(".h-tile[data-id]")).toHaveCount(8);
    const box = async (sel: string) => (await page.locator(sel).boundingBox())!;
    const from = await box('[data-id="app-run"]');
    const to = await box('[data-id="app-budget"]');
    const cdp = await page.context().newCDPSession(page);
    const touch = (type: "touchStart" | "touchMove" | "touchEnd", x: number, y: number) => cdp.send("Input.dispatchTouchEvent", { type, touchPoints: type === "touchEnd" ? [] : [{ x, y, id: 1 }] });
    const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
    const [x1, y1] = [to.x + to.width / 2, to.y + to.height / 2];
    await touch("touchStart", x0, y0);
    await page.clock.runFor(450);
    for (let i = 1; i <= 8; i++) { await touch("touchMove", x0 + (x1 - x0) * i / 8, y0 + (y1 - y0) * i / 8); await page.clock.runFor(30); }
    await page.clock.runFor(150);
    await touch("touchEnd", x1, y1);
    await page.clock.runFor(1500);
    await expect.poll(() => page.locator('.h-section[data-section="all"] .h-tile[data-id]').evaluateAll(els => els.map(e => (e as HTMLElement).dataset.id).indexOf("app-run"))).toBe(4);
    const notes = await box('[data-id="app-notes"]');
    await page.mouse.move(notes.x + 30, notes.y + 30);
    await page.mouse.down();
    await page.mouse.move(notes.x + 30, notes.y + 10, { steps: 2 });
    await expect(page.locator(".h-drop")).toBeVisible();
    const zone = await box(".h-drop");
    await page.mouse.move(zone.x + 40, zone.y + 40, { steps: 6 });
    await page.mouse.up();
    await page.clock.runFor(1500);
    await expect(page.locator('.h-section[data-section="starred"] [data-id="app-notes"]')).toHaveCount(1);
  });
```

Add a render of a drag midway to "renders": mount over `fakeDb({ "layout/main": { v: 1, starred: ["app-run"], order: [ids without app-run], seen: ids } })`, press `[data-id="app-habits"]` with the mouse, move it 60 px right and 40 px down in steps (`page.mouse.move(..., { steps: 4 })`), screenshot `home-drag-${size}-${scheme}.png` with the button still down and `animations: "disabled"`, then `page.mouse.up()`. Update `expect(tests.passed)`.

- [ ] **Step 6: Run the tests**

Run: `pnpm --dir web exec vitest run home_unit && bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: PASS.

- [ ] **Step 7: Look at the renders** (native run as in Task 3 Step 8). Read `target/flutter-app/home/home-drag-{phone,desktop}-{light,dark}.png`: the dragged tile is lifted (larger, shadowed) and above its neighbours, which have moved aside to show its landing cell; nothing is clipped at the grid's edge; the shadow reads in dark mode. Fix and rerun until they do.

- [ ] **Step 8: On the phone, if one is attached.** `adb devices` lists it: run `just app-phone` (installs HEAD's APK, keeping the app's data; it never touches `~/.clax`). Reset frame stats, drag and toss a few tiles with `adb shell input touchscreen draganddrop <x0> <y0> <x1> <y1> 600` (and by hand when the owner is there), then read jank:

```bash
adb shell dumpsys gfxinfo dev.empathic.clax reset
adb shell input touchscreen draganddrop 150 600 650 600 600
adb shell dumpsys gfxinfo dev.empathic.clax | grep -E "Total frames|Janky frames|90th|95th"
adb shell screenrecord --time-limit 12 /sdcard/clax-drag.mp4   # while dragging by hand
adb pull /sdcard/clax-drag.mp4 target/flutter-app/phone/clax-drag.mp4
```

Report the numbers. No phone attached: record a concern on `apps/clax_app/home/motion.js` tagged `status:needs-decision` asking the owner to judge a drag and a toss on the phone, with the commands above.

- [ ] **Step 9: Pack release 1.2.0** (`"version": 9`, `"release": "1.2.0"`), as Task 3 Step 9, then `bazelisk test //apps/clax_app:home_home_bundle_test --test_output=errors` (PASS).

- [ ] **Step 10: Commit**

```bash
git add apps/clax_app/home apps/clax_app/assets/home web/e2e/home-app.spec.ts
git -c commit.gpgsign=false commit -m "Let the person hold and drag home tiles into place, with springs and toss; home release 1.2.0"
```

---

## Phase 3 — Groups, both looks (release 1.3.0)

Task 8 is pure and can run while Task 7 is reviewed (it touches `arrange.js`, `grid.js` and `home_unit/` only). Tasks 9 and 10 both edit `home.js`, `home.css`, `tests/home.test.js` and `home-app.spec.ts`: run them one after the other.

### Task 8: Group rules and the pile's tilt

**Files:**
- Modify: `apps/clax_app/home/arrange.js` — `groupNameOf`, `makeGroup`, `addToGroup`, `removeFromGroup`, `moveInGroup`, `renameGroup`, `setLook`, `ungroup`
- Modify: `apps/clax_app/home/grid.js` — `centreUnder`, `tiltOf`
- Modify: `apps/clax_app/home_unit/arrange.test.js`, `apps/clax_app/home_unit/grid.test.js`

**Interfaces:**
- Produces (`arrange.js`):
  ```js
  export function groupNameOf(a: App, b: App): string;                       // common icon word, capitalised, else 'Group'
  export function makeGroup(layout, targetId, draggedId, gid, name): Layout;  // a folder at target's place in All apps: [target, dragged]
  export function addToGroup(layout, gid, id, index = Infinity): Layout;
  export function removeFromGroup(layout, id): Layout;                        // to All apps right after its group
  export function moveInGroup(layout, gid, id, index): Layout;
  export function renameGroup(layout, gid, name): Layout;                     // trimmed, at most 40 characters; empty changes nothing
  export function setLook(layout, gid, look: 'folder'|'pile'): Layout;
  export function ungroup(layout, gid): Layout;                               // its apps take its place, in its order
  ```
- Produces (`grid.js`): `centreUnder(cells, g, x, y, radius = 24): number` (index of the cell whose centre is within `radius` of the point, or -1); `tiltOf(gid, i): number` (degrees, −8 to 8, the same for one group and position every time).

- [ ] **Step 1: Write the failing tests.** Append to `arrange.test.js`:

```js
import { groupNameOf, makeGroup, addToGroup, removeFromGroup, moveInGroup, renameGroup, setLook, ungroup } from '../home/arrange.js';

describe('groups', () => {
  const apps = [A, B, C, D];
  const base = () => reconcile({ starred: ['d'], order: ['a', 'b', 'c'], seen: ['a', 'b', 'c', 'd'] }, apps);

  test('a group is named from the common icon word, else Group', () => {
    expect(groupNameOf(app('x', 0, { word: 'Money' }), app('y', 0, { word: 'money ' }))).toBe('Money');
    expect(groupNameOf(app('x', 0, { word: 'money' }), app('y', 0, { word: 'chart' }))).toBe('Group');
    expect(groupNameOf(app('x', 0, { emoji: '🍲' }), app('y', 0, {}))).toBe('Group');
  });

  test("dropping an app on another makes a folder at the target's place, target first", () => {
    const l = reconcile(makeGroup(base(), 'b', 'a', 'g1', 'Group'), apps);
    expect(l.order).toEqual(['g:g1', 'c']);
    expect(l.groups.g1).toEqual({ name: 'Group', look: 'folder', apps: ['b', 'a'] });
  });

  test('a starred target leaves Starred for the new group; a group never goes in a group', () => {
    const l = reconcile(makeGroup(base(), 'd', 'c', 'g1', 'G'), apps);
    expect(l.starred).toEqual([]);
    expect(l.order).toEqual(['a', 'b', 'g:g1']);
    const g = reconcile(makeGroup(base(), 'a', 'b', 'g1', 'G'), apps);
    expect(makeGroup(g, 'c', 'g:g1', 'g2', 'H')).toEqual(g);
    expect(addToGroup(g, 'g1', 'g:g1')).toEqual(g);
  });

  test('add, move inside, remove, rename, look, ungroup', () => {
    let l = reconcile(makeGroup(base(), 'a', 'b', 'g1', 'G'), apps);
    l = reconcile(addToGroup(l, 'g1', 'c'), apps);
    expect(l.groups.g1.apps).toEqual(['a', 'b', 'c']);
    expect(l.order).toEqual(['g:g1']);
    l = moveInGroup(l, 'g1', 'c', 0);
    expect(l.groups.g1.apps).toEqual(['c', 'a', 'b']);
    l = reconcile(removeFromGroup(l, 'a'), apps);
    expect(l.order).toEqual(['g:g1', 'a']);
    expect(renameGroup(l, 'g1', '  Tools  ').groups.g1.name).toBe('Tools');
    expect(renameGroup(l, 'g1', '   ').groups.g1.name).toBe('G');
    expect(renameGroup(l, 'g1', 'x'.repeat(60)).groups.g1.name).toHaveLength(40);
    expect(setLook(l, 'g1', 'pile').groups.g1.look).toBe('pile');
    const u = reconcile(ungroup(l, 'g1'), apps);
    expect(u.order).toEqual(['c', 'b', 'a']);
    expect(u.groups).toEqual({});
  });

  test('removing the second-last app dissolves the group into the one left', () => {
    let l = reconcile(makeGroup(base(), 'a', 'b', 'g1', 'G'), apps);
    l = reconcile(removeFromGroup(l, 'b'), apps);
    expect(l.order).toEqual(['a', 'b', 'c']);
    expect(l.groups).toEqual({});
  });
});
```

Append to `grid.test.js`:

```js
import { centreUnder, tiltOf } from '../home/grid.js';

test('centreUnder finds a tile only near its centre', () => {
  const g = geometry(412, 4);
  const { cells } = pack(['a', 'b'].map((key) => ({ key })), 4);
  const b = cellBox(cells[1], g);
  expect(centreUnder(cells, g, b.x + b.w / 2 + 10, b.y + b.h / 2 - 10)).toBe(1);
  expect(centreUnder(cells, g, b.x + 4, b.y + 4)).toBe(-1);
});

test('a pile tilts the same way every time, within 8°', () => {
  const a = [0, 1, 2, 3, 4].map((i) => tiltOf('g1', i));
  expect([0, 1, 2, 3, 4].map((i) => tiltOf('g1', i))).toEqual(a);
  expect(a.every((d) => d >= -8 && d <= 8)).toBe(true);
  expect(new Set(a).size).toBeGreaterThan(1);
  expect(tiltOf('g2', 0)).not.toBe(tiltOf('g1', 0));
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: FAIL: the functions are not exported.

- [ ] **Step 3: Implement.** In `arrange.js`:

```js
const NAME_MAX = 40;

/** A new group's name: the icon word `a` and `b` share, capitalised, else "Group". */
export function groupNameOf(a, b) {
  const word = (x) => (typeof x?.icon?.word === 'string' ? x.icon.word.trim().toLowerCase() : '');
  const w = word(a);
  return w && w === word(b) ? w[0].toUpperCase() + w.slice(1) : 'Group';
}

/** App `draggedId` dropped on app `targetId`: a new folder `gid` named
 * `name`, holding the target then the dragged app, at the target's place
 * in All apps (a starred target leaves Starred: groups are not starred).
 * A group dropped on anything makes nothing. */
export function makeGroup(layout, targetId, draggedId, gid, name) {
  const l = normalize(layout);
  if (isGroupKey(targetId) || isGroupKey(draggedId) || targetId === draggedId || l.groups[gid]) return l;
  // The target's place among All apps' entries once the dragged app is
  // out; a starred target's is the end.
  const rest = l.order.filter((k) => k !== draggedId);
  const at = rest.indexOf(targetId);
  const out = moveTo(moveTo(l, draggedId, 'all', 0), targetId, 'all', 0);
  const order = out.order.filter((k) => k !== targetId && k !== draggedId);
  const i = at >= 0 ? at : order.length;
  return {
    ...out,
    order: [...order.slice(0, i), groupKey(gid), ...order.slice(i)],
    groups: { ...out.groups, [gid]: { name, look: 'folder', apps: [targetId, draggedId] } },
  };
}

/** App `id` into group `gid` at `index` (clamped), out of wherever it was. */
export function addToGroup(layout, gid, id, index = Infinity) {
  const l = normalize(layout);
  if (isGroupKey(id) || !l.groups[gid]) return l;
  const out = moveTo(l, id, 'all', 0);
  const order = out.order.filter((k) => k !== id);
  const apps = out.groups[gid].apps.filter((k) => k !== id);
  const i = Math.max(0, Math.min(Number.isFinite(index) ? Math.trunc(index) : apps.length, apps.length));
  return { ...out, order, groups: { ...out.groups, [gid]: { ...out.groups[gid], apps: [...apps.slice(0, i), id, ...apps.slice(i)] } } };
}

/** App `id` out of its group, into All apps right after the group. */
export function removeFromGroup(layout, id) {
  const l = normalize(layout);
  const gid = groupOf(l, id);
  if (!gid) return l;
  return moveTo(l, id, 'all', l.order.indexOf(groupKey(gid)) + 1);
}

/** App `id` to `index` inside its group `gid`. */
export const moveInGroup = (layout, gid, id, index) => addToGroup(layout, gid, id, index);

/** Group `gid` named `name`, trimmed to at most NAME_MAX characters; an empty name changes nothing. */
export function renameGroup(layout, gid, name) {
  const l = normalize(layout);
  const n = [...String(name ?? '').trim()].slice(0, NAME_MAX).join('');
  if (!n || !l.groups[gid]) return l;
  return { ...l, groups: { ...l.groups, [gid]: { ...l.groups[gid], name: n } } };
}

/** Group `gid` shown as a folder or as a pile. */
export function setLook(layout, gid, look) {
  const l = normalize(layout);
  if (!l.groups[gid] || (look !== 'folder' && look !== 'pile')) return l;
  return { ...l, groups: { ...l.groups, [gid]: { ...l.groups[gid], look } } };
}

/** Group `gid` gone, its apps in its place in All apps, in its order. */
export function ungroup(layout, gid) {
  const l = normalize(layout);
  const g = l.groups[gid];
  if (!g) return l;
  const groups = { ...l.groups };
  delete groups[gid];
  const order = l.order.flatMap((k) => (k === groupKey(gid) ? g.apps : [k]));
  return { ...l, order, groups };
}
```

In `grid.js`:

```js
/** The index of the cell whose centre is within `radius` px of (x, y), or -1. */
export function centreUnder(cells, g, x, y, radius = 24) {
  const i = slotAt(cells, g, x, y);
  if (i < 0) return -1;
  const b = cellBox(cells[i], g);
  return Math.hypot(b.x + b.w / 2 - x, b.y + b.h / 2 - y) <= radius ? i : -1;
}

/** The tilt, in degrees from −8 to 8, of the `i`th icon of group `gid`'s
 * pile: from a hash of both, so a pile looks the same each time. */
export function tiltOf(gid, i) {
  const s = `${gid}:${i}`;
  let h = 0x811c9dc5;
  for (let k = 0; k < s.length; k++) h = Math.imul(h ^ s.charCodeAt(k), 0x01000193) >>> 0;
  return ((h % 1601) / 100) - 8;
}
```

Add the case of a dragged app before its target to Step 1's tests: `reconcile(makeGroup(base(), 'c', 'a', 'g1', 'G'), apps).order` is `['b', 'g:g1']`.

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/clax_app/home/arrange.js apps/clax_app/home/grid.js apps/clax_app/home_unit
git -c commit.gpgsign=false commit -m "Add the home screen's group rules and the pile's tilt"
```

### Task 9: Folders: making, opening, the group menu, and Remove from group

**Files:**
- Modify: `apps/clax_app/home/home.js` — the group tile; hover 600 ms to group; drop on a group; the folder overlay (rename, reorder, drag out); the group's own menu; "Remove from group"; `tiles()` while a folder is open
- Modify: `apps/clax_app/home/home.css`
- Modify: `apps/clax_app/home/tests/home.test.js`, `web/e2e/home-app.spec.ts`
- Modify: `apps/clax_app/assets/prompt/system.md` (one line: groups ship)

**Interfaces:**
- Consumes: Task 8's group rules, `centreUnder`; Task 7's drag pipeline.
- Produces (DOM): a group tile `.h-tile.h-group[data-group=<gid>][data-look="folder"|"pile"]` (no `data-id`, so `tiles()` never reports it and a tap on the start-up picture there opens nothing); a hover target `[data-grouping]` with `.h-group-outline`; the overlay `.h-folder[role="dialog"]` with `.h-folder-name` (a button; tap to rename into an `<input aria-label="Group name">`) and its own `.h-grid` of `.h-tile[data-id]`; the group menu `.h-group-menu[role="menu"]` with items Rename, Show as pile / Show as folder, Ungroup. Opening a folder pushes a history entry (`#group`); back closes it.

- [ ] **Step 1: Write the failing harness tests**:

```js
const groupLayout = (look = 'folder') => {
  const ids = SAMPLE.apps.map((a) => a.id);
  return { v: 1, starred: [], order: ['app-run', 'g:g1', ...ids.filter((id) => !['app-run', 'app-groceries', 'app-notes'].includes(id))], groups: { g1: { name: 'Lists', look, apps: ['app-groceries', 'app-notes'] } }, seen: ids };
};

test('holding a tile 600 ms over another app outlines a group, and dropping makes a folder named Group', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  const el = document.querySelector('[data-id="app-mood"]');
  const a = centre('app-mood');
  const t = centre('app-habits');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  // Between the two rows, near no tile's centre, then onto the target's centre at once.
  el.dispatchEvent(pointer('pointermove', { x: a.x + 40, y: (a.y + t.y) / 2 })); clock.advance(100);
  el.dispatchEvent(pointer('pointermove', t));
  clock.advance(599);
  expect(ui.exists('[data-grouping]')).toBe(false);
  clock.advance(1);
  await wait(() => ui.exists('[data-id="app-habits"][data-grouping]'));
  el.dispatchEvent(pointer('pointerup', t)); clock.advance(1500);
  await wait(() => ui.exists('.h-group[data-look="folder"]'));
  const l = await stored();
  const [gid] = Object.keys(l.groups);
  expect(l.groups[gid]).toEqual({ name: 'Group', look: 'folder', apps: ['app-habits', 'app-mood'] });
  expect(ui.text('.h-group .h-label')).toBe('Group');
});

test('dropping an app on a group adds it', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  const g = document.querySelector('.h-group').getBoundingClientRect();
  await drag(clock, 'app-plants', { x: g.x + g.width / 2, y: g.y + g.height / 2 });
  await wait(async () => (await stored()).groups.g1.apps.includes('app-plants'));
});

test('a tap opens the folder over a dimmed grid; its name renames; back closes it', async () => {
  const home = await show({ makes: [] }, { layout: groupLayout() });
  await ui.click('.h-group');
  await wait(() => ui.exists('.h-folder'));
  expect([...document.querySelectorAll('.h-folder .h-tile[data-id]')].map((t) => t.dataset.id)).toEqual(['app-groceries', 'app-notes']);
  await ui.click('.h-folder-name');
  const input = document.querySelector('.h-folder input[aria-label="Group name"]');
  input.value = 'Shopping';
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await wait(async () => (await stored()).groups.g1.name === 'Shopping');
  await ui.click('.h-folder [data-id="app-notes"]');
  await wait(() => home.calls.some((c) => c.method === 'open'));
  history.back();
  await wait(() => !ui.exists('.h-folder'));
});

test("the group's own menu renames, switches its look and ungroups", async () => {
  await show({ makes: [] }, { layout: groupLayout() });
  document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.exists('.h-group-menu'));
  expect([...document.querySelectorAll('.h-group-menu [role="menuitem"]')].map((b) => b.textContent)).toEqual(['Rename', 'Show as pile', 'Ungroup']);
  await ui.click('Show as pile');
  await wait(async () => (await stored()).groups.g1.look === 'pile');
  document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.exists('.h-group-menu'));
  expect(ui.text('.h-group-menu')).toContain('Show as folder');
  await ui.click('Ungroup');
  await wait(async () => Object.keys((await stored()).groups).length === 0);
  expect(section('all').slice(0, 3)).toEqual(['app-run', 'app-groceries', 'app-notes']);
});

test('an app in a group offers Remove from group in its menu', async () => {
  const home = await show({ makes: [] }, { layout: groupLayout() });
  await ui.click('.h-group');
  await wait(() => ui.exists('.h-folder'));
  home.menuPick = 'remove';
  document.querySelector('.h-folder [data-id="app-notes"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(async () => !(await stored()).groups.g1);
  expect(home.calls.at(-1).args[2]).toEqual({ items: [{ id: 'star', label: 'Star' }, { id: 'remove', label: 'Remove from group' }] });
});

test('dragging an app out past the folder edge takes it out of the group', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: { ...groupLayout(), groups: { g1: { name: 'Lists', look: 'folder', apps: ['app-groceries', 'app-notes', 'app-plants'] } } } });
  await ui.click('.h-group');
  await wait(() => ui.exists('.h-folder'));
  const panel = document.querySelector('.h-folder-panel').getBoundingClientRect();
  await drag(clock, 'app-plants', { x: panel.x + panel.width / 2, y: panel.bottom + 60 });
  await wait(async () => !(await stored()).groups.g1.apps.includes('app-plants'));
  expect(ui.exists('.h-folder')).toBe(false);
});

test('tiles() leaves group tiles out, and while a folder is open gives only its apps', async () => {
  const home = fakeHome({ makes: [] });
  const reports = [];
  home.tiles = async (list) => { reports.push(list.map((t) => t.id)); };
  db = fakeDb({ [LAYOUT]: groupLayout() });
  mount(root(), home, { db });
  await wait(() => reports.length >= 1);
  expect(reports.at(-1)).not.toContain('app-groceries');
  expect(reports.at(-1).every((id) => id.startsWith('app-'))).toBe(true);
  await ui.click('.h-group');
  await wait(() => reports.at(-1).join() === 'app-groceries,app-notes');
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: FAIL: no `.h-group`.

- [ ] **Step 3: Implement in `home.js`.**

1. **The group tile** (`GroupTile`): a button `.h-tile.h-group` with `data-group`, `data-look`, `aria-label="<name>, group of <n> apps"`, its own `createPress` (a tap opens the folder; a hold-release or right click opens the group menu; a hold-move drags it like an app, never into Starred and never onto another tile as a group). The folder face is a 60×60 `.h-face.h-folder-face` with up to four 26×26 `AppFace`s in a 2×2 grid; the label is the group's name.
2. **Hover to group.** In `dragMove`, when the dragged entry is an app and `centreUnder(cells, g, x, y)` names another entry (an app or a group), the provisional order is left as it is (the tile under the pointer stays put, so it can be grouped with) and a `clock.after(600, …)` starts that sets `grouping = key` (rendering `data-grouping` and `.h-group-outline` around that tile); moving off it (`centreUnder` changes) cancels the timer and clears `grouping`. Outside every centre zone the order follows the pointer as in Task 7. A release on a centre before the 600 ms is an ordinary drop into that slot, so Task 7's reorder test still holds. `dragEnd` with `grouping` set: an app target → `change((l) => makeGroup(l, target, dragged, gid, groupNameOf(targetApp, draggedApp)))` with `gid = Math.random().toString(36).slice(2, 10)`; a group target → `change((l) => addToGroup(l, gid, dragged))`. A group being dragged never sets `grouping`.
3. **The folder overlay** (`Folder`): when `open = gid`, a `.h-scrim` dims the grid and a `.h-folder[role="dialog"][aria-label=<name>]` shows a `.h-folder-panel` with the name (`.h-folder-name`, a button; tapping swaps in an `<input aria-label="Group name" maxlength="40">`, Enter or blur → `renameGroup`, Escape cancels) and the group's apps in a `.h-grid` laid out by `pack` with the same columns (at most 4). Inside, the press works as in the main grid with the folder's own cells: dropping inside reorders (`moveInGroup`); a drop more than 24 px outside the panel's rect → `change((l) => removeFromGroup(l, id))` and the overlay closes. Opening pushes `history.pushState({ clax: 'group' }, '', '#group')`; a `popstate` while open closes it; closing by the scrim or Escape calls `history.back()` when the entry is ours.
4. **The menus.** `menuItems(layout, id)` adds `{ id: 'remove', label: 'Remove from group' }` when `groupOf(layout, id)`; a pick of `remove` → `removeFromGroup`. The group menu is the page's own `.h-group-menu[role="menu"]`, styled like `.h-menu`, anchored at the group tile (below it, or above near the bottom), with "Rename" (opens the folder with its name in edit), "Show as pile" or "Show as folder" (`setLook`), "Ungroup" (`ungroup`); Escape or the scrim closes it.
5. **Tiles.** `useTiles` queries `.h-folder .h-tile[data-id]` while a folder is open, else `.h-sections .h-tile[data-id]`.

In `system.md`'s home section add: "`groups` (folders and piles) and `g:<groupId>` entries in `order` are shown by the shipped home app from release 1.3.0." Run `bazelisk test //apps/clax_app:agent_prompt_test`.

- [ ] **Step 4: Styles.** `.h-folder-face` (the 2×2 mini grid on `var(--h-sc)`, radius 15px, 4 px padding, 2 px gaps), `.h-group-outline` (a 3 px `var(--h-primary)` ring of radius 20px around the target tile, scaled 1.06 on a 120 ms transition), `.h-folder` (fixed, centred; panel `var(--h-menu-bg)`, radius 28px, `var(--h-menu-shadow)`, max-width `min(480px, calc(100vw - 32px))`), its scrim `rgba(0,0,0,0.32)`, `.h-group-menu` as `.h-menu`. Dark mode comes from the tokens; check it in the renders.

- [ ] **Step 5: Browser test and render.** In "over a fake host": make a group by hovering with the mouse (`page.clock.install()`; mouse down on `app-mood`, move to `app-habits`'s centre in steps, `page.clock.runFor(650)`, expect `[data-grouping]`, mouse up, `runFor(1500)`, expect `.h-group[data-look="folder"]`), then tap it and expect `.h-folder` with two tiles. In "renders", `home-groups-${size}-${scheme}.png`: a layout with two starred apps, one folder of three apps and one pile of four (the pile draws as a folder until Task 10, so this render is retaken there), and a second shot with the folder open, `home-folder-${size}-${scheme}.png`. Update `expect(tests.passed)`.

- [ ] **Step 6: Run the tests**

Run: `pnpm --dir web exec vitest run home_unit && bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts //apps/clax_app:agent_prompt_test --test_output=errors`
Expected: PASS.

- [ ] **Step 7: Look at the renders** (native run as in Task 3 Step 8): `home-groups-*` and `home-folder-*`, phone and desktop, light and dark. The folder face reads as a folder (four small icons on a tonal square, the name under it); the open folder's panel is centred, its name looks tappable, its grid aligns with the main grid's cells; the scrim dims both themes evenly.

- [ ] **Step 8: Commit**

```bash
git add apps/clax_app/home web/e2e/home-app.spec.ts apps/clax_app/assets/prompt/system.md
git -c commit.gpgsign=false commit -m "Let the person gather home apps into folders by hovering, and rename, reorder and ungroup them"
```

### Task 10: Piles, and release 1.3.0

**Files:**
- Modify: `apps/clax_app/home/home.js` — the pile face; the fan-out row; gathering it back
- Modify: `apps/clax_app/home/home.css`
- Modify: `apps/clax_app/home/tests/home.test.js`, `web/e2e/home-app.spec.ts`
- Modify: `apps/clax_app/home/manifest.json` (`10`, `"1.3.0"`); regenerate `assets/home/home.clax`, `release.json`

**Interfaces:**
- Consumes: Task 8's `tiltOf`; Task 9's `GroupTile`, group menu and `useTiles` rule; Task 6's `motion`.
- Produces (DOM): a pile's face `.h-pile-face` holding up to five `.h-pile-card` (each an `AppFace` turned by `tiltOf(gid, i)`); a fanned pile renders, in the section's grid, a full-width row `.h-fan[data-group=<gid>]` (`grid-column: 1 / -1`) right after the pile's row, holding its apps as `.h-tile[data-id]` in a horizontally scrolling flex row; the rows after it move down by FLIP springs. A tap outside the fan (or back, or Escape) gathers it. `tiles()` reports a fanned pile's apps.

- [ ] **Step 1: Write the failing harness tests**:

```js
test('a pile shows up to five tilted icons, the same tilt every load', async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const layout = { v: 1, starred: [], order: ['g:p1', 'app-mood', 'app-notes'], groups: { p1: { name: 'Lots', look: 'pile', apps: ids.filter((id) => !['app-mood', 'app-notes'].includes(id)) } }, seen: ids };
  await show({ makes: [] }, { layout });
  const tilts = () => [...document.querySelectorAll('.h-pile-card')].map((c) => c.style.transform);
  expect(tilts()).toHaveLength(5);
  const first = tilts();
  await show({ makes: [] }, { layout });
  expect(tilts()).toEqual(first);
});

test('a tap fans the pile out in a row below it, pushing the grid down; a tap outside gathers it', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock, layout: groupLayout('pile') });
  const below = document.querySelector('[data-id="app-mood"]').getBoundingClientRect().y;
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan[data-group="g1"]'));
  expect([...document.querySelectorAll('.h-fan .h-tile[data-id]')].map((t) => t.dataset.id)).toEqual(['app-groceries', 'app-notes']);
  expect(document.querySelector('[data-id="app-mood"]').getBoundingClientRect().y).toBeGreaterThan(below);
  await ui.click('.h-fan [data-id="app-notes"]');
  await wait(() => home.calls.some((c) => c.method === 'open' && c.args[0] === 'app-notes'));
  document.querySelector('.h-key, .h-bar').click();
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan'));
});

test('tiles() reports a fanned pile\'s apps', async () => {
  const home = fakeHome({ makes: [] });
  const reports = [];
  home.tiles = async (list) => { reports.push(list.map((t) => t.id)); };
  db = fakeDb({ [LAYOUT]: groupLayout('pile') });
  const clock = manualClock();
  mount(root(), home, { db, clock });
  await wait(() => reports.length >= 1);
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => reports.at(-1).includes('app-notes'));
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: FAIL: no `.h-pile-card`, no `.h-fan`.

- [ ] **Step 3: Implement.** `GroupTile` with `look === 'pile'` draws `.h-pile-face`: up to five of its apps' `AppFace`s, absolutely stacked in a 60×60 box, the last on top, each `style="transform: rotate(<tiltOf(gid, i)>deg)"` with a 1 px `var(--h-surface)` outline so the cards separate. A tap sets `fanned = gid` (state) instead of opening a folder; `Sections` then inserts, in that section's grid, a `.h-fan` entry right after the last cell of the pile's row, placed `grid-column: 1 / -1; grid-row: <row+2>`, and packs the rest of the entries from the next row down (`pack` over the entries after the pile's row, offset by two rows); the FLIP pass springs every moved tile down, and the fan's tiles start at the pile's position (`motion.from(key, el, pileX - x, pileY - y)`) so they spread out of it. Gathering reverses it (the fan's tiles spring back into the pile's position, then the row goes). Opening pushes `#group` as the folder does; a document click outside `.h-fan` and `.h-group` gathers. Reduced motion: the row appears and goes at once.

- [ ] **Step 4: Styles**: `.h-pile-face { position: relative; width: 60px; height: 60px; }`, `.h-pile-card { position: absolute; inset: 0; }` with the card `.h-face` given `outline: 1px solid var(--h-surface)`; `.h-fan { display: flex; gap: 4px; overflow-x: auto; scroll-snap-type: x proximity; padding: 4px 0; border-radius: 20px; background: color-mix(in srgb, var(--h-sc) 50%, transparent); }`, `.h-fan .h-tile { flex: 0 0 var(--cell-w); scroll-snap-align: start; }` (with `--cell-w` set from the measured `geometry`).

- [ ] **Step 5: Browser test and renders.** In "over a fake host", with a pile layout: tap the pile, `page.clock.runFor(1000)`, expect `.h-fan .h-tile[data-id]` count 2, tap the top bar, `runFor(1000)`, expect `.h-fan` count 0. Retake `home-groups-*` (now with a real pile) and add `home-pile-${size}-${scheme}.png` with the pile fanned (`animations: "disabled"` after `runFor`). Update `expect(tests.passed)`.

- [ ] **Step 6: Run the tests and look at the renders** (Bazel run, then native renders as in Task 3 Step 8). In `home-groups-*` the pile reads as a loose stack (not a folder), its tilts small and varied, its name under it; in `home-pile-*` the fan's row sits under the pile's row, its tiles aligned to the grid's cell width, scrolling sideways when wider than the screen, and the rows below pushed down, not overlapped.

- [ ] **Step 7: Pack release 1.3.0** (`"version": 10`, `"release": "1.3.0"`) as Task 3 Step 9; run `bazelisk test //apps/clax_app:home_home_bundle_test --test_output=errors` (PASS).

- [ ] **Step 8: Commit**

```bash
git add apps/clax_app/home apps/clax_app/assets/home web/e2e/home-app.spec.ts
git -c commit.gpgsign=false commit -m "Show a home group as a pile that fans out in place; home release 1.3.0"
```

---

## Phase 4 — Search; pull-to-refresh goes (release 1.4.0)

Task 11 is pure (`search.js`, `home_unit/search.test.js`) and can run any time, in parallel with any other task. Task 12 edits `home.js` and must follow Task 10.

### Task 11: Search ranking

**Files:**
- Create: `apps/clax_app/home/search.js`
- Create: `apps/clax_app/home_unit/search.test.js`

**Interfaces:**
- Produces:
  ```js
  export function fold(s: string): string;   // NFD, marks removed, lower case
  export function search(query: string, entries: {app: App, group: string|null}[]): {app, group}[];
  ```
  Matches where a word of the title starts with the query come first, then those containing it anywhere, each in `entries`' order. An empty or blank query matches nothing.

- [ ] **Step 1: Write the failing test** `apps/clax_app/home_unit/search.test.js`:

```js
import { describe, expect, test } from 'vitest';
import { fold, search } from '../home/search.js';

const e = (title, group = null) => ({ app: { id: title, title }, group });

describe('search', () => {
  const all = [e('Run tracker'), e('Groceries', 'Lists'), e('Café budget'), e('Brunch spots'), e('Habit streaks'), e('Strength')];

  test('folds case and accents', () => {
    expect(fold('Café ÉCLAIR')).toBe('cafe eclair');
  });

  test('word starts first, then anywhere, each in home order', () => {
    expect(search('run', all).map((x) => x.app.title)).toEqual(['Run tracker', 'Brunch spots']);
    expect(search('str', all).map((x) => x.app.title)).toEqual(['Habit streaks', 'Strength']);
    expect(search('CAFE', all).map((x) => x.app.title)).toEqual(['Café budget']);
    expect(search('ries', all).map((x) => x.app.title)).toEqual(['Groceries']);
  });

  test("keeps the group an app is in, and matches a whole phrase", () => {
    expect(search('groc', all)).toEqual([e('Groceries', 'Lists')]);
    expect(search('run tr', all).map((x) => x.app.title)).toEqual(['Run tracker']);
  });

  test('a blank query matches nothing', () => {
    expect(search('', all)).toEqual([]);
    expect(search('   ', all)).toEqual([]);
  });
});
```

(`'str'` puts "Habit streaks" first: both start a word, so home order decides.)

- [ ] **Step 2: Run it to verify it fails**

Run: `pnpm --dir web exec vitest run home_unit/search`
Expected: FAIL: module missing.

- [ ] **Step 3: Implement `search.js`**:

```js
// Finding an app by its title: case- and accent-insensitive, matches where
// a word of the title starts with the query before those that contain it
// anywhere, each kept in the home screen's order.

/** `s` without case or accents. */
export const fold = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}+/gu, '').toLowerCase();

const LETTER = /[\p{L}\p{N}]/u;

const atWordStart = (title, q) => {
  for (let i = title.indexOf(q); i >= 0; i = title.indexOf(q, i + 1)) {
    if (i === 0 || !LETTER.test(title[i - 1])) return true;
  }
  return false;
};

/** The entries ({app, group}, in the home screen's order: Starred, then
 * All apps with a group's apps at its place) whose app's title matches
 * `query`. */
export function search(query, entries) {
  const q = fold(query).trim();
  if (!q) return [];
  const starts = [];
  const inside = [];
  for (const entry of entries) {
    const title = fold(entry.app.title);
    if (atWordStart(title, q)) starts.push(entry);
    else if (title.includes(q)) inside.push(entry);
  }
  return [...starts, ...inside];
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `pnpm --dir web exec vitest run home_unit/search`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/clax_app/home/search.js apps/clax_app/home_unit/search.test.js
git -c commit.gpgsign=false commit -m "Add the home screen's search ranking"
```

### Task 12: The search field, and release 1.4.0

**Files:**
- Modify: `apps/clax_app/home/home.js` — the ⌕ action; pull-down opens search (`usePull` becomes `usePullSearch`; `PullRing` goes); the field; the flat results; Enter, Escape, Backspace, back; `tiles()` during search
- Modify: `apps/clax_app/home/home.css` — the field; the group caption; the pull's reveal; the pull ring's rules go
- Modify: `apps/clax_app/home/tests/home.test.js` — 'a pull down from the top loads everything again' is replaced; new tests below
- Modify: `web/e2e/home-app.spec.ts`
- Modify: `apps/clax_app/home/manifest.json` (`11`, `"1.4.0"`); regenerate `assets/home/home.clax`, `release.json`

**Interfaces:**
- Consumes: Task 11's `search`, `fold`; Task 3's `sections`, `openApp`.
- Produces (DOM): a top bar button `[aria-label="Search"]` first in `.h-actions`; `.h-search` holding `<input type="search" aria-label="Search apps" enterkeyhint="go">` above the sections; while it has text, `.h-results` (one `.h-grid` of `.h-tile[data-id]`, each in a group with `.h-in-group` text, its group's name, under the label) replaces the sections; no match shows "No apps match “<query>”". Opening search pushes `#search`. `tiles()` reports the results while search shows them.

- [ ] **Step 1: Write the failing harness tests** (replacing the pull-to-refresh test; 'Refresh in More loads everything again' stays):

```js
test('the search icon opens a focused field; typing filters every app, groups too; Enter opens the first', async () => {
  const clock = manualClock(0, Date.UTC(2026, 9, 16, 12));
  const home = await show({ makes: [] }, { clock, layout: groupLayout() });
  await ui.click('[aria-label="Search"]');
  await wait(() => ui.exists('.h-search input'));
  const input = document.querySelector('.h-search input');
  expect(document.activeElement).toBe(input);
  input.value = 'NOT';
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await wait(() => ui.exists('.h-results'));
  expect([...document.querySelectorAll('.h-results .h-tile[data-id]')].map((t) => t.dataset.id)).toEqual(['app-notes']);
  expect(ui.text('.h-results [data-id="app-notes"] .h-in-group')).toBe('Lists');
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await wait(() => home.calls.some((c) => c.method === 'open'));
  expect(home.calls.find((c) => c.method === 'open').args).toEqual(['app-notes']);
  await wait(async () => (await stored()).opens['app-notes']?.n === 1);
});

test('no match says so; Escape closes search; Backspace in an empty field closes it; back closes it', async () => {
  await show({ makes: [] });
  const open = async () => { await ui.click('[aria-label="Search"]'); await wait(() => ui.exists('.h-search input')); return document.querySelector('.h-search input'); };
  let input = await open();
  input.value = 'zzz';
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await wait(() => ui.text('.h-results').includes('No apps match “zzz”'));
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await wait(() => !ui.exists('.h-search'));
  expect(ui.exists('.h-section[data-section="all"]')).toBeTruthy();
  input = await open();
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true }));
  await wait(() => !ui.exists('.h-search'));
  await open();
  history.back();
  await wait(() => !ui.exists('.h-search'));
});

test('a pull down from the top past 64 px opens search, and a short one does not; nothing reloads', async () => {
  if (typeof Touch !== 'function') return;
  const home = await show({ makes: [] });
  let loads = 0;
  const apps = home.apps;
  home.apps = () => { loads++; return apps(); };
  scrollTo(0, 0);
  const touch = (y) => new Touch({ identifier: 1, target: document.body, clientX: 200, clientY: y });
  const send = (type, y) => document.body.dispatchEvent(new TouchEvent(type, { bubbles: true, cancelable: true, touches: type === 'touchend' ? [] : [touch(y)], changedTouches: [touch(y)] }));
  send('touchstart', 100); send('touchmove', 140); send('touchend', 140);
  await new Promise((r) => setTimeout(r, 0));
  expect(ui.exists('.h-search')).toBe(false);
  send('touchstart', 100); send('touchmove', 160); send('touchmove', 260); send('touchend', 260);
  await wait(() => ui.exists('.h-search input'));
  expect(loads).toBe(0);
  expect(ui.exists('.h-pull')).toBe(false);
});

test('while search shows results, tiles() gives them', async () => {
  const home = fakeHome({ makes: [] });
  const reports = [];
  home.tiles = async (list) => { reports.push(list.map((t) => t.id)); };
  db = fakeDb();
  mount(root(), home, { db });
  await wait(() => reports.length >= 1);
  await ui.click('[aria-label="Search"]');
  const input = document.querySelector('.h-search input');
  input.value = 'gro';
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await wait(() => reports.at(-1).join() === 'app-groceries');
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: FAIL: no Search action, the pull still refreshes.

- [ ] **Step 3: Implement.** In `Bar`, a first action `<button class="h-action" aria-label="Search" onClick=${openSearch}><${Glyph} name="search" /></button>`. `usePull(refresh)` becomes `usePullSearch(onOpen)`: the same touch tracking (`PULL_PX` 64, `PULL_MAX` 96, only from `scrollY <= 0`), but while pulling it sets a `--pull` CSS variable (0 to 1) that slides a ghost of the field down under the bar, and a release past `PULL_PX` calls `onOpen()`; `PullRing` and the busy state go. `openSearch()` sets `query = ''`, `searching = true`, pushes `history.pushState({ clax: 'search' }, '', '#search')`, and focuses the input after render (`useLayoutEffect`). A `popstate` while searching closes it; `closeSearch()` (Escape; Backspace with an empty value; the field's clear) calls `history.back()` when the top entry is `#search`, which then closes it, so the app's back and the page's own close agree. `entries` for `search` is Starred's apps (`group: null`), then All apps with each group's apps at its place (`group: name`). With a query, `.h-results` replaces the sections: `pack` over the results (1×1), each an `AppTile` with `.h-in-group` (its group's name) under the label when `group`; no results → `<p class="h-results-empty">No apps match “${query}”</p>`. Enter opens `results[0]` through `openApp` (which counts the open). Drag is off in the results (the press's hold-release still opens the menu). `useTiles` queries `.h-results .h-tile[data-id]` while results show. "Refresh" stays in More.

- [ ] **Step 4: Styles.** `.h-search { padding: 4px 16px 8px; }`; its input: height 48px, radius 24px, `var(--h-sc)` background, `var(--h-on-sc)` text, 16px font (no zoom on focus on Android), a leading search glyph; `.h-in-group` like `.h-status`; `.h-results-empty` like `.h-message-hint`, centred, 48px top margin. Remove the `.h-pull*` rules; keep `html { overscroll-behavior-y: contain; }`.

- [ ] **Step 5: Browser test and renders.** In "over a fake host": tap `[aria-label="Search"]`, `page.keyboard.type("gro")`, expect `.h-results .h-tile[data-id]` count 1, press Enter, expect the fake's calls to end with `{ method: "open", args: ["app-groceries"] }`; a CDP touch pull (start at y 120, move to 300 in steps, end) opens `.h-search`. Render `home-search-${size}-${scheme}.png` with the query "re" over a layout with a group (results from Starred, All apps and the group, one with its group caption). Update `expect(tests.passed)`.

- [ ] **Step 6: Run the tests and look at the renders** (Bazel, then native renders). In `home-search-*` the field sits under the bar, aligned to the grid's gutter; results keep tile size; the group caption reads as secondary text; dark mode's field contrasts with the surface.

- [ ] **Step 7: Pack release 1.4.0** (`"version": 11`, `"release": "1.4.0"`) as Task 3 Step 9; `bazelisk test //apps/clax_app:home_home_bundle_test` (PASS).

- [ ] **Step 8: Commit**

```bash
git add apps/clax_app/home apps/clax_app/assets/home web/e2e/home-app.spec.ts
git -c commit.gpgsign=false commit -m "Find home apps by pulling down or the search icon, instead of pull-to-refresh; home release 1.4.0"
```

---

## Phase 5 — Grow with use (release 1.5.0)

Task 13 is pure and edits `arrange.js` and `home_unit/arrange.test.js` (shared with Tasks 2 and 8, which are done by then). Task 14 follows it.

### Task 13: Which apps grow

**Files:**
- Modify: `apps/clax_app/home/arrange.js` — `BIG_TOP`, `BIG_MIN`, `bigApps`, `setBigTiles`
- Modify: `apps/clax_app/home_unit/arrange.test.js`

**Interfaces:**
- Produces: `export const BIG_TOP = 3, BIG_MIN = 5;` `export function bigApps(layout, now: Date): Set<string>` (the at most BIG_TOP apps under All apps, outside groups and Starred, with at least BIG_MIN opens in the last OPEN_DAYS UTC days, most first, ties to the later `last`, then the ID; empty when `layout.bigTiles === false`); `export function setBigTiles(layout, on: boolean): Layout`.

- [ ] **Step 1: Write the failing tests**:

```js
import { bigApps, setBigTiles } from '../home/arrange.js';

describe('bigApps', () => {
  const now = new Date('2026-10-16T12:00:00Z');
  const opensOf = (n, day = '2026-10-16', last = `${day}T10:00:00.000Z`) => ({ n, last, days: { [day]: n } });
  const apps = ['a', 'b', 'c', 'd', 'e', 's', 'g1', 'g2'].map((id) => app(id));
  const layout = (extra = {}) => reconcile({
    starred: ['s'], order: ['a', 'b', 'c', 'd', 'e', 'g:x'], groups: { x: { name: 'X', apps: ['g1', 'g2'] } }, seen: [],
    opens: { a: opensOf(9), b: opensOf(7), c: opensOf(5, '2026-10-03'), d: opensOf(12, '2026-10-02'), e: opensOf(4), s: opensOf(30), g1: opensOf(30) },
    ...extra,
  }, apps);

  test('the three opened most in the last 14 days, at least 5 opens each, outside Starred and groups', () => {
    expect([...bigApps(layout(), now)].sort()).toEqual(['a', 'b', 'c']);
  });

  test('opens older than 14 days do not count', () => {
    expect(bigApps(layout(), new Date('2026-10-17T00:30:00Z')).has('c')).toBe(false);
  });

  test('ties go to the later open, then the ID; only three grow', () => {
    const l = layout({ opens: { a: opensOf(6), b: opensOf(6, '2026-10-16', '2026-10-16T11:00:00.000Z'), c: opensOf(6), d: opensOf(6) } });
    expect([...bigApps(l, now)]).toEqual(['b', 'a', 'c']);
  });

  test('turned off, nothing grows; the setting is kept', () => {
    const off = setBigTiles(layout(), false);
    expect(off.bigTiles).toBe(false);
    expect(bigApps(off, now).size).toBe(0);
    expect(bigApps(setBigTiles(off, true), now).size).toBe(3);
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm --dir web exec vitest run home_unit/arrange`
Expected: FAIL: `bigApps` is not exported.

- [ ] **Step 3: Implement** in `arrange.js`:

```js
/** How many apps under All apps grow to a 2×2 tile, and the opens in the
 * last OPEN_DAYS days each needs. */
export const BIG_TOP = 3;
export const BIG_MIN = 5;

/** The apps under All apps (not in Starred or a group) that take a 2×2
 * cell at `now`: the BIG_TOP opened most from the home screen in the last
 * OPEN_DAYS UTC days, with at least BIG_MIN opens each, most first, ties
 * to the one opened later, then by ID. None when `bigTiles` is false. */
export function bigApps(layout, now) {
  const l = normalize(layout);
  if (l.bigTiles === false) return new Set();
  const since = daysBefore(now, OPEN_DAYS - 1);
  const loose = new Set(l.order.filter((k) => !isGroupKey(k)));
  const ranked = Object.entries(l.opens)
    .filter(([id]) => loose.has(id))
    .map(([id, o]) => ({ id, n: Object.entries(o.days).reduce((sum, [d, c]) => (d >= since ? sum + c : sum), 0), last: o.last ?? '' }))
    .filter((r) => r.n >= BIG_MIN)
    .sort((a, b) => b.n - a.n || b.last.localeCompare(a.last) || a.id.localeCompare(b.id));
  return new Set(ranked.slice(0, BIG_TOP).map((r) => r.id));
}

/** Big tiles on or off (`bigTiles`, true when absent). */
export const setBigTiles = (layout, on) => ({ ...normalize(layout), bigTiles: !!on });
```

Add `bigTiles` (absent means true) to the header comment's document shape.

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm --dir web exec vitest run home_unit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/clax_app/home/arrange.js apps/clax_app/home_unit/arrange.test.js
git -c commit.gpgsign=false commit -m "Choose the home apps that grow to big tiles from their opens in the last 14 days"
```

### Task 14: Big tiles in the home app; release 1.5.0; the branch's checks

**Files:**
- Modify: `apps/clax_app/home/home.js` — span 2 in `pack` for `bigApps`; the big face; the ⋮ item; `tiles()` gives the 2×2 rect
- Modify: `apps/clax_app/home/home.css`
- Modify: `apps/clax_app/home/tests/home.test.js`, `web/e2e/home-app.spec.ts`
- Modify: `apps/clax_app/assets/prompt/system.md` (one line: `bigTiles`)
- Modify: `apps/clax_app/home/manifest.json` (`12`, `"1.5.0"`); regenerate `assets/home/home.clax`, `release.json`

**Interfaces:**
- Consumes: Task 13's `bigApps`, `setBigTiles`; Task 5's `pack` with `span: 2`; Task 3's `opened`.
- Produces (DOM): a grown tile `.h-tile[data-id][data-big]` spanning two columns and two rows, its face 120×120 (`.h-face` at `--face: 120px`); the More menu's `[role="menuitemcheckbox"][aria-checked]` "Big tiles for apps you use". `tiles()` reports a big tile's whole rect.

- [ ] **Step 1: Write the failing harness tests**:

```js
const busy = () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const o = (n) => ({ n, last: '2026-10-16T10:00:00.000Z', days: { '2026-10-16': n } });
  return { v: 1, starred: [], order: ids, seen: ids, opens: { 'app-budget': o(9), 'app-mood': o(6), 'app-plants': o(5), 'app-notes': o(4) } };
};

test('the three apps opened most take 2×2 tiles with a larger icon; the order is kept as cells allow', async () => {
  const clock = manualClock(0, Date.UTC(2026, 9, 16, 12));
  await show({ makes: [] }, { clock, layout: busy() });
  await wait(() => document.querySelectorAll('[data-big]').length === 3);
  expect([...document.querySelectorAll('[data-big]')].map((t) => t.dataset.id).sort()).toEqual(['app-budget', 'app-mood', 'app-plants']);
  const big = document.querySelector('[data-id="app-budget"]').getBoundingClientRect();
  const small = document.querySelector('[data-id="app-run"]').getBoundingClientRect();
  expect(big.width).toBeGreaterThan(small.width * 2);
  expect(big.height).toBeGreaterThan(small.height * 2);
  expect(document.querySelector('[data-id="app-budget"] .h-face').getBoundingClientRect().width).toBe(120);
  expect(section('all')).toEqual(SAMPLE.apps.map((a) => a.id));
});

test('Big tiles for apps you use, in More, turns them off and on, and is kept', async () => {
  const clock = manualClock(0, Date.UTC(2026, 9, 16, 12));
  await show({ makes: [] }, { clock, layout: busy() });
  await wait(() => document.querySelectorAll('[data-big]').length === 3);
  await ui.click('[aria-label="More"]');
  const item = () => document.querySelector('[role="menuitemcheckbox"]');
  expect(item().textContent).toContain('Big tiles for apps you use');
  expect(item().getAttribute('aria-checked')).toBe('true');
  item().click();
  await wait(() => document.querySelectorAll('[data-big]').length === 0);
  expect((await stored()).bigTiles).toBe(false);
});

test("tiles() gives a big tile's whole rect", async () => {
  const home = fakeHome({ makes: [] });
  const reports = [];
  home.tiles = async (list) => { reports.push(list); };
  db = fakeDb({ [LAYOUT]: busy() });
  mount(root(), home, { db, clock: manualClock(0, Date.UTC(2026, 9, 16, 12)) });
  await wait(() => reports.at(-1)?.length === SAMPLE.apps.length && document.querySelectorAll('[data-big]').length === 3);
  await wait(() => {
    const r = document.querySelector('[data-id="app-budget"]').getBoundingClientRect();
    const t = reports.at(-1).find((x) => x.id === 'app-budget');
    return t.rect.w === r.width && t.rect.h === r.height;
  });
});

test('a starred app stays 1×1 however often it is opened', async () => {
  const l = busy();
  await show({ makes: [] }, { clock: manualClock(0, Date.UTC(2026, 9, 16, 12)), layout: { ...l, starred: ['app-budget'], order: l.order.filter((id) => id !== 'app-budget') } });
  await wait(() => section('starred').length === 1);
  expect(ui.exists('[data-id="app-budget"][data-big]')).toBe(false);
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `bazelisk test //web:e2e_rest --test_arg=e2e/home-app.spec.ts --test_output=errors`
Expected: FAIL: nothing grows.

- [ ] **Step 3: Implement.** In `Sections`, `const big = bigApps(layout, clock.date())` and All apps' items take `span: big.has(e.key) ? 2 : 1`; Starred's stay 1; groups and makes stay 1. `AppTile` with `big` sets `data-big`, and its face draws at 120 px (`style="--face:120px"`; `AppFace`'s `<img>` gets `width`/`height` 120; emoji 56px; monogram 44px; word glyph 56px with stroke 2). The FLIP pass springs a tile that grows or shrinks from its old box (the transform carries position; the size change is immediate). In `More`, after Refresh:

```js
        <button type="button" role="menuitemcheckbox" aria-checked=${bigOn} class="h-menu-item h-menu-check" onClick=${() => { setOpen(false); onBigTiles(!bigOn); }}>
          <span>Big tiles for apps you use</span>${bigOn && html`<${Glyph} name="check" size=${20} />`}
        </button>
```

with `onBigTiles = (on) => change((l) => setBigTiles(l, on))` and `bigOn = layout?.bigTiles !== false`. `useTiles` already reads each tile's `getBoundingClientRect()`, which is the 2×2 box. In `system.md`'s home section add: "`bigTiles: false` (in `layout/main`) turns off the 2×2 tiles the shipped home app gives the three apps opened most in the last 14 days (release 1.5.0)." Run `bazelisk test //apps/clax_app:agent_prompt_test`.

- [ ] **Step 4: Styles.** `.h-face { width: var(--face, 60px); height: var(--face, 60px); border-radius: calc(var(--face, 60px) / 4); }` with `.h-face img` following `--face`; `.h-tile[data-big] { height: auto; justify-content: center; }`, `.h-tile[data-big] .h-icon { width: 120px; height: 120px; }`; `.h-menu-check { display: flex; align-items: center; justify-content: space-between; gap: 16px; }`. The Ring over a building big tile stays 36px, centred.

- [ ] **Step 5: Renders.** Add `home-big-${size}-${scheme}.png` (the `busy()` layout plus one starred app and one folder). Update `expect(tests.passed)`.

- [ ] **Step 6: Run the tests and look at the renders.** Bazel run, then the native renders. Read every render the branch adds, at both sizes and in both themes: `home-starred-*`, `home-drag-*`, `home-groups-*`, `home-folder-*`, `home-pile-*`, `home-search-*`, `home-big-*`, plus the unchanged `home-*`, `home-states-*`, `home-empty-*`. In `home-big-*` the 2×2 tiles sit on whole cells, the 1×1 tiles fill the gaps beside them in order, the larger icons are sharp (no upscaled 60 px image: `{url}` icons are SVG), and the labels stay one size.

- [ ] **Step 7: Pack release 1.5.0** (`"version": 12`, `"release": "1.5.0"`) as Task 3 Step 9.

- [ ] **Step 8: The branch's checks.** In the background, logged in the worktree:

```bash
scripts/quality_gates.sh > target/quality-gates.log 2>&1; echo "exit $?" >> target/quality-gates.log
```

Wait on it in bounded stretches (`tail -n 5 target/quality-gates.log`). Expected: `exit 0`. A lane that fails in code this branch did not touch is rerun alone and both results reported. Then:

```bash
Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"
"$Q" exec diff main
"$Q" exec review
```

Record, in one `--stdin` batch (dry run first), anything the next person needs: the lease-based write (decision 1) as a `waiver` on `apps/clax_app/home/store.js`, the `days` field (decision 2) as an `alternative` with `revisit:` on `apps/clax_app/home/arrange.js`'s `opened`, and the phone's frame numbers or the open phone check from Task 7 Step 8.

- [ ] **Step 9: Commit**

```bash
git add apps/clax_app/home apps/clax_app/assets/home apps/clax_app/assets/prompt/system.md web/e2e/home-app.spec.ts
git -c commit.gpgsign=false commit -m "Grow the three home apps opened most into big tiles, with a switch in More; home release 1.5.0"
git add -A '*.qual'
git -c commit.gpgsign=false commit -m "Record the arrangement's lease-based writes, its open counts and the phone's drag check"
```

---

## Spec coverage

| Spec | Tasks |
|---|---|
| Owner's decisions: shipped in Clax; Star in the menu; Starred above All apps; snapping grid; one kind of group, two looks; New first; search | 3, 1; 3; 7; 9, 10; 3; 12 |
| §1 Layout, sections hide when empty, newest first unarranged, 2×2 cells | 3, 14 |
| §2 `layout/main`, reconcile, `seen`, whole writes on conflict, survives Undo/Reset/updates, opens from home only | 2, 3 (decisions 1, 2, 5, 6) |
| §3 `menu(id, rect, {items})`, Star/Unstar, Remove from group, the group's own menu, contract/types/prompt/skills/fake | 1, 2, 3, 9, 4 |
| §4 hold 400 ms, 8 px drag, release → menu; neighbours slide; sections; hover 600 ms groups; drop on group; edge scroll; toss; springs; reduced motion; right click, mouse drag, Alt+arrows | 6, 7, 9 |
| §5 folder look, pile look (seeded tilt, fan-out), switch look, no nesting, search shows group name | 8, 9, 10, 12 |
| §6 grow with use, packing, the ⋮ switch | 5, 13, 14 |
| §7 search: pull past 64 px, ⌕, filter, Enter, Escape/back, pull-to-refresh goes | 11, 12 |
| §8 `tiles()` (2×2 rect, groups not reported, snapshot retaken on change) | 7, 9, 10, 12, 14 |
| §9 the agent prompt | 4, 9, 14 |
| §10 vitest, Playwright, Dart, renders, phone | every task; 7 Step 8 |
| §11 phases and releases 1.1.0 onward | 3, 7, 10, 12, 14 |
