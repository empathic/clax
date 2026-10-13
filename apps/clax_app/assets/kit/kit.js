// Clax kit components: Preact components over kit.css.
//
//   import { html, render } from './kit/preact.js';
//   import { App, AppBar, List, ListItem, useCollection } from './kit/kit.js';
//   render(html`<${App} routes=${{ '/': Home, '/item/:id': Item }} tabs=${tabs} />`, document.body);
//
// Routes live in location.hash (`#/item/3`). A screen component gets
// `{params, path}` and renders its own <AppBar> and content.

import {
  html, Fragment, createContext, useState, useEffect, useMemo, useRef, useCallback, useContext,
} from './preact.js';
import { icon as svgIcon } from './icons.js';

const cx = (...c) => c.filter(Boolean).join(' ');

/** An icon from icons.js by name, or `name` itself when it is a node. */
export function Icon({ name, size = 22, stroke = 2, label, class: cls }) {
  if (name == null) return null;
  if (typeof name !== 'string') return name;
  return html`<span class=${cx('k-icon', cls)} style="display:inline-grid;line-height:0"
    dangerouslySetInnerHTML=${{ __html: svgIcon(name, { size, stroke, label }) }} />`;
}

/** Raw markup, such as a chart.js chart. */
export function Markup({ html: markup, class: cls, style }) {
  return html`<div class=${cls} style=${style} dangerouslySetInnerHTML=${{ __html: markup || '' }} />`;
}

// ---------------------------------------------------------------------------
// Router.

function readHash() {
  const raw = decodeURIComponent(location.hash.replace(/^#/, '')) || '/';
  const [p, q = ''] = raw.split('?');
  const path = p.startsWith('/') ? p : `/${p}`;
  return { path, query: Object.fromEntries(new URLSearchParams(q)) };
}

function match(pattern, path) {
  if (pattern === '*') return {};
  const a = pattern.split('/').filter(Boolean);
  const b = path.split('/').filter(Boolean);
  if (a.length !== b.length) return null;
  const params = {};
  for (let i = 0; i < a.length; i++) {
    if (a[i].startsWith(':')) params[a[i].slice(1)] = b[i];
    else if (a[i] !== b[i]) return null;
  }
  return params;
}

/** Shows `path` (`'/item/3'`), adding a history entry. */
export function go(path) {
  const next = `#${path.startsWith('/') ? path : `/${path}`}`;
  if (location.hash !== next) location.hash = next;
}

/** Goes back one screen, or to `fallback` when there is no screen to go back to. */
export function back(fallback = '/') {
  if (history.length > 1 && stack.length > 1) history.back();
  else go(fallback);
}

const stack = [];
const RouteCtx = createContext({ path: '/', params: {}, query: {} });

/** The current route: `{path, params, query, go, back}`. */
export function useRoute() {
  return { ...useContext(RouteCtx), go, back };
}

function useHashRoute(tabPaths) {
  const [state, setState] = useState(() => {
    const r = readHash();
    stack.push(r.path);
    return { ...r, dir: 'none' };
  });
  useEffect(() => {
    const on = () => {
      const r = readHash();
      let dir = 'forward';
      if (stack.length > 1 && stack[stack.length - 2] === r.path) {
        stack.pop();
        dir = 'back';
      } else {
        stack.push(r.path);
        if (stack.length > 50) stack.shift();
      }
      setState((prev) => {
        if (tabPaths.includes(r.path) && tabPaths.includes(prev.path)) dir = 'tab';
        return { ...r, dir };
      });
      window.scrollTo(0, 0);
    };
    window.addEventListener('hashchange', on);
    return () => window.removeEventListener('hashchange', on);
  }, []);
  return state;
}

/**
 * The app shell: routes over location.hash, the tab bar, toasts, safe areas.
 * - `routes`: `{'/': Home, '/item/:id': Item, '*': NotFound}`.
 * - `tabs`: `[{path: '/', label: 'Home', icon: 'home', badge?}]` (optional).
 * - `theme`: `'light'` or `'dark'` forces one; omitted follows the system.
 * - `children`: rendered instead of routes when `routes` is omitted.
 */
export function App({ routes, tabs, theme, children }) {
  const tabPaths = (tabs || []).map((t) => t.path);
  const route = useHashRoute(tabPaths);
  useEffect(() => {
    if (theme) document.documentElement.setAttribute('data-theme', theme);
    else document.documentElement.removeAttribute('data-theme');
  }, [theme]);
  let screen = children;
  let params = {};
  if (routes) {
    const entry = Object.entries(routes).find(([p]) => p !== '*' && match(p, route.path))
      || (routes['*'] ? ['*', routes['*']] : null);
    if (entry) {
      params = match(entry[0], route.path) || {};
      const C = entry[1];
      screen = html`<${C} params=${params} path=${route.path} query=${route.query} />`;
    } else {
      screen = html`<${Empty} icon="compass" title="Nothing here" body="This screen does not exist."
        action=${html`<${Button} variant="tonal" onClick=${() => go('/')}>Go home<//>`} />`;
    }
  }
  const anim = { back: 'k-back', tab: 'k-tab-switch', none: '', forward: '' }[route.dir];
  return html`<${RouteCtx.Provider} value=${{ path: route.path, params, query: route.query }}>
    <div class=${cx('k-app', tabs && tabs.length && 'k-has-tabs')}>
      <main class="k-main">
        <div class=${cx('k-screen', anim)} key=${route.path}>${screen}</div>
      </main>
      ${tabs && tabs.length ? html`<${TabBar} tabs=${tabs} />` : null}
      <${ToastHost} />
    </div>
  <//>`;
}

/** The bottom tab bar; `App` renders it from its `tabs`. */
export function TabBar({ tabs }) {
  const { path } = useContext(RouteCtx);
  const active = tabs.reduce((best, t) => (
    (path === t.path || (t.path !== '/' && path.startsWith(`${t.path}/`))) && (!best || t.path.length > best.path.length) ? t : best
  ), null);
  return html`<nav class="k-tabbar" aria-label="Tabs">
    ${tabs.map((t) => html`<button class="k-tab" key=${t.path} aria-current=${t === active ? 'page' : undefined}
        onClick=${() => go(t.path)}>
      <span class="k-tab-icon"><${Icon} name=${t.icon} size=${22} stroke=${t === active ? 2.3 : 1.9} /></span>
      ${t.badge ? html`<span class="k-tab-badge">${t.badge}</span>` : null}
      <span>${t.label}</span>
    </button>`)}
  </nav>`;
}

/**
 * The top bar of a screen.
 * - `title`, `subtitle`; `large` shows a large title that folds into the bar on scroll.
 * - `back`: `true` or the `back` function (history back), a path, or any function to call;
 *   `backLabel` the text beside the chevron.
 * - `leading`, `actions`: nodes at either end (usually IconButtons).
 */
export function AppBar({ title, subtitle, large, back: backTo, backLabel = 'Back', leading, actions }) {
  const [scrolled, setScrolled] = useState(false);
  useEffect(() => {
    const limit = large ? 40 : 2;
    const on = () => setScrolled(window.scrollY > limit);
    on();
    window.addEventListener('scroll', on, { passive: true });
    return () => window.removeEventListener('scroll', on);
  }, [large]);
  const onBack = () => {
    if (typeof backTo === 'function') backTo();
    else if (typeof backTo === 'string') go(backTo);
    else back();
  };
  const bare = !backTo && !leading && !actions;
  const lead = backTo
    ? html`<button class="k-icon-btn k-back-btn" style="width:auto" onClick=${onBack} aria-label=${backLabel}>
        <${Icon} name="chevronLeft" size=${28} stroke=${2.2} /><span>${backLabel}</span></button>`
    : leading;
  return html`<${Fragment}>
    <header class=${cx('k-appbar', scrolled && 'k-scrolled', large && 'k-large-mode', bare && 'k-bare')}>
      <div class="k-appbar-row">
        <div class="k-appbar-lead">${lead}</div>
        <h1 class="k-appbar-title">${title}</h1>
        <div class="k-appbar-actions">${actions}</div>
      </div>
    </header>
    ${large ? html`<div class=${cx('k-large-title', bare && 'k-bare')}><h2 class="k-large">${title}</h2>
      ${subtitle ? html`<div class="k-subtitle">${subtitle}</div>` : null}</div>` : null}
  <//>`;
}

/** A titled block with the side gutter; `action` sits at the right of the title. */
export function Section({ title, action, children, class: cls }) {
  return html`<section class=${cx('k-section', cls)}>
    ${title ? html`<h3 class="k-section-title"><span>${title}</span>${action}</h3>` : null}
    ${children}
  </section>`;
}

// ---------------------------------------------------------------------------
// Content.

/** An inset grouped list with an optional `header` and `footer`. */
export function List({ header, footer, flush, children, class: cls }) {
  const items = [children].flat(Infinity).filter((c) => c != null && c !== false);
  return html`<div class=${cx(!flush && 'k-list-wrap', cls)}>
    ${header ? html`<div class="k-list-header">${header}</div>` : null}
    <ul class=${cx('k-list', flush && 'k-flush')}>
      ${items.map((c, i) => html`<li key=${c.key ?? i} class=${c.props && (c.props.lead || c.props.avatar || c.props.check) ? 'k-has-lead' : ''}>${c}</li>`)}
    </ul>
    ${footer ? html`<div class="k-list-footer">${footer}</div>` : null}
  </div>`;
}

function initials(s) {
  return String(s || '?').split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0].toUpperCase()).join('');
}

/**
 * A row of a List.
 * - `title`, `subtitle`; `trail` (text or node) at the right.
 * - `lead`: an icon name or node, on a tile of `leadColor` (a CSS color; `'none'` for a bare icon).
 * - `avatar`: a name whose initials fill a round tile.
 * - `check`: `{checked, onChange}` shows a check circle in front.
 * - `onClick` or `href` makes it tappable with a chevron (`chevron={false}` hides it).
 * - `destructive`, `done` style the title.
 */
export function ListItem({ title, subtitle, trail, lead, leadColor, avatar, check, onClick, href, chevron, destructive, done, children }) {
  const tappable = onClick || href;
  const Tag = href ? 'a' : onClick ? 'button' : 'div';
  const leadNode = check
    ? html`<${Check} checked=${check.checked} onChange=${check.onChange} label=${title} />`
    : avatar
      ? html`<span class="k-item-lead k-avatar" style=${`background:${leadColor || 'var(--k-accent)'}`}>${initials(avatar)}</span>`
      : lead != null
        ? html`<span class=${cx('k-item-lead', leadColor === 'none' && 'k-plain-lead')} style=${leadColor && leadColor !== 'none' ? `background:${leadColor}` : ''}>
            <${Icon} name=${lead} size=${leadColor === 'none' ? 24 : 19} /></span>`
        : null;
  return html`<${Tag} class=${cx('k-item', destructive && 'k-destructive', done && 'k-done')}
      href=${href} onClick=${onClick} type=${Tag === 'button' ? 'button' : undefined}>
    ${leadNode}
    <div class="k-item-text">
      <div class="k-item-title">${title}</div>
      ${subtitle ? html`<div class="k-item-sub">${subtitle}</div>` : null}
      ${children}
    </div>
    ${trail != null || (tappable && chevron !== false) ? html`<div class="k-item-trail">
      ${trail}
      ${tappable && chevron !== false ? html`<span class="k-item-chevron"><${Icon} name="chevronRight" size=${20} /></span>` : null}
    </div>` : null}
  <//>`;
}

/** A round check button. */
export function Check({ checked, onChange, label }) {
  return html`<button type="button" class="k-check" role="checkbox" aria-checked=${checked ? 'true' : 'false'} aria-label=${label}
    onClick=${(e) => { e.stopPropagation(); onChange && onChange(!checked); }}>
    <${Icon} name="check" size=${16} stroke=${3} />
  </button>`;
}

/** A surface. `title`/`subtitle`/`action` make a head; `hero` fills it with the accent gradient; `onClick` makes it tappable. */
export function Card({ title, subtitle, action, hero, onClick, children, class: cls, style }) {
  return html`<div class=${cx('k-card', hero && 'k-hero', onClick && 'k-tappable', cls)} style=${style} onClick=${onClick}
      role=${onClick ? 'button' : undefined} tabindex=${onClick ? 0 : undefined}>
    ${title || action ? html`<div class="k-card-head"><div>
        ${title ? html`<h3 class="k-card-title">${title}</h3>` : null}
        ${subtitle ? html`<p class="k-card-sub">${subtitle}</p>` : null}
      </div>${action}</div>` : null}
    ${children}
  </div>`;
}

/**
 * A button. `variant`: `filled` (default), `tonal`, `outline`, `plain`, `danger`, `gradient`;
 * `size`: `small`, `large`; `block` spans the width; `icon` an icon name; `loading` shows a spinner.
 */
export function Button({ variant = 'filled', size, block, icon, loading, disabled, onClick, type = 'button', children, class: cls }) {
  return html`<button type=${type} class=${cx('k-btn', variant !== 'filled' && (variant === 'danger' ? 'k-danger' : `k-${variant}`),
      size === 'small' && 'k-small', size === 'large' && 'k-large-btn', block && 'k-block', cls)}
      disabled=${disabled || loading} onClick=${onClick}>
    ${loading ? html`<span class="k-spinner" />` : icon ? html`<${Icon} name=${icon} size=${20} />` : null}
    ${children}
  </button>`;
}

/** A 44 px round icon button; `label` is its accessible name. */
export function IconButton({ icon, label, onClick, filled, neutral, disabled, size = 24 }) {
  return html`<button type="button" class=${cx('k-icon-btn', filled && 'k-filled', neutral && 'k-neutral')} aria-label=${label} title=${label}
    disabled=${disabled} onClick=${onClick}><${Icon} name=${icon} size=${size} /></button>`;
}

/** A pill badge; `tone`: `success`, `warning`, `danger`. */
export function Badge({ tone, children }) {
  return html`<span class=${cx('k-badge', tone && `k-${tone}`)}>${children}</span>`;
}

/** A progress bar of `value` out of `max` (default 1). */
export function Progress({ value = 0, max = 1, color }) {
  const pct = Math.max(0, Math.min(100, (value / (max || 1)) * 100));
  return html`<div class="k-progress" role="progressbar" aria-valuenow=${value} aria-valuemax=${max}>
    <div style=${`width:${pct}%${color ? `;background:${color}` : ''}`} /></div>`;
}

let fieldSeq = 0;

/**
 * A labelled input. `type`: `text` (default), `number`, `date`, `time`, `email`, `textarea`,
 * `select` (with `options`: strings or `{value, label}`), or `toggle`.
 * `onChange(value)` gets the new value: a number for `number` (null when empty), a boolean for `toggle`.
 */
export function Field({ label, type = 'text', value, onChange, options = [], placeholder, hint, error, min, max, step, rows = 4, autofocus }) {
  const id = useMemo(() => `k-f${++fieldSeq}`, []);
  if (type === 'toggle') {
    return html`<label class="k-toggle-row" for=${id}>
      <span class="k-body">${label}</span>
      <span class="k-switch"><input id=${id} type="checkbox" role="switch" checked=${!!value}
        onChange=${(e) => onChange && onChange(e.currentTarget.checked)} /><span class="k-switch-track" /></span>
    </label>`;
  }
  const emit = (e) => {
    const v = e.currentTarget.value;
    if (!onChange) return;
    if (type === 'number') onChange(v === '' ? null : Number(v));
    else onChange(v);
  };
  let control;
  if (type === 'select') {
    control = html`<select id=${id} class="k-input" value=${value ?? ''} onChange=${emit}>
      ${placeholder ? html`<option value="" disabled>${placeholder}</option>` : null}
      ${options.map((o) => (typeof o === 'object'
        ? html`<option value=${o.value}>${o.label ?? o.value}</option>`
        : html`<option value=${o}>${o}</option>`))}
    </select>`;
  } else if (type === 'textarea') {
    control = html`<textarea id=${id} class="k-input" rows=${rows} value=${value ?? ''} placeholder=${placeholder} onInput=${emit} autofocus=${autofocus} />`;
  } else {
    control = html`<input id=${id} class="k-input" type=${type} value=${value ?? ''} placeholder=${placeholder}
      min=${min} max=${max} step=${step} inputmode=${type === 'number' ? 'decimal' : undefined}
      onInput=${emit} autofocus=${autofocus} />`;
  }
  return html`<div class=${cx('k-field', error && 'k-invalid')}>
    ${label ? html`<label class="k-field-label" for=${id}>${label}</label>` : null}
    ${control}
    ${error ? html`<div class="k-field-error">${error}</div>` : hint ? html`<div class="k-field-hint">${hint}</div>` : null}
  </div>`;
}

const optValue = (o) => (typeof o === 'object' ? o.value : o);
const optLabel = (o) => (typeof o === 'object' ? (o.label ?? o.value) : o);

/** An iOS-style segmented control over `options` (strings or `{value, label}`). */
export function SegmentedControl({ options, value, onChange }) {
  const i = Math.max(0, options.findIndex((o) => optValue(o) === value));
  const n = options.length || 1;
  return html`<div class="k-segmented" role="tablist">
    <div class="k-segmented-thumb" style=${`width:calc((100% - 4px) / ${n});transform:translateX(${i * 100}%)`} />
    ${options.map((o) => html`<button type="button" role="tab" class="k-segment" aria-selected=${optValue(o) === value}
      onClick=${() => onChange && onChange(optValue(o))}>${optLabel(o)}</button>`)}
  </div>`;
}

/**
 * Filter chips over `options` (strings or `{value, label, icon}`). With `multiple`, `value`
 * is an array and `onChange` gets the new array; otherwise tapping the chosen chip clears it (null).
 */
export function Chips({ options, value, onChange, multiple, wrap }) {
  const on = (v) => (multiple ? (value || []).includes(v) : value === v);
  const tap = (v) => {
    if (!onChange) return;
    if (multiple) onChange(on(v) ? value.filter((x) => x !== v) : [...(value || []), v]);
    else onChange(on(v) ? null : v);
  };
  return html`<div class=${cx('k-chips', wrap && 'k-wrap')}>
    ${options.map((o) => html`<button type="button" class="k-chip" aria-pressed=${on(optValue(o)) ? 'true' : 'false'}
      onClick=${() => tap(optValue(o))}>${typeof o === 'object' && o.icon ? html`<${Icon} name=${o.icon} size=${16} />` : null}${optLabel(o)}</button>`)}
  </div>`;
}

/** A figure: `label`, `value`, `unit`, and a `delta` (number; positive is up) with `deltaLabel`. */
export function Stat({ label, value, unit, delta, deltaLabel, icon, invert }) {
  const up = delta > 0;
  const good = invert ? !up : up;
  return html`<div class="k-stat">
    <div class="k-stat-label">${icon ? html`<${Icon} name=${icon} size=${15} />` : null}${label}</div>
    <div class="k-stat-value">${value}${unit ? html`<span class="k-stat-unit">${unit}</span>` : null}</div>
    ${delta != null && delta !== 0 ? html`<div class=${cx('k-stat-delta', good ? 'k-up' : 'k-down')}>
      <${Icon} name=${up ? 'trendingUp' : 'trendingDown'} size=${14} stroke=${2.4} />${deltaLabel ?? `${up ? '+' : ''}${delta}`}</div>` : null}
  </div>`;
}

/** An empty state: `icon`, `title`, `body`, and an `action` node. */
export function Empty({ icon = 'inbox', title, body, action }) {
  return html`<div class="k-empty">
    <div class="k-empty-icon"><${Icon} name=${icon} size=${34} stroke=${1.8} /></div>
    ${title ? html`<h3 class="k-empty-title">${title}</h3>` : null}
    ${body ? html`<p class="k-empty-body">${body}</p>` : null}
    ${action}
  </div>`;
}

/** A floating action button above the tab bar; `label` makes it extended. */
export function Fab({ icon = 'plus', label, onClick }) {
  return html`<button type="button" class=${cx('k-fab', label && 'k-extended')} aria-label=${label || 'Add'} onClick=${onClick}>
    <${Icon} name=${icon} size=${24} stroke=${2.4} />${label ? html`<span>${label}</span>` : null}
  </button>`;
}

// ---------------------------------------------------------------------------
// Overlays.

/** Keeps an overlay mounted through its closing animation: `[mounted, closing, onAnimationEnd]`. */
function usePresence(open) {
  const [mounted, setMounted] = useState(open);
  useEffect(() => { if (open) setMounted(true); }, [open]);
  const closing = mounted && !open;
  const onEnd = useCallback((e) => { if (e.target === e.currentTarget && !open) setMounted(false); }, [open]);
  return [mounted, closing, onEnd];
}

function useEscape(open, onClose) {
  useEffect(() => {
    if (!open || !onClose) return undefined;
    const on = (e) => { if (e.key === 'Escape') onClose(); };
    document.addEventListener('keydown', on);
    const prev = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => { document.removeEventListener('keydown', on); document.body.style.overflow = prev; };
  }, [open]);
}

/**
 * A bottom sheet. `open`, `onClose`, `title`; `action: {label, onClick, disabled}` puts a
 * button at the top right (a Cancel sits at the left).
 */
export function Sheet({ open, onClose, title, action, children }) {
  const [mounted, closing, onEnd] = usePresence(open);
  useEscape(open, onClose);
  if (!mounted) return null;
  return html`<${Fragment}>
    <div class=${cx('k-scrim', closing && 'k-closing')} onClick=${onClose} />
    <div class=${cx('k-sheet', closing && 'k-closing')} role="dialog" aria-modal="true" aria-label=${title} onAnimationEnd=${onEnd}>
      <div class="k-sheet-grabber" />
      <div class="k-sheet-head">
        <div><button type="button" class="k-btn k-plain" onClick=${onClose}>Cancel</button></div>
        <h2 class="k-appbar-title">${title}</h2>
        <div>${action ? html`<button type="button" class="k-btn k-plain" style="font-weight:700" disabled=${action.disabled}
          onClick=${action.onClick}>${action.label}</button>` : null}</div>
      </div>
      <div class="k-sheet-body">${children}</div>
    </div>
  <//>`;
}

/**
 * A centered alert. `open`, `onClose`, `title`, `body`, and `actions`:
 * `[{label, variant, onClick}]` (two sit side by side, more stack).
 */
export function Dialog({ open, onClose, title, body, actions = [{ label: 'OK' }], children }) {
  const [mounted, closing, onEnd] = usePresence(open);
  useEscape(open, onClose);
  if (!mounted) return null;
  return html`<${Fragment}>
    <div class=${cx('k-scrim', closing && 'k-closing')} onClick=${onClose} />
    <div class=${cx('k-dialog', closing && 'k-closing')} role="alertdialog" aria-modal="true" aria-label=${title} onAnimationEnd=${onEnd}>
      ${title ? html`<h2 class="k-dialog-title">${title}</h2>` : null}
      ${body ? html`<p class="k-dialog-body">${body}</p>` : null}
      ${children}
      <div class=${cx('k-dialog-actions', actions.length === 2 && 'k-two')}>
        ${actions.map((a) => html`<${Button} variant=${a.variant || 'tonal'} block onClick=${() => { a.onClick && a.onClick(); onClose && onClose(); }}>${a.label}<//>`)}
      </div>
    </div>
  <//>`;
}

// Toasts.
let toastSeq = 0;
let toasts = [];
const toastListeners = new Set();
const emitToasts = () => toastListeners.forEach((f) => f(toasts));

/** Shows a short message at the top: `toast('Saved', {tone: 'success', icon, ms})`. */
export function toast(message, { tone, icon, ms = 2600 } = {}) {
  const id = ++toastSeq;
  toasts = [...toasts, { id, message, tone, icon, closing: false }];
  emitToasts();
  setTimeout(() => {
    toasts = toasts.map((t) => (t.id === id ? { ...t, closing: true } : t));
    emitToasts();
  }, ms);
  return id;
}

function ToastHost() {
  const [list, setList] = useState(toasts);
  useEffect(() => { toastListeners.add(setList); return () => toastListeners.delete(setList); }, []);
  const gone = (id) => { toasts = toasts.filter((t) => t.id !== id); emitToasts(); };
  return html`<div class="k-toasts" role="status" aria-live="polite">
    ${list.map((t) => html`<${Toast} key=${t.id} message=${t.message} tone=${t.tone} icon=${t.icon} closing=${t.closing}
      onGone=${() => t.closing && gone(t.id)} />`)}
  </div>`;
}

/** One toast; use `toast()` to show one from anywhere (App hosts them). */
export function Toast({ message, tone, icon, closing, onGone }) {
  const name = icon || (tone === 'danger' ? 'alert' : tone === 'success' ? 'checkCircle' : 'info');
  return html`<div class=${cx('k-toast', tone && `k-${tone}`, closing && 'k-closing')} onAnimationEnd=${onGone}>
    <span class="k-toast-icon"><${Icon} name=${name} size=${20} stroke=${2.2} /></span>${message}
  </div>`;
}

// ---------------------------------------------------------------------------
// Data: the db capability, or memory when the page has none.

let dbPromise;
/**
 * The `db` capability, or null when the page runs without it (not declared, or outside the
 * viewer); then the hooks keep data in memory, lost on reload, and warn once in the console.
 */
export function getDb() {
  if (!dbPromise) {
    dbPromise = Promise.resolve()
      .then(() => (window.claude && window.claude.use ? window.claude.use('db') : null))
      .catch(() => null)
      .then((db) => {
        if (!db) {
          console.warn('[kit] The db capability is not available (declare {"db": {}}); '
            + 'useDoc and useCollection keep data in memory only, lost on reload.');
        }
        return db;
      });
  }
  return dbPromise;
}

const memDocs = new Map();
const memListeners = new Set();
const memChanged = () => memListeners.forEach((f) => f());
const memWatch = (f) => { memListeners.add(f); f(); return () => memListeners.delete(f); };
const memCollection = (col) => {
  const depth = col.split('/').length + 1;
  const out = [];
  for (const [p, d] of memDocs) {
    if (p.startsWith(`${col}/`) && p.split('/').length === depth) out.push({ ...d, id: p.slice(col.length + 1) });
  }
  return out;
};
const isObj = (v) => v != null && typeof v === 'object' && !Array.isArray(v);
// Merges as db's update does: nested objects recursively, anything else replaced.
const deepMerge = (a, b) => {
  const out = { ...a };
  for (const [k, v] of Object.entries(b)) out[k] = isObj(v) && isObj(a[k]) ? deepMerge(a[k], v) : v;
  return out;
};
const invalid = (message) => Object.assign(new Error(message), { code: 'invalid_argument' });

/** A fresh document ID. */
export function uid() {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

// One write at a time per document path. A failed write shows a toast and
// rejects the promise the caller holds.
const writeChains = new Map();
function serial(path, fn) {
  const prev = writeChains.get(path) || Promise.resolve();
  const next = prev.catch(() => {}).then(fn).catch((e) => {
    toast(`Could not save: ${(e && e.message) || e}`, { tone: 'danger' });
    throw e;
  });
  writeChains.set(path, next);
  return next;
}

const strip = (d) => { const { id, ...rest } = d || {}; return rest; };

/**
 * One document at `path` (an even number of segments, such as `settings/main`):
 * `{data, save, ready, error}`. `data` is `initial` until the document exists;
 * `save(next)` replaces it (`next` may be a function of the current data) and returns a
 * promise that rejects, after showing a toast, when the write fails.
 */
export function useDoc(path, initial = null) {
  const [state, setState] = useState({ data: initial, ready: false, error: null });
  const current = useRef(initial);
  current.current = state.data;
  useEffect(() => {
    let dead = false;
    let unsub = () => {};
    getDb().then((db) => {
      if (dead) return;
      if (!db) {
        unsub = memWatch(() => setState({ data: memDocs.has(path) ? memDocs.get(path) : initial, ready: true, error: null }));
        return;
      }
      unsub = db.doc(path).onSnapshot(
        (s) => setState({ data: s.exists ? s.data() : initial, ready: true, error: null }),
        (e) => setState((st) => ({ ...st, ready: true, error: e })),
      );
    });
    return () => { dead = true; unsub(); };
  }, [path]);
  const save = useCallback((next) => {
    const value = typeof next === 'function' ? next(current.current) : next;
    current.current = value;
    return serial(path, async () => {
      if (!isObj(value)) throw invalid('a document must be an object');
      const db = await getDb();
      if (db) await db.doc(path).set(value);
      else { memDocs.set(path, value); memChanged(); }
    });
  }, [path]);
  return { data: state.data, save, ready: state.ready, error: state.error };
}

/**
 * The documents of collection `path` (an odd number of segments, such as `tasks`), each as
 * `{id, ...data}`. Options: `orderBy` (a field), `dir` (`asc`/`desc`), `limit`.
 * Returns `{items, ready, error, add(data) -> id, set(id, data), update(id, patch), remove(id)}`;
 * each write returns a promise that rejects, after showing a toast, when the write fails.
 */
export function useCollection(path, { orderBy, dir = 'asc', limit } = {}) {
  const [state, setState] = useState({ items: [], ready: false, error: null });
  useEffect(() => {
    let dead = false;
    let unsub = () => {};
    getDb().then((db) => {
      if (dead) return;
      if (!db) {
        unsub = memWatch(() => {
          let items = memCollection(path);
          if (orderBy) {
            const k = dir === 'desc' ? -1 : 1;
            items.sort((a, b) => (a[orderBy] == null ? 1 : b[orderBy] == null ? -1 : a[orderBy] < b[orderBy] ? -k : a[orderBy] > b[orderBy] ? k : 0));
          } else items.sort((a, b) => (a.id < b.id ? -1 : 1));
          if (limit) items = items.slice(0, limit);
          setState({ items, ready: true, error: null });
        });
        return;
      }
      let q = db.collection(path);
      if (orderBy) q = q.orderBy(orderBy, dir);
      if (limit) q = q.limit(limit);
      unsub = q.onSnapshot(
        (s) => setState({ items: s.docs.map((d) => ({ ...d.data(), id: d.id })), ready: true, error: null }),
        (e) => setState((st) => ({ ...st, ready: true, error: e })),
      );
    });
    return () => { dead = true; unsub(); };
  }, [path, orderBy, dir, limit]);
  const api = useMemo(() => ({
    async add(data) {
      const id = uid();
      await api.set(id, data);
      return id;
    },
    set(id, data) {
      const p = `${path}/${id}`;
      return serial(p, async () => {
        if (!isObj(data)) throw invalid('a document must be an object');
        const db = await getDb();
        if (db) await db.doc(p).set(strip(data));
        else { memDocs.set(p, strip(data)); memChanged(); }
      });
    },
    update(id, patch) {
      const p = `${path}/${id}`;
      return serial(p, async () => {
        const db = await getDb();
        if (db) await db.doc(p).update(strip(patch));
        else if (memDocs.has(p)) { memDocs.set(p, deepMerge(memDocs.get(p), strip(patch))); memChanged(); }
        else throw invalid(`update of ${p}, which does not exist`);
      });
    },
    remove(id) {
      const p = `${path}/${id}`;
      return serial(p, async () => {
        const db = await getDb();
        if (db) await db.doc(p).delete();
        else { memDocs.delete(p); memChanged(); }
      });
    },
  }), [path]);
  return { ...state, ...api };
}

// ---------------------------------------------------------------------------
// Formatting.

/** Formats a number: `fmt.number(1234.5)`, `fmt.money(9.5, 'USD')`, `fmt.percent(0.42)`. */
export const fmt = {
  number: (v, digits = 0) => new Intl.NumberFormat(undefined, { maximumFractionDigits: digits }).format(v || 0),
  money: (v, currency = 'USD') => new Intl.NumberFormat(undefined, { style: 'currency', currency }).format(v || 0),
  percent: (v, digits = 0) => new Intl.NumberFormat(undefined, { style: 'percent', maximumFractionDigits: digits }).format(v || 0),
};
