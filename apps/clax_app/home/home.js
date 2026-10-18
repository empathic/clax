// Clax's home screen: the person's apps and the makes going on, as a grid,
// with Inbox, Settings and Import app… above it and New app below. Every
// list and every action goes through the Clax app's `home` capability
// (`await claude.use("home")`), which only the app recorded as the home
// screen is granted; anywhere else the page says to open it in Clax.
//
// The arrangement (Starred, the order of All apps, which apps are new, how
// often each is opened) is the `layout/main` document in the page's own
// `db` (arrange.js, store.js). The apps under All apps opened most lately
// (arrange.js's `bigApps`) take a 2×2 cell with a 120 px icon, unless Big
// tiles for apps you use, in the ⋮ menu, is off.
//
// Each tile is placed in its cell (grid.js's `pack`) and moved by
// motion.js's springs on the page's clock: a held tile lifts and can be
// dragged across Starred and All apps, its neighbours making room; a quick
// release tosses it on (physics.js); Alt+arrows move a focused tile.
// Held 600 ms over another app's centre, a dragged app makes a group of
// the two; dropped on a group, it joins it. A group shows as a folder tile
// that opens over the grid, where its apps are renamed, reordered and
// dragged out, or as a pile of tilted icons that fans out in place into a
// row below it, pushing the rows after it down; its own menu renames,
// switches its look and ungroups it.
//
// The ⌕ in the top bar, or a pull down from the top of the page, opens a
// search field above the grid (search.js ranks the matches): while it has
// text, every app whose title matches, groups' included, shows in one
// grid in their place. Enter opens the first; Escape, Backspace in the
// empty field, its close button and back close it.
//
// `start()` mounts the page over the real capabilities; `mount(root, home,
// {db, clock})` mounts it over any objects with the same calls (the tests
// pass kit/fake-home.js's fakeHome and fakeDb).
import { html, render, useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from './kit/preact.js';
import { icon as kitIcon } from './kit/icons.js';
import {
  reconcile, normalize, sections, star, unstar, opened, isNew, moveTo, isGroupKey, groupOf,
  groupNameOf, makeGroup, addToGroup, removeFromGroup, moveInGroup, renameGroup, setLook, ungroup,
  bigApps, setBigTiles,
} from './arrange.js';
import { layoutStore } from './store.js';
import { realClock, velocityOf, tossRest, TOSS_MIN } from './physics.js';
import { pack, geometry, cellBox, slotAt, centreUnder, tiltOf } from './grid.js';
import { createPress } from './drag.js';
import { createMotion } from './motion.js';
import { search } from './search.js';

// Lucide glyphs the kit does not carry (lucide-static 0.460.0, ISC; see
// kit/LICENSES.txt).
const EXTRA = {
  hourglass: '<path d="M5 22h14"/><path d="M5 2h14"/><path d="M17 22v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22"/><path d="M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2"/>',
  cloudOff: '<path d="m2 2 20 20"/><path d="M5.782 5.782A7 7 0 0 0 9 19h8.5a4.5 4.5 0 0 0 1.307-.193"/><path d="M21.532 16.5A4.5 4.5 0 0 0 17.5 10h-1.79A7.008 7.008 0 0 0 10 5.07"/>',
  rotateCcw: '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
  alertCircle: '<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>',
  key: '<path d="M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"/><circle cx="16.5" cy="7.5" r=".5" fill="currentColor"/>',
};

function svg(name, size = 24, stroke = 2) {
  const extra = EXTRA[name];
  if (extra === undefined) return kitIcon(name, { size, stroke });
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${stroke}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${extra}</svg>`;
}

const Glyph = ({ name, size = 24, stroke = 2, class: cls }) =>
  html`<span class=${`h-glyph ${cls || ''}`} dangerouslySetInnerHTML=${{ __html: svg(name, size, stroke) }} />`;

// ---------------------------------------------------------------------------
// Tile colours.

// The tile colours of the Clax app's launcher (Material 3 tonal schemes of a
// seed of HSL hue 0°, 10°, … 350°, saturation and lightness 50%): each pair
// is [light container, dark container]; light mode draws the first with the
// second as its foreground, dark mode the reverse.
const TONES = [
  ['#ffdad7', '#733331'], ['#ffdad3', '#733428'], ['#ffdbcc', '#713719'], ['#ffdcbf', '#6a3b01'], ['#ffdea5', '#5d4200'], ['#f7e388', '#524600'],
  ['#e8e78f', '#494900'], ['#dcea97', '#414b08'], ['#d2ec9f', '#394d11'], ['#cbeea5', '#334e17'], ['#c5efab', '#2d4f1c'], ['#c1efaf', '#294f20'],
  ['#bef0b2', '#265022'], ['#baf0b6', '#215026'], ['#b5f1bd', '#19512a'], ['#aef2c6', '#085231'], ['#a6f2d2', '#00513b'], ['#9ff2e0', '#005046'],
  ['#9cf1f0', '#004f50'], ['#a6eeff', '#004e5b'], ['#c4e7ff', '#004c69'], ['#d1e4ff', '#184975'], ['#d9e2ff', '#2e4578'], ['#dee0ff', '#394379'],
  ['#e1dfff', '#404178'], ['#e5deff', '#473f77'], ['#eaddff', '#4e3d75'], ['#f0dbff', '#563b71'], ['#f7d9ff', '#5c396c'], ['#ffd6ff', '#613767'],
  ['#ffd7f7', '#653662'], ['#ffd7f0', '#68355c'], ['#ffd8eb', '#6b3456'], ['#ffd9e5', '#6e334e'], ['#ffd9e0', '#713344'], ['#ffdadb', '#72333b'],
];

/** The hue the launcher gives app `id` (FNV-1a over its UTF-16 code units,
 * then murmur3's finalizer), so a tile keeps the launcher's colour. */
export function hueOf(id) {
  let h = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 0x01000193) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  h = Math.imul(h, 0x85ebca6b) >>> 0;
  h = (h ^ (h >>> 13)) >>> 0;
  h = Math.imul(h, 0xc2b2ae35) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  return h % 360;
}

const toneStyle = (hue) => {
  const [a, b] = TONES[((Math.round(hue / 10) % 36) + 36) % 36];
  return `--t-a:${a};--t-b:${b}`;
};

// The kit glyph for an app's icon word, as the launcher maps words to icons.
const WORDS = [
  [['chart', 'graph', 'stats', 'analytics'], 'barChart'],
  [['calendar', 'schedule', 'date'], 'calendar'],
  [['recipe', 'food', 'cooking', 'meal'], 'utensils'],
  [['run', 'running', 'fitness', 'workout', 'exercise'], 'activity'],
  [['list', 'todo', 'checklist', 'tasks', 'task'], 'checkSquare'],
  [['note', 'notes', 'journal', 'diary', 'text'], 'pencil'],
  [['money', 'budget', 'finance', 'expense', 'expenses'], 'piggyBank'],
  [['shopping', 'cart', 'groceries', 'grocery'], 'cart'],
  [['timer', 'clock', 'time', 'stopwatch'], 'timer'],
  [['map', 'location', 'travel', 'trip'], 'map'],
  [['book', 'books', 'reading', 'library'], 'bookOpen'],
  [['music', 'song', 'audio'], 'music'],
  [['photo', 'image', 'camera', 'gallery'], 'image'],
  [['game', 'games', 'puzzle'], 'target'],
  [['habit', 'habits', 'streak'], 'flame'],
  [['water', 'drink'], 'droplet'],
  [['sleep', 'moon'], 'moon'],
  [['health', 'medical', 'heart'], 'heart'],
  [['weather', 'sun'], 'sun'],
  [['home', 'house'], 'home'],
  [['people', 'contacts', 'team', 'person'], 'users'],
  [['chat', 'message', 'mail', 'email'], 'message'],
  [['code', 'dev', 'terminal'], 'file'],
  [['calculator', 'math'], 'grid'],
  [['dashboard', 'home-screen'], 'grid'],
  [['plant', 'garden', 'plants'], 'leaf'],
  [['pet', 'dog', 'cat'], 'heart'],
  [['car', 'drive', 'fuel'], 'car'],
  [['star', 'favorite', 'rating'], 'star'],
  [['settings', 'tool', 'tools'], 'settings'],
  [['app', 'apps', 'widget'], 'grid'],
];
const WORD_GLYPH = new Map(WORDS.flatMap(([words, g]) => words.map((w) => [w, g])));

/** One or two capitals from the first two words of `title`. */
export function monogramOf(title) {
  const words = String(title || '').trim().split(/\s+/).filter(Boolean);
  if (!words.length) return '?';
  return words.slice(0, 2).map((w) => [...w][0].toUpperCase()).join('');
}

/** What an app's tile shows: its cached icon, its emoji, the glyph of its
 * icon word, else a monogram. */
function faceOf(app) {
  const i = app.icon || {};
  if (typeof i.url === 'string' && i.url) return { kind: 'url', url: i.url };
  if (typeof i.emoji === 'string' && i.emoji) return { kind: 'emoji', emoji: [...i.emoji][0] };
  const glyph = typeof i.word === 'string' ? WORD_GLYPH.get(i.word.trim().toLowerCase()) : undefined;
  if (glyph) return { kind: 'word', glyph };
  return { kind: 'monogram', letters: typeof i.monogram === 'string' && i.monogram ? i.monogram : monogramOf(app.title), hue: i.hue };
}

// ---------------------------------------------------------------------------
// Pieces.

/** A ring filled to `done` of `total`, with "3/7" in it. */
function Ring({ done, total }) {
  const r = 16.5;
  const c = 2 * Math.PI * r;
  const f = total > 0 ? Math.min(1, Math.max(0.04, done / total)) : 0;
  return html`<span class="h-ring">
    <svg width="36" height="36" viewBox="0 0 36 36" aria-hidden="true">
      <circle class="h-ring-track" cx="18" cy="18" r=${r} />
      <circle class="h-ring-arc" cx="18" cy="18" r=${r} stroke-dasharray=${`${f * c} ${c}`} transform="rotate(-90 18 18)" />
    </svg>
    <span class="h-ring-text">${done}/${total}</span>
  </span>`;
}

/** A make's ring before its plan has features. */
const Spinner = () => html`<span class="h-spinner" aria-hidden="true">
  <svg width="28" height="28" viewBox="0 0 28 28"><circle class="h-ring-track" cx="14" cy="14" r="12.5" /><circle class="h-spin-arc" cx="14" cy="14" r="12.5" /></svg>
</span>`;

function AppFace({ app }) {
  const [broken, setBroken] = useState(false);
  let face = faceOf(app);
  if (face.kind === 'url' && broken) face = { kind: 'monogram', letters: monogramOf(app.title) };
  const style = face.kind === 'url' ? '' : toneStyle(face.hue ?? hueOf(app.id));
  return html`<span class="h-face" data-kind=${face.kind} style=${style}>
    ${face.kind === 'url' && html`<img src=${face.url} alt="" width="60" height="60" draggable="false" onError=${() => setBroken(true)} />`}
    ${face.kind === 'emoji' && html`<span class="h-emoji">${face.emoji}</span>`}
    ${face.kind === 'word' && html`<${Glyph} name=${face.glyph} size=${28} stroke=${2.25} />`}
    ${face.kind === 'monogram' && html`<span class="h-mono">${face.letters}</span>`}
  </span>`;
}

/** The viewport rectangle of `el`'s icon (its unscaled box), in whole CSS
 * pixels. */
function faceRect(el) {
  const r = (el.querySelector('.h-icon') || el).getBoundingClientRect();
  return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
}

/** A tile's grid placement for its cell `c`, as a style object (so a
 * re-render never clears the transform the motion draws). */
const cellStyle = (c) => (c ? { gridColumn: `${c.col + 1} / span ${c.span}`, gridRow: `${c.row + 1} / span ${c.span}` } : undefined);

/** App `app`'s tile, in cell `cell`: its press goes to `ctl` (drag.js's
 * press: a tap opens, a hold then a release asks for the menu, a hold then
 * a move drags); a context menu outside a press asks for the menu at once;
 * Alt+arrows move it. `fresh` marks it "New"; `group`, the name of the
 * group it is in, shows under its name (in search's results). A tile whose
 * cell spans 2 is big (`data-big`): its icon draws at 120 px. */
function AppTile({ app, fresh, cell, dragging, group, ctl, onOpen, onMenu, tkey = tileKeyOf(app.id) }) {
  const b = app.build;
  const press = ctl.pressOf(app.id);
  const built = b ? `${app.title}, building, ${b.done} of ${b.total} done` : app.title;
  const said = group ? `${built}, in ${group}` : built;
  const label = fresh ? `${said}, new` : said;
  return html`<button type="button" class="h-tile" data-id=${app.id} data-key=${tkey} data-new=${fresh ? '' : undefined}
    data-dragging=${dragging ? '' : undefined} data-big=${cell?.span === 2 ? '' : undefined} aria-label=${label} style=${cellStyle(cell)}
    onPointerDown=${(e) => ctl.down(e, app.id)}
    onContextMenu=${(e) => { e.preventDefault(); if (!press.contextMenu()) onMenu(app, faceRect(e.currentTarget)); }}
    onClick=${() => { if (press.takeClick()) onOpen(app); }}
    onKeyDown=${(e) => ctl.key(app.id, e)}>
    <span class="h-icon">
      <${AppFace} app=${app} />
      ${b && html`<span class="h-over"><${Ring} done=${b.done} total=${b.total} /></span>`}
      ${fresh && html`<span class="h-new" aria-hidden="true">New</span>`}
      <span class="h-group-outline" aria-hidden="true" />
    </span>
    <span class=${`h-label ${group ? 'h-one' : 'h-two'}`}>${app.title}</span>
    ${group && html`<span class="h-status h-in-group">${group}</span>`}
  </button>`;
}

/** How many of a pile's icons its face stacks. */
const PILE_MAX = 5;
/** The `i`th card of group `gid`'s pile: turned by `tiltOf`, shifted up
 * to 4 px each way (from the tilts past the pile's), and a little smaller
 * than an icon, so the cards under the top one show at its edges. */
const pileTransform = (gid, i) => {
  const dx = Math.round(tiltOf(gid, i + PILE_MAX) * 5) / 10;
  const dy = Math.round(tiltOf(gid, i + 2 * PILE_MAX) * 5) / 10;
  return `translate(${dx}px, ${dy}px) rotate(${tiltOf(gid, i)}deg) scale(0.86)`;
};

/** Group `entry`'s tile (`{key, group: {id, name, look}, apps}`), in cell
 * `cell`, over its name: a folder's face is its first four apps' icons on
 * a tonal square; a pile's is its first five apps' icons stacked, the
 * first on top, each turned and shifted by `pileTransform` (so a pile
 * looks the same each time). Its press goes to `ctl` as an app's does: a tap opens a folder or
 * fans a pile out (`fanned` while it is), a hold then a release or a
 * context menu asks for the group's own menu, a hold then a move drags it.
 * It has no `data-id`: Clax is never told of it, so a tap on the start-up
 * image there opens nothing. */
function GroupTile({ entry, cell, dragging, fanned, ctl, onOpenGroup, onFanGroup, onGroupMenu }) {
  const { group, apps } = entry;
  const press = ctl.pressOf(entry.key);
  const pile = group.look === 'pile';
  const cards = apps.slice(0, PILE_MAX).map((a, i) => ({ a, i })).reverse();
  return html`<button type="button" class="h-tile h-group" data-group=${group.id} data-look=${group.look} data-key=${entry.key}
    data-dragging=${dragging ? '' : undefined} data-fanned=${fanned ? '' : undefined} aria-expanded=${pile ? !!fanned : undefined}
    aria-label=${`${group.name}, group of ${apps.length} apps`} style=${cellStyle(cell)}
    onPointerDown=${(e) => ctl.down(e, entry.key)}
    onContextMenu=${(e) => { e.preventDefault(); if (!press.contextMenu()) onGroupMenu(group.id, faceRect(e.currentTarget)); }}
    onClick=${() => { if (press.takeClick()) (pile ? onFanGroup : onOpenGroup)(group.id); }}
    onKeyDown=${(e) => ctl.key(entry.key, e)}>
    <span class="h-icon">
      ${pile
        ? html`<span class="h-pile-face" data-kind="pile">${cards.map(({ a, i }) => html`<span key=${a.id} class="h-pile-card" style=${{ transform: pileTransform(group.id, i) }}><${AppFace} app=${a} /></span>`)}</span>`
        : html`<span class="h-face h-folder-face" data-kind="folder">${apps.slice(0, 4).map((a) => html`<${AppFace} key=${a.id} app=${a} />`)}</span>`}
      <span class="h-group-outline" aria-hidden="true" />
    </span>
    <span class="h-label h-two">${group.name}</span>
  </button>`;
}

const MAKE_STATUS = { making: 'Making…', offline: 'Offline', waiting: 'Waiting…', interrupted: 'Interrupted', failed: "Couldn't make it" };
const MAKE_GLYPH = { offline: 'cloudOff', waiting: 'hourglass', interrupted: 'rotateCcw', failed: 'alertCircle' };

function MakeTile({ make, cell, act }) {
  const state = MAKE_STATUS[make.state] ? make.state : 'making';
  const status = MAKE_STATUS[state];
  const said = state !== 'making';
  const p = make.progress;
  const spoken = state === 'offline' ? 'Waiting for connection' : status;
  return html`<button type="button" class="h-tile" data-make=${make.id} data-key=${`m:${make.id}`} style=${cellStyle(cell)} aria-label=${`${make.request}, ${spoken}`} onClick=${() => act('openMake', [make.id], `open ${make.request}`)}>
    <span class="h-icon">
      <span class=${`h-face h-make${state === 'failed' ? ' h-failed' : ''}`} data-kind="make">
        ${!said && (p && p.total > 0 ? html`<${Ring} done=${p.done} total=${p.total} />` : html`<${Spinner} />`)}
        ${said && html`<${Glyph} name=${MAKE_GLYPH[state]} size=${28} />`}
      </span>
    </span>
    <span class=${`h-label ${said ? 'h-one' : 'h-two'}`}>${make.request}</span>
    ${said && html`<span class=${`h-status${state === 'failed' ? ' h-error' : ''}`}>${status}</span>`}
  </button>`;
}

function Message({ glyph, title, hint, action }) {
  return html`<div class="h-message">
    <${Glyph} name=${glyph} size=${48} class="h-message-glyph" />
    <p class="h-message-title">${title}</p>
    ${hint && html`<p class="h-message-hint">${hint}</p>`}
    ${action}
  </div>`;
}

/** The ⋮ menu: Import app…, Refresh, and, once the arrangement is read
 * (`bigOn` not null), the Big tiles for apps you use switch (`onBigTiles(on)`). */
function More({ act, refresh, bigOn, onBigTiles }) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return undefined;
    const key = (e) => { if (e.key === 'Escape') setOpen(false); };
    addEventListener('keydown', key);
    return () => removeEventListener('keydown', key);
  }, [open]);
  return html`<span class="h-more">
    <button type="button" class="h-action" aria-label="More" aria-haspopup="menu" aria-expanded=${open} onClick=${() => setOpen(!open)}>
      <${Glyph} name="moreVertical" />
    </button>
    ${open && html`<button type="button" class="h-scrim" aria-label="Close menu" onClick=${() => setOpen(false)} />
      <div class="h-menu" role="menu">
        <button type="button" role="menuitem" class="h-menu-item" onClick=${() => { setOpen(false); act('importApp', [], 'import an app'); }}>Import app…</button>
        <button type="button" role="menuitem" class="h-menu-item" onClick=${() => { setOpen(false); refresh(); }}>Refresh</button>
        ${bigOn !== null && html`<button type="button" role="menuitemcheckbox" aria-checked=${bigOn ? 'true' : 'false'} class="h-menu-item h-menu-check"
          onClick=${() => { setOpen(false); onBigTiles(!bigOn); }}>
          <span>Big tiles for apps you use</span>${bigOn && html`<${Glyph} name="check" size=${20} />`}
        </button>`}
      </div>`}
  </span>`;
}

function Bar({ act, refresh, unread, onSearch, bigOn, onBigTiles }) {
  return html`<header class="h-bar">
    <h1 class="h-title">Clax</h1>
    <div class="h-actions">
      <button type="button" class="h-action" aria-label="Search" onClick=${onSearch}><${Glyph} name="search" /></button>
      <button type="button" class="h-action" aria-label=${unread > 0 ? `Inbox, ${unread} unread` : 'Inbox'} onClick=${() => act('openInbox', [], 'open the inbox')}>
        <span class="h-badged">
          <${Glyph} name="inbox" />
          ${unread > 0 && html`<span class="h-badge">${unread > 999 ? '999+' : unread}</span>`}
        </span>
      </button>
      <button type="button" class="h-action" aria-label="Settings" onClick=${() => act('openSettings', [], 'open Settings')}><${Glyph} name="settings" /></button>
      <${More} act=${act} refresh=${refresh} bigOn=${bigOn} onBigTiles=${onBigTiles} />
    </div>
  </header>`;
}

/** The launcher's column count for a grid `width` wide: cells at most 112
 * px, 4 px apart (Flutter's SliverGridDelegateWithMaxCrossAxisExtent). */
const columnsFor = (width) => Math.max(1, Math.ceil(width / (112 + 4)));

/** The launcher's column count for the element `ref` holds, following its width. */
function useColumns(ref) {
  const [cols, setCols] = useState(4);
  useLayoutEffect(() => {
    const el = ref.current;
    const measure = () => setCols(columnsFor(el.clientWidth));
    measure();
    if (typeof ResizeObserver === 'function') {
      const o = new ResizeObserver(measure);
      o.observe(el);
      return () => o.disconnect();
    }
    addEventListener('resize', measure);
    return () => removeEventListener('resize', measure);
  }, []);
  return cols;
}

/** A tile's key in the page: "a:<app ID>", "g:<group ID>" (an order
 * entry's own key), "m:<make ID>", or "f:<app ID>" in an open folder. */
const tileKeyOf = (id) => `a:${id}`;
/** The tile key of an entry of Starred or All apps: an app's ID or a group's key. */
const entryTileKey = (key) => (isGroupKey(key) ? key : tileKeyOf(key));
/** The key of app `id`'s tile in a fanned pile's row, which is also its
 * press's ID in the grid's arranger. */
const fanKeyOf = (id) => `p:${id}`;
const isFanKey = (key) => key.startsWith('p:');
/** The tile key of a press ID of the grid's arranger. */
const pressTileKey = (id) => (isFanKey(id) ? id : entryTileKey(id));

/** The last row of the band of `cells` holding row `row`: the first row
 * from `row` on that no 2×2 cell runs past. */
function bandEnd(cells, row) {
  let end = row;
  while (cells.some((c) => c.row <= end && c.row + c.span - 1 > end)) end++;
  return end;
}

/** Each tile's cell by its key, with its section: Starred's apps, then
 * All apps' makes, apps and groups, each packed in `cols` columns. Under
 * All apps the apps `bigApps` picks at `now` take a 2×2 cell, every other
 * entry a cell. With group `fan` fanned out, the rows of All apps after its
 * pile's band (its row, and the second row of a 2×2 beside it) move down
 * one, leaving that row to its fan. */
function placement(layout, apps, makes, cols, fan, now) {
  const s = sections(layout, apps);
  const big = bigApps(layout, now);
  const lists = {
    starred: s.starred.map((e) => ({ key: tileKeyOf(e.app.id), span: 1 })),
    all: [...makes.map((m) => ({ key: `m:${m.id}`, span: 1 })), ...s.all.map((e) => ({ key: entryTileKey(e.key), span: e.app && big.has(e.key) ? 2 : 1 }))],
  };
  const out = new Map();
  for (const [section, items] of Object.entries(lists)) {
    const { cells } = pack(items, cols);
    const pile = section === 'all' && fan ? cells.find((c) => c.key === `g:${fan}`) : null;
    const end = pile ? bandEnd(cells, pile.row) : null;
    for (const c of cells) out.set(c.key, { ...c, row: pile && c.row > end ? c.row + 1 : c.row, section });
  }
  return out;
}

/** The 1-based grid row of group `fan`'s fan, under its pile's band in
 * `cells` (placement's), or null. */
const fanRowOf = (cells, fan) => {
  const c = fan ? cells.get(`g:${fan}`) : null;
  return c ? bandEnd([...cells.values()].filter((x) => x.section === 'all'), c.row) + 2 : null;
};

/** Pile `entry`'s apps fanned out in grid row `row`, across the grid, in
 * a row of cells as wide as the grid's that scrolls sideways; each opens
 * on a tap and asks for its menu as in the grid (through the arranger's
 * `ctl.fan`). */
function Fan({ entry, row, layout, ctl, onOpen, onMenu }) {
  return html`<div class="h-fan" data-group=${entry.group.id} role="group" aria-label=${entry.group.name}
    style=${{ gridColumn: '1 / -1', gridRow: `${row}` }}>
    ${entry.apps.map((a) => html`<${AppTile} key=${fanKeyOf(a.id)} tkey=${fanKeyOf(a.id)} app=${a} fresh=${isNew(layout, a.id)}
      ctl=${ctl.fan} onOpen=${onOpen} onMenu=${onMenu} />`)}
  </div>`;
}

/** Starred (only when something is starred, headed) above All apps (headed
 * then), each a grid of `cols` columns; the makes go first under All apps,
 * and its apps opened most at `now` take 2×2 cells.
 * A group takes a cell as its folder or pile tile; the pile `fan.gid`
 * fanned out has its row of apps under it. While app `drag` is dragged,
 * All apps is headed, and an empty Starred is a zone to drop on (a group
 * is never starred: dragging one opens no zone). */
function Sections({ layout, apps, makes, cols, now, drag, fan, ctl, onOpen, onMenu, onOpenGroup, onFanGroup, onGroupMenu, act }) {
  const s = sections(layout, apps);
  const fanned = fan ? s.all.find((e) => e.group?.id === fan.gid && e.group.look === 'pile') ?? null : null;
  const cells = placement(layout, apps, makes, cols, fanned ? fan.gid : null, now);
  const tile = (app) => html`<${AppTile} key=${tileKeyOf(app.id)} app=${app} fresh=${isNew(layout, app.id)} cell=${cells.get(tileKeyOf(app.id))}
    dragging=${drag === app.id} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} />`;
  const starred = s.starred.length > 0;
  const zone = !starred && drag != null && !isGroupKey(drag);
  const entry = (e) => {
    if (e.app) return tile(e.app);
    const group = html`<${GroupTile} key=${e.key} entry=${e} cell=${cells.get(e.key)} dragging=${drag === e.key} fanned=${e === fanned}
      ctl=${ctl} onOpenGroup=${onOpenGroup} onFanGroup=${onFanGroup} onGroupMenu=${onGroupMenu} />`;
    if (e !== fanned) return group;
    return [group, html`<${Fan} key=${`fan:${e.group.id}`} entry=${e} row=${fanRowOf(cells, fan.gid)} layout=${layout} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} />`];
  };
  return html`
    ${starred && html`<section class="h-section" data-section="starred" aria-label="Starred" style=${`--cols:${cols}`}>
      <h2 class="h-heading">Starred</h2>
      <div class="h-grid">${s.starred.map((e) => tile(e.app))}</div>
    </section>`}
    ${zone && html`<section class="h-section h-drop" data-section="starred" aria-label="Starred"><${Glyph} name="star" /><span>Drop here to star</span></section>`}
    <section class="h-section" data-section="all" aria-label="All apps" style=${`--cols:${cols}`}>
      ${(starred || zone) && html`<h2 class="h-heading">All apps</h2>`}
      <div class="h-grid">
        ${makes.map((m) => html`<${MakeTile} key=${`m:${m.id}`} make=${m} cell=${cells.get(`m:${m.id}`)} act=${act} />`)}
        ${s.all.map(entry)}
      </div>
    </section>`;
}

/** How near the viewport's top (below the bar) or bottom a dragged tile
 * scrolls the page, and how fast at the very edge, in px/s. */
const EDGE_PX = 48;
const EDGE_SPEED = 720;
/** The share of a toss's velocity a tile it passes is bumped by. */
const BUMP = 0.15;

/** `el`'s transform offset as the motion drew it. */
function offsetOf(el) {
  const m = /translate\((-?[\d.]+)px, (-?[\d.]+)px\)/.exec(el.style.transform);
  return m ? { x: Number(m[1]), y: Number(m[2]) } : { x: 0, y: 0 };
}
/** Where the page lays `el` out (its box less its transform), from the
 * document's top left. */
function layoutPos(el, t = offsetOf(el)) {
  const r = el.getBoundingClientRect();
  return { x: r.left - t.x + scrollX, y: r.top - t.y + scrollY };
}
const inside = (r, p) => p.x >= r.left && p.x <= r.right && p.y >= r.top && p.y <= r.bottom;
const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
/** The distance from point `c` to the segment from `a` to `b`. */
function toSegment(c, a, b) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const n = dx * dx + dy * dy;
  const f = n > 0 ? clamp(((c.x - a.x) * dx + (c.y - a.y) * dy) / n, 0, 1) : 0;
  return Math.hypot(a.x + f * dx - c.x, a.y + f * dy - c.y);
}

/** How long a dragged app rests on another app's centre before a drop
 * there makes a group of the two. */
const HOVER_MS = 600;
/** How long the pointer rests in a new slot before the neighbours make
 * room there: a quick move onto a tile's centre leaves that tile in place,
 * to be grouped with. */
const REORDER_MS = 120;

/** The grid's arranging, over the state `st` the Grid keeps current
 * (`wrap`, `layout`, `shown` (what the page shows, a drop's or a key's
 * move included before its write lands: every move starts from it),
 * `apps`, `makes`, `cols`, `fanGid` (the pile fanned out, or null),
 * `motion`, `reduced`, `change`, `onMenu`, `onGroupMenu`, `setDrag`,
 * `setHeld`, `onDragging`, `gatherNow`): each entry's press (`pressOf`, by
 * app ID or group key, or by fan key for an app in the fanned pile's row,
 * through `fan`), the drag it starts, and the keyboard's moves. During a
 * drag the tiles are placed and moved here, at once, with no render; the
 * page renders the drag's start and its drop. A drag gathers a fanned pile
 * at once. A dragged app resting HOVER_MS on another app's centre outlines
 * it, and a drop then makes a group of the two; on a group's centre it
 * outlines it at once, and a drop adds the app to it. A fanned pile's apps
 * spring out of the pile as its row opens. */
function arranger(st, clock) {
  const presses = new Map();
  let active = null;
  let drag = null;
  let pos = new Map();
  let lastCols = null;
  let allTop = null;
  // The page's scroll when the tiles were last placed or scrolled by a drag.
  let scrolled = 0;
  let dropped = false;
  let landing = null;
  let focus = null;
  let block = false;
  let edge = null;
  let seq = 0;
  // The tile under the dragged app's pointer, near its centre:
  // {key, ready, cancel}, `ready` once a drop there groups.
  let hover = null;
  // A slot the pointer moved to, waiting REORDER_MS: {t, cancel}.
  let pending = null;
  const tileEl = (key) => st.wrap?.querySelector(`.h-tile[data-key="${CSS.escape(key)}"]`) ?? null;
  const gridOf = (section) => st.wrap?.querySelector(`.h-section[data-section="${section}"] .h-grid`) ?? null;

  /** Places every tile in its cell of the shown arrangement (the drag's
   * while one is on) and springs each one that moved from where it was.
   * With `steady`, a render that moved All apps' grid up or down (the drop zone
   * and the headings coming or going) scrolls the page with it, so the
   * tiles stay under the finger. */
  function relayout(steady = false) {
    if (!st.wrap) return;
    const cells = placement(drag?.prov ?? st.shown, st.apps, st.makes, st.cols, st.fanGid, clock.date());
    const animate = lastCols === st.cols;
    lastCols = st.cols;
    const els = [...st.wrap.querySelectorAll('.h-tile[data-key]')];
    const offsets = els.map((el) => offsetOf(el));
    for (const el of els) {
      const c = cells.get(el.dataset.key);
      if (!c || c.section !== el.closest('.h-section')?.dataset.section) continue;
      const s = cellStyle(c);
      if (el.style.gridColumn !== s.gridColumn) el.style.gridColumn = s.gridColumn;
      if (el.style.gridRow !== s.gridRow) el.style.gridRow = s.gridRow;
      // A tile that grows or shrinks while no render runs (mid-drag) takes
      // its size at once; the spring below carries it from where it was.
      if (el.dataset.id && ('big' in el.dataset) !== (c.span === 2)) el.toggleAttribute('data-big', c.span === 2);
    }
    const all = gridOf('all');
    const top = all ? all.getBoundingClientRect().top + scrollY : null;
    // A page that shrank may have scrolled back already: `shift` is how far
    // it scrolled since the tiles were placed, which moves nothing.
    if (steady && top != null && allTop != null && Math.abs(top - allTop) >= 0.5) scrollTo(scrollX, scrolled + top - allTop);
    const shift = steady ? scrollY - scrolled : 0;
    allTop = top;
    scrolled = scrollY;
    const next = new Map();
    const moves = [];
    const pileEl = st.fanGid ? tileEl(`g:${st.fanGid}`) : null;
    const pile = pileEl ? layoutPos(pileEl) : null;
    els.forEach((el, i) => {
      const key = el.dataset.key;
      const p = layoutPos(el, offsets[i]);
      next.set(key, p);
      if (drag && key === drag.key) return;
      // A fanned pile's app new to the page comes out of the pile.
      if (isFanKey(key) && !pos.has(key)) {
        if (pile && animate) moves.push([key, el, pile.x - p.x, pile.y - p.y, 0, 0]);
        return;
      }
      if (landing?.key === key) {
        moves.push([key, el, landing.x - p.x, landing.y - p.y, landing.vx, landing.vy]);
        landing = null;
        return;
      }
      const o = pos.get(key);
      if (!o || !animate || (Math.abs(o.x - p.x) < 0.5 && Math.abs(o.y + shift - p.y) < 0.5)) return;
      moves.push([key, el, o.x + offsets[i].x - p.x, o.y + shift + offsets[i].y - p.y, 0, 0]);
    });
    pos = next;
    for (const [key, el, dx, dy, vx, vy] of moves) {
      st.motion.from(key, el, dx, dy);
      if (vx || vy) st.motion.bump(key, el, vx, vy);
    }
    if (drag) hold();
    if (focus && (!document.activeElement || document.activeElement === document.body)) tileEl(pressTileKey(focus))?.focus();
  }

  /** Holds the dragged tile under the pointer, where it was taken. */
  function hold() {
    const el = tileEl(drag.key);
    if (!el) return;
    const p = layoutPos(el);
    st.motion.hold(drag.key, el, drag.last.x + scrollX - drag.grab.x - p.x, drag.last.y + scrollY - drag.grab.y - p.y);
  }

  /** The slot at viewport point `p` in `section`'s grid: the index among
   * its entries (the dragged one left out) the dragged entry would take
   * there, the one whose cell, packed after the entries before it (a big
   * app's 2×2 included), has its centre nearest the point; a 1×1 drag's
   * point is the pointer, a 2×2's the centre of the tile held under it.
   * Nearest a make's cell, the first slot. */
  function slotIn(section, p) {
    const grid = gridOf(section);
    const entries = sections(st.shown, st.apps)[section].filter((e) => e.key !== drag.id);
    if (!grid) return { section, index: 0 };
    const big = section === 'all' ? bigApps(st.shown, clock.date()) : new Set();
    const spanOf = (key) => (big.has(key) ? 2 : 1);
    const lead = Array.from({ length: section === 'all' ? st.makes.length : 0 }, () => ({ key: null, span: 1 }));
    const r = grid.getBoundingClientRect();
    const g = geometry(r.width, st.cols);
    const n = spanOf(drag.id);
    const el = tileEl(drag.key);
    const q = n === 2 && el
      ? { x: p.x - drag.grab.x + el.offsetWidth / 2 - r.left, y: p.y - drag.grab.y + el.offsetHeight / 2 - r.top }
      : { x: p.x - r.left, y: p.y - r.top };
    const far = (cell) => {
      const b = cellBox(cell, g);
      return Math.hypot(b.x + b.w / 2 - q.x, b.y + b.h / 2 - q.y);
    };
    let index = 0;
    let dist = Math.min(Infinity, ...pack(lead, st.cols).cells.map(far));
    const before = [...lead];
    for (let i = 0; i <= entries.length; i++) {
      const { cells } = pack([...before, { key: drag.id, span: n }], st.cols);
      const d = far(cells.at(-1));
      if (d < dist) { dist = d; index = i; }
      if (i < entries.length) before.push({ key: entries[i].key, span: spanOf(entries[i].key) });
    }
    return { section, index };
  }

  /** Where a drop at viewport point `p` puts the dragged entry: a group
   * stays in All apps. */
  function targetAt(p) {
    if (isGroupKey(drag.id)) return slotIn('all', p);
    const star = st.wrap?.querySelector('.h-section[data-section="starred"]');
    if (star && inside(star.getBoundingClientRect(), p)) {
      return star.classList.contains('h-drop') ? { section: 'starred', index: 0, zone: true } : slotIn('starred', p);
    }
    return slotIn('all', p);
  }

  /** The key of the tile, as drawn, whose centre the dragged app's
   * pointer `p` is near (another app's or a group's), or null. */
  function hoverAt(p) {
    if (isGroupKey(drag.id)) return null;
    const cells = placement(drag.prov ?? st.shown, st.apps, st.makes, st.cols, st.fanGid, clock.date());
    for (const section of ['starred', 'all']) {
      const grid = gridOf(section);
      if (!grid) continue;
      const r = grid.getBoundingClientRect();
      if (!inside(r, p)) continue;
      const list = [...cells.values()].filter((c) => c.section === section);
      const i = centreUnder(list, geometry(r.width, st.cols), p.x - r.left, p.y - r.top);
      const key = i < 0 ? null : list[i].key;
      return key && key !== drag.key && !key.startsWith('m:') ? key : null;
    }
    return null;
  }

  function clearHover() {
    if (!hover) return;
    hover.cancel?.();
    tileEl(hover.key)?.removeAttribute('data-grouping');
    if (drag) tileEl(drag.key)?.removeAttribute('data-grouping-with');
    hover = null;
  }
  /** Hovers tile `key` (or none): a group is outlined at once, an app
   * after HOVER_MS. */
  function setHover(key) {
    if (hover?.key === key) return;
    clearHover();
    if (!key) return;
    const h = { key, ready: false, cancel: null };
    hover = h;
    const ready = () => {
      if (hover !== h) return;
      h.ready = true;
      tileEl(key)?.setAttribute('data-grouping', '');
      // The dragged tile's name gives way to the one it groups with.
      if (drag) tileEl(drag.key)?.setAttribute('data-grouping-with', '');
    };
    if (isGroupKey(key)) ready();
    else h.cancel = clock.after(HOVER_MS, ready);
  }

  const cancelPending = () => { pending?.cancel(); pending = null; };
  const sameSlot = (a, b) => !!a && !!b && a.section === b.section && a.index === b.index && !!a.zone === !!b.zone;
  /** The neighbours make room for the dragged entry at slot `t`. */
  function commit(t) {
    drag.target = t;
    drag.prov = moveTo(st.shown, drag.id, t.section, t.index);
    relayout();
  }

  /** Puts the empty Starred zone's words on the side away from the
   * dragged tile, so the tile never covers them. */
  function sideOfZone() {
    const zone = st.wrap?.querySelector('.h-drop');
    const icon = tileEl(drag.key)?.querySelector('.h-icon');
    if (!zone || !icon) return;
    const z = zone.getBoundingClientRect();
    const r = icon.getBoundingClientRect();
    zone.dataset.side = r.left + r.width / 2 < z.left + z.width / 2 ? 'end' : 'start';
  }

  /** Follows the pointer: near another tile's centre it hovers that tile
   * and moves nothing; elsewhere, once the pointer rests REORDER_MS in a
   * new slot, the neighbours make room there. */
  function track() {
    const t = targetAt(drag.last);
    st.wrap?.querySelector('.h-drop')?.toggleAttribute('data-over', !!t.zone);
    sideOfZone();
    const over = t.zone ? null : hoverAt(drag.last);
    setHover(over);
    if (over) { cancelPending(); return; }
    if (sameSlot(t, drag.target)) { cancelPending(); return; }
    if (!drag.target) { cancelPending(); commit(t); return; }
    if (sameSlot(t, pending?.t)) return;
    cancelPending();
    const mine = { t, cancel: null };
    pending = mine;
    mine.cancel = clock.after(REORDER_MS, () => {
      if (pending !== mine || !drag) return;
      pending = null;
      commit(t);
    });
  }

  const blocking = (on) => { block = on; };
  const speedAt = (y) => {
    const top = document.querySelector('.h-bar')?.getBoundingClientRect().bottom ?? 0;
    if (y < top + EDGE_PX) return -EDGE_SPEED * Math.min(1, (top + EDGE_PX - y) / EDGE_PX);
    if (y > innerHeight - EDGE_PX) return EDGE_SPEED * Math.min(1, (y - innerHeight + EDGE_PX) / EDGE_PX);
    return 0;
  };
  const stopEdge = () => { edge?.(); edge = null; };
  function startEdge() {
    stopEdge();
    let last = clock.now();
    const tick = (now) => {
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      if (!drag) return;
      const v = speedAt(drag.last.y);
      if (v) {
        const before = scrollY;
        scrollBy(0, v * dt);
        scrolled = scrollY;
        if (scrollY !== before) { hold(); track(); }
      }
      edge = clock.frame(tick);
    };
    edge = clock.frame(tick);
  }

  function end(d) {
    stopEdge();
    cancelPending();
    clearHover();
    tileEl(d.key)?.removeAttribute('data-grouping-with');
    blocking(false);
    drag = null;
    dropped = true;
    st.setDrag(null);
    st.onDragging(false);
    const el = tileEl(d.key);
    if (el) delete el.dataset.held;
  }

  /** App `id` dropped on hovered tile `target` (an app's or a group's
   * key): a new folder of the two, or the app added to the group, shown
   * at once and written. */
  function groupDrop(d, target) {
    let op;
    if (isGroupKey(target)) {
      const gid = target.slice(2);
      op = (l) => addToGroup(l, gid, d.id);
    } else {
      const other = target.slice(2);
      const gid = Math.random().toString(36).slice(2, 10);
      const name = groupNameOf(st.apps.find((a) => a.id === other), st.apps.find((a) => a.id === d.id));
      op = (l) => makeGroup(l, other, d.id, gid, name);
    }
    clearHover();
    d.prov = op(st.shown);
    st.motion.let(d.key);
    end(d);
    const mine = ++seq;
    st.shown = d.prov;
    st.setHeld(d.prov);
    st.change(op).then(() => { if (mine === seq) st.setHeld(null); });
  }

  /** The press of app `app`'s tile in the fanned pile's row (fan key
   * `key`): a hold then a release asks for its menu; it does not drag. */
  function fanHandlersFor(key, app) {
    const unlift = () => {
      const el = tileEl(key);
      if (el) delete el.dataset.held;
      blocking(false);
    };
    return {
      lift() {
        const el = tileEl(key);
        if (el) el.dataset.held = '';
        blocking(true);
      },
      unlift,
      menu() {
        unlift();
        const el = tileEl(key);
        const a = st.apps.find((x) => x.id === app);
        if (el && a) st.onMenu(a, faceRect(el));
      },
      dragStart() {},
      dragMove() {},
      dragEnd: unlift,
      dragCancel: unlift,
    };
  }

  function handlersFor(id) {
    if (isFanKey(id)) return fanHandlersFor(id, id.slice(2));
    const key = entryTileKey(id);
    return {
      lift() {
        const el = tileEl(key);
        if (el) el.dataset.held = '';
        blocking(true);
      },
      unlift() {
        const el = tileEl(key);
        if (el) delete el.dataset.held;
        blocking(false);
      },
      menu() {
        this.unlift();
        const el = tileEl(key);
        if (!el) return;
        if (isGroupKey(id)) { st.onGroupMenu(id.slice(2), faceRect(el)); return; }
        const app = st.apps.find((a) => a.id === id);
        if (app) st.onMenu(app, faceRect(el));
      },
      dragStart(p) {
        const el = tileEl(key);
        if (!el) return;
        st.gatherNow();
        const r = el.getBoundingClientRect();
        focus = null;
        drag = { id, key, from: st.shown.starred.includes(id) ? 'starred' : 'all', grab: { x: p.x - r.left, y: p.y - r.top }, last: p, target: null, prov: null };
        el.dataset.held = '';
        blocking(true);
        st.setDrag(id);
        st.onDragging(true);
        hold();
        startEdge();
      },
      dragMove(p) {
        if (!drag) return;
        drag.last = p;
        hold();
        track();
      },
      dragEnd(p, samples) {
        if (!drag) return;
        const d = drag;
        d.last = p;
        cancelPending();
        if (hover?.ready && hover.key === hoverAt(p)) { groupDrop(d, hover.key); return; }
        const v = st.reduced ? { vx: 0, vy: 0 } : velocityOf(samples);
        const toss = Math.hypot(v.vx, v.vy) > TOSS_MIN;
        let t = targetAt(p);
        if (toss && !t.zone) {
          const grid = gridOf(t.section);
          if (grid) {
            const r = grid.getBoundingClientRect();
            const rest = tossRest({ x: p.x, y: p.y, vx: v.vx, vy: v.vy });
            const q = { x: clamp(rest.x, r.left, r.right), y: clamp(rest.y, r.top, r.bottom) };
            t = slotIn(t.section, q);
            const half = geometry(r.width, st.cols).cellW / 2;
            for (const el of grid.querySelectorAll('.h-tile[data-key]')) {
              if (el.dataset.key === key) continue;
              const b = el.getBoundingClientRect();
              if (toSegment({ x: b.left + b.width / 2, y: b.top + b.height / 2 }, p, q) < half) st.motion.bump(el.dataset.key, el, v.vx * BUMP, v.vy * BUMP);
            }
          }
        }
        d.target = t;
        d.prov = moveTo(st.shown, id, t.section, t.index);
        relayout();
        const el = tileEl(key);
        if (t.section === d.from) {
          st.motion.let(key);
          if (toss && el) st.motion.bump(key, el, v.vx, v.vy);
        } else if (el) {
          // It lands in the other section once that renders.
          const r = el.getBoundingClientRect();
          landing = { key, x: r.left + scrollX, y: r.top + scrollY, vx: toss ? v.vx : 0, vy: toss ? v.vy : 0 };
        }
        end(d);
        const mine = ++seq;
        st.setHeld(d.prov);
        st.change((l) => moveTo(l, id, t.section, t.index)).then(() => { if (mine === seq) st.setHeld(null); });
      },
      dragCancel() {
        if (!drag) return;
        const d = drag;
        d.prov = null;
        relayout();
        st.motion.let(key);
        end(d);
      },
    };
  }

  const ctl = {
    relayout,
    get dragging() { return drag !== null; },
    pressOf(id) {
      let p = presses.get(id);
      if (!p) presses.set(id, (p = createPress(handlersFor(id), clock)));
      return p;
    },
    down(e, id) {
      if (drag || (active && active.phase !== 'idle')) return;
      focus = null;
      active = this.pressOf(id);
      if (e.pointerType === 'mouse' && e.button === 0) {
        try { e.currentTarget.setPointerCapture(e.pointerId); } catch { /* a pointer the browser does not know */ }
      }
      active.down(e);
    },
    /** A press or a focus anywhere but on the tile last moved by keyboard:
     * a later render leaves the focus where the person put it. */
    elsewhere(e) {
      if (focus && !(e.target instanceof Element && e.target.closest(`.h-tile[data-key="${CSS.escape(pressTileKey(focus))}"]`))) focus = null;
    },
    /** The page hidden or left, or the pointer lost, mid-press: the press
     * ends as if cancelled and every tile goes to its place at once, so
     * nothing stays held and the tiles are reported again. */
    abandon() {
      if (!active || active.phase === 'idle') return;
      active.cancel();
      st.motion.rest();
    },
    /** A pointer event anywhere on the page, for the press it belongs to. */
    pointer(type, e) { active?.[type](e); },
    touchMove(e) { if (block && e.cancelable) e.preventDefault(); },
    afterRender() {
      relayout(drag !== null || dropped);
      dropped = false;
      if (drag) track();
    },
    /** Alt+arrows on entry `id`'s tile (an app's ID or a group's key): one
     * entry left or right, or a row (`cols` entries) up or down, within its
     * section, from where it is shown; on an app in the fanned pile's row
     * (a fan key), Alt+Left and Alt+Right move it one place along its
     * group. */
    key(id, e) {
      if (!e.altKey || drag) return;
      if (isFanKey(id)) { fanKey(id, e); return; }
      const n = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -st.cols, ArrowDown: st.cols }[e.key];
      if (!n) return;
      e.preventDefault();
      const l = st.shown;
      const section = l.starred.includes(id) ? 'starred' : 'all';
      const entries = sections(l, st.apps)[section];
      const i = entries.findIndex((x) => x.key === id);
      const to = clamp(i + n, 0, entries.length - 1);
      if (i < 0 || to === i) return;
      focus = id;
      // Shown at once, so a press before the write lands moves on from here.
      const next = moveTo(l, id, section, to);
      st.shown = next;
      const mine = ++seq;
      st.setHeld(next);
      st.change((x) => moveTo(x, id, section, to)).then(() => { if (mine === seq) st.setHeld(null); });
    },
    stop() {
      stopEdge();
      for (const p of presses.values()) p.cancel();
    },
  };
  /** The fanned pile's tiles' calls, by app ID, onto their fan keys. */
  ctl.fan = {
    pressOf: (id) => ctl.pressOf(fanKeyOf(id)),
    down: (e, id) => ctl.down(e, fanKeyOf(id)),
    key: (id, e) => ctl.key(fanKeyOf(id), e),
  };
  return ctl;

  /** Alt+Left or Alt+Right on fan key `id`: its app one place along its group. */
  function fanKey(id, e) {
    const n = { ArrowLeft: -1, ArrowRight: 1 }[e.key];
    if (!n) return;
    e.preventDefault();
    const app = id.slice(2);
    const l = st.shown;
    const gid = groupOf(l, app);
    const list = gid ? l.groups[gid].apps : [];
    const i = list.indexOf(app);
    const to = clamp(i + n, 0, list.length - 1);
    if (i < 0 || to === i) return;
    focus = id;
    const next = moveInGroup(l, gid, app, to);
    st.shown = next;
    const mine = ++seq;
    st.setHeld(next);
    st.change((x) => moveInGroup(x, gid, app, to)).then(() => { if (mine === seq) st.setHeld(null); });
  }
}

/** The grid of Starred and All apps, arranged by `arranger`. `fan`
 * ({gid, closing}, or null) is the pile fanned out: its row shows under
 * it, still shown while `closing` (its apps going back into the pile, by
 * `gatherFan`). `gatherNow()` drops the fan with no motion, as a drag
 * starts. */
/** The longest a fan's gathering waits for its apps to reach the pile. */
const GATHER_MS = 1000;

/** Springs each app of the fanned row back onto group `gid`'s pile, then
 * calls `done` (at once with `reduced`, or with no row or pile; at the
 * latest GATHER_MS on, should a later layout move a tile on). */
function gatherFan(gid, motion, reduced, clock, done) {
  const els = [...document.querySelectorAll('.h-fan .h-tile[data-key]')];
  const pileEl = document.querySelector(`.h-tile[data-key="${CSS.escape(`g:${gid}`)}"]`);
  if (reduced || !pileEl || !els.length) { done(); return; }
  const pile = layoutPos(pileEl);
  let left = els.length;
  let stop = null;
  const end = () => { stop?.(); done(); };
  for (const el of els) {
    const p = layoutPos(el);
    motion.to(el.dataset.key, el, pile.x - p.x, pile.y - p.y, () => { if (--left === 0) end(); });
  }
  if (left > 0) stop = clock.after(GATHER_MS, done);
}

function Grid({ layout, apps, makes, motion, reduced, clock, change, fan, onOpen, onMenu, onOpenGroup, onFanGroup, onGroupMenu, act, onDragging, gatherNow }) {
  const ref = useRef(null);
  const cols = useColumns(ref);
  const [drag, setDrag] = useState(null);
  // The arrangement a drop made, shown until its write settles.
  const [held, setHeld] = useState(null);
  const shown = held ?? layout;
  const fanGid = fan && shown.groups[fan.gid]?.look === 'pile' ? fan.gid : null;
  const st = useRef({}).current;
  Object.assign(st, { wrap: ref.current, layout, shown, apps, makes, cols, fanGid, motion, reduced, change, onMenu, onGroupMenu, setDrag, setHeld, onDragging, gatherNow });
  const ctl = useMemo(() => arranger(st, clock), [clock]);
  useLayoutEffect(() => {
    st.wrap = ref.current;
    ctl.afterRender();
  });
  // A fan opened again while it gathers springs its apps back out.
  const closing = !!fan?.closing;
  useLayoutEffect(() => {
    if (closing) return;
    for (const el of ref.current?.querySelectorAll('.h-fan .h-tile[data-key]') ?? []) {
      const o = offsetOf(el);
      if (o.x || o.y) motion.from(el.dataset.key, el, o.x, o.y);
    }
  }, [closing, fan?.gid]);
  useEffect(() => {
    const move = (e) => ctl.pointer('move', e);
    const up = (e) => ctl.pointer('up', e);
    const cancel = (e) => ctl.pointer('cancel', e);
    const touch = (e) => ctl.touchMove(e);
    const elsewhere = (e) => ctl.elsewhere(e);
    const abandon = () => ctl.abandon();
    const hidden = () => { if (document.visibilityState === 'hidden') ctl.abandon(); };
    document.addEventListener('visibilitychange', hidden);
    document.addEventListener('lostpointercapture', abandon);
    addEventListener('pagehide', abandon);
    addEventListener('blur', abandon);
    document.addEventListener('pointerdown', elsewhere, true);
    document.addEventListener('focusin', elsewhere);
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', cancel);
    document.addEventListener('touchmove', touch, { passive: false });
    return () => {
      document.removeEventListener('visibilitychange', hidden);
      document.removeEventListener('lostpointercapture', abandon);
      removeEventListener('pagehide', abandon);
      removeEventListener('blur', abandon);
      document.removeEventListener('pointerdown', elsewhere, true);
      document.removeEventListener('focusin', elsewhere);
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', cancel);
      document.removeEventListener('touchmove', touch);
      ctl.stop();
    };
  }, [ctl]);
  return html`<div class="h-grid-wrap" ref=${ref}>
    <${Sections} layout=${shown} apps=${apps} makes=${makes} cols=${cols} now=${clock.date()} drag=${drag} fan=${fanGid ? fan : null} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu}
      onOpenGroup=${onOpenGroup} onFanGroup=${onFanGroup} onGroupMenu=${onGroupMenu} act=${act} />
  </div>`;
}

/** The presses of search's results, by app ID: a tap opens, a hold then a
 * release asks for the menu (`st.onMenu`), and nothing drags. */
function resultsPresses(st, clock) {
  const presses = new Map();
  let active = null;
  let block = false;
  const tileEl = (id) => st.wrap?.querySelector(`.h-tile[data-id="${CSS.escape(id)}"]`) ?? null;
  const handlersFor = (id) => {
    const unlift = () => {
      const el = tileEl(id);
      if (el) delete el.dataset.held;
      block = false;
    };
    return {
      lift() {
        const el = tileEl(id);
        if (el) el.dataset.held = '';
        block = true;
      },
      unlift,
      menu() {
        unlift();
        const el = tileEl(id);
        const app = st.apps.find((a) => a.id === id);
        if (el && app) st.onMenu(app, faceRect(el));
      },
      dragStart() {},
      dragMove() {},
      dragEnd: unlift,
      dragCancel: unlift,
    };
  };
  return {
    pressOf(id) {
      let p = presses.get(id);
      if (!p) presses.set(id, (p = createPress(handlersFor(id), clock)));
      return p;
    },
    down(e, id) {
      if (active && active.phase !== 'idle') return;
      active = this.pressOf(id);
      active.down(e);
    },
    key() {},
    pointer(type, e) { active?.[type](e); },
    touchMove(e) { if (block && e.cancelable) e.preventDefault(); },
    abandon() { if (active && active.phase !== 'idle') active.cancel(); },
    stop() { for (const p of presses.values()) p.cancel(); },
  };
}

/** What search found for `query` (`found`, {app, group}, best first), as
 * one grid of the launcher's columns, each app in a group with the group's
 * name under its own; or that nothing matches. */
function Results({ query, found, layout, apps, clock, onOpen, onMenu }) {
  const ref = useRef(null);
  const cols = useColumns(ref);
  const st = useRef({}).current;
  Object.assign(st, { wrap: ref.current, apps, onMenu });
  const ctl = useMemo(() => resultsPresses(st, clock), [clock]);
  useLayoutEffect(() => { st.wrap = ref.current; });
  useEffect(() => {
    const move = (e) => ctl.pointer('move', e);
    const up = (e) => ctl.pointer('up', e);
    const cancel = (e) => ctl.pointer('cancel', e);
    const touch = (e) => ctl.touchMove(e);
    const abandon = () => ctl.abandon();
    const hidden = () => { if (document.visibilityState === 'hidden') ctl.abandon(); };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', cancel);
    document.addEventListener('touchmove', touch, { passive: false });
    document.addEventListener('visibilitychange', hidden);
    document.addEventListener('lostpointercapture', abandon);
    addEventListener('pagehide', abandon);
    addEventListener('blur', abandon);
    return () => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', cancel);
      document.removeEventListener('touchmove', touch);
      document.removeEventListener('visibilitychange', hidden);
      document.removeEventListener('lostpointercapture', abandon);
      removeEventListener('pagehide', abandon);
      removeEventListener('blur', abandon);
      ctl.stop();
    };
  }, [ctl]);
  const { cells } = pack(found.map((f) => ({ key: f.app.id, span: 1 })), cols);
  return html`<div class="h-grid-wrap" ref=${ref}>
    <section class="h-results" aria-label="Search results" style=${`--cols:${cols}`}>
      ${found.length
        ? html`<div class="h-grid">${found.map((f, i) => html`<${AppTile} key=${f.app.id} tkey=${`r:${f.app.id}`} app=${f.app} fresh=${isNew(layout, f.app.id)}
            cell=${cells[i]} group=${f.group ?? undefined} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} />`)}</div>`
        : html`<p class="h-results-empty">No apps match “${query.trim()}”</p>`}
    </section>
  </div>`;
}

/** The entries search looks through, in the home screen's order: Starred's
 * apps, then All apps' with each group's apps at its place, under its name. */
const searchEntries = (layout, apps) => {
  const s = sections(layout, apps);
  return [
    ...s.starred.map((e) => ({ app: e.app, group: null })),
    ...s.all.flatMap((e) => (e.app ? [{ app: e.app, group: null }] : e.apps.map((app) => ({ app, group: e.group.name })))),
  ];
};

/** The search field, focused as it opens: Enter opens the first match
 * (`onGo`), Backspace in an empty field and its close button close search
 * (`onClose`). */
function SearchField({ query, setQuery, onGo, onClose }) {
  const field = useRef(null);
  useLayoutEffect(() => { field.current?.focus(); }, []);
  return html`<div class="h-search" role="search">
    <label class="h-search-box">
      <${Glyph} name="search" class="h-search-glyph" />
      <input ref=${field} type="search" class="h-search-input" aria-label="Search apps" placeholder="Search apps" enterkeyhint="go"
        autocomplete="off" autocapitalize="off" spellcheck="false" value=${query}
        onInput=${(e) => setQuery(e.currentTarget.value)}
        onKeyDown=${(e) => {
          if (e.key === 'Enter') { e.preventDefault(); onGo(); }
          else if (e.key === 'Backspace' && e.currentTarget.value === '') { e.preventDefault(); onClose(); }
        }} />
      <button type="button" class="h-search-close" aria-label="Close search" onClick=${onClose}><${Glyph} name="x" /></button>
    </label>
  </div>`;
}

/** How far past an open folder's panel a dragged app must be dropped to
 * leave the group. */
const OUT_PX = 24;

/** An open folder's measures for a page `width` wide: the home grid's
 * cell width and the folder's columns (the grid's, at most 4). */
function folderMeasures(width) {
  const cols = columnsFor(width);
  const cellW = (width - 32 - 4 * (cols - 1)) / cols;
  const n = Math.min(4, cols);
  return { cols: n, width: n * cellW + (n - 1) * 4 };
}

/** An open folder's arranging, over the state `st` the Folder keeps
 * current (`wrap`, `gid`, `ids`, `apps`, `cols`, `motion`, `change`,
 * `onMenu`, `setHeld`, `close`): each app's press, as in the grid. A drag
 * inside reorders the group; a drop more than OUT_PX outside the panel
 * takes the app out of the group and closes the folder. Alt+arrows on a
 * focused app move it one place left or right, or a row up or down. */
function folderArranger(st, clock) {
  const presses = new Map();
  let active = null;
  let drag = null;
  let pos = new Map();
  let block = false;
  let seq = 0;
  // The app last moved by keyboard, kept focused across renders.
  let focus = null;
  const keyOf = (id) => `f:${id}`;
  const tileEl = (id) => st.wrap?.querySelector(`.h-tile[data-key="${CSS.escape(keyOf(id))}"]`) ?? null;
  const panel = () => st.wrap?.querySelector('.h-folder-panel') ?? null;
  /** Where `el` is laid out in the viewport (its box less its transform). */
  const boxOf = (el, t = offsetOf(el)) => { const r = el.getBoundingClientRect(); return { x: r.left - t.x, y: r.top - t.y }; };
  const place = (ids, id, i) => { const rest = ids.filter((k) => k !== id); return [...rest.slice(0, i), id, ...rest.slice(i)]; };

  /** Places each tile in its cell of the shown order (the drag's while one
   * is on) and springs each one that moved from where it was. */
  function relayout() {
    if (!st.wrap) return;
    const order = drag?.order ?? st.ids;
    const at = new Map(pack(order.map((id) => ({ key: keyOf(id), span: 1 })), st.cols).cells.map((c) => [c.key, c]));
    const els = [...st.wrap.querySelectorAll('.h-tile[data-key]')];
    const offsets = els.map((el) => offsetOf(el));
    for (const el of els) {
      const c = at.get(el.dataset.key);
      if (!c) continue;
      const s = cellStyle(c);
      if (el.style.gridColumn !== s.gridColumn) el.style.gridColumn = s.gridColumn;
      if (el.style.gridRow !== s.gridRow) el.style.gridRow = s.gridRow;
    }
    const next = new Map();
    const moves = [];
    els.forEach((el, i) => {
      const key = el.dataset.key;
      const p = boxOf(el, offsets[i]);
      next.set(key, p);
      if (drag && key === drag.key) return;
      const o = pos.get(key);
      if (!o || (Math.abs(o.x - p.x) < 0.5 && Math.abs(o.y - p.y) < 0.5)) return;
      moves.push([key, el, o.x + offsets[i].x - p.x, o.y + offsets[i].y - p.y]);
    });
    pos = next;
    for (const [key, el, dx, dy] of moves) st.motion.from(key, el, dx, dy);
    if (drag) hold();
    if (focus && (!document.activeElement || document.activeElement === document.body || document.activeElement === st.wrap)) tileEl(focus)?.focus();
  }

  function hold() {
    const el = tileEl(drag.id);
    if (!el) return;
    const b = boxOf(el);
    st.motion.hold(drag.key, el, drag.last.x - drag.grab.x - b.x, drag.last.y - drag.grab.y - b.y);
  }

  /** The index among the group's apps the dragged one takes at viewport point `p`. */
  function slotOf(p) {
    const grid = st.wrap?.querySelector('.h-folder-panel .h-grid');
    if (!grid) return 0;
    const r = grid.getBoundingClientRect();
    const { cells } = pack(st.ids.map((_, key) => ({ key, span: 1 })), st.cols);
    return Math.max(0, slotAt(cells, geometry(r.width, st.cols), p.x - r.left, p.y - r.top));
  }

  /** Whether viewport point `p` is more than OUT_PX outside the panel. */
  function outside(p) {
    const r = panel()?.getBoundingClientRect();
    return !!r && (p.x < r.left - OUT_PX || p.x > r.right + OUT_PX || p.y < r.top - OUT_PX || p.y > r.bottom + OUT_PX);
  }

  function finish(id) {
    block = false;
    const el = tileEl(id);
    if (el) { delete el.dataset.held; delete el.dataset.dragging; }
    st.wrap?.removeAttribute('data-out');
  }

  function handlersFor(id) {
    const key = keyOf(id);
    return {
      lift() { const el = tileEl(id); if (el) el.dataset.held = ''; block = true; },
      unlift() { finish(id); },
      menu() {
        finish(id);
        const el = tileEl(id);
        const app = st.apps.find((a) => a.id === id);
        if (el && app) st.onMenu(app, faceRect(el));
      },
      dragStart(p) {
        const el = tileEl(id);
        if (!el) return;
        const r = el.getBoundingClientRect();
        drag = { id, key, grab: { x: p.x - r.left, y: p.y - r.top }, last: p, order: st.ids };
        el.dataset.held = '';
        el.dataset.dragging = '';
        block = true;
        hold();
      },
      dragMove(p) {
        if (!drag) return;
        drag.last = p;
        hold();
        const out = outside(p);
        st.wrap?.toggleAttribute('data-out', out);
        if (out) return;
        const order = place(st.ids, id, slotOf(p));
        if (order.join() === drag.order.join()) return;
        drag.order = order;
        relayout();
      },
      dragEnd(p) {
        if (!drag) return;
        drag = null;
        finish(id);
        if (outside(p)) {
          st.motion.let(key);
          st.change((l) => removeFromGroup(l, id));
          st.close();
          return;
        }
        const i = slotOf(p);
        const order = place(st.ids, id, i);
        const gid = st.gid;
        st.ids = order;
        relayout();
        st.motion.let(key);
        const mine = ++seq;
        st.setHeld(order);
        st.change((l) => moveInGroup(l, gid, id, i)).then(() => { if (mine === seq) st.setHeld(null); });
      },
      dragCancel() {
        if (!drag) return;
        drag = null;
        relayout();
        st.motion.let(key);
        finish(id);
      },
    };
  }

  return {
    keyOf,
    pressOf(id) {
      let p = presses.get(id);
      if (!p) presses.set(id, (p = createPress(handlersFor(id), clock)));
      return p;
    },
    down(e, id) {
      if (drag || (active && active.phase !== 'idle')) return;
      focus = null;
      active = this.pressOf(id);
      if (e.pointerType === 'mouse' && e.button === 0) {
        try { e.currentTarget.setPointerCapture(e.pointerId); } catch { /* a pointer the browser does not know */ }
      }
      active.down(e);
    },
    abandon() {
      if (!active || active.phase === 'idle') return;
      active.cancel();
      st.motion.rest();
    },
    pointer(type, e) { active?.[type](e); },
    touchMove(e) { if (block && e.cancelable) e.preventDefault(); },
    afterRender: relayout,
    key(id, e) {
      if (!e.altKey || drag) return;
      const n = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -st.cols, ArrowDown: st.cols }[e.key];
      if (!n) return;
      e.preventDefault();
      const i = st.ids.indexOf(id);
      const to = clamp(i + n, 0, st.ids.length - 1);
      if (i < 0 || to === i) return;
      focus = id;
      // Shown at once, so a press before the write lands moves on from here.
      const order = place(st.ids, id, to);
      const gid = st.gid;
      st.ids = order;
      const mine = ++seq;
      st.setHeld(order);
      st.change((l) => moveInGroup(l, gid, id, to)).then(() => { if (mine === seq) st.setHeld(null); });
    },
    stop() { for (const p of presses.values()) p.cancel(); },
  };
}

/** Group `group`'s apps (`apps`, in its order) over the dimmed home
 * screen: its name (tap to rename; Enter or leaving the field keeps the
 * new name, Escape the old), and its apps in a grid of the home grid's
 * cell width, opened and arranged as there. `editing` opens it with its
 * name in the field. The scrim, Escape or back closes it (`close`). */
function Folder({ group, apps, layout, motion, clock, change, onOpen, onMenu, close, editing: editFirst }) {
  const ref = useRef(null);
  const [editing, setEditing] = useState(!!editFirst);
  const [held, setHeld] = useState(null);
  const [named, setNamed] = useState(null);
  const [width, setWidth] = useState(() => document.documentElement.clientWidth);
  const byId = new Map(apps.map((a) => [a.id, a]));
  const ids = (held ?? apps.map((a) => a.id)).filter((id) => byId.has(id));
  const m = folderMeasures(width);
  const st = useRef({}).current;
  Object.assign(st, { wrap: ref.current, gid: group.id, ids, apps, cols: m.cols, motion, change, onMenu, setHeld, close });
  const ctl = useMemo(() => folderArranger(st, clock), [clock]);
  const done = useRef(false);
  const field = useRef(null);
  useLayoutEffect(() => {
    st.wrap = ref.current;
    ctl.afterRender();
  });
  useLayoutEffect(() => {
    if (!editing) return;
    field.current?.focus();
    field.current?.select();
  }, [editing]);
  // Before the page can paint, so a press right after the folder opens is followed.
  useLayoutEffect(() => {
    if (!editFirst) ref.current?.focus();
    const move = (e) => ctl.pointer('move', e);
    const up = (e) => ctl.pointer('up', e);
    const cancel = (e) => ctl.pointer('cancel', e);
    const touch = (e) => ctl.touchMove(e);
    const abandon = () => ctl.abandon();
    const hidden = () => { if (document.visibilityState === 'hidden') ctl.abandon(); };
    const key = (e) => { if (e.key === 'Escape') close(); };
    const resize = () => setWidth(document.documentElement.clientWidth);
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', cancel);
    document.addEventListener('touchmove', touch, { passive: false });
    document.addEventListener('visibilitychange', hidden);
    document.addEventListener('lostpointercapture', abandon);
    document.addEventListener('keydown', key);
    addEventListener('pagehide', abandon);
    addEventListener('blur', abandon);
    addEventListener('resize', resize);
    return () => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', cancel);
      document.removeEventListener('touchmove', touch);
      document.removeEventListener('visibilitychange', hidden);
      document.removeEventListener('lostpointercapture', abandon);
      document.removeEventListener('keydown', key);
      removeEventListener('pagehide', abandon);
      removeEventListener('blur', abandon);
      removeEventListener('resize', resize);
      ctl.stop();
    };
  }, [ctl]);
  const rename = (value) => {
    if (done.current) return;
    done.current = true;
    setEditing(false);
    const name = String(value ?? '').trim();
    if (!name || name === group.name) return;
    setNamed(name);
    change((l) => renameGroup(l, group.id, name)).then(() => setNamed(null));
  };
  const name = named ?? group.name;
  const { cells } = pack(ids.map((id) => ({ key: ctl.keyOf(id), span: 1 })), m.cols);
  return html`<div class="h-folder" ref=${ref} role="dialog" aria-modal="true" aria-label=${name} tabindex="-1">
    <button type="button" class="h-scrim h-folder-scrim" aria-label="Close group" tabindex="-1" onClick=${close} />
    <div class="h-folder-panel" style=${`--cols:${m.cols};width:${m.width}px`}>
      <div class="h-folder-head">
        ${editing
          ? html`<input ref=${field} class="h-folder-input" aria-label="Group name" maxlength="40" value=${name}
              onKeyDown=${(e) => {
                if (e.key === 'Enter') { e.preventDefault(); rename(e.currentTarget.value); }
                else if (e.key === 'Escape') { e.stopPropagation(); done.current = true; setEditing(false); }
              }}
              onBlur=${(e) => rename(e.currentTarget.value)} />`
          : html`<button type="button" class="h-folder-name" onClick=${() => { done.current = false; setEditing(true); }}>
              <span class="h-folder-name-text">${name}</span><${Glyph} name="pencil" size=${18} />
            </button>`}
      </div>
      <div class="h-grid">
        ${ids.map((id, i) => html`<${AppTile} key=${id} tkey=${ctl.keyOf(id)} app=${byId.get(id)} fresh=${isNew(layout, id)} cell=${cells[i]}
          ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} />`)}
      </div>
    </div>
  </div>`;
}

/** Group `group`'s own menu, anchored at its tile's icon `rect`: Rename,
 * Show as pile or Show as folder, and Ungroup; `pick` gets "rename",
 * "look" or "ungroup". The scrim or Escape closes it. */
function GroupMenu({ group, rect, pick, close }) {
  useEffect(() => {
    const key = (e) => { if (e.key === 'Escape') close(); };
    addEventListener('keydown', key);
    return () => removeEventListener('keydown', key);
  }, [close]);
  const below = rect.y + rect.h + 8 + 168 <= innerHeight;
  const left = clamp(rect.x, 8, Math.max(8, innerWidth - 8 - 200));
  const style = `left:${left}px;${below ? `top:${rect.y + rect.h + 8}px` : `bottom:${innerHeight - rect.y + 8}px`}`;
  const item = (what, label) => html`<button type="button" role="menuitem" class="h-menu-item" onClick=${() => pick(what)}>${label}</button>`;
  return html`<button type="button" class="h-scrim h-group-scrim" aria-label="Close menu" onClick=${close} />
    <div class="h-menu h-group-menu" role="menu" aria-label=${group.name} style=${style}>
      ${item('rename', 'Rename')}
      ${item('look', group.look === 'pile' ? 'Show as folder' : 'Show as pile')}
      ${item('ungroup', 'Ungroup')}
    </div>`;
}

const codeOf = (e) => (typeof e === 'string' ? e : e?.code ?? e?.error);
const isRefused = (e) => codeOf(e) === 'refused';

/** The latest value of one topic: `load`'s answer, then each of `home`'s
 * `event`s, which carries the topic's whole value (an event without one
 * loads it again); an answer older than what is shown is dropped. `reload`
 * resolves once its answer is in, or failed. */
function useTopic(home, event, load, initial) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState(null);
  const seq = useRef(0);
  const reload = useCallback(() => {
    if (!home) return Promise.resolve();
    const n = ++seq.current;
    return Promise.resolve()
      .then(() => load(home))
      .then((v) => { if (n === seq.current) { setValue(v); setError(null); } })
      .catch((e) => { if (n === seq.current) setError(e ?? new Error('failed')); });
  }, [home]);
  useEffect(() => {
    if (!home) return undefined;
    reload();
    let stop = null;
    let gone = false;
    try {
      const heard = (v) => {
        if (v === undefined) { reload(); return; }
        ++seq.current;
        setValue(v);
        setError(null);
      };
      const r = home.on(event, heard);
      Promise.resolve(r).then((f) => { if (typeof f !== 'function') return; if (gone) f(); else stop = f; }, () => {});
    } catch { /* a capability without events: the first load stands */ }
    return () => { gone = true; if (stop) stop(); };
  }, [home]);
  return [value, error, reload];
}

const NOTICE_MS = 4000;

/** `act(method, args, what)`: calls `home[method](...args)` and resolves
 * its value, or `undefined` when it failed. `no_gesture` (the call did not
 * come with the person's tap) keeps the call for the next tap on the page
 * that lands on no other control, and says "Tap again to <what>"; the
 * promise then resolves with the retried call's value, or `undefined` once
 * the kept call is dropped. Another failure says so briefly. A new action,
 * a tap on another control, or the notice going drops a kept one. */
function useActions(home) {
  const [notice, setNotice] = useState(null);
  const pending = useRef(null);
  const timer = useRef(0);
  const drop = () => {
    const kept = pending.current;
    pending.current = null;
    kept?.resolve(undefined);
  };
  // A notice replaces the one shown, and with it any call kept for a tap.
  const say = (text) => {
    drop();
    clearTimeout(timer.current);
    setNotice({ text });
    timer.current = setTimeout(() => { setNotice(null); drop(); }, NOTICE_MS);
  };
  const act = useCallback((method, args, what) => {
    drop();
    let p;
    try {
      p = Promise.resolve(home[method](...args));
    } catch (e) {
      p = Promise.reject(e);
    }
    return p.then((v) => { setNotice(null); return v; }, (e) => {
      const code = codeOf(e);
      if (code === 'no_gesture') {
        return new Promise((resolve) => {
          say(`Tap again to ${what}`);
          pending.current = { method, args, what, resolve };
        });
      }
      if (code === 'bad_request' && args.length) say('That app is no longer here');
      else say(`Couldn't ${what}`);
      return undefined;
    });
  }, [home]);
  useEffect(() => {
    const tap = (e) => {
      const kept = pending.current;
      if (!kept) return;
      const other = e.target?.closest?.('button, a, [role=button], input, select, textarea');
      if (other && !other.classList.contains('h-notice')) { drop(); return; }
      pending.current = null;
      kept.resolve(act(kept.method, kept.args, kept.what));
    };
    document.addEventListener('click', tap, true);
    return () => { document.removeEventListener('click', tap, true); clearTimeout(timer.current); drop(); };
  }, [act]);
  return [act, notice, say];
}

/** How far a pull down from the top of the page goes (half the finger's
 * travel) before its release opens search, and the most it shows. */
const PULL_PX = 64;
const PULL_MAX = 96;

/** A touch drag down from the top of the page, while `on`: as it goes,
 * `data-pulling` on the page's root and `--pull` (0 to 1, at PULL_PX)
 * slide a ghost of the search field down under the bar; released past
 * PULL_PX it calls `onOpen`. A held tile or an open folder has the finger
 * instead. */
function usePullSearch(onOpen, on) {
  const go = useRef(onOpen);
  go.current = onOpen;
  useEffect(() => {
    if (!on) return undefined;
    const root = document.documentElement;
    let s = { y: null, d: 0 };
    const show = (d) => {
      if (d > 0) {
        root.setAttribute('data-pulling', '');
        root.style.setProperty('--pull', String(Math.min(1, d / PULL_PX)));
      } else {
        root.removeAttribute('data-pulling');
        root.style.removeProperty('--pull');
      }
    };
    const start = (e) => {
      const free = e.touches.length === 1 && scrollY <= 0 && !document.querySelector('.h-folder');
      s = { y: free ? e.touches[0].clientY : null, d: 0 };
    };
    const move = (e) => {
      if (s.y == null) return;
      // A held or dragged tile has the finger, not the page.
      if (document.querySelector('.h-tile[data-held]')) { s = { y: null, d: 0 }; show(0); return; }
      const dy = e.touches[0].clientY - s.y;
      if (dy <= 0) { s.d = 0; show(0); return; }
      if (e.cancelable) e.preventDefault();
      s.d = Math.min(PULL_MAX, dy * 0.5);
      show(s.d);
    };
    const end = () => {
      const far = s.d >= PULL_PX;
      s = { y: null, d: 0 };
      show(0);
      if (far) go.current();
    };
    addEventListener('touchstart', start, { passive: true });
    addEventListener('touchmove', move, { passive: false });
    addEventListener('touchend', end);
    addEventListener('touchcancel', end);
    return () => {
      show(0);
      removeEventListener('touchstart', start);
      removeEventListener('touchmove', move);
      removeEventListener('touchend', end);
      removeEventListener('touchcancel', end);
    };
  }, [on]);
}

/** The search field as a pull shows it, before it opens. */
const SearchGhost = () => html`<div class="h-search-ghost" aria-hidden="true">
  <span class="h-search-box"><${Glyph} name="search" class="h-search-glyph" /><span class="h-search-hint">Search apps</span></span>
</div>`;

const MAX_TILES = 512;
/** How long a burst of renders or scroll events settles before the tiles go. */
const TILES_MS = 50;

/** Tells Clax where each app's tile is (`home.tiles`; an open folder's
 * apps only, while it is open; never a group's tile), once the home
 * screen is `drawn` (after `shown`: Clax takes no tiles before it), then
 * after each render that moves one, on scroll and resize, and each time
 * the tiles come to rest, so the image of the home screen it shows at its
 * next start opens the app under a tap. Nothing goes while a tile moves
 * (`motion.moving`). Returns the report, for the motion's settling. */
function useTiles(home, drawn, motion) {
  const frame = useRef(0);
  const last = useRef('');
  const report = useCallback(() => {
    if (!home?.tiles || !drawn || motion.moving) return;
    // A short timer, not an animation frame: a WebView that is covered or
    // in a window the system counts hidden runs no frames, and Clax still
    // wants the tiles under its image.
    clearTimeout(frame.current);
    frame.current = setTimeout(() => {
      if (motion.moving) return;
      // An open folder covers the grid: only its apps are under a tap.
      const where = document.querySelector('.h-folder') ? '.h-folder .h-tile[data-id]' : '.h-grid-wrap .h-tile[data-id]';
      const tiles = [...document.querySelectorAll(where)].slice(0, MAX_TILES).map((el) => {
        const r = el.getBoundingClientRect();
        return { id: el.dataset.id, rect: { x: r.x, y: r.y, w: r.width, h: r.height } };
      });
      const key = JSON.stringify(tiles);
      if (key === last.current) return;
      last.current = key;
      Promise.resolve(home.tiles(tiles)).catch(() => {});
    }, TILES_MS);
  }, [home, drawn, motion]);
  useEffect(report);
  useEffect(() => {
    addEventListener('scroll', report, { passive: true });
    addEventListener('resize', report);
    return () => {
      clearTimeout(frame.current);
      removeEventListener('scroll', report);
      removeEventListener('resize', report);
    };
  }, [report]);
  return report;
}

/** Whether the person asks for reduced motion, following its changes. */
function useReducedMotion() {
  const q = useMemo(() => (typeof matchMedia === 'function' ? matchMedia('(prefers-reduced-motion: reduce)') : null), []);
  const [reduced, setReduced] = useState(() => !!q?.matches);
  useEffect(() => {
    if (!q?.addEventListener) return undefined;
    const heard = () => setReduced(q.matches);
    q.addEventListener('change', heard);
    return () => q.removeEventListener('change', heard);
  }, [q]);
  return reduced;
}

/** How long the first read of the arrangement may take before the grid
 * draws without it. */
const LAYOUT_MS = 2000;

/** What the page says when the arrangement could not be written. */
const UNSAVED_TEXT = "Couldn't save your arrangement";

/** The arrangement over `apps`: null until the apps and the stored layout
 * (or LAYOUT_MS without an answer) are in. `change(op)` applies `op` to
 * the stored layout made to match the apps and writes the result, which
 * shows once it is written; a write that fails changes nothing on screen
 * and calls `failed()`. With no layout stored yet the first load writes
 * one with every listed app seen. With an empty app list the stored layout
 * is changed as it is, so a list that is briefly empty never drops an ID. */
function useLayout(store, apps, clock, failed) {
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
  }).then((b) => { setBody(b); return b; }, () => { failed(); return undefined; }), [store, fit, apps]);
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

/** The page's own history entries, for the window (one listener, so a
 * close's back that lands while the page mounts again is still heard):
 * "group" while a folder is open or a pile fanned out, "search" while
 * search is open. `enter(kind)` takes kind's entry, `leave(kind)` goes
 * back off it; `back` is set while such a `history.back()` is on its way,
 * and `push` names the entry to take once it lands. Any other back calls
 * the `closers` of each kind whose entry is no longer the top one. */
const nav = {
  back: false,
  push: null,
  closers: {},
  listening: false,
  listen() {
    if (this.listening) return;
    this.listening = true;
    addEventListener('popstate', () => {
      if (this.back) {
        this.back = false;
        const kind = this.push;
        this.push = null;
        if (kind) history.pushState({ clax: kind }, '', `#${kind}`);
        return;
      }
      const top = history.state?.clax;
      for (const [kind, close] of Object.entries(this.closers)) if (top !== kind) close?.();
    });
  },
  enter(kind) {
    if (this.back) this.push = kind;
    else if (history.state?.clax !== kind) history.pushState({ clax: kind }, '', `#${kind}`);
  },
  leave(kind) {
    if (this.push === kind) this.push = null;
    if (!this.back && history.state?.clax === kind) {
      this.back = true;
      history.back();
    }
  },
};

/** What the home app adds to app `id`'s menu: Star or Unstar, and Remove
 * from group for an app in a group. */
const menuItems = (layout, id) => [
  layout.starred.includes(id) ? { id: 'unstar', label: 'Unstar' } : { id: 'star', label: 'Star' },
  ...(groupOf(layout, id) ? [{ id: 'remove', label: 'Remove from group' }] : []),
];

function Home({ home, db, clock }) {
  const [apps, appsError, reloadApps] = useTopic(home, 'apps', (h) => h.apps(), null);
  const [makes, , reloadMakes] = useTopic(home, 'makes', (h) => h.makes(), []);
  const [unread, , reloadUnread] = useTopic(home, 'unread', (h) => h.unread(), 0);
  const [needsKey, , reloadKey] = useTopic(home, 'key', (h) => h.needsKey(), false);
  const refresh = useCallback(
    () => Promise.all([reloadApps(), reloadMakes(), reloadUnread(), reloadKey()]),
    [reloadApps, reloadMakes, reloadUnread, reloadKey],
  );
  const [act, notice, say] = useActions(home);
  const store = useMemo(() => layoutStore(db, { clock }), [db, clock]);
  const [layout, change] = useLayout(store, apps, clock, useCallback(() => say(UNSAVED_TEXT), []));
  const onBigTiles = useCallback((on) => change((l) => setBigTiles(l, on)), [change]);
  const openApp = useCallback((app) => act('open', [app.id], `open ${app.title}`).then((v) => {
    if (v !== undefined) change((l) => opened(l, app.id, clock.date()));
  }), [act, change, clock]);
  const menuOf = useCallback((app, rect) => act('menu', [app.id, rect, { items: menuItems(layout, app.id) }], `show ${app.title}'s menu`).then((picked) => {
    if (picked === 'star') change((l) => star(l, app.id));
    else if (picked === 'unstar') change((l) => unstar(l, app.id));
    else if (picked === 'remove') change((l) => removeFromGroup(l, app.id));
  }), [act, change, layout]);
  // The open folder ({gid, editing}), with a history entry of its own so
  // back closes it; and the group menu shown ({gid, rect}).
  const [folder, setFolder] = useState(null);
  const [groupMenu, setGroupMenu] = useState(null);
  const reduced = useReducedMotion();
  const settled = useRef(() => {});
  const motion = useMemo(() => createMotion(clock, { reduced, onSettle: () => settled.current() }), [clock, reduced]);
  useEffect(() => () => motion.stop(), [motion]);
  // The pile fanned out ({gid, closing}), on the same history entry.
  const [fan, setFan] = useState(null);
  const moving = useRef(null);
  moving.current = { motion, reduced };
  const fanNow = useRef(null);
  fanNow.current = fan;
  const pushEntry = () => nav.enter('group');
  const popEntry = () => nav.leave('group');
  // Search: null while closed, else the field's text, with a history entry
  // of its own so back closes it.
  const [query, setQuery] = useState(null);
  const searching = query !== null;
  const searchOpen = useRef(false);
  searchOpen.current = searching;
  const closeSearch = useCallback(() => {
    if (!searchOpen.current) return;
    searchOpen.current = false;
    setQuery(null);
    nav.leave('search');
  }, []);
  const openGroup = useCallback((gid, editing = false) => {
    closeSearch();
    setGroupMenu(null);
    setFan(null);
    setFolder({ gid, editing });
    pushEntry();
  }, []);
  const closeGroup = useCallback(() => {
    setFolder(null);
    popEntry();
  }, []);
  // The fan's apps go back into the pile, from the event that gathers it,
  // and then the row goes.
  const closeFan = () => {
    const f = fanNow.current;
    if (!f || f.closing) return;
    const closing = { ...f, closing: true };
    fanNow.current = closing;
    setFan(closing);
    gatherFan(f.gid, moving.current.motion, moving.current.reduced, clock, () => setFan((x) => (x === closing ? null : x)));
  };
  const gather = useCallback(() => {
    if (!fanNow.current || fanNow.current.closing) return;
    closeFan();
    popEntry();
  }, []);
  // A tap on a pile fans it out, or gathers it when it is fanned already.
  const fanGroup = useCallback((gid) => {
    closeSearch();
    setGroupMenu(null);
    const f = fanNow.current;
    if (f && f.gid === gid && !f.closing) { gather(); return; }
    fanNow.current = { gid, closing: false };
    setFan(fanNow.current);
    pushEntry();
  }, []);
  const gatherNow = useCallback(() => {
    if (!fanNow.current) return;
    fanNow.current = null;
    setFan(null);
    popEntry();
  }, []);
  // Opening search gathers a fanned pile and closes the group menu first,
  // so its entry is the only one of the page's own.
  const openSearch = useCallback(() => {
    if (searchOpen.current) return;
    setGroupMenu(null);
    gatherNow();
    searchOpen.current = true;
    setQuery('');
    nav.enter('search');
  }, []);
  useEffect(() => {
    const close = () => { searchOpen.current = false; setQuery(null); };
    nav.listen();
    nav.closers.search = close;
    return () => { if (nav.closers.search === close) delete nav.closers.search; };
  }, []);
  // Escape closes search, unless a menu or a folder takes it.
  useEffect(() => {
    if (!searching) return undefined;
    const key = (e) => { if (e.key === 'Escape' && !document.querySelector('.h-folder, .h-group-menu, .h-more .h-menu')) closeSearch(); };
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [searching]);
  useEffect(() => {
    const close = () => { setFolder(null); closeFan(); };
    nav.listen();
    nav.closers.group = close;
    return () => { if (nav.closers.group === close) delete nav.closers.group; };
  }, []);
  // While a pile is fanned out, a tap outside its row and the group tiles,
  // or Escape, gathers it.
  const fanOpen = !!fan && !fan.closing;
  useEffect(() => {
    if (!fanOpen) return undefined;
    const click = (e) => { if (!(e.target instanceof Element && e.target.closest('.h-fan, .h-group'))) gather(); };
    const key = (e) => { if (e.key === 'Escape' && !document.querySelector('.h-folder, .h-group-menu, .h-more .h-menu')) gather(); };
    document.addEventListener('click', click);
    document.addEventListener('keydown', key);
    return () => { document.removeEventListener('click', click); document.removeEventListener('keydown', key); };
  }, [fanOpen]);
  // A folder, a fan or a menu whose group is gone (dissolved, ungrouped,
  // removed elsewhere) closes; so does a fan whose group became a folder.
  useEffect(() => {
    if (folder && layout && !layout.groups[folder.gid]) closeGroup();
    if (groupMenu && layout && !layout.groups[groupMenu.gid]) setGroupMenu(null);
    if (fan && layout && layout.groups[fan.gid]?.look !== 'pile') gatherNow();
  }, [layout, folder, groupMenu, fan]);
  const showGroupMenu = useCallback((gid, rect) => setGroupMenu({ gid, rect }), []);
  const closeGroupMenu = useCallback(() => setGroupMenu(null), []);
  const pickGroup = (what) => {
    const { gid } = groupMenu;
    setGroupMenu(null);
    if (what === 'rename') openGroup(gid, true);
    else if (what === 'look') {
      const look = layout.groups[gid]?.look === 'pile' ? 'folder' : 'pile';
      change((l) => setLook(l, gid, look));
    } else if (what === 'ungroup') change((l) => ungroup(l, gid));
  };
  // Clax keeps its own home screen over this page until it is told the
  // grid (or the empty state, or the load error) is drawn: once per load.
  const drawn = (Array.isArray(apps) && layout !== null) || !!appsError;
  const toldShown = useRef(false);
  useEffect(() => {
    if (!home || !drawn || toldShown.current) return;
    toldShown.current = true;
    // Clax may have opened an app from a tap on its image of this page
    // before it was drawn: that is an open from the home screen too.
    Promise.resolve(home.shown?.()).then((v) => {
      if (typeof v?.opened === 'string') change((l) => opened(l, v.opened, clock.date()));
    }).catch(() => {});
  }, [home, drawn]);
  settled.current = useTiles(home, drawn, motion);
  const [dragging, setDragging] = useState(false);
  usePullSearch(openSearch, !searching && drawn && !appsError);
  const goFirst = useRef(() => {});

  if (!home || (appsError && isRefused(appsError))) {
    return html`<main class="h-page h-away">
      <${Message} glyph="home" title="This is a Clax home screen." hint="Open it in the Clax app." />
    </main>`;
  }
  const list = Array.isArray(apps) && layout !== null ? apps : null;
  const going = Array.isArray(makes) ? makes : [];
  goFirst.current = () => {};
  let body;
  if (!list && appsError) {
    body = html`<${Message} glyph="cloudOff" title="Couldn't load your apps"
      action=${html`<button type="button" class="h-text-button" onClick=${reloadApps}>Try again</button>`} />`;
  } else if (!list) {
    body = null;
  } else if (searching && query.trim()) {
    const found = search(query, searchEntries(layout, list));
    goFirst.current = () => { if (found[0]) openApp(found[0].app); };
    body = html`<${Results} query=${query} found=${found} layout=${layout} apps=${list} clock=${clock} onOpen=${openApp} onMenu=${menuOf} />`;
  } else if (!list.length && !going.length) {
    body = html`<${Message} glyph="sparkles" title="Say what you want, and Clax makes it." hint="“A tracker for my runs with a weekly total”" />`;
  } else {
    body = html`<${Grid} layout=${layout} apps=${list} makes=${going} motion=${motion} reduced=${reduced} clock=${clock} change=${change} fan=${fan}
      onOpen=${openApp} onMenu=${menuOf} onOpenGroup=${openGroup} onFanGroup=${fanGroup} onGroupMenu=${showGroupMenu} act=${act} onDragging=${setDragging}
      gatherNow=${gatherNow} />`;
  }
  const entryOf = (gid) => (list ? sections(layout, list).all.find((e) => e.group?.id === gid) ?? null : null);
  const open = folder && entryOf(folder.gid);
  const menuFor = groupMenu && entryOf(groupMenu.gid);
  return html`<main class="h-page" data-dragging=${dragging ? '' : undefined}>
    <${Bar} act=${act} refresh=${refresh} unread=${typeof unread === 'number' ? unread : 0} onSearch=${openSearch}
      bigOn=${layout ? layout.bigTiles !== false : null} onBigTiles=${onBigTiles} />
    ${searching
      ? html`<${SearchField} query=${query} setQuery=${setQuery} onGo=${() => goFirst.current()} onClose=${closeSearch} />`
      : list && html`<${SearchGhost} />`}
    ${needsKey === true && html`<div class="h-key">
      <${Glyph} name="key" />
      <span class="h-key-text">Add your Claude API key to start making apps</span>
      <button type="button" class="h-text-button" onClick=${() => act('openSettings', [], 'open Settings')}>Add key</button>
    </div>`}
    ${body}
    ${open && html`<${Folder} key=${folder.gid} group=${open.group} apps=${open.apps} layout=${layout} motion=${motion} clock=${clock}
      change=${change} onOpen=${openApp} onMenu=${menuOf} close=${closeGroup} editing=${folder.editing} />`}
    ${menuFor && html`<${GroupMenu} group=${menuFor.group} rect=${groupMenu.rect} pick=${pickGroup} close=${closeGroupMenu} />`}
    ${notice && html`<button type="button" class="h-notice" role="status">${notice.text}</button>`}
    <button type="button" class="h-fab" aria-label="New app" onClick=${() => act('newApp', [], 'make a new app')}>
      <${Glyph} name="mic" />
      <span>New app</span>
    </button>
  </main>`;
}

let mounts = 0;

/** Shows the home screen in `root` over `home` (the `home` capability, or
 * null when it is unavailable) and `db` (the page's documents, where the
 * arrangement is kept; null keeps it in memory), with `clock` timing the
 * hold, the motion and the reads, replacing what was shown there. */
export function mount(root, home, { db = null, clock = realClock } = {}) {
  render(html`<${Home} key=${++mounts} home=${home ?? null} db=${db} clock=${clock} />`, root);
}

/** A `db` that refuses every read and write: the page's own documents
 * when they cannot be had, so each change says it could not be saved
 * instead of living in memory until the next load. */
const UNAVAILABLE = (() => {
  const refuse = () => Promise.reject(Object.assign(new Error('db is unavailable'), { code: 'unavailable' }));
  return Object.freeze({ doc: () => ({ get: refuse, acquire: refuse, set: refuse, onSnapshot: () => () => {} }) });
})();

/** Shows the home screen over the `home` and `db` capabilities `claude`
 * grants (the page's own, `window.claude`, by default). With `home` but no
 * `db`, the arrangement cannot be saved, and the page says so. */
export async function start(root = document.getElementById('app'), claude = window.claude) {
  let home = null;
  let db = null;
  try { home = (await claude?.use?.('home')) ?? null; } catch { home = null; }
  try { db = home ? ((await claude?.use?.('db')) ?? null) : null; } catch { db = null; }
  mount(root, home, { db: home && !db ? UNAVAILABLE : db });
}
