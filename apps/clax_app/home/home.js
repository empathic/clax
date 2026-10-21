// Clax's home screen: the person's apps and the makes going on, as a grid,
// with Inbox, Settings and Import app… above it and New app below. Every
// list and every action goes through the Clax app's `home` capability
// (`await claude.use("home")`), which only the app recorded as the home
// screen is granted; anywhere else the page says to open it in Clax.
//
// The arrangement (Starred, the order of All apps, which apps are new, how
// often each is opened) is the `layout/main` document in the page's own
// `db` (arrange.js, store.js). Every tile takes one cell.
//
// Each tile is placed in its cell (grid.js's `pack`) and moved by
// motion.js's springs on the page's clock: a held tile lifts and can be
// dragged across Starred and All apps, its neighbours making room; a quick
// release tosses it on (physics.js); Alt+arrows move a focused tile.
// Held still 600 ms over another app's icon's centre (which grows onto a
// folder's backing), a dragged app makes a group of the two, named from
// their icon words and offered for renaming at once; so held over a group,
// it joins it; either way the app flies into the group's preview. A toss
// never groups. A group's tile is a rounded square showing its first four
// apps' icons; a tap fans it out in place into a row below it, pushing the
// rows after it down, where its apps are reordered and dragged out of it;
// its own menu renames and ungroups it.
//
// The ⌕ in the top bar opens a
// search field above the grid (search.js ranks the matches): while it has
// text, every app whose title matches, groups' included, shows in one
// grid in their place. Enter opens the first; Escape, Backspace in the
// empty field, its close button and back close it. Under the window's own
// toolbar (`chrome().toolbar`) the page draws no bar or field: the
// toolbar's search field's text (`chrome().query`) filters the grid the
// same way.
//
// `start()` mounts the page over the real capabilities; `mount(root, home,
// {db, clock})` mounts it over any objects with the same calls (the tests
// pass kit/fake-home.js's fakeHome and fakeDb).
import { html, render, useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from './kit/preact.js';
import { symbol } from './symbols.js';
import {
  reconcile, normalize, sections, star, unstar, opened, isNew, moveTo, isGroupKey, groupOf,
  groupNameOf, makeGroup, addToGroup, removeFromGroup, moveInGroup, renameGroup, ungroup,
} from './arrange.js';
import { layoutStore } from './store.js';
import { realClock, velocityOf, tossRest, TOSS_MIN } from './physics.js';
import { pack, geometry, cellBox, slotAt, centreUnder, iconCentre, tileLabel } from './grid.js';
import { createPress } from './drag.js';
import { createMotion } from './motion.js';
import { search } from './search.js';
import { WORDS } from './words.js';

/** Material Symbol `name` (symbols.js) in a span, `size` px square. */
const Glyph = ({ name, size = 24, class: cls }) =>
  html`<span class=${`h-glyph ${cls || ''}`} dangerouslySetInnerHTML=${{ __html: symbol(name, size) }} />`;

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

// The filled Material Symbol for an app's icon word (words.js).
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

/** The breathing mark of a make going on, in a ring filled to `done` of
 * `total` once its plan has features. */
function MakeRing({ done, total }) {
  const r = 16.5;
  const c = 2 * Math.PI * r;
  const f = Math.min(1, Math.max(0.04, done / total));
  return html`<span class="h-ring">
    <svg width="36" height="36" viewBox="0 0 36 36" aria-hidden="true">
      <circle class="h-ring-track" cx="18" cy="18" r=${r} />
      <circle class="h-ring-arc" cx="18" cy="18" r=${r} stroke-dasharray=${`${f * c} ${c}`} transform="rotate(-90 18 18)" />
    </svg>
    <${Mark} size=${20} working />
  </span>`;
}

/** A tile's count, as the app's ClaxBadge draws it, at its icon's corner. */
const TileBadge = ({ done, total }) => html`<span class="h-badge h-tile-badge" aria-hidden="true">${done}/${total}</span>`;

/** A 2D context to measure labels with; null where the page has none. */
let measurer;
function measureContext() {
  if (measurer === undefined) {
    try {
      measurer = document.createElement('canvas').getContext('2d') ?? null;
    } catch {
      measurer = null;
    }
  }
  return measurer;
}

/** Calls each watched element's function when its size changes. */
const sizeWatch = typeof ResizeObserver === 'function'
  ? (() => {
      const fns = new Map();
      const o = new ResizeObserver((entries) => { for (const e of entries) fns.get(e.target)?.(); });
      return (el, fn) => { fns.set(el, fn); o.observe(el); return () => { fns.delete(el); o.unobserve(el); }; };
    })()
  : (el, fn) => { addEventListener('resize', fn); return () => removeEventListener('resize', fn); };

/** The width inside `el`'s padding. */
function contentWidth(el) {
  const cs = getComputedStyle(el);
  return el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
}

/** A tile's label: `text` in at most `lines` lines (2 or 1) of whole
 * words, laid out by grid.js's tileLabel in the label's own font across
 * its tile's width, the last line cut with an ellipsis (`data-cut`) when
 * it is too wide. Until it is measured, or where the page cannot measure,
 * CSS wraps it between words. */
function Label({ text, lines = 2 }) {
  const ref = useRef(null);
  const [laid, setLaid] = useState(null);
  useLayoutEffect(() => {
    const el = ref.current;
    const box = el?.parentElement;
    const ctx = measureContext();
    if (!box || !ctx) return undefined;
    let live = true;
    const lay = () => {
      if (!live) return;
      const width = contentWidth(box);
      if (!(width > 0)) return;
      const cs = getComputedStyle(el);
      ctx.font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
      const next = tileLabel(text, { measure: (t) => ctx.measureText(t).width, width, lines });
      setLaid((was) => (was && was.ellipsis === next.ellipsis && was.lines.join('\n') === next.lines.join('\n') ? was : next));
    };
    lay();
    document.fonts?.ready.then(lay);
    const stop = sizeWatch(box, lay);
    return () => { live = false; stop(); };
  }, [text, lines]);
  if (!laid) return html`<span ref=${ref} class=${`h-label ${lines < 2 ? 'h-one' : 'h-two'}`}>${text}</span>`;
  return html`<span ref=${ref} class="h-label" data-cut=${laid.ellipsis ? '' : undefined}>${laid.lines.map((l, i) => html`${i > 0 ? ' ' : ''}<span class="h-line">${l}</span>`)}</span>`;
}

/** App `app`'s icon; `slot` marks it as that app's place in a group's preview. */
function AppFace({ app, slot }) {
  const [broken, setBroken] = useState(false);
  let face = faceOf(app);
  if (face.kind === 'url' && broken) face = { kind: 'monogram', letters: monogramOf(app.title) };
  const style = face.kind === 'url' ? '' : toneStyle(face.hue ?? hueOf(app.id));
  return html`<span class="h-face" data-kind=${face.kind} data-app=${slot ? app.id : undefined} style=${style}>
    ${face.kind === 'url' && html`<img src=${face.url} alt="" width="60" height="60" draggable="false" onError=${() => setBroken(true)} />`}
    ${face.kind === 'emoji' && html`<span class="h-emoji">${face.emoji}</span>`}
    ${face.kind === 'word' && html`<${Glyph} name=${face.glyph} size=${28} />`}
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
 * group it is in, shows under its name (in search's results). */
function AppTile({ app, fresh, cell, dragging, group, ctl, onOpen, onMenu, tkey = tileKeyOf(app.id) }) {
  const b = app.build;
  const press = ctl.pressOf(app.id);
  const built = b ? `${app.title}, building, ${b.done} of ${b.total} done` : app.title;
  const said = group ? `${built}, in ${group}` : built;
  const label = fresh ? `${said}, new` : said;
  return html`<button type="button" class="h-tile" data-id=${app.id} data-key=${tkey} data-new=${fresh ? '' : undefined} data-building=${b ? '' : undefined}
    data-dragging=${dragging ? '' : undefined} aria-label=${label} style=${cellStyle(cell)}
    onPointerDown=${(e) => ctl.down(e, app.id)}
    onContextMenu=${(e) => { e.preventDefault(); if (!press.contextMenu()) onMenu(app, faceRect(e.currentTarget)); }}
    onClick=${() => { if (press.takeClick()) onOpen(app); }}
    onKeyDown=${(e) => ctl.key(app.id, e)}>
    <span class="h-icon">
      <span class="h-folder-back" aria-hidden="true" />
      <${AppFace} app=${app} />
      ${b && html`<span class="h-build-ring" aria-hidden="true" /><${TileBadge} done=${b.done} total=${b.total} />`}
      ${fresh && html`<span class="h-new" aria-hidden="true">New</span>`}
    </span>
    <${Label} text=${app.title} lines=${group ? 1 : 2} />
    ${group && html`<span class="h-status h-in-group">${group}</span>`}
  </button>`;
}

/** Group `entry`'s tile (`{key, group: {id, name}, apps}`), in cell
 * `cell`: a rounded square showing its first four apps' icons, two by two
 * (fewer apps fill fewer cells), over its name. Its press goes to `ctl` as
 * an app's does: a tap fans it out in place, or gathers it when it is
 * (`fanned`); a hold then a release or a context menu asks for the group's
 * own menu; a hold then a move drags it. It has no `data-id`: Clax is
 * never told of it, so a tap on the start-up image there opens nothing. */
function GroupTile({ entry, cell, dragging, fanned, ctl, onFanGroup, onGroupMenu }) {
  const { group, apps } = entry;
  const press = ctl.pressOf(entry.key);
  return html`<button type="button" class="h-tile h-group" data-group=${group.id} data-key=${entry.key}
    data-dragging=${dragging ? '' : undefined} data-fanned=${fanned ? '' : undefined} aria-expanded=${!!fanned}
    aria-label=${`${group.name}, group of ${apps.length} apps`} style=${cellStyle(cell)}
    onPointerDown=${(e) => ctl.down(e, entry.key)}
    onContextMenu=${(e) => { e.preventDefault(); if (!press.contextMenu()) onGroupMenu(group.id, faceRect(e.currentTarget)); }}
    onClick=${() => { if (press.takeClick()) onFanGroup(group.id); }}
    onKeyDown=${(e) => ctl.key(entry.key, e)}>
    <span class="h-icon">
      <span class="h-face h-folder-face" data-kind="folder">${apps.slice(0, 4).map((a) => html`<${AppFace} key=${a.id} app=${a} slot />`)}</span>
    </span>
    <${Label} text=${group.name} />
  </button>`;
}

const MAKE_STATUS = { making: 'Making…', offline: 'Can’t reach Claude', waiting: 'Waiting…', interrupted: 'Interrupted', failed: 'Couldn’t make it' };
const MAKE_GLYPH = { offline: 'cloud_off_fill', waiting: 'hourglass_empty', interrupted: 'replay', failed: 'error' };

/** Make `make`'s tile: its request on one line and how it stands on the
 * second, in that state's colour; its face the breathing mark on
 * `accent-tint` while it is made (in a ring with a "3/7" badge once its
 * plan has features), else the state's glyph. */
function MakeTile({ make, cell, act }) {
  const state = MAKE_STATUS[make.state] ? make.state : 'making';
  const status = MAKE_STATUS[state];
  const p = make.progress;
  const counted = state === 'making' && p && p.total > 0;
  const spoken = state === 'offline' ? 'Waiting for Claude' : status;
  return html`<button type="button" class="h-tile" data-make=${make.id} data-key=${`m:${make.id}`} style=${cellStyle(cell)} aria-label=${`${make.request}, ${spoken}`} onClick=${() => act('openMake', [make.id], `open ${make.request}`)}>
    <span class="h-icon">
      <span class=${`h-face h-make${state === 'failed' ? ' h-failed' : ''}`} data-kind="make">
        ${state === 'making' && (counted ? html`<${MakeRing} done=${p.done} total=${p.total} />` : html`<${Mark} size=${28} working />`)}
        ${state !== 'making' && html`<${Glyph} name=${MAKE_GLYPH[state]} size=${28} />`}
      </span>
      ${counted && html`<${TileBadge} done=${p.done} total=${p.total} />`}
    </span>
    <${Label} text=${make.request} lines=${1} />
    <span class="h-status" data-state=${state}>${status}</span>
  </button>`;
}

/** The Clax mark, ) · (, drawn as mark.css describes: at rest, or
 * breathing toward Clax while `working`. */
const Mark = ({ size = 24, working = false }) => html`<svg class=${`clax-mark h-mark${working ? ' is-working' : ''}`} viewBox="0 0 22 16" style=${`width: ${size}px`} aria-hidden="true">
  <path class="clax-mark-you" d="M1.5 2.5a5.5 5.5 0 0 1 0 11"/>
  <circle class="clax-mark-dot" cx="11" cy="8" r="1.7"/>
  <path class="clax-mark-agent" d="M20.5 2.5a5.5 5.5 0 0 0 0 11"/>
</svg>`;

/** An empty or error state, as the app's ClaxEmpty draws it: centred, the
 * mark (`glyph` "mark") or a glyph, the title, a muted hint, then the
 * action. */
function Message({ glyph, title, hint, action }) {
  return html`<div class="h-message">
    ${glyph === 'mark' ? html`<${Mark} size=${64} />` : html`<${Glyph} name=${glyph} size=${48} class="h-message-glyph" />`}
    <p class="h-message-title">${title}</p>
    ${hint && html`<p class="h-message-hint">${hint}</p>`}
    ${action}
  </div>`;
}

/** A standing message above the grid, as the app's ClaxBanner draws it: a
 * leading glyph (`glyph`, in `agent`, or `danger` with `danger`), the
 * title, an optional muted `body`, and `actions`, beside the text when the
 * page is wide and under it, at the end, when narrow. */
function Banner({ class: cls, glyph, danger, title, body, actions }) {
  return html`<div class=${`h-banner ${cls || ''}`}>
    <${Glyph} name=${glyph} class=${`h-banner-icon${danger ? ' h-danger' : ''}`} />
    <div class="h-banner-text">
      <p class="h-banner-title">${title}</p>
      ${body && html`<p class="h-banner-body">${body}</p>`}
    </div>
    ${actions && html`<div class="h-banner-actions">${actions}</div>`}
  </div>`;
}

/** The ⋮ menu: Import app… and Refresh. */
function More({ act, refresh }) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return undefined;
    const key = (e) => { if (e.key === 'Escape') setOpen(false); };
    addEventListener('keydown', key);
    return () => removeEventListener('keydown', key);
  }, [open]);
  return html`<span class="h-more">
    <button type="button" class="h-action" aria-label="More" aria-haspopup="menu" aria-expanded=${open} onClick=${() => setOpen(!open)}>
      <${Glyph} name="more_vert" />
    </button>
    ${open && html`<button type="button" class="h-scrim" aria-label="Close menu" onClick=${() => setOpen(false)} />
      <div class="h-menu" role="menu">
        <button type="button" role="menuitem" class="h-menu-item" onClick=${() => { setOpen(false); act('importApp', [], 'import an app'); }}>Import app…</button>
        <button type="button" role="menuitem" class="h-menu-item" onClick=${() => { setOpen(false); refresh(); }}>Refresh</button>
      </div>`}
  </span>`;
}

/** What the inbox's badge reads for `n` unread: the count to 99, then
 * "99+" (the app's ClaxBadge). */
export const badgeText = (n) => (n > 99 ? '99+' : String(n));

/** The top bar: the mark and "Clax", then Search, Inbox (with the unread
 * count), Settings and the ⋮ menu. */
function Bar({ act, refresh, unread, onSearch }) {
  return html`<header class="h-bar">
    <h1 class="h-title"><${Mark} size=${24} /><span>Clax</span></h1>
    <div class="h-actions">
      <button type="button" class="h-action" aria-label="Search" onClick=${onSearch}><${Glyph} name="search" /></button>
      <button type="button" class="h-action" aria-label=${unread > 0 ? `Inbox, ${unread} unread` : 'Inbox'} onClick=${() => act('openInbox', [], 'open the inbox')}>
        <span class="h-badged">
          <${Glyph} name="inbox" />
          ${unread > 0 && html`<span class="h-badge">${badgeText(unread)}</span>`}
        </span>
      </button>
      <button type="button" class="h-action" aria-label="Settings" onClick=${() => act('openSettings', [], 'open Settings')}><${Glyph} name="settings" /></button>
      <${More} act=${act} refresh=${refresh} />
    </div>
  </header>`;
}

/** The launcher's column count for a grid `width` wide: cells at most 112
 * px, 4 px apart (Flutter's SliverGridDelegateWithMaxCrossAxisExtent). */
/** The widest the grid is drawn, centred (home.css `.h-grid-wrap`). */
const GRID_MAX = 960;

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
 * entry's own key), "m:<make ID>", "p:<app ID>" in a fanned group's row,
 * or "r:<app ID>" in search's results. */
const tileKeyOf = (id) => `a:${id}`;
/** The tile key of an entry of Starred or All apps: an app's ID or a group's key. */
const entryTileKey = (key) => (isGroupKey(key) ? key : tileKeyOf(key));
/** The key of app `id`'s tile in a fanned group's row, which is also its
 * press's ID in the grid's arranger. */
const fanKeyOf = (id) => `p:${id}`;
const isFanKey = (key) => key.startsWith('p:');
/** The tile key of a press ID of the grid's arranger. */
const pressTileKey = (id) => (isFanKey(id) ? id : entryTileKey(id));

/** Each tile's cell by its key, with its section: Starred's apps, then
 * All apps' makes, apps and groups, each packed in `cols` columns, a cell
 * each. With group `fan` fanned out, the rows of All apps after its tile's
 * row move down one, leaving that row to its fan. */
function placement(layout, apps, makes, cols, fan) {
  const s = sections(layout, apps);
  const lists = {
    starred: s.starred.map((e) => ({ key: tileKeyOf(e.app.id), span: 1 })),
    all: [...makes.map((m) => ({ key: `m:${m.id}`, span: 1 })), ...s.all.map((e) => ({ key: entryTileKey(e.key), span: 1 }))],
  };
  const out = new Map();
  for (const [section, items] of Object.entries(lists)) {
    const { cells } = pack(items, cols);
    const g = section === 'all' && fan ? cells.find((c) => c.key === `g:${fan}`) : null;
    for (const c of cells) out.set(c.key, { ...c, row: g && c.row > g.row ? c.row + 1 : c.row, section });
  }
  return out;
}

/** The 1-based grid row of group `fan`'s fan, under its tile's row in
 * `cells` (placement's), or null. */
const fanRowOf = (cells, fan) => {
  const c = fan ? cells.get(`g:${fan}`) : null;
  return c ? c.row + 2 : null;
};

/** Group `entry`'s apps fanned out in grid row `row`, across the grid, in
 * a row of cells as wide as the grid's that scrolls sideways; each opens
 * on a tap, asks for its menu and drags as in the grid (through the
 * arranger's `ctl.fan`). */
function Fan({ entry, row, layout, ctl, onOpen, onMenu }) {
  return html`<div class="h-fan" data-group=${entry.group.id} role="group" aria-label=${entry.group.name}
    style=${{ gridColumn: '1 / -1', gridRow: `${row}` }}>
    ${entry.apps.map((a) => html`<${AppTile} key=${fanKeyOf(a.id)} tkey=${fanKeyOf(a.id)} app=${a} fresh=${isNew(layout, a.id)}
      ctl=${ctl.fan} onOpen=${onOpen} onMenu=${onMenu} />`)}
  </div>`;
}

/** Starred (only when something is starred, headed) above All apps (headed
 * then), each a grid of `cols` columns; the makes go first under All apps.
 * A group takes a cell as its tile; the group `fan.gid`
 * fanned out has its row of apps under it. While app `drag` is dragged,
 * All apps is headed, and an empty Starred is a zone to drop on (a group
 * is never starred: dragging one opens no zone). */
function Sections({ layout, apps, makes, cols, drag, fan, ctl, onOpen, onMenu, onFanGroup, onGroupMenu, act }) {
  const s = sections(layout, apps);
  const fanned = fan ? s.all.find((e) => e.group?.id === fan.gid) ?? null : null;
  const cells = placement(layout, apps, makes, cols, fanned ? fan.gid : null);
  const tile = (app) => html`<${AppTile} key=${tileKeyOf(app.id)} app=${app} fresh=${isNew(layout, app.id)} cell=${cells.get(tileKeyOf(app.id))}
    dragging=${drag === app.id} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} />`;
  const starred = s.starred.length > 0;
  const zone = !starred && drag != null && !isGroupKey(drag);
  const entry = (e) => {
    if (e.app) return tile(e.app);
    const group = html`<${GroupTile} key=${e.key} entry=${e} cell=${cells.get(e.key)} dragging=${drag === e.key} fanned=${e === fanned}
      ctl=${ctl} onFanGroup=${onFanGroup} onGroupMenu=${onGroupMenu} />`;
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

/** How long a dragged app's pointer rests within CENTRE_PX of another
 * tile's icon's centre (grid.js iconCentre), moving at most STILL_PX,
 * before a drop there groups. */
const HOVER_MS = 600;
const CENTRE_PX = 24;
const STILL_PX = 8;
/** How long the pointer rests in a new slot before the neighbours make
 * room there, on a tile's centre as anywhere else. */
const REORDER_MS = 120;

/** How long a dropped app flies into its group's preview, and how long
 * the preview then takes to settle, in ms. */
const FLY_MS = 280;
const SETTLE_MS = 240;

/** App `g.id`, let go with its icon at viewport rect `g.from` (`g.face`, a
 * copy of it), flies into group tile `tile`'s preview: to its own place
 * there when it is one of the first four, which shows it once it lands,
 * else into the preview's middle, shrinking away; then the preview
 * settles. Nothing flies with `reduced`, or where the page cannot animate. */
function gatherInto(tile, g, reduced, clock) {
  const preview = tile.querySelector('.h-folder-face');
  if (reduced || !preview || typeof preview.animate !== 'function') return;
  const slot = preview.querySelector(`[data-app="${CSS.escape(g.id)}"]`);
  const to = (slot ?? preview).getBoundingClientRect();
  const ghost = document.createElement('div');
  ghost.className = 'h-gather';
  ghost.setAttribute('aria-hidden', 'true');
  ghost.style.cssText = `left:${g.from.x}px;top:${g.from.y}px;--face:${g.from.w}px`;
  ghost.append(g.face);
  document.body.append(ghost);
  if (slot) slot.style.visibility = 'hidden';
  const dx = to.left + to.width / 2 - (g.from.x + g.from.w / 2);
  const dy = to.top + to.height / 2 - (g.from.y + g.from.h / 2);
  const scale = slot ? to.width / g.from.w : 0.25;
  let landed = false;
  const land = () => {
    if (landed) return;
    landed = true;
    ghost.remove();
    if (slot) slot.style.visibility = '';
    preview.animate([{ transform: 'scale(1.1)' }, { transform: 'scale(0.97)', offset: 0.6 }, { transform: 'scale(1)' }], { duration: SETTLE_MS, easing: 'ease-out' });
  };
  const fly = ghost.animate(
    [{ transform: 'none', opacity: 1 }, { transform: `translate(${dx}px, ${dy}px) scale(${scale})`, opacity: slot ? 1 : 0 }],
    { duration: FLY_MS, easing: 'cubic-bezier(0.2, 0, 0, 1)', fill: 'forwards' },
  );
  fly.finished.then(land, land);
  // A page that runs no animations (hidden) still puts the icon in place.
  clock.after(FLY_MS * 4, land);
}

/** The grid's arranging, over the state `st` the Grid keeps current
 * (`wrap`, `layout`, `shown` (what the page shows, a drop's or a key's
 * move included before its write lands: every move starts from it),
 * `apps`, `makes`, `cols`, `fanGid` (the group fanned out, or null),
 * `motion`, `reduced`, `change`, `onMenu`, `onGroupMenu`, `onMade`,
 * `setDrag`, `setHeld`, `onDragging`, `gatherNow`): each entry's press (`pressOf`, by
 * app ID or group key, or by fan key for an app in the fanned group's row,
 * through `fan`), the drag it starts, and the keyboard's moves. During a
 * drag the tiles are placed and moved here, at once, with no render; the
 * page renders the drag's start and its drop. Dragging a fanned group
 * gathers it at once; any other drag leaves its row open, and an app in
 * that row drags along it (its neighbours there making room) or out of it
 * into the grid, out of the group. The neighbours make room wherever the pointer rests, a tile's
 * centre included; a dragged app whose pointer stays HOVER_MS near another
 * tile's centre, all but still, brings that tile back under it, outlined,
 * and a drop then makes a group of the two apps or adds the app to the
 * group, and the app flies into the group's preview (gatherInto). A toss
 * never groups. A fanned group's apps spring out of its tile
 * as its row opens. */
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
  // The tile whose icon's centre the dragged app's pointer is near: {key,
  // section, c (that centre, from its grid's top left, where it was drawn
  // as the hover began), still (where the pointer's rest began), before
  // (the drag's slot then, in which the tile is under the pointer), ready
  // (once a drop there groups), cancel}.
  let hover = null;
  // A slot the pointer moved to, waiting REORDER_MS: {t, cancel}.
  let pending = null;
  // The key of a group a drop just made, to scroll into view once drawn.
  let reveal = null;
  // An app dropped into a group, to fly into its preview once drawn:
  // {gid, id, from (its icon's viewport rect as let go), face (a copy)}.
  let gathering = null;
  const tileEl = (key) => st.wrap?.querySelector(`.h-tile[data-key="${CSS.escape(key)}"]`) ?? null;
  const gridOf = (section) => st.wrap?.querySelector(`.h-section[data-section="${section}"] .h-grid`) ?? null;

  /** Places every tile in its cell of the shown arrangement (the drag's
   * while one is on) and springs each one that moved from where it was.
   * With `steady`, a render that moved All apps' grid up or down (the drop zone
   * and the headings coming or going) scrolls the page with it, so the
   * tiles stay under the finger. */
  function relayout(steady = false) {
    if (!st.wrap) return;
    const cells = placement(drag?.prov ?? st.shown, st.apps, st.makes, st.cols, st.fanGid);
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
    }
    // The fanned group's row follows its tile; while one of its apps is
    // dragged along it, the app the drag would go before makes room.
    const fanEl = st.wrap.querySelector('.h-fan');
    if (fanEl) {
      const row = fanRowOf(cells, st.fanGid);
      if (row && fanEl.style.gridRow !== String(row)) fanEl.style.gridRow = String(row);
      const gap = drag?.fan && drag.target?.section === 'fan' ? drag.target.index : -1;
      const w = drag?.fan ? `${(tileEl(drag.key)?.offsetWidth ?? 0) + 4}px` : '';
      [...fanEl.querySelectorAll('.h-tile[data-key]')].filter((el) => el.dataset.key !== drag?.key).forEach((el, i) => {
        const m = i === gap ? w : '';
        if (el.style.marginLeft !== m) el.style.marginLeft = m;
      });
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
    const fanTile = st.fanGid ? tileEl(`g:${st.fanGid}`) : null;
    const from = fanTile ? layoutPos(fanTile) : null;
    els.forEach((el, i) => {
      const key = el.dataset.key;
      const p = layoutPos(el, offsets[i]);
      next.set(key, p);
      if (drag && key === drag.key) return;
      // A fanned group's app new to the page comes out of the group's tile.
      if (isFanKey(key) && !pos.has(key)) {
        if (from && animate) moves.push([key, el, from.x - p.x, from.y - p.y, 0, 0]);
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
   * there, the one whose cell, packed after the entries before it, has its
   * centre nearest the pointer. Nearest a make's cell, the first slot.
   * With a group fanned out, the cells past its row are a row lower, as
   * placement draws them. */
  function slotIn(section, p) {
    const grid = gridOf(section);
    const entries = sections(st.shown, st.apps)[section].filter((e) => e.key !== drag.id);
    if (!grid) return { section, index: 0 };
    const lead = Array.from({ length: section === 'all' ? st.makes.length : 0 }, () => ({ key: null, span: 1 }));
    const r = grid.getBoundingClientRect();
    const g = geometry(r.width, st.cols);
    const q = { x: p.x - r.left, y: p.y - r.top };
    const far = (cell) => {
      const b = cellBox(cell, g);
      return Math.hypot(b.x + b.w / 2 - q.x, b.y + b.h / 2 - q.y);
    };
    const items = entries.map((e) => ({ key: e.key, span: 1 }));
    const fanned = section === 'all' && st.fanGid ? `g:${st.fanGid}` : null;
    let index = 0;
    let dist = Math.min(Infinity, ...pack(lead, st.cols).cells.map(far));
    for (let i = 0; i <= entries.length; i++) {
      // The dragged entry's cell at slot `i`: packing the entries before it
      // is enough, unless a fanned group after it moves the rows past its own.
      const list = [...lead, ...items.slice(0, i), { key: null, span: 1 }, ...(fanned ? items.slice(i) : [])];
      const { cells } = pack(list, st.cols);
      let c = cells[lead.length + i];
      if (fanned) {
        const pc = cells.find((x) => x.key === fanned);
        if (pc && c.row > pc.row) c = { ...c, row: c.row + 1 };
      }
      const d = far(c);
      if (d < dist) { dist = d; index = i; }
    }
    return { section, index };
  }

  /** The slot among the fanned group's other apps at viewport point `p`,
   * for an app of it dragged, when `p` is over its row: {section: "fan",
   * index}; else null. */
  function fanSlot(p) {
    const row = drag.fan ? st.wrap?.querySelector('.h-fan') : null;
    if (!row || !inside(row.getBoundingClientRect(), p)) return null;
    const others = (st.shown.groups[drag.fan]?.apps ?? []).filter((id) => id !== drag.id);
    const w = (tileEl(drag.key)?.offsetWidth ?? 1) + 4;
    const x = p.x - row.getBoundingClientRect().left + row.scrollLeft;
    return { section: 'fan', index: clamp(Math.floor(x / w), 0, others.length) };
  }

  /** Where a drop at viewport point `p` puts the dragged entry: a group
   * stays in All apps; an app of the fanned group over its row moves along
   * it. */
  function targetAt(p) {
    if (isGroupKey(drag.id)) return slotIn('all', p);
    const fan = fanSlot(p);
    if (fan) return fan;
    const star = st.wrap?.querySelector('.h-section[data-section="starred"]');
    if (star && inside(star.getBoundingClientRect(), p)) {
      return star.classList.contains('h-drop') ? { section: 'starred', index: 0, zone: true } : slotIn('starred', p);
    }
    return slotIn('all', p);
  }

  /** Viewport point `p` from `section`'s grid's top left, or null. */
  function inGrid(section, p) {
    const r = gridOf(section)?.getBoundingClientRect();
    return r ? { x: p.x - r.left, y: p.y - r.top } : null;
  }

  /** The tile, as drawn, whose icon's centre is within CENTRE_PX of the dragged
   * app's pointer `p` (another app's or a group's): {key, section, c},
   * or null. */
  function hoverAt(p) {
    if (isGroupKey(drag.id)) return null;
    const cells = placement(drag.prov ?? st.shown, st.apps, st.makes, st.cols, st.fanGid, clock.date());
    for (const section of ['starred', 'all']) {
      const grid = gridOf(section);
      if (!grid) continue;
      const r = grid.getBoundingClientRect();
      if (!inside(r, p)) continue;
      const list = [...cells.values()].filter((c) => c.section === section);
      const g = geometry(r.width, st.cols);
      const i = centreUnder(list, g, p.x - r.left, p.y - r.top, CENTRE_PX);
      const key = i < 0 ? null : list[i].key;
      if (!key || key === drag.key || key === `g:${drag.fan}` || key.startsWith('m:')) return null;
      return { key, section, c: iconCentre(list[i], g) };
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

  /** Whether viewport point `p` is still within CENTRE_PX of the hovered
   * tile's icon's centre, where it was drawn as the hover began. */
  function onHover(p) {
    const q = hover && inGrid(hover.section, p);
    return !!q && Math.hypot(q.x - hover.c.x, q.y - hover.c.y) <= CENTRE_PX;
  }

  /** Follows the hover: the pointer leaving the hovered centre ends it, a
   * move of more than STILL_PX starts its rest again, and a rest of
   * HOVER_MS readies it; with none, a centre under the pointer starts one.
   * Ready, the tile is put back under the pointer (the neighbours may have
   * made room since) and outlined. */
  function followHover(p, none) {
    if (hover && (none || !onHover(p))) clearHover();
    if (!hover) {
      const at = none ? null : hoverAt(p);
      if (!at) return;
      hover = { ...at, still: null, before: drag.target, ready: false, cancel: null };
    }
    if (hover.ready) return;
    const h = hover;
    const q = inGrid(h.section, p);
    if (h.still && Math.hypot(q.x - h.still.x, q.y - h.still.y) <= STILL_PX) return;
    h.still = q;
    h.cancel?.();
    h.cancel = clock.after(HOVER_MS, () => {
      if (hover !== h || !drag) return;
      h.ready = true;
      cancelPending();
      if (!sameSlot(h.before, drag.target)) {
        if (h.before) commit(h.before);
        else { drag.target = null; drag.prov = null; relayout(); }
      }
      tileEl(h.key)?.setAttribute('data-grouping', '');
      // The dragged tile's name gives way to the one it groups with.
      tileEl(drag.key)?.setAttribute('data-grouping-with', '');
    });
  }

  const cancelPending = () => { pending?.cancel(); pending = null; };
  const sameSlot = (a, b) => !!a && !!b && a.section === b.section && a.index === b.index && !!a.zone === !!b.zone;
  /** The neighbours make room for the dragged entry at slot `t`: in its
   * fanned group's row, the apps there; anywhere else, the grid's tiles. */
  function commit(t) {
    drag.target = t;
    drag.prov = t.section === 'fan' ? null : moveTo(st.shown, drag.id, t.section, t.index);
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

  /** Follows the pointer: once it rests REORDER_MS in a new slot, the
   * neighbours make room there; near another tile's centre it also hovers
   * that tile (followHover), and once that hover is ready nothing moves
   * until the pointer leaves the centre. */
  function track() {
    const t = targetAt(drag.last);
    st.wrap?.querySelector('.h-drop')?.toggleAttribute('data-over', !!t.zone);
    sideOfZone();
    followHover(drag.last, !!t.zone);
    if (hover?.ready) { cancelPending(); return; }
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
    if (d.fan && el) unfix(d.key, el);
  }

  /** Lifts app tile `el` (fan key `key`) of a fanned group's row out of the
   * row's clipping, where it is, for a drag: the row closes up behind it. */
  function fix(el) {
    const t = offsetOf(el);
    const r = el.getBoundingClientRect();
    Object.assign(el.style, { position: 'fixed', left: `${r.left - t.x}px`, top: `${r.top - t.y}px`, width: `${r.width}px`, height: `${r.height}px`, margin: '0' });
    el.dataset.dragging = '';
  }

  /** Puts a tile `fix` lifted back in its row, springing from where it shows. */
  function unfix(key, el) {
    const seen = el.getBoundingClientRect();
    for (const k of ['position', 'left', 'top', 'width', 'height', 'margin']) el.style[k] = '';
    delete el.dataset.dragging;
    const t = offsetOf(el);
    const r = el.getBoundingClientRect();
    st.motion.from(key, el, seen.left - (r.left - t.x), seen.top - (r.top - t.y));
    pos.set(key, layoutPos(el));
  }

  /** App `id` dropped on hovered tile `target` (an app's or a group's
   * key): a new group of the two, named from their icon words, scrolled
   * into view once drawn and its name offered for editing once written
   * (`onMade`), or
   * the app added to the group; shown at once and written, the app flying
   * from where it was let go into the group's preview. */
  function groupDrop(d, target) {
    let op;
    let gid;
    if (isGroupKey(target)) {
      gid = target.slice(2);
      op = (l) => addToGroup(l, gid, d.id);
    } else {
      const other = target.slice(2);
      gid = Math.random().toString(36).slice(2, 10);
      const name = groupNameOf([other, d.id].map((id) => st.apps.find((a) => a.id === id)));
      op = (l) => makeGroup(l, other, d.id, gid, name);
      // Made on a starred app, it lands under All apps, maybe off screen.
      reveal = `g:${gid}`;
    }
    const face = tileEl(d.key)?.querySelector('.h-icon > .h-face');
    if (face) {
      const r = face.getBoundingClientRect();
      gathering = { gid, id: d.id, from: { x: r.left, y: r.top, w: r.width, h: r.height }, face: face.cloneNode(true) };
    }
    const made = reveal === `g:${gid}`;
    clearHover();
    d.prov = op(st.shown);
    st.motion.let(d.key);
    end(d);
    const mine = ++seq;
    st.shown = d.prov;
    st.setHeld(d.prov);
    st.change(op).then((written) => {
      if (mine === seq) st.setHeld(null);
      // A new group's name is offered once it is written.
      if (made && written) st.onMade(gid);
    });
  }

  /** The press of `pressId`'s tile: an entry of Starred or All apps (an
   * app's ID or a group's key), or an app in the fanned group's row (its fan
   * key), which drags along the row or out of it into the grid. */
  function handlersFor(pressId) {
    const inFan = isFanKey(pressId);
    const id = inFan ? pressId.slice(2) : pressId;
    const key = pressTileKey(pressId);
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
        // A fanned group dragged gathers; the fan's row stays for any other.
        if (id === `g:${st.fanGid}`) st.gatherNow();
        const fan = inFan ? groupOf(st.shown, id) : null;
        if (inFan && !fan) return;
        const r = el.getBoundingClientRect();
        focus = null;
        const from = fan ? 'fan' : st.shown.starred.includes(id) ? 'starred' : 'all';
        drag = { id, key, fan, from, grab: { x: p.x - r.left, y: p.y - r.top }, last: p, target: null, prov: null };
        if (fan) {
          fix(el);
          drag.target = { section: 'fan', index: st.shown.groups[fan].apps.indexOf(id) };
          relayout();
        }
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
        // A release at speed is a toss, never a group.
        const v = st.reduced ? { vx: 0, vy: 0 } : velocityOf(samples);
        const toss = Math.hypot(v.vx, v.vy) > TOSS_MIN;
        if (!toss && hover?.ready && onHover(p)) { groupDrop(d, hover.key); return; }
        let t = targetAt(p);
        if (t.section === 'fan') { fanDrop(d, t); return; }
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
          // It lands in the other section (or, out of a fanned group, in the
          // grid) once that renders.
          const r = el.getBoundingClientRect();
          landing = { key: entryTileKey(id), x: r.left + scrollX, y: r.top + scrollY, vx: toss ? v.vx : 0, vy: toss ? v.vy : 0 };
          if (d.fan) el.style.visibility = 'hidden';
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
        d.target = null;
        relayout();
        st.motion.let(key);
        end(d);
      },
    };
  }

  /** App `d.id` of a fanned group dropped on its row at slot `t`: moved
   * there along its group, shown at once and written. */
  function fanDrop(d, t) {
    const gid = d.fan;
    const app = d.id;
    end(d);
    relayout();
    const next = moveInGroup(st.shown, gid, app, t.index);
    st.shown = next;
    const mine = ++seq;
    st.setHeld(next);
    st.change((l) => moveInGroup(l, gid, app, t.index)).then(() => { if (mine === seq) st.setHeld(null); });
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
     * nothing stays held. */
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
      const shown = reveal && tileEl(reveal);
      if (shown) { reveal = null; shown.scrollIntoView({ block: 'nearest' }); }
      const g = gathering;
      if (g && st.shown.groups[g.gid]?.apps.includes(g.id) && tileEl(`g:${g.gid}`)) {
        gathering = null;
        gatherInto(tileEl(`g:${g.gid}`), g, st.reduced, clock);
      }
      if (drag) track();
    },
    /** Alt+arrows on entry `id`'s tile (an app's ID or a group's key): one
     * entry left or right, or a row (`cols` entries) up or down, within its
     * section, from where it is shown; on an app in the fanned group's row
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
  /** The fanned group's tiles' calls, by app ID, onto their fan keys. */
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

/** The longest a fan's gathering waits for its apps to reach the group's tile. */
const GATHER_MS = 1000;

/** Springs each app of the fanned row back onto group `gid`'s tile, then
 * calls `done` (at once with `reduced`, or with no row or tile; at the
 * latest GATHER_MS on, should a later layout move a tile on). */
function gatherFan(gid, motion, reduced, clock, done) {
  const els = [...document.querySelectorAll('.h-fan .h-tile[data-key]')];
  const tileEl = document.querySelector(`.h-tile[data-key="${CSS.escape(`g:${gid}`)}"]`);
  if (reduced || !tileEl || !els.length) { done(); return; }
  const to = layoutPos(tileEl);
  let left = els.length;
  let stop = null;
  const end = () => { stop?.(); done(); };
  for (const el of els) {
    const p = layoutPos(el);
    motion.to(el.dataset.key, el, to.x - p.x, to.y - p.y, () => { if (--left === 0) end(); });
  }
  if (left > 0) stop = clock.after(GATHER_MS, done);
}

/** The grid of Starred and All apps, arranged by `arranger`. `fan`
 * ({gid, closing}, or null) is the group fanned out: its row shows under
 * it, still shown while `closing` (its apps going back into the group's
 * tile, by `gatherFan`). `gatherNow()` drops the fan with no motion, as a
 * drag of its group starts. `onMade(gid)` hears of a group a drop made. */
function Grid({ layout, apps, makes, motion, reduced, clock, change, fan, onOpen, onMenu, onFanGroup, onGroupMenu, onMade, act, onDragging, gatherNow }) {
  const ref = useRef(null);
  const cols = useColumns(ref);
  const [drag, setDrag] = useState(null);
  // The arrangement a drop made, shown until its write settles.
  const [held, setHeld] = useState(null);
  const shown = held ?? layout;
  const fanGid = fan && shown.groups[fan.gid] ? fan.gid : null;
  const st = useRef({}).current;
  Object.assign(st, { wrap: ref.current, layout, shown, apps, makes, cols, fanGid, motion, reduced, change, onMenu, onGroupMenu, onMade, setDrag, setHeld, onDragging, gatherNow });
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
    <${Sections} layout=${shown} apps=${apps} makes=${makes} cols=${cols} drag=${drag} fan=${fanGid ? fan : null} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu}
      onFanGroup=${onFanGroup} onGroupMenu=${onGroupMenu} act=${act} />
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
      <button type="button" class="h-search-close" aria-label="Close search" onClick=${onClose}><${Glyph} name="close" /></button>
    </label>
  </div>`;
}

/** Group `group`'s own menu, anchored at its tile's icon `rect`: Rename
 * and Ungroup; `pick` gets "rename" or "ungroup". The scrim, Escape or
 * back closes it. */
function GroupMenu({ group, rect, pick, close }) {
  useEffect(() => {
    const key = (e) => { if (e.key === 'Escape') close(); };
    addEventListener('keydown', key);
    return () => removeEventListener('keydown', key);
  }, [close]);
  const below = rect.y + rect.h + 8 + 112 <= innerHeight;
  const left = clamp(rect.x, 8, Math.max(8, innerWidth - 8 - 200));
  const style = `left:${left}px;${below ? `top:${rect.y + rect.h + 8}px` : `bottom:${innerHeight - rect.y + 8}px`}`;
  const item = (what, label) => html`<button type="button" role="menuitem" class="h-menu-item" onClick=${() => pick(what)}>${label}</button>`;
  return html`<button type="button" class="h-scrim h-group-scrim" aria-label="Close menu" onClick=${close} />
    <div class="h-menu h-group-menu" role="menu" aria-label=${group.name} style=${style}>
      ${item('rename', 'Rename')}
      ${item('ungroup', 'Ungroup')}
    </div>`;
}

/** The widest a group's name field is drawn, in px. */
const NAME_W = 220;

/** Group `group`'s name in a field over its tile's name, focused with all
 * of it selected (a new group's suggested name, or Rename's): Enter or a
 * tap away keeps what it says (`rename(name)`, an empty or unchanged name
 * changing nothing); Escape and back keep the name it had (`close()`). */
function GroupName({ group, rename, close }) {
  const field = useRef(null);
  const done = useRef(false);
  const [at, setAt] = useState(null);
  useLayoutEffect(() => {
    let frame = 0;
    const place = () => {
      const tile = document.querySelector(`.h-tile[data-key="${CSS.escape(`g:${group.id}`)}"]`);
      if (!tile) { frame = requestAnimationFrame(place); return; }
      const r = tile.getBoundingClientRect();
      const label = tile.querySelector('.h-label')?.getBoundingClientRect() ?? r;
      const w = Math.min(NAME_W, document.documentElement.clientWidth - 16);
      const x = clamp(r.left + r.width / 2 - w / 2, 8, document.documentElement.clientWidth - 8 - w);
      setAt({ x: x + scrollX, y: label.top - 6 + scrollY, w });
    };
    place();
    return () => cancelAnimationFrame(frame);
  }, [group.id]);
  useLayoutEffect(() => {
    const f = field.current;
    if (!at || !f || document.activeElement === f) return;
    f.focus({ preventScroll: true });
    // All of the name selected, from its start: a long one shows its first
    // words, not its last.
    f.setSelectionRange(0, f.value.length, 'backward');
    f.scrollLeft = 0;
  }, [at]);
  // A field taken away (back, the group gone) keeps nothing: the browser
  // may blur it as it goes.
  useLayoutEffect(() => () => { done.current = true; }, []);
  const keep = (value) => {
    if (done.current) return;
    done.current = true;
    rename(String(value ?? '').trim());
  };
  return html`<button type="button" class="h-scrim h-name-scrim" aria-label="Done" tabindex="-1" onClick=${() => keep(field.current?.value)} />
    <div class="h-name" style=${at ? `left:${at.x}px;top:${at.y}px;width:${at.w}px` : 'visibility:hidden'}>
      <input ref=${field} class="h-name-input" aria-label="Group name" maxlength="40" value=${group.name} enterkeyhint="done"
        autocomplete="off" spellcheck="false"
        onKeyDown=${(e) => {
          if (e.key === 'Enter') { e.preventDefault(); keep(e.currentTarget.value); }
          else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done.current = true; close(); }
        }}
        onBlur=${(e) => keep(e.currentTarget.value)} />
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

/** What a Clax without `chrome()` (an older app) is taken to be: a phone
 * by touch, with no toolbar of its own. */
export const PHONE_CHROME = Object.freeze({ platform: 'android', form: 'phone', toolbar: false, pointer: 'touch' });

/** `home.chrome()`'s answer, or PHONE_CHROME when it has none or refuses. */
function chromeOf(home) {
  if (typeof home.chrome !== 'function') return PHONE_CHROME;
  return Promise.resolve()
    .then(() => home.chrome())
    .then((c) => (c && typeof c === 'object' ? { ...PHONE_CHROME, ...c } : PHONE_CHROME), () => PHONE_CHROME);
}

/** The words for pointing in `chrome`'s pointer: "Click" with a mouse or a
 * trackpad, "Tap" by touch. */
export const verbsOf = (chrome) => (chrome?.pointer === 'mouse' ? { tap: 'Click' } : { tap: 'Tap' });

const NOTICE_MS = 4000;

/** `act(method, args, what)`: calls `home[method](...args)` and resolves
 * its value, or `undefined` when it failed. `no_gesture` (the call did not
 * come with the person's tap) keeps the call for the next tap on the page
 * that lands on no other control, and says "Tap again to <what>" (in
 * `verbs`' word: "Click again" with a mouse); the
 * promise then resolves with the retried call's value, or `undefined` once
 * the kept call is dropped. Another failure says so briefly. A new action,
 * a tap on another control, or the notice going drops a kept one. */
function useActions(home, verbs) {
  const [notice, setNotice] = useState(null);
  const tapWord = useRef(verbs.tap);
  tapWord.current = verbs.tap;
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
          say(`${tapWord.current} again to ${what}`);
          pending.current = { method, args, what, resolve };
        });
      }
      if (code === 'bad_request' && args.length) say('That app is no longer here');
      else say(`Couldn’t ${what}`);
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
const UNSAVED_TEXT = 'Couldn’t save your arrangement';

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
 * "group" while a group is fanned out, "menu" while a group's own menu
 * is open (over a fanned group's entry, when one is), "name" while a
 * group's name field is (over either),
 * "search" while search is open. Each entry's state names its kind
 * (`clax`) and the page's entries under it (`under`, bottom first).
 * `enter(kind)` takes kind's entry; `leave(kind)` goes back off it, and off
 * any entry above it, or undoes an enter of kind still waiting. `backs` counts the entries being gone back off and
 * `going` those the traversal on its way leaves: a leave while one is on
 * its way adds to `backs`, and the rest go once it lands, so each leave
 * takes its own entry. `push` names the entry to take once every back has
 * landed, unless the last one landed on one of that kind. Any other back
 * calls the `closers` of each kind whose entry is no longer the top one. */
const nav = {
  backs: 0,
  going: 0,
  push: null,
  closers: {},
  listening: false,
  /** The page's own entries, bottom first, as history stands now. */
  stack() {
    const s = history.state;
    return s?.clax ? [...(Array.isArray(s.under) ? s.under : []), s.clax] : [];
  },
  /** Takes kind's entry over the page's others. */
  take(kind) {
    history.pushState({ clax: kind, under: this.stack() }, '', `#${kind}`);
  },
  listen() {
    if (this.listening) return;
    this.listening = true;
    addEventListener('popstate', () => {
      if (this.going) {
        this.backs -= this.going;
        this.going = 0;
        if (this.backs > 0) { this.going = this.backs; history.go(-this.backs); return; }
        const kind = this.push;
        this.push = null;
        if (kind && history.state?.clax !== kind) this.take(kind);
        return;
      }
      const top = history.state?.clax;
      for (const [kind, close] of Object.entries(this.closers)) if (top !== kind) close?.();
    });
  },
  enter(kind) {
    if (this.backs) this.push = kind;
    else if (history.state?.clax !== kind) this.take(kind);
  },
  leave(kind) {
    // An enter still waiting for the backs to land is undone, and with it
    // the leave: the entry it would have taken or reused stays as it is.
    if (this.push === kind) { this.push = null; return; }
    // The entries as they will stand once the backs on their way land.
    const ahead = this.stack();
    ahead.length = Math.max(0, ahead.length - this.backs);
    const i = ahead.lastIndexOf(kind);
    if (i < 0) return;
    this.backs += ahead.length - i;
    if (!this.going) { this.going = this.backs; history.go(-this.backs); }
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
  const [chrome] = useTopic(home, 'chrome', chromeOf, PHONE_CHROME);
  // With the window's toolbar (a split window's sidebar, menus and toolbar)
  // the page draws no bar, search or New app of its own.
  const ownBar = chrome?.toolbar !== true;
  // There the window's toolbar search is the page's search: its text comes
  // as chrome's query.
  const toolbarQuery = !ownBar && typeof chrome?.query === 'string' ? chrome.query : '';
  const refresh = useCallback(
    () => Promise.all([reloadApps(), reloadMakes(), reloadUnread(), reloadKey()]),
    [reloadApps, reloadMakes, reloadUnread, reloadKey],
  );
  const [act, notice, say] = useActions(home, verbsOf(chrome));
  const store = useMemo(() => layoutStore(db, { clock }), [db, clock]);
  const [layout, change] = useLayout(store, apps, clock, useCallback(() => say(UNSAVED_TEXT), []));
  const openApp = useCallback((app) => act('open', [app.id], `open ${app.title}`).then((v) => {
    if (v !== undefined) change((l) => opened(l, app.id, clock.date()));
  }), [act, change, clock]);
  const menuOf = useCallback((app, rect) => act('menu', [app.id, rect, { items: menuItems(layout, app.id) }], `show ${app.title}'s menu`).then((picked) => {
    if (picked === 'star') change((l) => star(l, app.id));
    else if (picked === 'unstar') change((l) => unstar(l, app.id));
    else if (picked === 'remove') change((l) => removeFromGroup(l, app.id));
  }), [act, change, layout]);
  // The group menu shown ({gid, rect}), with a history entry of its own
  // so back closes it; and the group whose name field shows ({gid}), with
  // one too.
  const [groupMenu, setGroupMenu] = useState(null);
  const [naming, setNaming] = useState(null);
  const namingOpen = useRef(false);
  const menuOpen = useRef(false);
  const closeGroupMenu = useCallback(() => {
    if (!menuOpen.current) return;
    menuOpen.current = false;
    setGroupMenu(null);
    nav.leave('menu');
  }, []);
  const reduced = useReducedMotion();
  const motion = useMemo(() => createMotion(clock, { reduced }), [clock, reduced]);
  useEffect(() => () => motion.stop(), [motion]);
  // The group fanned out ({gid, closing}), with its own history entry.
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
  // A group's name field: opened for a new group or by Rename, closed as
  // its name is kept or left.
  const openName = useCallback((gid) => {
    closeSearch();
    closeGroupMenu();
    namingOpen.current = true;
    setNaming({ gid });
    nav.enter('name');
  }, []);
  const closeName = useCallback(() => {
    if (!namingOpen.current) return;
    namingOpen.current = false;
    setNaming(null);
    nav.leave('name');
  }, []);
  // The fan's apps go back into the group's tile, from the event that gathers it,
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
  // A tap on a group fans it out, or gathers it when it is fanned already.
  const fanGroup = useCallback((gid) => {
    closeSearch();
    closeGroupMenu();
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
  // Opening search gathers a fanned group and closes the group menu first,
  // so its entry is the only one of the page's own.
  const openSearch = useCallback(() => {
    if (searchOpen.current) return;
    closeGroupMenu();
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
  // Escape closes search, unless a menu or a name field takes it.
  useEffect(() => {
    if (!searching) return undefined;
    const key = (e) => { if (e.key === 'Escape' && !document.querySelector('.h-name, .h-group-menu, .h-more .h-menu')) closeSearch(); };
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [searching]);
  useEffect(() => {
    const close = () => closeFan();
    nav.listen();
    nav.closers.group = close;
    return () => { if (nav.closers.group === close) delete nav.closers.group; };
  }, []);
  useEffect(() => {
    const close = () => { namingOpen.current = false; setNaming(null); };
    nav.listen();
    nav.closers.name = close;
    return () => { if (nav.closers.name === close) delete nav.closers.name; };
  }, []);
  useEffect(() => {
    const close = () => { menuOpen.current = false; setGroupMenu(null); };
    nav.listen();
    nav.closers.menu = close;
    return () => { if (nav.closers.menu === close) delete nav.closers.menu; };
  }, []);
  // While a group is fanned out, a tap outside its row and the group tiles,
  // or Escape, gathers it.
  const fanOpen = !!fan && !fan.closing;
  useEffect(() => {
    if (!fanOpen) return undefined;
    const click = (e) => { if (!(e.target instanceof Element && e.target.closest('.h-fan, .h-group'))) gather(); };
    const key = (e) => { if (e.key === 'Escape' && !document.querySelector('.h-name, .h-group-menu, .h-more .h-menu')) gather(); };
    document.addEventListener('click', click);
    document.addEventListener('keydown', key);
    return () => { document.removeEventListener('click', click); document.removeEventListener('keydown', key); };
  }, [fanOpen]);
  // A fan, a menu or a name field whose group is gone (dissolved,
  // ungrouped, removed elsewhere) closes.
  useEffect(() => {
    if (naming && layout && !layout.groups[naming.gid]) closeName();
    if (groupMenu && layout && !layout.groups[groupMenu.gid]) closeGroupMenu();
    if (fan && layout && !layout.groups[fan.gid]) gatherNow();
  }, [layout, naming, groupMenu, fan]);
  const showGroupMenu = useCallback((gid, rect) => {
    menuOpen.current = true;
    setGroupMenu({ gid, rect });
    nav.enter('menu');
  }, []);
  const pickGroup = (what) => {
    const { gid } = groupMenu;
    closeGroupMenu();
    if (what === 'rename') openName(gid);
    else if (what === 'ungroup') change((l) => ungroup(l, gid));
  };
  // The name a group's field keeps, shown until its write lands.
  const [named, setNamed] = useState(null);
  const keepName = (name) => {
    const gid = naming?.gid;
    closeName();
    if (!gid || !name || name === layout?.groups[gid]?.name) return;
    setNamed({ gid, name });
    change((l) => renameGroup(l, gid, name)).then(() => setNamed((n) => (n?.gid === gid && n.name === name ? null : n)));
  };
  // Clax keeps its own home screen over this page until it is told the
  // grid (or the empty state, or the load error) is drawn: once per load.
  const drawn = (Array.isArray(apps) && layout !== null) || !!appsError;
  const toldShown = useRef(false);
  useEffect(() => {
    if (!home || !drawn || toldShown.current) return;
    toldShown.current = true;
    Promise.resolve(home.shown?.()).catch(() => {});
  }, [home, drawn]);
  const [dragging, setDragging] = useState(false);
  // Search goes with the bar.
  useEffect(() => { if (!ownBar) closeSearch(); }, [ownBar]);
  const goFirst = useRef(() => {});

  if (!home || (appsError && isRefused(appsError))) {
    return html`<main class="h-page h-away">
      <${Message} glyph="home_fill" title="This is a Clax home screen." hint="Open it in the Clax app." />
    </main>`;
  }
  const list = Array.isArray(apps) && layout !== null ? apps : null;
  const going = Array.isArray(makes) ? makes : [];
  goFirst.current = () => {};
  let body;
  // A failed list offers no New app, and an empty one offers it in its
  // invitation.
  let fab = true;
  if (!list && appsError) {
    fab = false;
    // Every app is on this device, from Clax's own service: no cloud.
    body = html`<${Message} glyph="apps" title="Couldn’t show your apps" hint="Clax’s service didn’t answer."
      action=${html`<button type="button" class="h-filled-button" onClick=${reloadApps}>Try again</button>`} />`;
  } else if (!list) {
    body = null;
  } else if ((searching && query.trim()) || toolbarQuery.trim()) {
    const text = ownBar ? query : toolbarQuery;
    const found = search(text, searchEntries(layout, list));
    goFirst.current = () => { if (found[0]) openApp(found[0].app); };
    body = html`<${Results} query=${text} found=${found} layout=${layout} apps=${list} clock=${clock} onOpen=${openApp} onMenu=${menuOf} />`;
  } else if (!list.length && !going.length) {
    fab = false;
    body = html`<${Message} glyph="mark" title="Say what you need, and Clax makes it." hint="Like “a tracker for my runs, with a weekly total.”"
      action=${html`<button type="button" class="h-filled-button h-message-new" onClick=${() => act('newApp', [], 'make a new app')}>
        <${Glyph} name="mic_fill" size=${18} /><span>New app</span>
      </button>`} />`;
  } else {
    // A name just kept shows at once, before its write lands.
    const view = named && layout.groups[named.gid] ? renameGroup(layout, named.gid, named.name) : layout;
    body = html`<${Grid} layout=${view} apps=${list} makes=${going} motion=${motion} reduced=${reduced} clock=${clock} change=${change} fan=${fan}
      onOpen=${openApp} onMenu=${menuOf} onFanGroup=${fanGroup} onGroupMenu=${showGroupMenu} onMade=${openName} act=${act} onDragging=${setDragging}
      gatherNow=${gatherNow} />`;
  }
  const entryOf = (gid) => (list ? sections(layout, list).all.find((e) => e.group?.id === gid) ?? null : null);
  const nameFor = naming && entryOf(naming.gid);
  const menuFor = groupMenu && entryOf(groupMenu.gid);
  return html`<main class="h-page" data-dragging=${dragging ? '' : undefined} data-toolbar=${ownBar ? undefined : ''}>
    ${ownBar && html`<${Bar} act=${act} refresh=${refresh} unread=${typeof unread === 'number' ? unread : 0} onSearch=${openSearch} />`}
    ${ownBar && searching && html`<${SearchField} query=${query} setQuery=${setQuery} onGo=${() => goFirst.current()} onClose=${closeSearch} />`}
    ${needsKey === true && html`<${Banner} class="h-key" glyph="key_fill" title="Add your Claude API key to start making apps"
      actions=${html`<button type="button" class="h-text-button" onClick=${() => act('openSettings', [], 'open Settings')}>Add key</button>`} />`}
    ${body}
    ${nameFor && html`<${GroupName} key=${naming.gid} group=${nameFor.group} rename=${keepName} close=${closeName} />`}
    ${menuFor && html`<${GroupMenu} group=${menuFor.group} rect=${groupMenu.rect} pick=${pickGroup} close=${closeGroupMenu} />`}
    ${notice && html`<button type="button" class="h-notice" role="status">${notice.text}</button>`}
    ${fab && ownBar && html`<button type="button" class="h-fab" aria-label="New app" onClick=${() => act('newApp', [], 'make a new app')}>
      <${Glyph} name="mic_fill" />
      <span>New app</span>
    </button>`}
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
