// Clax kit map: a slippy map (Web Mercator z/x/y tiles) whose tiles come
// through the page's `net` capability and show as blob: images, since a page
// may load nothing from another host itself.
//
//   import { html } from './kit/preact.js';
//   import { MapView, fitBounds } from './kit/map.js';
//   html`<${MapView} center=${[51.507, -0.128]} zoom=${13}
//     markers=${[{ lat: 51.5, lng: -0.12, label: 'Office', onClick: open }]} />`
//
// The app declares the tile host: {"net": {"hosts": ["tile.openstreetmap.org"]}}.
// The first tile asks the owner once for it (the shell's consent dialog);
// after Allow the map fills in, and without it the map says why it is empty.
// An app that declares {"location": {}} gets a "Show my location" button: a
// tap follows the device (location.watch) with a dot and its accuracy
// circle, or only the circle for an approximate position.
//
// In the app's inspect view (the agent's renders and tests), where nobody is
// asked, a map without tiles draws a stand-in grid under its markers and
// controls, captioned "Map preview", rather than the permission note.

import { html, useState, useEffect, useRef, useMemo, useLayoutEffect } from './preact.js';
import { icon, names } from './icons.js';

/** OpenStreetMap's standard tiles; its tile policy wants the attribution always visible. */
export const OSM = {
  tiles: 'https://tile.openstreetmap.org/{z}/{x}/{y}.png',
  attribution: '© OpenStreetMap contributors',
  maxZoom: 19,
};

const TILE = 256;
const MAX_LAT = 85.0511287798;
const EARTH = 40075016.686;
const CACHE_MAX = 300;
const CONCURRENCY = 6;
/** The longest wait before a failed tile is tried again; the first retry waits RETRY_FIRST_MS, each next one twice as long. */
const RETRY_MS = 15000;
const RETRY_FIRST_MS = 1000;
/** The frame name the shell gives the page in the app's inspect view (bridge/src/protocol.ts INSPECT_FRAME). */
const INSPECT_FRAME = 'clax-inspect';
const ICONS = new Set(names);

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const wrap = (v, n) => ((v % n) + n) % n;
/** `{lat, lng}` from `[lat, lng]`, `{lat, lng}` or `{lat, lon}` (the location capability's shape). */
const pt = (p) => (Array.isArray(p) ? { lat: +p[0], lng: +p[1] } : { lat: +p.lat, lng: +(p.lng ?? p.lon) });
const valid = (p) => Number.isFinite(p.lat) && Number.isFinite(p.lng);

/** World coordinates (Web Mercator at zoom 0: 0..256 each way) of a latitude and longitude. */
export function project(lat, lng) {
  const s = Math.sin((clamp(lat, -MAX_LAT, MAX_LAT) * Math.PI) / 180);
  return [((lng + 180) / 360) * TILE, (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * TILE];
}

/** The `[lat, lng]` of world coordinates. */
export function unproject(x, y) {
  const n = Math.PI - (2 * Math.PI * y) / TILE;
  return [(180 / Math.PI) * Math.atan(Math.sinh(n)), (x / TILE) * 360 - 180];
}

/**
 * The `{center: [lat, lng], zoom}` that shows every point (`[lat, lng]` or `{lat, lng}`) in a
 * `width` by `height` map, `padding` pixels in from each edge; null without points.
 */
export function fitBounds(points, { width = 360, height = 320, padding = 48, maxZoom = 16 } = {}) {
  const ps = (points || []).map(pt).filter(valid);
  if (!ps.length) return null;
  const xy = ps.map((p) => project(p.lat, p.lng));
  const xs = xy.map((p) => p[0]);
  const ys = xy.map((p) => p[1]);
  const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const w = Math.max(1, width - 2 * padding);
  const h = Math.max(1, height - 2 * padding);
  const z = Math.min(Math.log2(w / Math.max(x1 - x0, 1e-9)), Math.log2(h / Math.max(y1 - y0, 1e-9)));
  return { center: unproject((x0 + x1) / 2, (y0 + y1) / 2), zoom: clamp(Math.floor(z), 1, maxZoom) };
}

// ---------------------------------------------------------------------------
// Tiles: fetched through `net`, kept as blob: URLs in a least-recently-used
// cache shared by every map of the page.

const cache = new Map(); // url -> {src} | {fail, at, tries}
const inflight = new Set();
const wanted = new Map(); // map key -> urls, nearest the center first
const listeners = new Set();
const blocked = new Map(); // host ('*' for every host) -> code
const TERMINAL = new Set(['not_granted', 'not_declared', 'https_only', 'redirect_refused', 'blocked_address', 'session_expired', 'no_net']);
let netPromise;
let retryTimer = null;
let retryAt = 0;

/** The wait before a tile that failed `tries` times in a row is tried again. */
const backoff = (tries) => Math.min(RETRY_MS, RETRY_FIRST_MS * 2 ** Math.max(0, tries - 1));

/** Runs pump() in `ms`, unless a run is already due sooner. */
function retryIn(ms) {
  const at = Date.now() + ms;
  if (retryTimer !== null && retryAt <= at) return;
  if (retryTimer !== null) clearTimeout(retryTimer);
  retryAt = at;
  retryTimer = setTimeout(() => { retryTimer = null; pump(); }, ms);
}

function getNet() {
  if (!netPromise) {
    netPromise = Promise.resolve()
      .then(() => (window.claude && window.claude.use ? window.claude.use('net') : null))
      .catch(() => null);
  }
  return netPromise;
}

const hostOf = (url) => { try { return new URL(url).hostname; } catch { return ''; } };
const emit = () => listeners.forEach((f) => f());

function put(url, v) {
  const old = cache.get(url);
  if (old && old.src && old.src !== v.src) URL.revokeObjectURL(old.src);
  cache.delete(url);
  cache.set(url, v);
  while (cache.size > CACHE_MAX) {
    const [k, e] = cache.entries().next().value;
    cache.delete(k);
    if (e.src) URL.revokeObjectURL(e.src);
  }
}

/** A cached tile, marked as just used. */
function touch(url) {
  const v = cache.get(url);
  if (v) { cache.delete(url); cache.set(url, v); }
  return v;
}

async function load(url) {
  inflight.add(url);
  try {
    const net = await getNet();
    if (!net) throw Object.assign(new Error('no net'), { code: 'no_net' });
    const res = await net.fetch(url, { headers: { accept: 'image/*' } });
    if (!res.ok) throw Object.assign(new Error(`HTTP ${res.status}`), { code: 'http' });
    put(url, { src: URL.createObjectURL(await res.blob()) });
  } catch (e) {
    const code = (e && e.code) || 'error';
    if (code === 'no_net') {
      blocked.set('*', code);
      console.warn('[kit] map: the net capability is not available here; declare '
        + `{"net": {"hosts": ["${hostOf(url)}"]}} to load tiles.`);
    } else if (TERMINAL.has(code)) blocked.set(hostOf(url), code);
    const prev = cache.get(url);
    put(url, { fail: code, at: Date.now(), tries: (prev && prev.fail ? prev.tries || 1 : 0) + 1 });
  } finally {
    inflight.delete(url);
    emit();
    pump();
  }
}

/**
 * Starts the wanted tiles not yet cached, up to CONCURRENCY at once; a tile no map wants any
 * more is never fetched. A tile that failed waits out its backoff, and a timer runs pump()
 * again when the first such wait ends, so a failed tile is retried without the map moving.
 */
function pump() {
  const now = Date.now();
  let wait = Infinity;
  for (const urls of wanted.values()) {
    for (const u of urls) {
      if (inflight.size >= CONCURRENCY) return;
      if (inflight.has(u) || blocked.has('*') || blocked.has(hostOf(u))) continue;
      const c = cache.get(u);
      if (c && c.src) continue;
      if (c) {
        const left = c.at + backoff(c.tries) - now;
        if (left > 0) { wait = Math.min(wait, left); continue; }
      }
      load(u);
    }
  }
  if (wait < Infinity) retryIn(wait);
}

const tileUrl = (tmpl, z, x, y) => tmpl.replace('{z}', z).replace('{x}', x).replace('{y}', y);

// ---------------------------------------------------------------------------
// Styles, added to the page once.

const CSS = `
.k-map { position: relative; overflow: hidden; touch-action: none; user-select: none; -webkit-user-select: none;
  -webkit-touch-callout: none; background-color: var(--k-surface-3, #e9e9ef); border-radius: var(--k-radius, 14px);
  cursor: grab; isolation: isolate; outline: none; contain: layout paint;
  background-image: linear-gradient(var(--k-line, rgba(60,60,67,.16)) 1px, transparent 1px),
    linear-gradient(90deg, var(--k-line, rgba(60,60,67,.16)) 1px, transparent 1px); }
.k-map:active { cursor: grabbing; }
.k-map:focus-visible { box-shadow: 0 0 0 3px var(--k-accent-soft, rgba(59,91,219,.3)); }
.k-map-tile, .k-map-up { position: absolute; left: 0; top: 0; transform-origin: 0 0; pointer-events: none; }
.k-map-up { overflow: hidden; }
.k-map-tile { animation: k-map-in 160ms ease-out; -webkit-user-drag: none; }
.k-map-up .k-map-tile { animation: none; }
@keyframes k-map-in { from { opacity: 0; } to { opacity: 1; } }
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) .k-map-dark .k-map-tile { filter: invert(1) hue-rotate(180deg) brightness(0.9) contrast(0.88); }
}
:root[data-theme="dark"] .k-map-dark .k-map-tile { filter: invert(1) hue-rotate(180deg) brightness(0.9) contrast(0.88); }
.k-map-pin { position: absolute; left: 0; top: 0; padding: 0; margin: 0; border: 0; background: none; cursor: pointer;
  width: 30px; height: 40px; translate: -15px -38px; line-height: 0; filter: drop-shadow(0 2px 2px rgba(0,0,0,.3)); }
.k-map-pin:focus-visible { outline: 2px solid var(--k-accent, #3b5bdb); outline-offset: 2px; border-radius: 8px; }
.k-map-pin-glyph { position: absolute; left: 7px; top: 6px; width: 16px; height: 16px; display: grid; place-items: center; color: #fff; }
.k-map-label { position: absolute; left: 50%; bottom: 44px; translate: -50% 0; white-space: nowrap; padding: 3px 8px;
  border-radius: 999px; background: var(--k-surface, #fff); color: var(--k-fg, #111); font: 600 12px/1.3 var(--k-font, system-ui);
  box-shadow: var(--k-shadow-2, 0 2px 8px rgba(0,0,0,.15)); }
.k-map-here { position: absolute; left: 0; top: 0; pointer-events: none; }
.k-map-here-ring { position: absolute; border-radius: 50%; background: rgba(10,132,255,.16); border: 1px solid rgba(10,132,255,.4); }
.k-map-here-area { background: rgba(10,132,255,.12); border: 2px dashed rgba(10,132,255,.55); }
.k-map-locate { position: absolute; right: 10px; top: 92px; z-index: 3; width: 36px; height: 36px; display: grid; place-items: center;
  border: 0; padding: 0; margin: 0; border-radius: 10px; background: var(--k-surface, #fff); color: var(--k-fg, #111); cursor: pointer;
  box-shadow: var(--k-shadow-2, 0 2px 8px rgba(0,0,0,.15)); }
.k-map-locate[aria-pressed="true"] { color: #0a84ff; }
.k-map-here-dot { position: absolute; width: 16px; height: 16px; left: -8px; top: -8px; border-radius: 50%;
  background: #0a84ff; border: 3px solid #fff; box-shadow: 0 1px 4px rgba(0,0,0,.35); box-sizing: border-box; }
.k-map-zoom { position: absolute; right: 10px; top: 10px; display: flex; flex-direction: column; border-radius: 10px;
  overflow: hidden; background: var(--k-surface, #fff); box-shadow: var(--k-shadow-2, 0 2px 8px rgba(0,0,0,.15)); }
.k-map-zoom button { width: 36px; height: 36px; display: grid; place-items: center; border: 0; padding: 0; margin: 0;
  background: none; color: var(--k-fg, #111); cursor: pointer; }
.k-map-zoom button + button { border-top: 1px solid var(--k-line, rgba(60,60,67,.16)); }
.k-map-zoom button:active { background: var(--k-surface-3, #e9e9ef); }
.k-map-attr { position: absolute; right: 0; bottom: 0; padding: 2px 6px; border-top-left-radius: 6px; max-width: 80%;
  background: color-mix(in srgb, var(--k-surface, #fff) 82%, transparent); color: var(--k-fg-2, #5c5c66);
  font: 400 11px/1.35 var(--k-font, system-ui); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.k-map-note { position: absolute; left: 50%; top: 50%; translate: -50% -50%; width: min(280px, calc(100% - 32px));
  display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; padding: 14px 16px;
  border-radius: var(--k-radius, 14px); background: var(--k-surface, #fff); color: var(--k-fg, #111);
  box-shadow: var(--k-shadow-2, 0 2px 8px rgba(0,0,0,.15)); font: 400 14px/1.35 var(--k-font, system-ui); }
.k-map-note b { font-weight: 600; }
.k-map-note, .k-map-zoom, .k-map-attr { z-index: 3; }
.k-map-note span { color: var(--k-fg-2, #5c5c66); font-size: 13px; }
.k-map-standin { background-color: var(--k-surface-3, #e9e9ef);
  background-image: linear-gradient(var(--k-line, rgba(60,60,67,.16)) 1px, transparent 1px),
    linear-gradient(90deg, var(--k-line, rgba(60,60,67,.16)) 1px, transparent 1px),
    linear-gradient(color-mix(in srgb, var(--k-line, rgba(60,60,67,.16)) 45%, transparent) 1px, transparent 1px),
    linear-gradient(90deg, color-mix(in srgb, var(--k-line, rgba(60,60,67,.16)) 45%, transparent) 1px, transparent 1px); }
.k-map-caption { position: absolute; left: 8px; top: 8px; z-index: 3; padding: 2px 8px; border-radius: 999px;
  background: color-mix(in srgb, var(--k-surface, #fff) 82%, transparent); color: var(--k-fg-2, #5c5c66);
  font: 500 11px/1.4 var(--k-font, system-ui); pointer-events: none; }
.k-map-note-top { top: 10px; translate: -50% 0; flex-direction: row; padding: 6px 12px; width: auto; max-width: calc(100% - 100px); font-size: 13px; }
`;

let styled = false;
function addStyles() {
  if (styled || typeof document === 'undefined') return;
  styled = true;
  const s = document.createElement('style');
  s.dataset.kit = 'map';
  s.textContent = CSS;
  document.head.appendChild(s);
}

// ---------------------------------------------------------------------------
// The component.

/** A marker's pin; its fill is the button's `--k-map-pin` property, so a marker's color never enters markup. */
const PIN_SVG = `<svg width="30" height="40" viewBox="0 0 30 40" aria-hidden="true">
    <path d="M15 39C15 39 2 23.5 2 14.5 2 7 8 1.5 15 1.5S28 7 28 14.5C28 23.5 15 39 15 39Z" fill="var(--k-map-pin, var(--k-accent, #3b5bdb))" stroke="#fff" stroke-width="2"/>
  </svg>`;
const DOT_SVG = '<svg width="8" height="8" aria-hidden="true"><circle cx="4" cy="4" r="4" fill="#fff"/></svg>';

const NOTES = {
  no_net: ['Map unavailable', 'Map tiles cannot load in this view.'],
  not_granted: ['Map tiles need permission', (h) => `This app was not allowed to reach ${h}.`],
  not_declared: ['Map tiles are not declared', (h) => `The app must declare ${h} in capabilities.net.hosts.`],
};

/** What the map says when following the device fails (`location.watch`'s onError codes). */
const LOC_ERRORS = {
  not_granted: 'Location is not allowed for this app.',
  system_denied: 'Location is off for Clax in settings.',
  location_off: "The device's location is off.",
  unavailable: 'Your location is not available.',
};

let seq = 0;

/**
 * A slippy map. `center` `[lat, lng]` and `zoom` set the view (and move it when they change);
 * `fit` (points) fits the view to them instead. `markers: [{lat, lng, label, color, icon,
 * selected, onClick}]`, `here: {lat, lon|lng, accuracy, precise}` (the location capability's
 * `current()`) draws a "you are here" dot (only its accuracy circle when `precise` is false).
 * When the app declares `location` and `locate` is not false, a "Show my location" button
 * follows the device from a tap (`location.watch`), centring on the first position and
 * stopping on unmount or a second tap. `onMove({center, zoom, bounds})` follows the person's
 * panning and zooming once it settles. `tiles` (an https `{z}/{x}/{y}` template) and
 * `attribution` swap the tile source; `height` in pixels, `maxZoom`, `darkTiles` (dim the tiles
 * in dark mode; default true).
 */
export function MapView({
  center, zoom = 13, fit, markers = [], here, onMove, height = 320, tiles = OSM.tiles,
  attribution, minZoom = 1, maxZoom, darkTiles = true, label = 'Map', class: cls, style, locate = true,
}) {
  addStyles();
  const ref = useRef(null);
  const key = useMemo(() => ++seq, []);
  const topZoom = maxZoom ?? (tiles === OSM.tiles ? OSM.maxZoom : 18);
  const attr = attribution ?? (tiles === OSM.tiles ? OSM.attribution : '');
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [, setTick] = useState(0);
  const start = () => {
    const c = pt(center || [20, 0]);
    const [x, y] = project(c.lat, c.lng);
    return { x, y, z: clamp(center ? zoom : 2, minZoom, topZoom) };
  };
  const [view, setViewState] = useState(start);
  const live = useRef({ view, size, onMove });
  live.current.view = view;
  live.current.size = size;
  live.current.onMove = onMove;

  // Keeps the center inside the world's height and the longitude wrapped.
  const fix = (v) => {
    const z = clamp(v.z, minZoom, topZoom);
    const half = live.current.size.h / 2 / 2 ** z;
    const y = half * 2 >= TILE ? TILE / 2 : clamp(v.y, half, TILE - half);
    return { x: wrap(v.x, TILE), y, z };
  };
  const settle = useRef(0);
  const setView = (v, user = true) => {
    const next = fix(v);
    live.current.view = next;
    setViewState(next);
    if (!user) return;
    clearTimeout(settle.current);
    settle.current = setTimeout(() => {
      const f = live.current.onMove;
      if (!f) return;
      const { view: w, size: s } = live.current;
      const k = 2 ** w.z;
      const [north, west] = unproject(w.x - s.w / 2 / k, w.y - s.h / 2 / k);
      const [south, east] = unproject(w.x + s.w / 2 / k, w.y + s.h / 2 / k);
      f({ center: unproject(w.x, w.y), zoom: w.z, bounds: { north, south, east, west } });
    }, 250);
  };
  /** Zooms to `z`, keeping the point at screen offset (px, py) from the top left still. */
  const zoomAt = (z, px, py) => {
    const { view: v, size: s } = live.current;
    const nz = clamp(z, minZoom, topZoom);
    const dx = px - s.w / 2;
    const dy = py - s.h / 2;
    const ux = v.x + dx / 2 ** v.z;
    const uy = v.y + dy / 2 ** v.z;
    setView({ x: ux - dx / 2 ** nz, y: uy - dy / 2 ** nz, z: nz });
  };
  const panBy = (dx, dy) => {
    const v = live.current.view;
    setView({ x: v.x - dx / 2 ** v.z, y: v.y - dy / 2 ** v.z, z: v.z });
  };

  useLayoutEffect(() => {
    const el = ref.current;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    if (ro) ro.observe(el);
    return () => { if (ro) ro.disconnect(); clearTimeout(settle.current); };
  }, []);

  useEffect(() => {
    const on = () => setTick((t) => t + 1);
    listeners.add(on);
    return () => { listeners.delete(on); wanted.delete(key); };
  }, []);

  // The view follows `center`/`zoom`, or `fit`, when they change.
  const first = useRef(true);
  const c0 = center ? pt(center) : null;
  useEffect(() => {
    if (first.current) { first.current = false; return; }
    if (c0 && !fit) setView(start(), false);
  }, [c0 && c0.lat, c0 && c0.lng, zoom]);
  const fitKey = fit ? fit.map(pt).filter(valid).map((p) => `${p.lat.toFixed(6)},${p.lng.toFixed(6)}`).join(';') : '';
  useEffect(() => {
    if (!fitKey || !size.w) return;
    const f = fitBounds(fit, { width: size.w, height: size.h, maxZoom: Math.min(16, topZoom) });
    if (f) {
      const [x, y] = project(f.center[0], f.center[1]);
      setView({ x, y, z: f.zoom }, false);
    }
  }, [fitKey, size.w > 0]);

  // Pointers: one drags, two pinch, a quick second tap zooms in.
  const g = useRef({ ptrs: new Map(), moved: 0, t0: 0, last: null });
  const local = (e) => {
    const r = ref.current.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const onPointerDown = (e) => {
    if (e.button !== undefined && e.button > 0) return;
    ref.current.setPointerCapture?.(e.pointerId);
    const s = g.current;
    s.ptrs.set(e.pointerId, local(e));
    if (s.ptrs.size === 1) { s.moved = 0; s.t0 = e.timeStamp; }
  };
  const onPointerMove = (e) => {
    const s = g.current;
    const prev = s.ptrs.get(e.pointerId);
    if (!prev) return;
    const now = local(e);
    s.ptrs.set(e.pointerId, now);
    s.moved += Math.abs(now.x - prev.x) + Math.abs(now.y - prev.y);
    if (s.ptrs.size === 1) { panBy(now.x - prev.x, now.y - prev.y); return; }
    const other = [...s.ptrs.entries()].find(([id]) => id !== e.pointerId);
    if (!other) return;
    const o = other[1];
    const d0 = Math.hypot(prev.x - o.x, prev.y - o.y);
    const d1 = Math.hypot(now.x - o.x, now.y - o.y);
    const m0 = { x: (prev.x + o.x) / 2, y: (prev.y + o.y) / 2 };
    const m1 = { x: (now.x + o.x) / 2, y: (now.y + o.y) / 2 };
    panBy(m1.x - m0.x, m1.y - m0.y);
    if (d0 > 0 && d1 > 0) zoomAt(live.current.view.z + Math.log2(d1 / d0), m1.x, m1.y);
  };
  const onPointerUp = (e) => {
    const s = g.current;
    const p = s.ptrs.get(e.pointerId);
    if (!p) return;
    s.ptrs.delete(e.pointerId);
    if (e.type !== 'pointerup' || s.ptrs.size || s.moved > 8 || e.timeStamp - s.t0 > 350) return;
    const l = s.last;
    if (l && e.timeStamp - l.t < 350 && Math.hypot(p.x - l.x, p.y - l.y) < 30) {
      s.last = null;
      zoomAt(Math.round(live.current.view.z) + 1, p.x, p.y);
    } else s.last = { t: e.timeStamp, x: p.x, y: p.y };
  };
  useEffect(() => {
    const el = ref.current;
    const onWheel = (e) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 0.05 : e.deltaMode === 2 ? 1 : 0.0025;
      const dz = clamp(-e.deltaY * unit * (e.ctrlKey ? 4 : 1), -1, 1);
      const p = local(e);
      zoomAt(live.current.view.z + dz, p.x, p.y);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, []);
  const onKeyDown = (e) => {
    const step = 96;
    const { size: s, view: v } = live.current;
    const moves = { ArrowLeft: [step, 0], ArrowRight: [-step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] };
    if (moves[e.key]) panBy(...moves[e.key]);
    else if (e.key === '+' || e.key === '=') zoomAt(Math.round(v.z) + 1, s.w / 2, s.h / 2);
    else if (e.key === '-' || e.key === '_') zoomAt(Math.round(v.z) - 1, s.w / 2, s.h / 2);
    else return;
    e.preventDefault();
  };
  const stop = (e) => e.stopPropagation();

  // The device's position, followed from a tap on the locate button.
  const [locCap, setLocCap] = useState(null);
  const [mine, setMine] = useState(null);
  const [locMsg, setLocMsg] = useState('');
  const [watching, setWatching] = useState(false);
  const watchStop = useRef(null);
  const recentre = useRef(false);
  useEffect(() => {
    let alive = true;
    if (locate && typeof window !== 'undefined' && window.claude && window.claude.use) {
      window.claude.use('location').then((l) => { if (alive) setLocCap(l); }, () => {});
    }
    return () => { alive = false; if (watchStop.current) watchStop.current(); watchStop.current = null; };
  }, [locate]);
  const endWatch = () => { if (watchStop.current) watchStop.current(); watchStop.current = null; setMine(null); setWatching(false); };
  const onLocate = (e) => {
    e.stopPropagation();
    setLocMsg('');
    if (watchStop.current) { endWatch(); return; }
    recentre.current = true;
    let ended = false;
    let wasPrecise = null;
    const s = locCap.watch((p) => {
      if (ended) return;
      setMine(p);
      // The view moves to the first position, and to an approximate one's whole area when
      // the position turns approximate; the person's own panning is left alone otherwise.
      const precise = p.precise !== false;
      if (recentre.current || (!precise && wasPrecise !== false)) {
        recentre.current = false;
        const [x, y] = project(p.lat, p.lon);
        let z = Math.max(live.current.view.z, 15);
        if (!precise) {
          const { w: sw, h: sh } = live.current.size;
          const fitR = 0.4 * Math.min(sw || 320, sh || 320);
          z = Math.floor(Math.log2((fitR * EARTH * Math.cos((p.lat * Math.PI) / 180)) / (TILE * Math.max(p.accuracy, 1))));
        }
        setView({ x, y, z }, false);
      }
      wasPrecise = precise;
    }, {
      minDistance: 10,
      onError: (err) => {
        ended = true;
        watchStop.current = null;
        setMine(null);
        setWatching(false);
        const c = err && err.code;
        if (c !== 'stopped') setLocMsg(LOC_ERRORS[c] || 'Your location is not available.');
      },
    });
    if (!ended) { watchStop.current = s; setWatching(true); }
  };

  // The tiles in view, nearest the center first.
  const { w, h } = size;
  const z = clamp(Math.round(view.z), 0, topZoom);
  const scale = 2 ** (view.z - z);
  const n = 2 ** z;
  const ts = TILE * scale;
  const cx = view.x * n;
  const cy = view.y * n;
  const items = [];
  if (w && h) {
    const x0 = Math.floor((cx - w / 2 / scale) / TILE);
    const x1 = Math.floor((cx + w / 2 / scale) / TILE);
    const y0 = Math.max(0, Math.floor((cy - h / 2 / scale) / TILE));
    const y1 = Math.min(n - 1, Math.floor((cy + h / 2 / scale) / TILE));
    for (let ty = y0; ty <= y1; ty++) {
      for (let tx = x0; tx <= x1; tx++) {
        const left = (tx * TILE - cx) * scale + w / 2;
        const top = (ty * TILE - cy) * scale + h / 2;
        const url = tileUrl(tiles, z, wrap(tx, n), ty);
        const d = Math.hypot(left + ts / 2 - w / 2, top + ts / 2 - h / 2);
        items.push({ tx, ty, left, top, url, d });
      }
    }
    items.sort((a, b) => a.d - b.d);
  }
  const urls = items.map((t) => t.url);
  const urlKey = urls.join('|');
  useEffect(() => {
    wanted.set(key, urls);
    pump();
  }, [urlKey]);

  const tileEls = items.map((t) => {
    const c = touch(t.url);
    const pos = `transform:translate3d(${t.left}px,${t.top}px,0);width:${ts}px;height:${ts}px`;
    if (c && c.src) return html`<img key=${t.url} class="k-map-tile" src=${c.src} alt="" draggable="false" style=${pos} />`;
    // While a tile loads, its parent (one zoom out), when cached, stands in for it.
    const up = z > 0 && touch(tileUrl(tiles, z - 1, wrap(t.tx, n) >> 1, t.ty >> 1));
    if (!up || !up.src) return null;
    return html`<div key=${`up:${t.url}`} class="k-map-up" style=${pos}>
      <img class="k-map-tile" src=${up.src} alt="" draggable="false"
        style=${`width:${ts * 2}px;height:${ts * 2}px;transform:translate(${-(t.tx & 1) * ts}px,${-(t.ty & 1) * ts}px)`} />
    </div>`;
  });

  const k = 2 ** view.z;
  const screen = (lat, lng) => {
    const [mx, my] = project(lat, lng);
    const dx = wrap(mx - view.x + TILE / 2, TILE) - TILE / 2;
    return [w / 2 + dx * k, h / 2 + (my - view.y) * k];
  };
  const inView = ([x, y], m = 60) => x > -m && x < w + m && y > -m && y < h + m;

  // The followed position, else the `here` prop: a dot and its accuracy circle, or for an
  // approximate position only the circle of the area it is in.
  let hereEl = null;
  const at = mine || here;
  if (at && w) {
    const p = pt(at);
    if (valid(p)) {
      const s = screen(p.lat, p.lng);
      const metres = EARTH * Math.cos((p.lat * Math.PI) / 180) / (TILE * k);
      const r = at.accuracy > 0 ? at.accuracy / metres : 0;
      const area = at.precise === false;
      if (inView(s, r + 20)) {
        hereEl = html`<div class="k-map-here" style=${`transform:translate(${s[0]}px,${s[1]}px)`}>
          ${area || r > 10 ? html`<div class=${area ? 'k-map-here-ring k-map-here-area' : 'k-map-here-ring'}
            role=${area ? 'img' : undefined} aria-label=${area ? 'You are in this area' : undefined}
            style=${`width:${2 * Math.max(r, 12)}px;height:${2 * Math.max(r, 12)}px;left:${-Math.max(r, 12)}px;top:${-Math.max(r, 12)}px`} />` : null}
          ${area ? null : html`<div class="k-map-here-dot" role="img" aria-label="You are here" />`}
        </div>`;
      }
    }
  }

  const pins = (markers || []).map((m, i) => {
    const p = pt(m);
    if (!valid(p) || !w) return null;
    const s = screen(p.lat, p.lng);
    if (!inView(s)) return null;
    // Only a known icon's markup is set as HTML; the color is a style property, set through the DOM.
    const glyph = typeof m.icon === 'string' && ICONS.has(m.icon) ? icon(m.icon, { size: 14, stroke: 2.6 }) : DOT_SVG;
    const pinStyle = { transform: `translate(${s[0]}px,${s[1]}px)`, zIndex: m.selected ? 2 : 1 };
    if (typeof m.color === 'string' && m.color) pinStyle['--k-map-pin'] = m.color;
    return html`<button key=${m.id ?? i} type="button" class="k-map-pin" aria-label=${m.label || 'Place'} title=${m.label || ''}
      style=${pinStyle}
      onPointerDown=${stop} onClick=${(e) => { e.stopPropagation(); if (m.onClick) m.onClick(m, e); }}>
      <span dangerouslySetInnerHTML=${{ __html: PIN_SVG }} />
      <span class="k-map-pin-glyph" dangerouslySetInnerHTML=${{ __html: glyph }} />
      ${m.selected && m.label ? html`<span class="k-map-label">${m.label}</span>` : null}
    </button>`;
  });

  // Why the map is empty, when it is.
  const host = hostOf(tileUrl(tiles, 0, 0, 0));
  const code = blocked.get('*') || blocked.get(host);
  // In the inspect view nobody is asked for the tile host, so a refused or missing net shows
  // the map's layout over a stand-in grid instead of a note about permission.
  const standin = !!code && (code === 'not_granted' || code === 'no_net')
    && typeof window !== 'undefined' && window.name === INSPECT_FRAME;
  let note = null;
  if (standin) {
    note = html`<div class="k-map-caption">Map preview</div>`;
  } else if (code) {
    const [title, body] = NOTES[code] || ['Map tiles cannot load', () => `Requests to ${host} were refused (${code}).`];
    note = html`<div class="k-map-note" role="status">${html`<span dangerouslySetInnerHTML=${{ __html: icon('map', { size: 26 }) }} />`}
      <b>${title}</b><span>${typeof body === 'function' ? body(host) : body}</span></div>`;
  } else if (locMsg) {
    note = html`<div class="k-map-note k-map-note-top" role="status">${locMsg}</div>`;
  } else if (items.length && items.some((t) => cache.get(t.url)?.fail) && !items.some((t) => cache.get(t.url)?.src)) {
    note = html`<div class="k-map-note k-map-note-top" role="status">Map tiles did not load; retrying.</div>`;
  }

  const grid = standin ? `${ts}px ${ts}px,${ts}px ${ts}px,${ts / 4}px ${ts / 4}px,${ts / 4}px ${ts / 4}px` : `${ts}px ${ts}px`;
  return html`<div ref=${ref} class=${['k-map', darkTiles && 'k-map-dark', standin && 'k-map-standin', cls].filter(Boolean).join(' ')}
    style=${`height:${typeof height === 'number' ? `${height}px` : height};background-size:${grid};`
      + `background-position:${wrap(w / 2 - cx * scale, ts)}px ${wrap(h / 2 - cy * scale, ts)}px;${style || ''}`}
    tabindex="0" role="application" aria-label=${label} aria-roledescription="map"
    onPointerDown=${onPointerDown} onPointerMove=${onPointerMove} onPointerUp=${onPointerUp} onPointerCancel=${onPointerUp}
    onKeyDown=${onKeyDown}>
    ${tileEls}${hereEl}${pins}${note}
    <div class="k-map-zoom" onPointerDown=${stop}>
      <button type="button" aria-label="Zoom in" onClick=${() => zoomAt(Math.round(live.current.view.z) + 1, w / 2, h / 2)}
        dangerouslySetInnerHTML=${{ __html: icon('plus', { size: 18, stroke: 2.4 }) }} />
      <button type="button" aria-label="Zoom out" onClick=${() => zoomAt(Math.round(live.current.view.z) - 1, w / 2, h / 2)}
        dangerouslySetInnerHTML=${{ __html: icon('minus', { size: 18, stroke: 2.4 }) }} />
    </div>
    ${locCap && locate ? html`<button type="button" class="k-map-locate" onPointerDown=${stop} onClick=${onLocate}
      aria-label=${watching ? 'Stop showing my location' : 'Show my location'} aria-pressed=${watching ? 'true' : 'false'}
      dangerouslySetInnerHTML=${{ __html: icon('target', { size: 18, stroke: 2.2 }) }} />` : null}
    ${attr ? html`<div class="k-map-attr" onPointerDown=${stop}>${attr}</div>` : null}
  </div>`;
}
