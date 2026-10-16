// A stand-in for the Clax app's `home` capability (`await
// claude.use("home")`), with sample apps and makes, so a home app's tests and
// the agent's inspector can run it outside the home screen, where the real
// capability is unavailable.
//
//   import { fakeHome } from '../kit/fake-home.js';
//   const home = fakeHome();                  // the sample below
//   const home = fakeHome({ apps: [], makes: [], unread: 0, needsKey: true });
//
// It answers what the real one does: `apps()` (newest first), `makes()`,
// `unread()`, `needsKey()`, the actions `open(id)`, `openMake(id)`,
// `newApp()`, `menu(id, {x, y, w, h})`, `openInbox()`, `openSettings()`,
// `importApp()`, `shown()`, and `on(name, fn)` for `apps`, `makes`, `unread`
// and `key`, which returns a function that stops the calls. Each action is
// kept in `home.calls` as `{method, args}` instead of opening anything;
// `home.shownCount` counts the page's `shown()` calls (a test asserts the
// page said once that it was drawn: `home.shownCount === 1`); and
// `home.set({apps, makes, unread, needsKey})` changes the state and calls
// the listeners of what changed, as the app does when a make finishes or an
// app changes.

/** An app icon as the daemon serves it: a `data:` URL of an SVG with a
 * gradient and a white glyph. */
function svgIcon(from, to, glyph) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 60 60"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${from}"/><stop offset="1" stop-color="${to}"/></linearGradient></defs><rect width="60" height="60" fill="url(#g)"/><g fill="none" stroke="#fff" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round">${glyph}</g></svg>`;
  return { url: `data:image/svg+xml,${encodeURIComponent(svg)}` };
}

const hours = (n) => new Date(Date.UTC(2026, 9, 16, 12) - n * 3600_000).toISOString();

/** The sample a fake starts from when given nothing: what the launcher's
 * own renders show. */
export const SAMPLE = Object.freeze({
  makes: [
    { id: 'make-packing', request: 'A packing list for a weekend away', state: 'waiting', progress: null },
    { id: 'make-reading', request: 'A reading log', state: 'making', progress: null },
    { id: 'make-water', request: 'A water reminder every two hours', state: 'interrupted', progress: null },
  ],
  apps: [
    { id: 'app-run', title: 'Run tracker', updatedAt: hours(1), build: null, icon: svgIcon('#ff8a3d', '#e8451e', '<path d="M14 40 L25 29 L32 35 L46 19"/><circle cx="46" cy="19" r="2.5" fill="#fff"/><path d="M14 46 H46" stroke-width="2"/>') },
    { id: 'app-groceries', title: 'Groceries', updatedAt: hours(2), build: null, icon: svgIcon('#3cc46b', '#16893f', '<path d="M15 26 H45 L41 44 H19 Z"/><path d="M22 26 L28 15 M38 26 L32 15"/><path d="M25 32 V39 M30 32 V39 M35 32 V39"/>') },
    { id: 'app-habits', title: 'Habit streaks', updatedAt: hours(3), build: null, icon: svgIcon('#8b6cff', '#4c2fd1', '<circle cx="30" cy="30" r="15"/><path d="M23 30 L28 35 L37 25"/>') },
    { id: 'app-recipes', title: 'Recipes', updatedAt: hours(4), build: null, icon: { emoji: '🍲' } },
    { id: 'app-budget', title: 'Weekly budget', updatedAt: hours(5), build: null, icon: { word: 'money' } },
    { id: 'app-plants', title: 'Plant care', updatedAt: hours(6), build: null, icon: svgIcon('#2a6df4', '#0b2f8a', '<path d="M30 14 C38 25 42 31 42 37 A12 12 0 0 1 18 37 C18 31 22 25 30 14 Z"/>') },
    { id: 'app-mood', title: 'Mood journal', updatedAt: hours(7), build: null, icon: { monogram: 'MJ', hue: 270 } },
    { id: 'app-notes', title: 'Notes', updatedAt: hours(8), build: null, icon: { monogram: 'N', hue: 320 } },
  ],
  unread: 0,
  needsKey: false,
});

const copy = (v) => JSON.parse(JSON.stringify(v));

/** A fake `home` capability over `state` (missing fields from SAMPLE). */
export function fakeHome(state = {}) {
  const s = { ...copy(SAMPLE), ...copy(state) };
  const listeners = { apps: new Set(), makes: new Set(), unread: new Set(), key: new Set() };
  const calls = [];
  let shown = 0;
  const act = (method) => async (...args) => { calls.push({ method, args: copy(args) }); return null; };
  const emit = (name, data) => { for (const fn of [...listeners[name]]) fn(copy(data)); };
  return {
    apps: async () => copy(s.apps),
    makes: async () => copy(s.makes),
    unread: async () => s.unread,
    needsKey: async () => s.needsKey,
    open: act('open'),
    openMake: act('openMake'),
    newApp: act('newApp'),
    menu: act('menu'),
    openInbox: act('openInbox'),
    openSettings: act('openSettings'),
    importApp: act('importApp'),
    shown: async () => { shown++; return null; },
    get shownCount() { return shown; },
    on(name, fn) {
      if (!listeners[name]) throw new TypeError(`no home event named ${JSON.stringify(name)}`);
      listeners[name].add(fn);
      return () => listeners[name].delete(fn);
    },
    calls,
    set(patch) {
      const next = copy(patch);
      if ('apps' in next) { s.apps = next.apps; emit('apps', s.apps); }
      if ('makes' in next) { s.makes = next.makes; emit('makes', s.makes); }
      if ('unread' in next) { s.unread = next.unread; emit('unread', s.unread); }
      if ('needsKey' in next) { s.needsKey = next.needsKey; emit('key', s.needsKey); }
    },
  };
}
