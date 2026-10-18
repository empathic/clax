// Clax's home screen: the person's apps and the makes going on, as a grid,
// with Inbox, Settings and Import app… above it and New app below. Every
// list and every action goes through the Clax app's `home` capability
// (`await claude.use("home")`), which only the app recorded as the home
// screen is granted; anywhere else the page says to open it in Clax.
//
// The arrangement (Starred, the order of All apps, which apps are new, how
// often each is opened) is the `layout/main` document in the page's own
// `db` (arrange.js, store.js).
//
// Each tile is placed in its cell (grid.js's `pack`) and moved by
// motion.js's springs on the page's clock: a held tile lifts and can be
// dragged across Starred and All apps, its neighbours making room; a quick
// release tosses it on (physics.js); Alt+arrows move a focused tile.
//
// `start()` mounts the page over the real capabilities; `mount(root, home,
// {db, clock})` mounts it over any objects with the same calls (the tests
// pass kit/fake-home.js's fakeHome and fakeDb).
import { html, render, useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from './kit/preact.js';
import { icon as kitIcon } from './kit/icons.js';
import { reconcile, normalize, sections, star, unstar, opened, isNew, moveTo } from './arrange.js';
import { layoutStore } from './store.js';
import { realClock, velocityOf, tossRest, TOSS_MIN } from './physics.js';
import { pack, geometry, slotAt } from './grid.js';
import { createPress } from './drag.js';
import { createMotion } from './motion.js';

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
 * Alt+arrows move it. `fresh` marks it "New". */
function AppTile({ app, fresh, cell, dragging, ctl, onOpen, onMenu }) {
  const b = app.build;
  const press = ctl.pressOf(app.id);
  const said = b ? `${app.title}, building, ${b.done} of ${b.total} done` : app.title;
  const label = fresh ? `${said}, new` : said;
  return html`<button type="button" class="h-tile" data-id=${app.id} data-key=${tileKeyOf(app.id)} data-new=${fresh ? '' : undefined}
    data-dragging=${dragging ? '' : undefined} aria-label=${label} style=${cellStyle(cell)}
    onPointerDown=${(e) => ctl.down(e, app.id)}
    onContextMenu=${(e) => { e.preventDefault(); if (!press.contextMenu()) onMenu(app, faceRect(e.currentTarget)); }}
    onClick=${() => { if (press.takeClick()) onOpen(app); }}
    onKeyDown=${(e) => ctl.key(app.id, e)}>
    <span class="h-icon">
      <${AppFace} app=${app} />
      ${b && html`<span class="h-over"><${Ring} done=${b.done} total=${b.total} /></span>`}
      ${fresh && html`<span class="h-new" aria-hidden="true">New</span>`}
    </span>
    <span class="h-label h-two">${app.title}</span>
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
      <${Glyph} name="moreVertical" />
    </button>
    ${open && html`<button type="button" class="h-scrim" aria-label="Close menu" onClick=${() => setOpen(false)} />
      <div class="h-menu" role="menu">
        <button type="button" role="menuitem" class="h-menu-item" onClick=${() => { setOpen(false); act('importApp', [], 'import an app'); }}>Import app…</button>
        <button type="button" role="menuitem" class="h-menu-item" onClick=${() => { setOpen(false); refresh(); }}>Refresh</button>
      </div>`}
  </span>`;
}

function Bar({ act, refresh, unread }) {
  return html`<header class="h-bar">
    <h1 class="h-title">Clax</h1>
    <div class="h-actions">
      <button type="button" class="h-action" aria-label=${unread > 0 ? `Inbox, ${unread} unread` : 'Inbox'} onClick=${() => act('openInbox', [], 'open the inbox')}>
        <span class="h-badged">
          <${Glyph} name="inbox" />
          ${unread > 0 && html`<span class="h-badge">${unread > 999 ? '999+' : unread}</span>`}
        </span>
      </button>
      <button type="button" class="h-action" aria-label="Settings" onClick=${() => act('openSettings', [], 'open Settings')}><${Glyph} name="settings" /></button>
      <${More} act=${act} refresh=${refresh} />
    </div>
  </header>`;
}

/** The launcher's column count for a grid `width` wide: cells at most 112
 * px, 4 px apart (Flutter's SliverGridDelegateWithMaxCrossAxisExtent). */
const columnsFor = (width) => Math.max(1, Math.ceil(width / (112 + 4)));

/** A tile's key in the page: "a:<app ID>", or "m:<make ID>". */
const tileKeyOf = (id) => `a:${id}`;

/** Each tile's cell by its key, with its section: Starred's apps, then
 * All apps' makes and apps (a group's apps in the group's place), each
 * packed in `cols` columns. */
function placement(layout, apps, makes, cols) {
  const s = sections(layout, apps);
  const lists = {
    starred: s.starred.map((e) => tileKeyOf(e.app.id)),
    all: [...makes.map((m) => `m:${m.id}`), ...s.all.flatMap((e) => (e.app ? [e.app] : e.apps).map((a) => tileKeyOf(a.id)))],
  };
  const out = new Map();
  for (const [section, keys] of Object.entries(lists)) {
    for (const c of pack(keys.map((key) => ({ key, span: 1 })), cols).cells) out.set(c.key, { ...c, section });
  }
  return out;
}

/** Starred (only when something is starred, headed) above All apps (headed
 * then), each a grid of `cols` columns; the makes go first under All apps.
 * A group's apps show in the group's place. While app `drag` is dragged,
 * All apps is headed, and an empty Starred is a zone to drop on. */
function Sections({ layout, apps, makes, cols, drag, ctl, onOpen, onMenu, act }) {
  const s = sections(layout, apps);
  const cells = placement(layout, apps, makes, cols);
  const tile = (app) => html`<${AppTile} key=${tileKeyOf(app.id)} app=${app} fresh=${isNew(layout, app.id)} cell=${cells.get(tileKeyOf(app.id))}
    dragging=${drag === app.id} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} />`;
  const starred = s.starred.length > 0;
  const zone = !starred && drag != null;
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
        ${s.all.flatMap((e) => (e.app ? [tile(e.app)] : e.apps.map(tile)))}
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

/** The grid's arranging, over the state `st` the Grid keeps current
 * (`wrap`, `layout`, `shown`, `apps`, `makes`, `cols`, `motion`,
 * `reduced`, `change`, `onMenu`, `setDrag`, `setHeld`, `onDragging`):
 * each app's press (`pressOf`), the drag it starts, and the keyboard's
 * moves. During a drag the tiles are placed and moved here, at once, with
 * no render; the page renders the drag's start and its drop. */
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
  const tileEl = (key) => st.wrap?.querySelector(`.h-tile[data-key="${CSS.escape(key)}"]`) ?? null;
  const gridOf = (section) => st.wrap?.querySelector(`.h-section[data-section="${section}"] .h-grid`) ?? null;

  /** Places every tile in its cell of the shown arrangement (the drag's
   * while one is on) and springs each one that moved from where it was.
   * With `steady`, a render that moved All apps' grid up or down (the drop zone
   * and the headings coming or going) scrolls the page with it, so the
   * tiles stay under the finger. */
  function relayout(steady = false) {
    if (!st.wrap) return;
    const cells = placement(drag?.prov ?? st.shown, st.apps, st.makes, st.cols);
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
    els.forEach((el, i) => {
      const key = el.dataset.key;
      const p = layoutPos(el, offsets[i]);
      next.set(key, p);
      if (drag && key === drag.key) return;
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
    if (focus && (!document.activeElement || document.activeElement === document.body)) tileEl(tileKeyOf(focus))?.focus();
  }

  /** Holds the dragged tile under the pointer, where it was taken. */
  function hold() {
    const el = tileEl(drag.key);
    if (!el) return;
    const p = layoutPos(el);
    st.motion.hold(drag.key, el, drag.last.x + scrollX - drag.grab.x - p.x, drag.last.y + scrollY - drag.grab.y - p.y);
  }

  /** The slot at viewport point `p` in `section`'s grid: the index among
   * its entries (the dragged app's left out) the app would take there. */
  function slotIn(section, p) {
    const grid = gridOf(section);
    const s = sections(st.layout, st.apps);
    const entries = s[section].filter((e) => e.key !== drag.id);
    const owners = [];
    entries.forEach((e, i) => {
      const n = e.app ? 1 : e.apps.filter((a) => a.id !== drag.id).length;
      for (let k = 0; k < n; k++) owners.push(i);
    });
    if (!grid) return { section, index: 0 };
    const lead = section === 'all' ? st.makes.length : 0;
    const { cells } = pack(Array.from({ length: lead + owners.length + 1 }, (_, key) => ({ key, span: 1 })), st.cols);
    const r = grid.getBoundingClientRect();
    const c = Math.max(0, slotAt(cells, geometry(r.width, st.cols), p.x - r.left, p.y - r.top) - lead);
    return { section, index: c >= owners.length ? entries.length : owners[c] };
  }

  /** Where a drop at viewport point `p` puts the dragged app. */
  function targetAt(p) {
    const star = st.wrap?.querySelector('.h-section[data-section="starred"]');
    if (star && inside(star.getBoundingClientRect(), p)) {
      return star.classList.contains('h-drop') ? { section: 'starred', index: 0, zone: true } : slotIn('starred', p);
    }
    return slotIn('all', p);
  }

  /** Follows the pointer: when the slot under it changes, the neighbours
   * make room there. */
  function retarget() {
    const t = targetAt(drag.last);
    st.wrap?.querySelector('.h-drop')?.toggleAttribute('data-over', !!t.zone);
    if (drag.target && t.section === drag.target.section && t.index === drag.target.index) return;
    drag.target = t;
    drag.prov = moveTo(st.layout, drag.id, t.section, t.index);
    relayout();
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
        if (scrollY !== before) { hold(); retarget(); }
      }
      edge = clock.frame(tick);
    };
    edge = clock.frame(tick);
  }

  function end(d) {
    stopEdge();
    blocking(false);
    drag = null;
    dropped = true;
    st.setDrag(null);
    st.onDragging(false);
    const el = tileEl(d.key);
    if (el) delete el.dataset.held;
  }

  function handlersFor(id) {
    const key = tileKeyOf(id);
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
        const app = st.apps.find((a) => a.id === id);
        if (el && app) st.onMenu(app, faceRect(el));
      },
      dragStart(p) {
        const el = tileEl(key);
        if (!el) return;
        const r = el.getBoundingClientRect();
        focus = null;
        drag = { id, key, from: st.layout.starred.includes(id) ? 'starred' : 'all', grab: { x: p.x - r.left, y: p.y - r.top }, last: p, target: null, prov: null };
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
        retarget();
      },
      dragEnd(p, samples) {
        if (!drag) return;
        const d = drag;
        d.last = p;
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
        d.prov = moveTo(st.layout, id, t.section, t.index);
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

  return {
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
    /** A pointer event anywhere on the page, for the press it belongs to. */
    pointer(type, e) { active?.[type](e); },
    touchMove(e) { if (block && e.cancelable) e.preventDefault(); },
    afterRender() {
      relayout(drag !== null || dropped);
      dropped = false;
      if (drag) retarget();
    },
    /** Alt+arrows on app `id`'s tile: one entry left or right, or a row
     * (`cols` entries) up or down, within its section. */
    key(id, e) {
      if (!e.altKey || drag) return;
      const n = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -st.cols, ArrowDown: st.cols }[e.key];
      if (!n) return;
      e.preventDefault();
      const l = st.shown;
      const section = l.starred.includes(id) ? 'starred' : 'all';
      const entries = sections(l, st.apps)[section];
      const i = entries.findIndex((x) => x.key === id || x.apps?.some((a) => a.id === id));
      const to = clamp(i + n, 0, entries.length - 1);
      if (i < 0 || to === i) return;
      focus = id;
      st.change((x) => moveTo(x, id, section, to));
    },
    stop() {
      stopEdge();
      for (const p of presses.values()) p.cancel();
    },
  };
}

function Grid({ layout, apps, makes, motion, reduced, clock, change, onOpen, onMenu, act, onDragging }) {
  const ref = useRef(null);
  const [cols, setCols] = useState(4);
  const [drag, setDrag] = useState(null);
  // The arrangement a drop made, shown until its write settles.
  const [held, setHeld] = useState(null);
  const shown = held ?? layout;
  const st = useRef({}).current;
  Object.assign(st, { wrap: ref.current, layout, shown, apps, makes, cols, motion, reduced, change, onMenu, setDrag, setHeld, onDragging });
  const ctl = useMemo(() => arranger(st, clock), [clock]);
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
  useLayoutEffect(() => {
    st.wrap = ref.current;
    ctl.afterRender();
  });
  useEffect(() => {
    const move = (e) => ctl.pointer('move', e);
    const up = (e) => ctl.pointer('up', e);
    const cancel = (e) => ctl.pointer('cancel', e);
    const touch = (e) => ctl.touchMove(e);
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', cancel);
    document.addEventListener('touchmove', touch, { passive: false });
    return () => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', cancel);
      document.removeEventListener('touchmove', touch);
      ctl.stop();
    };
  }, [ctl]);
  return html`<div class="h-grid-wrap" ref=${ref}>
    <${Sections} layout=${shown} apps=${apps} makes=${makes} cols=${cols} drag=${drag} ctl=${ctl} onOpen=${onOpen} onMenu=${onMenu} act=${act} />
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

const PULL_PX = 64;
const PULL_MAX = 96;

/** Pull-to-refresh, as the launcher's: a touch drag down from the top of
 * the page shows the ring; released past PULL_PX it calls `refresh` and
 * spins until that settles. */
function usePull(refresh) {
  const [pull, setPull] = useState(0);
  const [busy, setBusy] = useState(false);
  const s = useRef({ y: null, d: 0 });
  useEffect(() => {
    const start = (e) => {
      s.current = { y: e.touches.length === 1 && scrollY <= 0 ? e.touches[0].clientY : null, d: 0 };
    };
    const move = (e) => {
      if (s.current.y == null) return;
      // A held or dragged tile has the finger, not the page.
      if (document.querySelector('.h-tile[data-held]')) { s.current = { y: null, d: 0 }; setPull(0); return; }
      const dy = e.touches[0].clientY - s.current.y;
      if (dy <= 0) { s.current.d = 0; setPull(0); return; }
      if (e.cancelable) e.preventDefault();
      s.current.d = Math.min(PULL_MAX, dy * 0.5);
      setPull(s.current.d);
    };
    const end = () => {
      const far = s.current.d >= PULL_PX;
      s.current = { y: null, d: 0 };
      setPull(0);
      if (!far) return;
      setBusy(true);
      Promise.resolve(refresh()).finally(() => setBusy(false));
    };
    addEventListener('touchstart', start, { passive: true });
    addEventListener('touchmove', move, { passive: false });
    addEventListener('touchend', end);
    addEventListener('touchcancel', end);
    return () => {
      removeEventListener('touchstart', start);
      removeEventListener('touchmove', move);
      removeEventListener('touchend', end);
      removeEventListener('touchcancel', end);
    };
  }, [refresh]);
  return { pull, busy };
}

function PullRing({ pull, busy }) {
  if (!busy && pull <= 0) return null;
  const y = busy ? PULL_PX : pull;
  const f = Math.min(1, pull / PULL_PX);
  return html`<div class=${`h-pull${busy ? ' h-pull-busy' : ''}`} role=${busy ? 'progressbar' : undefined} aria-label=${busy ? 'Refreshing' : undefined}
    style=${`transform:translate(-50%, ${y - 40}px);opacity:${busy ? 1 : f}`}>
    <svg width="24" height="24" viewBox="0 0 24 24" aria-hidden="true">
      <circle class="h-pull-arc" cx="12" cy="12" r="9" stroke-dasharray=${busy ? '20 100' : `${f * 50} 100`} />
    </svg>
  </div>`;
}

const MAX_TILES = 512;
/** How long a burst of renders or scroll events settles before the tiles go. */
const TILES_MS = 50;

/** Tells Clax where each app's tile is (`home.tiles`), once the home
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
      const tiles = [...document.querySelectorAll('.h-tile[data-id]')].slice(0, MAX_TILES).map((el) => {
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

/** What the home app adds to app `id`'s menu. */
const menuItems = (layout, id) => (layout.starred.includes(id) ? [{ id: 'unstar', label: 'Unstar' }] : [{ id: 'star', label: 'Star' }]);

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
  const { pull, busy } = usePull(refresh);
  const store = useMemo(() => layoutStore(db, { clock }), [db, clock]);
  const [layout, change] = useLayout(store, apps, clock, useCallback(() => say(UNSAVED_TEXT), []));
  const openApp = useCallback((app) => act('open', [app.id], `open ${app.title}`).then((v) => {
    if (v !== undefined) change((l) => opened(l, app.id, clock.date()));
  }), [act, change, clock]);
  const menuOf = useCallback((app, rect) => act('menu', [app.id, rect, { items: menuItems(layout, app.id) }], `show ${app.title}'s menu`).then((picked) => {
    if (picked === 'star') change((l) => star(l, app.id));
    else if (picked === 'unstar') change((l) => unstar(l, app.id));
  }), [act, change, layout]);
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
  const reduced = useReducedMotion();
  const settled = useRef(() => {});
  const motion = useMemo(() => createMotion(clock, { reduced, onSettle: () => settled.current() }), [clock, reduced]);
  useEffect(() => () => motion.stop(), [motion]);
  settled.current = useTiles(home, drawn, motion);
  const [dragging, setDragging] = useState(false);

  if (!home || (appsError && isRefused(appsError))) {
    return html`<main class="h-page h-away">
      <${Message} glyph="home" title="This is a Clax home screen." hint="Open it in the Clax app." />
    </main>`;
  }
  const list = Array.isArray(apps) && layout !== null ? apps : null;
  const going = Array.isArray(makes) ? makes : [];
  let body;
  if (!list && appsError) {
    body = html`<${Message} glyph="cloudOff" title="Couldn't load your apps"
      action=${html`<button type="button" class="h-text-button" onClick=${reloadApps}>Try again</button>`} />`;
  } else if (!list) {
    body = null;
  } else if (!list.length && !going.length) {
    body = html`<${Message} glyph="sparkles" title="Say what you want, and Clax makes it." hint="“A tracker for my runs with a weekly total”" />`;
  } else {
    body = html`<${Grid} layout=${layout} apps=${list} makes=${going} motion=${motion} reduced=${reduced} clock=${clock} change=${change}
      onOpen=${openApp} onMenu=${menuOf} act=${act} onDragging=${setDragging} />`;
  }
  return html`<main class="h-page" data-dragging=${dragging ? '' : undefined}>
    <${Bar} act=${act} refresh=${refresh} unread=${typeof unread === 'number' ? unread : 0} />
    <${PullRing} pull=${pull} busy=${busy} />
    ${needsKey === true && html`<div class="h-key">
      <${Glyph} name="key" />
      <span class="h-key-text">Add your Claude API key to start making apps</span>
      <button type="button" class="h-text-button" onClick=${() => act('openSettings', [], 'open Settings')}>Add key</button>
    </div>`}
    ${body}
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
