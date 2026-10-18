// The home screen's tests, run by the inspect view's harness (test, expect,
// ui). Each test mounts the page over its own fake `home` capability.
import { mount, start } from '../home.js';
import { fakeHome, fakeDb, SAMPLE } from '../kit/fake-home.js';
import { manualClock } from '../physics.js';

const root = () => document.getElementById('app');
const tiles = () => [...document.querySelectorAll('.h-tile')];
const labels = () => tiles().map((t) => t.querySelector('.h-label').textContent.trim());

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

const wait = (fn) => ui.waitFor(fn, 3000);

test('lists the apps newest first, with each kind of icon', async () => {
  const shuffled = [SAMPLE.apps[2], SAMPLE.apps[0], SAMPLE.apps[7], SAMPLE.apps[1]];
  await show({ makes: [], apps: shuffled });
  expect(labels()).toEqual(['Run tracker', 'Groceries', 'Habit streaks', 'Notes']);
  await show({ makes: [] });
  const faces = tiles().map((t) => t.querySelector('.h-face').dataset.kind);
  expect(faces).toEqual(['url', 'url', 'url', 'emoji', 'word', 'url', 'monogram', 'monogram']);
  expect(ui.text('[data-id="app-mood"] .h-face')).toBe('MJ');
  expect(ui.text('[data-id="app-recipes"] .h-face')).toBe('🍲');
  expect(document.querySelector('[data-id="app-run"] .h-face img').getAttribute('src')).toContain('data:image/svg+xml');
});

test('an unknown icon word falls back to the title\'s monogram', async () => {
  await show({ makes: [], apps: [{ id: 'x', title: 'Pocket money', updatedAt: SAMPLE.apps[0].updatedAt, build: null, icon: { word: 'zzz-unknown' } }] });
  expect(ui.text('[data-id="x"] .h-face')).toBe('PM');
});

test('making tiles come first, each saying how it stands', async () => {
  await show({
    makes: [
      { id: 'm1', request: 'A packing list', state: 'making', progress: null },
      { id: 'm2', request: 'A step counter', state: 'making', progress: { done: 3, total: 7 } },
      { id: 'm3', request: 'A water reminder', state: 'offline', progress: null },
      { id: 'm4', request: 'A sleep log', state: 'waiting', progress: null },
      { id: 'm5', request: 'A chore chart', state: 'interrupted', progress: null },
      { id: 'm6', request: 'A tide table', state: 'failed', progress: null },
    ],
  });
  expect(labels().slice(0, 7)).toEqual(['A packing list', 'A step counter', 'A water reminder', 'A sleep log', 'A chore chart', 'A tide table', 'Run tracker']);
  const status = (id) => ui.text(`[data-make="${id}"] .h-status`);
  expect(document.querySelector('[data-make="m1"]').getAttribute('aria-label')).toBe('A packing list, Making…');
  expect(ui.exists('[data-make="m1"] .h-status')).toBe(false);
  expect(ui.exists('[data-make="m1"] .h-spinner')).toBeTruthy();
  expect(ui.text('[data-make="m2"] .h-ring')).toBe('3/7');
  expect(status('m3')).toBe('Offline');
  expect(document.querySelector('[data-make="m3"]').getAttribute('aria-label')).toBe('A water reminder, Waiting for connection');
  expect(status('m4')).toBe('Waiting…');
  expect(status('m5')).toBe('Interrupted');
  expect(status('m6')).toBe("Couldn't make it");
});

test('an app whose build goes shows the 3/7 ring', async () => {
  const apps = SAMPLE.apps.map((a) => (a.id === 'app-notes' ? { ...a, build: { done: 3, total: 7 } } : a));
  await show({ makes: [], apps });
  expect(ui.text('[data-id="app-notes"] .h-ring')).toBe('3/7');
  expect(document.querySelector('[data-id="app-notes"]').getAttribute('aria-label')).toBe('Notes, building, 3 of 7 done');
});

test('a tap opens the app, and a making tile its make', async () => {
  const home = await show();
  await ui.click('[data-id="app-groceries"]');
  await ui.click('[data-make="make-water"]');
  expect(home.calls).toEqual([{ method: 'open', args: ['app-groceries'] }, { method: 'openMake', args: ['make-water'] }]);
});

test('a context menu asks for the menu at the tile; a long press arms, then asks on release, and does not open', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock });
  const tile = document.querySelector('[data-id="app-habits"]');
  tile.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 5, clientY: 5 }));
  await wait(() => home.calls.length === 1);
  const [first] = home.calls;
  expect(first.method).toBe('menu');
  expect(first.args[0]).toBe('app-habits');
  const r = tile.querySelector('.h-icon').getBoundingClientRect();
  const rect = { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
  expect(first.args[1]).toEqual(rect);
  expect(first.args[2]).toEqual({ items: [{ id: 'star', label: 'Star' }] });
  expect(rect.w).toBe(60);

  const opts = { bubbles: true, cancelable: true, pointerId: 7, isPrimary: true, pointerType: 'touch', clientX: r.x + 10, clientY: r.y + 10 };
  tile.dispatchEvent(new PointerEvent('pointerdown', opts));
  // The WebView's own long press comes while the finger is down: the release answers it.
  tile.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  clock.advance(399);
  expect('held' in tile.dataset).toBe(false);
  clock.advance(1);
  expect('held' in tile.dataset).toBe(true);
  expect(home.calls.length).toBe(1);
  tile.dispatchEvent(new PointerEvent('pointerup', opts));
  await wait(() => home.calls.length === 2);
  expect(home.calls[1]).toEqual({ method: 'menu', args: ['app-habits', rect, { items: [{ id: 'star', label: 'Star' }] }] });
  expect('held' in tile.dataset).toBe(false);
  tile.click();
  expect(home.calls.map((c) => c.method)).toEqual(['menu', 'menu']);
  tile.click();
  await wait(() => home.calls.length === 3);
  expect(home.calls[2].method).toBe('open');
});

test('a press that moves or is cancelled before it arms asks for nothing', async () => {
  const home = await show({ makes: [] });
  const tile = document.querySelector('[data-id="app-run"]');
  const r = tile.getBoundingClientRect();
  const at = (dx) => ({ bubbles: true, cancelable: true, pointerId: 3, isPrimary: true, pointerType: 'touch', clientX: r.x + 20 + dx, clientY: r.y + 20 });
  tile.dispatchEvent(new PointerEvent('pointerdown', at(0)));
  tile.dispatchEvent(new PointerEvent('pointermove', at(30)));
  tile.dispatchEvent(new PointerEvent('pointerup', at(30)));
  tile.dispatchEvent(new PointerEvent('pointerdown', at(0)));
  tile.dispatchEvent(new PointerEvent('pointercancel', at(0)));
  await new Promise((done) => setTimeout(done, 0));
  expect(home.calls).toEqual([]);
  expect('held' in tile.dataset).toBe(false);
});

test('an action refused for want of a tap says so, and the next tap on the notice tries it again', async () => {
  const home = await show({ makes: [] });
  const open = home.open;
  let refuse = true;
  home.open = (...a) => (refuse ? Promise.reject({ code: 'no_gesture', message: 'tap' }) : open(...a));
  await ui.click('[data-id="app-groceries"]');
  await wait(() => ui.exists('.h-notice'));
  expect(ui.text('.h-notice')).toBe('Tap again to open Groceries');
  refuse = false;
  await ui.click('.h-notice');
  await wait(() => home.calls.length === 1);
  expect(home.calls).toEqual([{ method: 'open', args: ['app-groceries'] }]);
  await wait(() => !ui.exists('.h-notice'));
});

test('an open or a menu retried from the notice keeps its change: the open counts, the picked Star stars', async () => {
  const seen = SAMPLE.apps.map((a) => a.id).filter((id) => id !== 'app-notes');
  const home = await show({ makes: [] }, { layout: { order: [], seen } });
  await wait(() => ui.exists('[data-id="app-notes"][data-new]'));
  const open = home.open;
  const menu = home.menu;
  let refuse = true;
  home.open = (...a) => (refuse ? Promise.reject({ code: 'no_gesture', message: 'tap' }) : open(...a));
  home.menu = (...a) => (refuse ? Promise.reject({ code: 'no_gesture', message: 'tap' }) : menu(...a));
  await ui.click('[data-id="app-notes"]');
  await wait(() => ui.text('.h-notice') === 'Tap again to open Notes');
  refuse = false;
  await ui.click('.h-notice');
  await wait(() => !ui.exists('[data-id="app-notes"][data-new]'));
  expect((await stored()).opens['app-notes'].n).toBe(1);

  refuse = true;
  home.menuPick = 'star';
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.text('.h-notice') === "Tap again to show Plant care's menu");
  refuse = false;
  await ui.click('.h-notice');
  await wait(() => section('starred').join() === 'app-plants');
  expect((await stored()).starred).toEqual(['app-plants']);
});

test('a tap on another control drops a refused action instead of retrying it', async () => {
  const home = await show({ makes: [] });
  home.open = () => Promise.reject({ code: 'no_gesture', message: 'tap' });
  await ui.click('[data-id="app-groceries"]');
  await wait(() => ui.exists('.h-notice'));
  await ui.click('[aria-label="Settings"]');
  await wait(() => home.calls.length === 1);
  expect(home.calls).toEqual([{ method: 'openSettings', args: [] }]);
});

test('other failed actions say what went wrong', async () => {
  const home = await show({ makes: [] });
  home.open = () => Promise.reject({ code: 'bad_request', message: 'gone' });
  home.newApp = () => Promise.reject({ code: 'unavailable', message: 'closed' });
  await ui.click('[data-id="app-groceries"]');
  await wait(() => ui.text('.h-notice') === 'That app is no longer here');
  await ui.click('New app');
  await wait(() => ui.text('.h-notice') === "Couldn't make a new app");
});

test('Refresh in More loads everything again', async () => {
  const home = await show({ makes: [] });
  const apps = home.apps;
  let loads = 0;
  home.apps = () => { loads++; return apps(); };
  await ui.click('[aria-label="More"]');
  await ui.click('Refresh');
  await wait(() => loads === 1);
});

test('a pull down from the top loads everything again', async () => {
  if (typeof Touch !== 'function') return;
  const home = await show({ makes: [] });
  const apps = home.apps;
  let loads = 0;
  home.apps = () => { loads++; return apps(); };
  scrollTo(0, 0);
  const touch = (y) => new Touch({ identifier: 1, target: document.body, clientX: 200, clientY: y });
  const send = (type, y) => document.body.dispatchEvent(new TouchEvent(type, { bubbles: true, cancelable: true, touches: type === 'touchend' ? [] : [touch(y)], changedTouches: [touch(y)] }));
  send('touchstart', 100);
  send('touchmove', 140);
  send('touchmove', 260);
  await wait(() => ui.exists('.h-pull'));
  send('touchend', 260);
  await wait(() => loads === 1);
  await wait(() => !ui.exists('.h-pull'));
  send('touchstart', 100);
  send('touchmove', 140);
  send('touchend', 140);
  await new Promise((done) => setTimeout(done, 0));
  expect(loads).toBe(1);
});

test('New app, Inbox, Settings and Import app… ask the app', async () => {
  const home = await show({ unread: 4 });
  expect(ui.text('.h-badge')).toBe('4');
  await ui.click('New app');
  await ui.click('[aria-label="Inbox, 4 unread"]');
  await ui.click('[aria-label="Settings"]');
  await ui.click('[aria-label="More"]');
  await ui.click('Import app…');
  expect(home.calls.map((c) => c.method)).toEqual(['newApp', 'openInbox', 'openSettings', 'importApp']);
  expect(ui.exists('.h-menu')).toBe(false);
});

test('the inbox has no badge when nothing is unread', async () => {
  await show({ unread: 0 });
  expect(ui.exists('.h-badge')).toBe(false);
  expect(ui.exists('[aria-label="Inbox"]')).toBeTruthy();
});

test('a missing key shows the banner, and Add key opens Settings', async () => {
  const home = await show({ needsKey: true });
  expect(ui.text()).toContain('Add your Claude API key to start making apps');
  await ui.click('Add key');
  expect(home.calls).toEqual([{ method: 'openSettings', args: [] }]);
  home.set({ needsKey: false });
  await wait(() => !ui.exists('.h-key'));
});

test('with no apps and no makes it says what to do', async () => {
  await show({ apps: [], makes: [] });
  expect(ui.text()).toContain('Say what you want, and Clax makes it.');
  expect(ui.exists('.h-grid')).toBe(false);
});

test('follows the app list, the makes and the unread count as they change', async () => {
  const home = await show({ unread: 0 });
  home.set({ makes: [{ id: 'm', request: 'A tide table', state: 'making', progress: { done: 1, total: 4 } }] });
  await wait(() => labels()[0] === 'A tide table');
  home.set({ makes: [], apps: [{ id: 'new', title: 'Tide table', updatedAt: '2026-10-17T00:00:00Z', build: null, icon: { monogram: 'TT', hue: 200 } }, ...SAMPLE.apps] });
  await wait(() => labels()[0] === 'Tide table');
  expect(labels().length).toBe(SAMPLE.apps.length + 1);
  home.set({ unread: 2 });
  await wait(() => ui.exists('.h-badge'));
  expect(ui.text('.h-badge')).toBe('2');
});

test('without the home capability the page says to open it in Clax, with no grid', async () => {
  mount(root(), null, { db: fakeDb() });
  await ui.waitFor(() => ui.exists('.h-message'));
  expect(ui.text()).toContain('This is a Clax home screen. Open it in the Clax app.');
  expect(ui.exists('.h-grid')).toBe(false);
  expect(ui.exists('.h-fab')).toBe(false);
});

test('a refused capability reads as unavailable; another failure offers Try again', async () => {
  const refused = { ...fakeHome(), apps: () => Promise.reject(Object.assign(new Error('refused'), { code: 'refused' })) };
  mount(root(), refused, { db: fakeDb() });
  await wait(() => ui.text().includes('This is a Clax home screen.'));
  let fail = true;
  const flaky = fakeHome();
  const apps = flaky.apps;
  flaky.apps = () => (fail ? Promise.reject(new Error('offline')) : apps());
  mount(root(), flaky, { db: fakeDb() });
  await wait(() => ui.text().includes("Couldn't load your apps"));
  fail = false;
  await ui.click('Try again');
  await wait(() => labels().includes('Run tracker'));
});

test('tells Clax once the grid is drawn, after the apps arrive', async () => {
  const home = fakeHome({ makes: [] });
  let release;
  const gate = new Promise((r) => { release = r; });
  const apps = home.apps;
  home.apps = async () => { await gate; return apps(); };
  const shown = [];
  home.shown = async () => { shown.push(tiles().length); };
  mount(root(), home, { db: fakeDb() });
  await ui.waitFor(() => document.querySelector('.h-fab'));
  expect(shown).toEqual([]);
  release();
  await wait(() => shown.length === 1);
  expect(shown[0]).toBe(SAMPLE.apps.length);
  home.set({ apps: SAMPLE.apps.slice(0, 2) });
  await wait(() => tiles().length === 2);
  expect(shown.length).toBe(1);
});

test('the shipped page calls shown() exactly once over the kit\'s fake host', async () => {
  const home = await show({ makes: [] });
  await wait(() => home.shownCount > 0);
  expect(labels().length).toBe(SAMPLE.apps.length);
  home.set({ apps: SAMPLE.apps.slice(0, 3) });
  await wait(() => tiles().length === 3);
  expect(home.shownCount).toBe(1);
});

test('tells Clax where each app tile is, again when they move', async () => {
  const home = fakeHome({ makes: [] });
  const reports = [];
  home.tiles = async (list) => { reports.push(list); };
  mount(root(), home, { db: fakeDb() });
  await wait(() => reports.length === 1);
  const first = reports[0];
  expect(first.map((t) => t.id)).toEqual(tiles().filter((t) => t.dataset.id).map((t) => t.dataset.id));
  const r = document.querySelector(`[data-id="${first[0].id}"]`).getBoundingClientRect();
  expect(first[0].rect).toEqual({ x: r.x, y: r.y, w: r.width, h: r.height });
  home.set({ apps: SAMPLE.apps.slice(0, 2) });
  await wait(() => reports.length === 2);
  expect(reports[1].length).toBe(2);
});

test('says where the tiles are only after it said it was drawn, even with no apps', async () => {
  const home = fakeHome({ makes: [], apps: [] });
  let release;
  const gate = new Promise((r) => { release = r; });
  const apps = home.apps;
  home.apps = async () => { await gate; return apps(); };
  const order = [];
  home.shown = async () => { order.push('shown'); };
  home.tiles = async (list) => { order.push(list.length); };
  mount(root(), home, { db: fakeDb() });
  await ui.waitFor(() => document.querySelector('.h-fab'));
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  expect(order).toEqual([]);
  release();
  await wait(() => order.length === 2);
  expect(order).toEqual(['shown', 0]);
});

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

test('an app Clax opened from a tap on its start-up image counts as opened and loses its New mark', async () => {
  const seen = SAMPLE.apps.map((a) => a.id).filter((id) => id !== 'app-notes');
  const home = fakeHome({ makes: [] });
  home.replayed = 'app-notes';
  db = fakeDb({ [LAYOUT]: { v: 1, starred: [], order: [], seen } });
  mount(root(), home, { db });
  await wait(() => home.shownCount === 1);
  await wait(async () => (await stored()).opens?.['app-notes']?.n === 1);
  await wait(() => ui.exists('[data-id="app-notes"]') && !ui.exists('[data-id="app-notes"][data-new]'));
  expect((await stored()).seen).toContain('app-notes');
});

test('Star from the menu moves the app to Starred; Unstar puts it first under All apps', async () => {
  const home = await show({ makes: [] });
  home.menuPick = 'star';
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => section('starred').length === 1);
  expect(section('starred')).toEqual(['app-plants']);
  expect(section('all').includes('app-plants')).toBe(false);
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

test('a Star whose write fails says so and leaves the app where it was', async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const home = fakeHome({ makes: [] });
  const base = fakeDb({ [LAYOUT]: { v: 1, starred: [], order: ids, seen: ids } });
  const failing = { doc: (path) => ({ ...base.doc(path), set: () => Promise.reject({ code: 'failed', message: 'disk full' }) }) };
  mount(root(), home, { db: failing });
  await wait(() => section('all').length === ids.length);
  home.menuPick = 'star';
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.text('.h-notice') === "Couldn't save your arrangement");
  expect(section('starred')).toEqual([]);
  expect(section('all')).toEqual(ids);
});

test('the save-failure notice drops the kept call it replaces: a tap on it opens nothing', async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const home = fakeHome({ makes: [] });
  const base = fakeDb({ [LAYOUT]: { v: 1, starred: [], order: ids, seen: ids } });
  const failing = { doc: (path) => ({ ...base.doc(path), set: () => Promise.reject({ code: 'failed', message: 'disk full' }) }) };
  let opens = 0;
  home.open = () => { opens++; return Promise.reject({ code: 'no_gesture', message: 'tap' }); };
  mount(root(), home, { db: failing });
  await wait(() => section('all').length === ids.length);
  await ui.click('[data-id="app-groceries"]');
  await wait(() => ui.text('.h-notice') === 'Tap again to open Groceries');
  const el = document.querySelector('[data-id="app-run"]');
  el.focus();
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true }));
  await wait(() => ui.text('.h-notice') === "Couldn't save your arrangement");
  await ui.click('.h-notice');
  await new Promise((r) => setTimeout(r, 0));
  expect(opens).toBe(1);
});

test('a page whose db cannot be had says the arrangement cannot be saved, and a Star changes nothing', async () => {
  const home = fakeHome({ makes: [] });
  home.menuPick = 'star';
  await start(root(), { use: async (name) => (name === 'home' ? home : Promise.reject(new Error('no db'))) });
  await wait(() => section('all').length === SAMPLE.apps.length);
  await wait(() => ui.text('.h-notice') === "Couldn't save your arrangement");
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => home.calls.length === 1);
  await new Promise((r) => setTimeout(r, 0));
  await wait(() => ui.text('.h-notice') === "Couldn't save your arrangement");
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
  expect(clock.now()).toBeGreaterThan(1999);
  expect(labels()).toEqual(SAMPLE.apps.map((a) => a.title));
});

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

test('Alt+arrows pressed again before the first move is written both count', async () => {
  await show({ makes: [] });
  await wait(async () => !!(await stored()));
  const el = document.querySelector('[data-id="app-run"]');
  el.focus();
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true }));
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true }));
  await wait(async () => (await stored()).order.indexOf('app-run') === 2);
  expect(section('all').indexOf('app-run')).toBe(2);
});

test('after an Alt+arrow move, a press elsewhere keeps a later render from focusing the tile', async () => {
  const home = await show({ makes: [], unread: 0 });
  await wait(async () => !!(await stored()));
  const el = document.querySelector('[data-id="app-run"]');
  el.focus();
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true }));
  await wait(async () => (await stored()).order.indexOf('app-run') === 1);
  document.activeElement.blur();
  document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 4, pointerType: 'mouse', clientX: 2, clientY: 600 }));
  home.set({ unread: 2 });
  await wait(() => ui.exists('.h-badge'));
  expect(document.activeElement?.dataset?.id ?? null).toBe(null);
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
  // An absence: no report may come while the tile is held.
  await new Promise((r) => setTimeout(r, 120));
  expect(reports.length).toBe(1);
  el.dispatchEvent(pointer('pointerup', { x: a.x + 120, y: a.y }));
  clock.advance(1500);
  await wait(() => reports.length === 2);
  const run = reports[1].find((t) => t.id === 'app-run');
  const r = document.querySelector('[data-id="app-run"]').getBoundingClientRect();
  expect(run.rect).toEqual({ x: r.x, y: r.y, w: r.width, h: r.height });
});
