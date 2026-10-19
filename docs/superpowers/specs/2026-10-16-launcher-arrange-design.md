# Arranging the home screen: stars, groups, drag, search

Date: 2026-10-16. Branch: `flutter-app`. Status: draft for the owner's review.
Builds on `2026-10-16-home-screen-app-design.md`.

## Goal

The person arranges their home screen by hand: they star the apps they use
most, which stay on top; they drag apps into an order that sticks; they
gather apps into groups that show as a folder or as a BumpTop pile; they
find any app by pulling down to search. Moving things feels physical:
tiles slide, bump their neighbours aside and settle with a little bounce.
Apps opened often grow into larger tiles.

All of it ships in the home app Clax bundles (`apps/clax_app/home/`), so
every unmodified home screen gets it on update. A person who changed their
home screen by commenting is asked, as for any home release.

## Owner's decisions

- Shipped in Clax, not left to commenting.
- Starring is in the long-press menu (Star / Unstar beside Open, Inspect,
  Share app).
- Starred apps sit in a **Starred** section above **All apps**, only there,
  in an order the person drags.
- All apps is a **snapping grid**: dragging reorders it; groups take a
  cell like an app. Physics is the feel, not free placement.
- **One kind of group, two looks**: a named group shows as a folder (tap
  opens it over the grid) or as a pile (tap fans it out in place); the
  person switches between them.
- **New apps** go first under All apps, marked "New" until opened.
- **Search**: pull the grid down, or tap a search icon in the top bar; a
  field filters apps by name, inside groups too; Enter opens the first
  match.

## 1. Layout

```
Clax                    ⌕ ⌂ ⚙ ⋮
Starred
[Runs] [Notes]
All apps
[New·Map] [Groc]   [Budget]
[▢ Tools] [≡ Fun]  [Timer]
```

- Sections hide when empty: no stars, no Starred heading. With nothing
  arranged, All apps is newest first, as today.
- A cell holds an app or a group. A frequently opened app may take a
  2×2 cell (§6).

## 2. Where the arrangement lives

In the home app's own documents (it already declares `db`), one document
`layout/main`:

```json
{
  "v": 1,
  "starred": ["appId", "…"],
  "order": ["appId", "g:groupId", "…"],
  "groups": {"groupId": {"name": "Tools", "look": "folder" | "pile", "apps": ["appId"]}},
  "seen": ["appId"],
  "opens": {"appId": {"n": 12, "last": "2026-10-16T…"}}
}
```

- Reconciled with `apps()` on every load and `apps` event: IDs no longer
  listed are dropped (a group left with one app dissolves into it, with
  none it goes); listed apps in no place are put first in `order`, unseen.
- `seen` drives the "New" mark: an app is new until opened from home.
- Written whole on each change, pinned to the version read; on a conflict
  re-read, re-apply, write again.
- Documents survive Undo, Reset and updates of the home app
  (`2026-10-16-home-screen-app-design.md` §6), so the arrangement does.
- `opens` counts opens from the home screen only (the home app sees its
  own `open` calls); no new capability.

## 3. The long-press menu gains the home app's items

Today `menu(id, rect)` shows Clax's menu (Open, Inspect, Share app, Resume
build; `apps/clax_app/lib/ui/launcher.dart`, reached through
`HomeHost` at `apps/clax_app/lib/home/home_host.dart:394`).

- `menu(id, rect, {items})`: `items` is up to 4 `{id, label}` the page
  adds, shown first, above a divider. It resolves with the chosen item's
  `id`, or `null` for Clax's own items or a dismissal. Labels are plain
  text, at most 32 characters; `bad_request` otherwise.
- The home app passes **Star** or **Unstar**, and, for an app in a group,
  **Remove from group**.
- A group's own menu (Rename, Show as pile / Show as folder, Ungroup) is
  drawn by the home app, since Clax has no group to act on.
- Contract, types, the agent prompt copies, the plugin skill copies and the
  kit's fake host change with it (`docs/contract.md`,
  `web/contract/clax-extensions.d.ts`, `scripts/sync-app-prompt.sh`,
  `kit/fake-home.js`).

## 4. Holding and dragging

The menu is Clax's own overlay, so the page can't keep a drag going under
it. The press therefore decides first:

1. Press and hold 400 ms: the tile lifts (scales up, shadow).
2. Moving more than 8 px while lifted starts a **drag**; no menu.
3. Releasing without moving opens the **menu** (on release, as on iOS).

While dragging:

- Neighbours slide aside to show where the tile will land (snapping to
  cells); dropping between sections moves it (into Starred stars it, out
  unstars it).
- Hovering 600 ms over another app's centre shows a group outline; dropping
  there makes a group of the two (named from their common icon word, else
  "Group"), shown as a folder.
- Dropping on a group adds the app to it.
- Near the top or bottom edge the grid scrolls.
- A quick release ("toss") carries the tile on with its velocity, decaying,
  and it snaps to the nearest free cell; tiles it passes are bumped aside
  briefly and settle back.

Physics is a small spring model in `home.js` (no library): each tile has a
position and velocity, springs to its cell, settles with one overshoot.
With `prefers-reduced-motion` tiles move without springs or toss.

Mouse and keyboard: right click opens the menu at once (as today); a
mouse drag starts at once without the hold; on a focused tile,
Alt+arrows move it one cell.

## 5. Groups

> Changed by the owner on 2026-10-19: one look, a folder tile showing its first four apps' icons two by two, that fans out in place like the pile did (no folder over the grid, no Show as pile / Show as folder); a drop on an app or a group flies the app into the preview, a new group is named from its apps' icon-word category (else "Group") and its name is offered for editing at once; a stored `look` is kept and ignored.

- **Folder look**: a tile showing up to four small icons and the name. A
  tap opens the group over a dimmed grid: its name (tap to rename), its
  apps in a grid, drag inside to reorder, drag out to the edge to take an
  app out.
- **Pile look**: up to five icons stacked with small random rotations
  (seeded by the group ID, so it looks the same each time), the name under
  it. A tap fans the pile out in place into a row that scrolls sideways,
  pushing the grid down with springs; a tap outside gathers it back.
- The look is switched from the group's menu; the apps don't change.
- A group never holds another group. Search finds apps in groups and shows
  the group's name under the match.

## 6. Grow with use

> Removed by the owner on 2026-10-19: there are no big tiles; every tile is
> one cell, and `bigTiles` and `big` in a stored `layout/main` are ignored.

- Under All apps, the three apps opened most in the last 14 days, with at
  least 5 opens each, take a 2×2 cell with a larger icon; the rest stay
  1×1. Starred apps keep 1×1 (Starred is already their place).
- Grid packing fills gaps left by 2×2 cells with the next 1×1 items in
  order, so the order is kept as nearly as cells allow.
- Turned off by an item in the ⋮ menu (**Big tiles for apps you use**,
  on by default), kept in `layout/main`.

## 7. Search

- Pulling down from the top of the grid past 64 px, or the ⌕ in the top
  bar, shows a field above Starred and focuses it.
- Typing filters every app by title, case- and accent-insensitive,
  matching word starts first, then anywhere; the matches show as one flat
  grid. Enter opens the first; Escape or an empty field with back closes
  search.
- Pull-to-refresh goes: Refresh stays in the ⋮ menu, and the list already
  follows `apps` events.

## 8. The start-up picture and tile reports

`tiles()` keeps reporting the visible apps' rectangles (`2026-10-16-home-
screen-app-design.md` §4), including a 2×2 tile's larger rect. A tap on the
picture that lands on a group does nothing; one on an app opens it as
today. The snapshot is retaken when the arrangement changes, as for any
change to home.

## 9. The agent

The home prompt section gets the layout document's shape and one rule:
keep `layout/main` readable by the shipped home app (add fields, never
change the meaning of existing ones), so stepping back a version never
loses the arrangement.

## 10. Testing

- Pure functions in `home.js` (reconcile, pack, search ranking, group
  rules, spring step) under vitest, with an injected clock.
- Home Playwright with the fake host: star from the menu, drag to reorder
  and to Starred, make a group by hovering, pile fan-out, search by pull and
  by icon, a new app's mark, the arrangement surviving a reload.
- Dart: `menu` with items resolves the chosen ID; bad items refused.
- Renders at phone and desktop sizes, light and dark, looked at: Starred
  plus groups both looks, a fanned pile, search, a 2×2 tile, a drag midway.
- On the phone: a drag and a toss feel smooth (no dropped frames visible in
  a screen recording).

## 11. Phases

1. `layout/main`, Starred, New marks, menu items (Flutter and contract).
2. Hold, drag, snap, physics and toss.
3. Groups, both looks.
4. Search; pull-to-refresh removed.
5. Grow with use.

Each phase ships a home bundle release (1.1.0 onward).

## Decisions taken in this draft (owner may override)

- Hold-then-release opens the menu; hold-then-move drags (the menu can't
  coexist with a page drag).
- Pull-down becomes search; refresh stays only in the ⋮ menu.
- Groups don't nest and can't be starred.
- New groups start as folders.
- Big tiles: top three apps, at least 5 opens in 14 days, on by default.
- Opens are counted only from the home screen.
