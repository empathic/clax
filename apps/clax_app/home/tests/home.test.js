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
  expect(status('m1')).toBe('Making…');
  expect(document.querySelector('[data-make="m1"] .h-status').dataset.state).toBe('making');
  // A make breathes the working mark, never a spinner.
  expect(ui.exists('[data-make="m1"] .h-face .clax-mark.is-working')).toBeTruthy();
  expect(ui.exists('.h-spinner')).toBe(false);
  expect(ui.exists('[data-make="m2"] .h-ring .clax-mark.is-working')).toBeTruthy();
  expect(ui.text('[data-make="m2"] .h-tile-badge')).toBe('3/7');
  // Its request is one line, its state the second.
  expect(document.querySelectorAll('[data-make="m2"] .h-label .h-line').length).toBe(1);
  expect(status('m3')).toBe('Can’t reach Claude');
  expect(document.querySelector('[data-make="m3"]').getAttribute('aria-label')).toBe('A water reminder, Waiting for Claude');
  expect(status('m4')).toBe('Waiting…');
  expect(status('m5')).toBe('Interrupted');
  expect(status('m6')).toBe('Couldn’t make it');
});

test('an app whose build goes shows the 3/7 ring', async () => {
  const apps = SAMPLE.apps.map((a) => (a.id === 'app-notes' ? { ...a, build: { done: 3, total: 7 } } : a));
  await show({ makes: [], apps });
  // Its icon at full strength, ringed, with a 3/7 badge: no scrim over it.
  expect(ui.text('[data-id="app-notes"] .h-tile-badge')).toBe('3/7');
  expect(ui.exists('[data-id="app-notes"] .h-build-ring')).toBeTruthy();
  expect(ui.exists('.h-over')).toBe(false);
  expect(getComputedStyle(document.querySelector('[data-id="app-notes"] .h-face')).opacity).toBe('1');
  expect(document.querySelector('[data-id="app-notes"]').getAttribute('aria-label')).toBe('Notes, building, 3 of 7 done');
});

test('labels break only between words, two lines at most, and a word wider than the cell is one cut line', async () => {
  const long = 'Donaudampfschifffahrtsgesellschaftskapitän';
  const titles = ['Kindergarten pickup rota and swaps', 'A tide table for the harbour mouth', long, 'Notes'];
  await show({ makes: [], apps: titles.map((title, i) => ({ id: `t${i}`, title, updatedAt: SAMPLE.apps[0].updatedAt, build: null, icon: { monogram: 'T', hue: 10 } })) });
  await wait(() => document.querySelector('[data-id="t0"] .h-line'));
  titles.forEach((title, i) => {
    const label = document.querySelector(`[data-id="t${i}"] .h-label`);
    const lines = [...label.querySelectorAll('.h-line')].map((l) => l.textContent);
    expect(lines.length > 0 && lines.length <= 2).toBe(true);
    const words = new Set(title.split(' '));
    for (const line of lines) for (const w of line.split(' ')) expect(words.has(w)).toBe(true);
    expect(label.textContent).toBe(lines.join(' '));
    // Each line is drawn on one line of its own.
    for (const l of label.querySelectorAll('.h-line')) expect(l.getBoundingClientRect().height <= 20).toBe(true);
  });
  expect([...document.querySelectorAll('[data-id="t2"] .h-line')].map((l) => l.textContent)).toEqual([long]);
  expect(document.querySelector('[data-id="t2"] .h-label').dataset.cut).toBe('');
  expect([...document.querySelectorAll('[data-id="t3"] .h-line')].map((l) => l.textContent)).toEqual(['Notes']);
  expect(document.documentElement.lang).toBe('en');
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

test('says Tap again with a touch pointer and Click again with a mouse, following chrome', async () => {
  const home = await show({ makes: [], chrome: { platform: 'android', form: 'phone', toolbar: false, pointer: 'touch' } });
  home.open = () => Promise.reject({ code: 'no_gesture', message: 'tap' });
  await ui.click('[data-id="app-groceries"]');
  await wait(() => ui.text('.h-notice') === 'Tap again to open Groceries');
  home.set({ chrome: { platform: 'android', form: 'wide', toolbar: false, pointer: 'mouse' } });
  await ui.click('[data-id="app-notes"]');
  await wait(() => ui.text('.h-notice') === 'Click again to open Notes');
});

test('chrome() unavailable keeps the phone layout and says Tap', async () => {
  const home = await show({ makes: [], chrome: null });
  expect(await home.chrome().then(() => null, (e) => e.code)).toBe('unavailable');
  expect(ui.exists('.h-grid')).toBe(true);
  expect(ui.exists('.h-fab')).toBe(true);
  home.open = () => Promise.reject({ code: 'no_gesture', message: 'tap' });
  await ui.click('[data-id="app-groceries"]');
  await wait(() => ui.text('.h-notice') === 'Tap again to open Groceries');
});

test('with the window\'s toolbar the page draws no bar, search or New app; back to a phone they return', async () => {
  const toolbar = { platform: 'macos', form: 'wide', toolbar: true, pointer: 'mouse' };
  const home = await show({ makes: [], chrome: toolbar });
  await wait(() => ui.exists('.h-tile[data-id]') && !ui.exists('.h-bar'));
  expect(ui.exists('.h-fab')).toBe(false);
  expect(ui.exists('[aria-label="Search"]')).toBe(false);
  home.set({ chrome: { platform: 'android', form: 'phone', toolbar: false, pointer: 'touch' } });
  await wait(() => ui.exists('.h-bar') && ui.exists('.h-fab'));
  expect(ui.exists('[aria-label="Search"]')).toBe(true);
  // Wide without the toolbar keeps the bar and New app.
  home.set({ chrome: { platform: 'android', form: 'wide', toolbar: false, pointer: 'touch' } });
  await wait(() => ui.exists('.h-bar') && ui.exists('.h-fab'));
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
  await wait(() => ui.text('.h-notice') === 'Couldn’t make a new app');
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

test('with no apps and no makes it says what to do over the mark, with one New app', async () => {
  const home = await show({ apps: [], makes: [] });
  expect(ui.text()).toContain('Say what you need, and Clax makes it.');
  expect(ui.exists('.h-message .clax-mark')).toBe(true);
  expect(ui.exists('.h-grid')).toBe(false);
  expect(ui.exists('.h-fab')).toBe(false);
  await ui.click('New app');
  expect(home.calls).toEqual([{ method: 'newApp', args: [] }]);
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
  await wait(() => ui.text().includes('Couldn’t show your apps'));
  expect(ui.text()).toContain('Clax’s service didn’t answer.');
  expect(ui.exists('.h-fab')).toBe(false);
  expect(ui.text().includes('New app')).toBe(false);
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
  await wait(() => ui.text('.h-notice') === 'Couldn’t save your arrangement');
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
  await wait(() => ui.text('.h-notice') === 'Couldn’t save your arrangement');
  await ui.click('.h-notice');
  await new Promise((r) => setTimeout(r, 0));
  expect(opens).toBe(1);
});

test('a page whose db cannot be had says the arrangement cannot be saved, and a Star changes nothing', async () => {
  const home = fakeHome({ makes: [] });
  home.menuPick = 'star';
  await start(root(), { use: async (name) => (name === 'home' ? home : Promise.reject(new Error('no db'))) });
  await wait(() => section('all').length === SAMPLE.apps.length);
  await wait(() => ui.text('.h-notice') === 'Couldn’t save your arrangement');
  document.querySelector('[data-id="app-plants"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => home.calls.length === 1);
  await new Promise((r) => setTimeout(r, 0));
  await wait(() => ui.text('.h-notice') === 'Couldn’t save your arrangement');
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
  await wait(() => ui.exists('.h-group[data-group="g1"]'));
  expect(section('all')).toEqual(['app-run', 'app-habits', 'app-recipes', 'app-budget', 'app-mood']);
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
const faceCentre = (id) => { const r = document.querySelector(`[data-id="${id}"] .h-face`).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; };
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

test('a second drag before the first drop is written starts from the arrangement shown', async () => {
  const clock = manualClock();
  const ids = SAMPLE.apps.map((a) => a.id);
  const base = fakeDb({ [LAYOUT]: { v: 1, starred: [], order: ids, seen: ids } });
  let release;
  const gate = new Promise((r) => { release = r; });
  db = { doc: (path) => { const d = base.doc(path); return { ...d, set: async (v) => { await gate; return d.set(v); } }; } };
  mount(root(), fakeHome({ makes: [] }), { db, clock });
  await wait(() => section('all').length === ids.length);
  await drag(clock, 'app-run', centre('app-budget'));
  const run = document.querySelector('[data-id="app-run"]');
  await wait(() => run.style.gridRow === '2 / span 1' && run.style.gridColumn === '1 / span 1');
  const el = document.querySelector('[data-id="app-notes"]');
  const a = centre('app-notes');
  el.dispatchEvent(pointer('pointerdown', a));
  clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 12, y: a.y }));
  expect([run.style.gridRow, run.style.gridColumn]).toEqual(['2 / span 1', '1 / span 1']);
  el.dispatchEvent(pointer('pointerup', { x: a.x + 12, y: a.y }));
  clock.advance(1500);
  release();
  await wait(async () => (await base.doc(LAYOUT).get()).data().order.indexOf('app-run') === 4);
  expect(section('all').indexOf('app-run')).toBe(4);
});

test('a drop into Starred dragged again before it is written lands in Starred', async () => {
  const clock = manualClock();
  const ids = SAMPLE.apps.map((a) => a.id);
  const base = fakeDb({ [LAYOUT]: { v: 1, starred: ['app-mood'], order: ids.filter((id) => id !== 'app-mood'), seen: ids } });
  let release;
  const gate = new Promise((r) => { release = r; });
  db = { doc: (path) => { const d = base.doc(path); return { ...d, set: async (v) => { await gate; return d.set(v); } }; } };
  mount(root(), fakeHome({ makes: [] }), { db, clock });
  await wait(() => section('starred').length === 1);
  await drag(clock, 'app-run', centre('app-mood'));
  await wait(() => section('starred').join() === 'app-run,app-mood');
  await drag(clock, 'app-run', centre('app-mood'));
  expect(document.querySelector('[data-id="app-run"]').style.transform).toBe('');
  release();
  await wait(async () => (await base.doc(LAYOUT).get()).data().starred.join() === 'app-mood,app-run');
  await wait(() => section('starred').join() === 'app-mood,app-run');
});

test('a drag whose release never comes ends when the page loses focus', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock });
  const el = document.querySelector('[data-id="app-run"]');
  const a = centre('app-run');
  el.dispatchEvent(pointer('pointerdown', a));
  clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 120, y: a.y }));
  await wait(() => ui.exists('.h-page[data-dragging]'));
  dispatchEvent(new Event('blur'));
  await wait(() => !ui.exists('.h-page[data-dragging]'));
  expect(ui.exists('.h-tile[data-held], .h-tile[data-dragging]')).toBe(false);
  expect(el.style.transform).toBe('');
  home.set({ apps: SAMPLE.apps.slice(0, 2) });
  await wait(() => tiles().length === 2);
  await ui.click('[data-id="app-groceries"]');
  await wait(() => home.calls.some((c) => c.method === 'open'));
});

test('a held press ends when the page is hidden or the pointer is lost', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock });
  const el = document.querySelector('[data-id="app-run"]');
  el.dispatchEvent(pointer('pointerdown', centre('app-run')));
  clock.advance(400);
  expect('held' in el.dataset).toBe(true);
  Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
  try { document.dispatchEvent(new Event('visibilitychange')); } finally { delete document.visibilityState; }
  expect('held' in el.dataset).toBe(false);
  el.dispatchEvent(pointer('pointerdown', centre('app-run'), 'mouse'));
  el.dispatchEvent(pointer('pointermove', { x: centre('app-run').x + 40, y: centre('app-run').y }, 'mouse'));
  await wait(() => ui.exists('.h-page[data-dragging]'));
  el.dispatchEvent(new PointerEvent('lostpointercapture', { bubbles: true, pointerId: 9, pointerType: 'mouse' }));
  await wait(() => !ui.exists('.h-page[data-dragging]'));
  el.dispatchEvent(pointer('pointerup', centre('app-run'), 'mouse'));
  expect(home.calls).toEqual([]);
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

/** Run tracker, then a group of Groceries and Notes ("Lists"), then the
 * rest; `look` (a field earlier releases wrote) when given. */
const groupLayout = (look) => {
  const ids = SAMPLE.apps.map((a) => a.id);
  return { v: 1, starred: [], order: ['app-run', 'g:g1', ...ids.filter((id) => !['app-run', 'app-groceries', 'app-notes'].includes(id))], groups: { g1: { name: 'Lists', ...(look ? { look } : {}), apps: ['app-groceries', 'app-notes'] } }, seen: ids };
};
const nameField = () => document.querySelector('.h-name input[aria-label="Group name"]');

test('holding a tile 600 ms over another app grows it onto a folder backing; a drop makes a group named Group, its name offered at once', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  const el = document.querySelector('[data-id="app-mood"]');
  const a = centre('app-mood');
  const t = centre('app-habits');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  // Off its own centre, near no other tile's, then onto the target's centre at once.
  el.dispatchEvent(pointer('pointermove', { x: a.x + 30, y: a.y + 10 })); clock.advance(100);
  el.dispatchEvent(pointer('pointermove', t));
  clock.advance(599);
  expect(ui.exists('[data-grouping]')).toBe(false);
  clock.advance(1);
  await wait(() => ui.exists('[data-id="app-habits"][data-grouping]'));
  expect(ui.exists('[data-id="app-habits"][data-grouping] .h-icon > .h-folder-back')).toBe(true);
  el.dispatchEvent(pointer('pointerup', t)); clock.advance(1500);
  await wait(() => ui.exists('.h-group'));
  const l = await stored();
  const [gid] = Object.keys(l.groups);
  expect(l.groups[gid]).toEqual({ name: 'Group', apps: ['app-habits', 'app-mood'] });
  expect(ui.text('.h-group .h-label')).toBe('Group');
  expect(ui.exists('[data-grouping]')).toBe(false);
  // The suggested name, all of it selected, ready to type over.
  await wait(() => nameField() && document.activeElement === nameField());
  const input = nameField();
  expect(input.value).toBe('Group');
  expect([input.selectionStart, input.selectionEnd]).toEqual([0, 5]);
  expect(history.state?.clax).toBe('name');
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  await wait(() => !ui.exists('.h-name') && history.state?.clax !== 'name');
  expect((await stored()).groups[gid].name).toBe('Group');
});

test('a group made of apps whose icon words share a category is offered its name; Enter keeps an edit', async () => {
  const clock = manualClock();
  const at = SAMPLE.apps[0].updatedAt;
  const apps = [
    { id: 'app-run', title: 'Run tracker', updatedAt: at, build: null, icon: { word: 'run' } },
    { id: 'app-gym', title: 'Gym log', updatedAt: at, build: null, icon: { word: 'workout' } },
    { id: 'app-budget', title: 'Weekly budget', updatedAt: at, build: null, icon: { word: 'money' } },
  ];
  await show({ makes: [], apps }, { clock, layout: { v: 1, starred: [], order: ['app-run', 'app-budget', 'app-gym'], seen: apps.map((a) => a.id) } });
  await drag(clock, 'app-gym', faceCentre('app-run'), { pause: 700 });
  await wait(() => nameField() && document.activeElement === nameField());
  expect(nameField().value).toBe('Fitness');
  const [gid] = Object.keys((await stored()).groups);
  expect((await stored()).groups[gid]).toEqual({ name: 'Fitness', apps: ['app-run', 'app-gym'] });
  nameField().value = 'Training';
  nameField().dispatchEvent(new Event('input', { bubbles: true }));
  nameField().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  await wait(() => !ui.exists('.h-name'));
  await wait(() => ui.text('.h-group .h-label') === 'Training');
  await wait(async () => (await stored()).groups[gid].name === 'Training');
});

test("a dropped app flies from where it was let go into its place in the group's preview, which shows it once it lands", async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  const g = document.querySelector('.h-group .h-face').getBoundingClientRect();
  // What shows as the flight starts: its icon over the page, its place in the preview empty.
  const seen = [];
  const watch = new MutationObserver(() => {
    const ghost = document.querySelector('.h-gather');
    if (ghost && !seen.length) seen.push([!!ghost.querySelector('.h-face'), document.querySelector('.h-group [data-app="app-plants"]')?.style.visibility]);
  });
  watch.observe(document.body, { childList: true });
  await drag(clock, 'app-plants', { x: g.x + g.width / 2, y: g.y + g.height / 2 }, { pause: 700 });
  await wait(() => seen.length > 0);
  watch.disconnect();
  expect(seen[0]).toEqual([true, 'hidden']);
  const slot = () => document.querySelector('.h-group [data-app="app-plants"]');
  await wait(() => !ui.exists('.h-gather') && slot()?.style.visibility === '');
  expect([...document.querySelectorAll('.h-group .h-folder-face > .h-face')].map((f) => f.dataset.app)).toEqual(['app-groceries', 'app-notes', 'app-plants']);
});

test('a release on an app before 600 ms is an ordinary drop into its slot', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  await drag(clock, 'app-run', centre('app-budget'));
  await wait(async () => (await stored()).order.indexOf('app-run') === 4);
  expect(Object.keys((await stored()).groups)).toEqual([]);
});

test('an app held over a starred app makes the group first in All apps, scrolled into view', async () => {
  const clock = manualClock();
  const at = SAMPLE.apps[7].updatedAt;
  const extra = Array.from({ length: 40 }, (_, i) => ({ id: `app-x${i}`, title: `Extra ${i}`, updatedAt: at, build: null, icon: { monogram: 'X', hue: 9 * i } }));
  const apps = [...SAMPLE.apps, ...extra];
  const ids = apps.map((a) => a.id);
  const starred = ['app-mood', 'app-notes', ...extra.map((a) => a.id)];
  await show({ makes: [], apps }, { clock, layout: { v: 1, starred, order: ids.filter((id) => !starred.includes(id)), seen: ids } });
  await wait(() => section('starred').length === starred.length);
  scrollTo(0, 0);
  await drag(clock, 'app-notes', centre('app-mood'), { pause: 700 });
  await wait(async () => Object.keys((await stored()).groups ?? {}).length === 1);
  await wait(() => ui.exists('.h-group'));
  const l = await stored();
  const [gid] = Object.keys(l.groups);
  expect(l.groups[gid].apps).toEqual(['app-mood', 'app-notes']);
  expect(l.starred.includes('app-mood') || l.starred.includes('app-notes')).toBe(false);
  expect(l.order[0]).toBe(`g:${gid}`);
  const r = document.querySelector('.h-group').getBoundingClientRect();
  expect(r.top >= 0 && r.bottom <= innerHeight).toBe(true);
  await wait(() => nameField());
  nameField().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  await wait(() => !ui.exists('.h-name'));
  scrollTo(0, 0);
});

test("resting on an app's centre makes room there, and a drop before 600 ms takes its place", async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  const habits = document.querySelector('[data-id="app-habits"]');
  const was = [habits.style.gridRow, habits.style.gridColumn];
  const el = document.querySelector('[data-id="app-mood"]');
  const a = centre('app-mood');
  const t = centre('app-habits');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 30, y: a.y + 10 })); clock.advance(100);
  el.dispatchEvent(pointer('pointermove', t));
  clock.advance(200);
  expect([habits.style.gridRow, habits.style.gridColumn].join() === was.join()).toBe(false);
  expect(ui.exists('[data-grouping]')).toBe(false);
  el.dispatchEvent(pointer('pointerup', t)); clock.advance(1500);
  await wait(async () => (await stored()).order.indexOf('app-mood') === 2);
  expect(Object.keys((await stored()).groups)).toEqual([]);
});

test("a pointer that keeps moving about an app's centre never groups", async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock });
  await wait(async () => !!(await stored()));
  const el = document.querySelector('[data-id="app-mood"]');
  const a = centre('app-mood');
  const t = centre('app-habits');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 30, y: a.y + 10 })); clock.advance(100);
  // Within the centre's 24 px, 12 px back and forth every 150 ms.
  for (let i = 0; i < 8; i++) {
    el.dispatchEvent(pointer('pointermove', { x: t.x + (i % 2 ? 6 : -6), y: t.y }));
    clock.advance(150);
  }
  expect(ui.exists('[data-grouping]')).toBe(false);
  el.dispatchEvent(pointer('pointerup', { x: t.x + 6, y: t.y })); clock.advance(1500);
  await wait(async () => (await stored()).order.indexOf('app-mood') === 2);
  expect(Object.keys((await stored()).groups)).toEqual([]);
});

test('a group is outlined only after a 600 ms rest on its centre; a drop before that takes its slot', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  const g = document.querySelector('.h-group').getBoundingClientRect();
  const t = { x: g.x + g.width / 2, y: g.y + g.height / 2 };
  const el = document.querySelector('[data-id="app-plants"]');
  const a = centre('app-plants');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 30, y: a.y + 10 })); clock.advance(100);
  el.dispatchEvent(pointer('pointermove', t));
  clock.advance(300);
  expect(ui.exists('[data-grouping]')).toBe(false);
  el.dispatchEvent(pointer('pointerup', t)); clock.advance(1500);
  await wait(async () => (await stored()).order.indexOf('app-plants') === 1);
  expect((await stored()).groups.g1.apps).toEqual(['app-groceries', 'app-notes']);
});

test('a toss or a fast move over a group never adds to it, even after a rest there', async () => {
  const clock = manualClock();
  const base = fakeDb({ [LAYOUT]: groupLayout() });
  let writes = 0;
  db = { doc: (path) => { const d = base.doc(path); return { ...d, set: (v) => { writes++; return d.set(v); } }; } };
  mount(root(), fakeHome({ makes: [] }), { db, clock });
  await wait(() => ui.exists('.h-group'));
  const groupCentre = () => { const g = document.querySelector('.h-group').getBoundingClientRect(); return { x: g.x + g.width / 2, y: g.y + g.height / 2 }; };
  // Thrown across the group's centre, released at speed on it.
  let n = writes;
  await drag(clock, 'app-plants', groupCentre(), { steps: 3, ms: 60, pause: 0 });
  await wait(() => writes > n);
  expect((await stored()).groups.g1.apps).toEqual(['app-groceries', 'app-notes']);
  expect((await stored()).order).toContain('app-plants');
  // Rested on it until outlined, then flicked 16 px and released at speed.
  const t = groupCentre();
  const el = document.querySelector('[data-id="app-plants"]');
  const a = centre('app-plants');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 30, y: a.y + 10 })); clock.advance(100);
  el.dispatchEvent(pointer('pointermove', t));
  clock.advance(700);
  await wait(() => ui.exists('.h-group[data-grouping]'));
  clock.advance(8);
  el.dispatchEvent(pointer('pointermove', { x: t.x + 8, y: t.y }));
  clock.advance(8);
  n = writes;
  el.dispatchEvent(pointer('pointerup', { x: t.x + 16, y: t.y })); clock.advance(1500);
  await wait(() => writes > n);
  expect((await stored()).groups.g1.apps).toEqual(['app-groceries', 'app-notes']);
  expect((await stored()).order).toContain('app-plants');
});

test('dropping an app on a group after a 600 ms rest adds it', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  const g = document.querySelector('.h-group').getBoundingClientRect();
  await drag(clock, 'app-plants', { x: g.x + g.width / 2, y: g.y + g.height / 2 }, { pause: 700 });
  await wait(async () => (await stored()).groups.g1.apps.includes('app-plants'));
  expect(ui.exists('[data-id="app-plants"]')).toBe(false);
});

test('a group drags like an app, never into another group', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  await wait(() => ui.exists('.h-group'));
  const el = document.querySelector('.h-group');
  const r = el.getBoundingClientRect();
  const a = { x: r.x + r.width / 2, y: r.y + r.height / 2 };
  const to = centre('app-budget');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  for (let i = 1; i <= 6; i++) { clock.advance(30); el.dispatchEvent(pointer('pointermove', { x: a.x + (to.x - a.x) * i / 6, y: a.y + (to.y - a.y) * i / 6 })); }
  clock.advance(700);
  expect(ui.exists('[data-grouping]')).toBe(false);
  el.dispatchEvent(pointer('pointerup', to)); clock.advance(1500);
  await wait(async () => (await stored()).order.indexOf('g:g1') === 4);
  expect((await stored()).groups.g1.apps).toEqual(['app-groceries', 'app-notes']);
});

test("the group's own menu renames and ungroups", async () => {
  await show({ makes: [] }, { layout: groupLayout() });
  document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.exists('.h-group-menu'));
  expect([...document.querySelectorAll('.h-group-menu [role="menuitem"]')].map((b) => b.textContent)).toEqual(['Rename', 'Ungroup']);
  await ui.click('Rename');
  await wait(() => nameField() && document.activeElement === nameField());
  expect(ui.exists('.h-group-menu')).toBe(false);
  expect(nameField().value).toBe('Lists');
  nameField().value = 'Shopping';
  nameField().dispatchEvent(new Event('input', { bubbles: true }));
  nameField().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  await wait(async () => (await stored()).groups.g1.name === 'Shopping');
  await wait(() => ui.text('.h-group .h-label') === 'Shopping' && !ui.exists('.h-name'));
  document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.exists('.h-group-menu'));
  await ui.click('Ungroup');
  await wait(async () => Object.keys((await stored()).groups).length === 0);
  expect(section('all').slice(0, 3)).toEqual(['app-run', 'app-groceries', 'app-notes']);
});

test("Escape and back leave a group's name as it was; a tap away keeps what the field says", async () => {
  const rename = async () => {
    document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
    await wait(() => ui.exists('.h-group-menu'));
    await ui.click('Rename');
    await wait(() => nameField() && document.activeElement === nameField());
    nameField().value = 'Nope';
    nameField().dispatchEvent(new Event('input', { bubbles: true }));
  };
  await show({ makes: [] }, { layout: groupLayout() });
  await rename();
  nameField().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  await wait(() => !ui.exists('.h-name') && history.state?.clax !== 'name');
  await rename();
  history.back();
  await wait(() => !ui.exists('.h-name'));
  expect((await stored()).groups.g1.name).toBe('Lists');
  expect(ui.text('.h-group .h-label')).toBe('Lists');
  await rename();
  nameField().value = 'Errands';
  nameField().dispatchEvent(new Event('input', { bubbles: true }));
  await ui.click('.h-name-scrim');
  await wait(async () => (await stored()).groups.g1.name === 'Errands');
  await wait(() => !ui.exists('.h-name') && history.state?.clax !== 'name');
});

test("back closes the group's own menu, and only it; Escape leaves its history entry too", async () => {
  const menu = () => document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await show({ makes: [] }, { layout: groupLayout() });
  menu();
  await wait(() => ui.exists('.h-group-menu'));
  expect(history.state?.clax).toBe('menu');
  history.back();
  await wait(() => !ui.exists('.h-group-menu'));
  expect(ui.exists('.h-group')).toBe(true);
  menu();
  await wait(() => ui.exists('.h-group-menu'));
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await wait(() => !ui.exists('.h-group-menu') && history.state?.clax !== 'menu');
  // Over a fanned group, back closes the menu and leaves the group fanned.
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  menu();
  await wait(() => ui.exists('.h-group-menu'));
  history.back();
  await wait(() => !ui.exists('.h-group-menu'));
  clock.advance(1000);
  expect(ui.exists('.h-fan')).toBe(true);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan') && history.state?.clax !== 'group');
});

test('an app in a fanned group offers Remove from group in its menu', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock, layout: groupLayout() });
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  home.menuPick = 'remove';
  document.querySelector('.h-fan [data-id="app-notes"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(async () => !(await stored()).groups.g1);
  expect(home.calls.at(-1).args[2]).toEqual({ items: [{ id: 'star', label: 'Star' }, { id: 'remove', label: 'Remove from group' }] });
  // Left with one app, the group gives way to it, and its row goes.
  await wait(() => !ui.exists('.h-fan') && !ui.exists('.h-group'));
});

test("a group's tile shows its first four apps' icons, two by two; fewer apps fill fewer cells", async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const six = ids.filter((id) => !['app-mood', 'app-notes'].includes(id));
  const layout = { v: 1, starred: [], order: ['g:p1', 'app-mood', 'app-notes'], groups: { p1: { name: 'Lots', apps: six } }, seen: ids };
  await show({ makes: [] }, { layout });
  const preview = (gid) => [...document.querySelectorAll(`.h-group[data-group="${gid}"] .h-folder-face > .h-face`)].map((f) => f.dataset.app);
  expect(preview('p1')).toEqual(six.slice(0, 4));
  expect(ui.text('.h-group .h-label')).toBe('Lots');
  expect(document.querySelector('.h-group').getAttribute('aria-label')).toBe('Lots, group of 6 apps');
  await show({ makes: [] }, { layout: groupLayout() });
  expect(preview('g1')).toEqual(['app-groceries', 'app-notes']);
});

test('groups an earlier release stored as a folder or a pile load as one look, their look kept', async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const layout = {
    v: 1, starred: [], order: ['g:f', 'g:p', 'app-mood', 'app-notes'], seen: ids,
    groups: { f: { name: 'Folder', look: 'folder', apps: ['app-run', 'app-groceries', 'app-habits'] }, p: { name: 'Pile', look: 'pile', apps: ['app-recipes', 'app-budget', 'app-plants'] } },
  };
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout });
  expect([...document.querySelectorAll('.h-group')].map((g) => [g.dataset.group, g.querySelectorAll('.h-folder-face > .h-face').length, g.hasAttribute('data-look')])).toEqual([['f', 3, false], ['p', 3, false]]);
  expect(ui.exists('.h-pile-face, .h-pile-card')).toBe(false);
  for (const gid of ['f', 'p']) {
    await ui.click(`.h-group[data-group="${gid}"]`);
    clock.advance(1000);
    await wait(() => ui.exists(`.h-fan[data-group="${gid}"]`));
    // Escape gathers it once the page listens for it, a frame or so after the row shows.
    await wait(() => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      clock.advance(1000);
      return !ui.exists('.h-fan');
    });
  }
  // A change keeps each look as it was stored.
  document.querySelector('.h-group[data-group="p"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.exists('.h-group-menu'));
  await ui.click('Rename');
  await wait(() => nameField());
  nameField().value = 'Kept';
  nameField().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  await wait(async () => (await stored()).groups.p.name === 'Kept');
  const l = await stored();
  expect(l.groups.f.look).toBe('folder');
  expect(l.groups.p.look).toBe('pile');
});

test('a tap fans a group out in a row below it, pushing the grid down; a tap outside gathers it', async () => {
  const clock = manualClock();
  const home = await show({ makes: [] }, { clock, layout: groupLayout() });
  const below = document.querySelector('[data-id="app-mood"]').getBoundingClientRect().y;
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan[data-group="g1"]'));
  expect(document.querySelector('.h-group').getAttribute('aria-expanded')).toBe('true');
  expect([...document.querySelectorAll('.h-fan .h-tile[data-id]')].map((t) => t.dataset.id)).toEqual(['app-groceries', 'app-notes']);
  expect(document.querySelector('[data-id="app-mood"]').getBoundingClientRect().y).toBeGreaterThan(below);
  await ui.click('.h-fan [data-id="app-notes"]');
  await wait(() => home.calls.some((c) => c.method === 'open' && c.args[0] === 'app-notes'));
  document.querySelector('.h-key, .h-bar').click();
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan'));
  clock.advance(1000);
  await wait(() => document.querySelector('[data-id="app-mood"]').getBoundingClientRect().y === below);
});

const groupOfThree = () => ({
  ...groupLayout(),
  order: ['app-run', 'g:g1', 'app-habits', 'app-recipes', 'app-budget', 'app-mood'],
  groups: { g1: { name: 'Lists', apps: ['app-groceries', 'app-notes', 'app-plants'] } },
});
const fanned = () => [...document.querySelectorAll('.h-fan .h-tile[data-id]')].map((t) => t.dataset.id).join();

test("an app dragged along a fanned group's row reorders the group, which stays fanned", async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupOfThree() });
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  await drag(clock, 'app-plants', centre('app-groceries'));
  await wait(async () => (await stored()).groups.g1.apps.join() === 'app-plants,app-groceries,app-notes');
  await wait(() => fanned() === 'app-plants,app-groceries,app-notes');
  expect(ui.exists('.h-tile[data-held], .h-tile[data-dragging]')).toBe(false);
  expect(document.querySelector('.h-fan [data-id="app-plants"]').style.position).toBe('');
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan'));
});

test("an app dragged out of a fanned group's row into the grid leaves the group there", async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupOfThree() });
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  await drag(clock, 'app-notes', centre('app-mood'));
  await wait(async () => (await stored()).groups.g1.apps.join() === 'app-groceries,app-plants');
  expect((await stored()).order.indexOf('app-notes')).toBe(5);
  await wait(() => section('all').includes('app-notes') && fanned() === 'app-groceries,app-plants');
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan'));
});

test("an app dragged out of a fanned group whose write fails shows in the row again", async () => {
  const clock = manualClock();
  const base = fakeDb({ [LAYOUT]: groupOfThree() });
  const failing = { doc: (path) => ({ ...base.doc(path), set: () => Promise.reject({ code: 'failed', message: 'disk full' }) }) };
  mount(root(), fakeHome({ makes: [] }), { db: failing, clock });
  await wait(() => ui.exists('.h-group'));
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  clock.advance(1000);
  const before = document.querySelector('.h-fan [data-id="app-notes"]');
  await drag(clock, 'app-notes', centre('app-mood'));
  await wait(() => ui.text('.h-notice') === 'Couldn’t save your arrangement');
  await wait(() => fanned() === 'app-groceries,app-notes,app-plants' && !ui.exists('.h-grid > [data-id="app-notes"]'));
  // The drop's render took the hidden tile out of the row; the row draws it afresh.
  const after = document.querySelector('.h-fan [data-id="app-notes"]');
  expect(after === before).toBe(false);
  expect(after.style.visibility).toBe('');
  await wait(() => {
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    clock.advance(1000);
    return !ui.exists('.h-fan');
  });
});

test("a drag while a group is fanned keeps it fanned and drops where the tiles show, past the fan's row", async () => {
  const clock = manualClock();
  const at = SAMPLE.apps[7].updatedAt;
  const extra = Array.from({ length: 10 }, (_, i) => ({ id: `app-a${i + 1}`, title: `Extra ${i + 1}`, updatedAt: at, build: null, icon: { monogram: 'A', hue: 30 * i } }));
  const apps = [SAMPLE.apps[1], SAMPLE.apps[7], ...extra];
  const ids = apps.map((a) => a.id);
  const layout = { v: 1, starred: [], order: ['g:g1', ...extra.map((a) => a.id)], groups: { g1: { name: 'Lists', apps: ['app-groceries', 'app-notes'] } }, seen: ids };
  await show({ makes: [], apps }, { clock, layout });
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  clock.advance(1000);
  const to = centre('app-a5');
  const el = document.querySelector('[data-id="app-a10"]');
  const a = centre('app-a10');
  el.dispatchEvent(pointer('pointerdown', a)); clock.advance(400);
  el.dispatchEvent(pointer('pointermove', { x: a.x + 20, y: a.y })); clock.advance(30);
  expect(ui.exists('.h-fan')).toBe(true);
  el.dispatchEvent(pointer('pointermove', to)); clock.advance(200);
  el.dispatchEvent(pointer('pointerup', to)); clock.advance(1500);
  await wait(async () => (await stored()).order.indexOf('app-a10') === 5);
  expect(ui.exists('.h-fan')).toBe(true);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan'));
});

test('Alt+arrows move a focused app along a fanned group', async () => {
  const clock = manualClock();
  await show({ makes: [] }, { clock, layout: groupLayout() });
  await ui.click('.h-group');
  clock.advance(1000);
  await wait(() => ui.exists('.h-fan'));
  const el = document.querySelector('.h-fan [data-id="app-groceries"]');
  el.focus();
  el.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true }));
  await wait(async () => (await stored()).groups.g1.apps.join() === 'app-notes,app-groceries');
  clock.advance(1000);
  await wait(() => [...document.querySelectorAll('.h-fan .h-tile[data-id]')].map((t) => t.dataset.id).join() === 'app-notes,app-groceries');
  expect(document.activeElement.dataset.id).toBe('app-groceries');
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  clock.advance(1000);
  await wait(() => !ui.exists('.h-fan'));
});

const searchField = async () => {
  await ui.click('[aria-label="Search"]');
  await wait(() => ui.exists('.h-search input'));
  return document.querySelector('.h-search input');
};
const typeIn = (input, value) => {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
};
const press = (input, key) => input.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));

test('the search icon opens a focused field; typing filters every app, groups too; Enter opens the first', async () => {
  const clock = manualClock(0, Date.UTC(2026, 9, 16, 12));
  const home = await show({ makes: [] }, { clock, layout: groupLayout() });
  const input = await searchField();
  expect(document.activeElement).toBe(input);
  expect(input.getAttribute('aria-label')).toBe('Search apps');
  expect(ui.exists('.h-section[data-section="all"]')).toBeTruthy();
  typeIn(input, 'NOT');
  await wait(() => ui.exists('.h-results'));
  expect(ui.exists('.h-section')).toBe(false);
  expect([...document.querySelectorAll('.h-results .h-tile[data-id]')].map((t) => t.dataset.id)).toEqual(['app-notes']);
  expect(ui.text('.h-results [data-id="app-notes"] .h-in-group')).toBe('Lists');
  press(input, 'Enter');
  await wait(() => home.calls.some((c) => c.method === 'open'));
  expect(home.calls.find((c) => c.method === 'open').args).toEqual(['app-notes']);
  await wait(async () => (await stored()).opens['app-notes']?.n === 1);
  press(input, 'Escape');
  await wait(() => !ui.exists('.h-search'));
});

test('with the window\'s toolbar, its search text (chrome\'s query) filters the grid as the page\'s own search does', async () => {
  const toolbar = { platform: 'macos', form: 'wide', toolbar: true, pointer: 'mouse', query: '' };
  const home = await show({ makes: [], chrome: toolbar }, { layout: groupLayout() });
  await wait(() => ui.exists('.h-section[data-section="all"]') && !ui.exists('.h-bar'));
  home.set({ chrome: { ...toolbar, query: 'NOT' } });
  await wait(() => ui.exists('.h-results'));
  expect(ui.exists('.h-section')).toBe(false);
  // The page draws no field of its own over the toolbar's.
  expect(ui.exists('.h-search')).toBe(false);
  expect([...document.querySelectorAll('.h-results .h-tile[data-id]')].map((t) => t.dataset.id)).toEqual(['app-notes']);
  expect(ui.text('.h-results [data-id="app-notes"] .h-in-group')).toBe('Lists');
  home.set({ chrome: { ...toolbar, query: 'zzz' } });
  await wait(() => ui.text('.h-results-empty') === 'No apps match “zzz”');
  // Spaces alone, or an empty field, show the grid again.
  home.set({ chrome: { ...toolbar, query: '  ' } });
  await wait(() => !ui.exists('.h-results') && ui.exists('.h-section[data-section="all"]'));
  // An older Clax sends no query: the grid stays.
  home.set({ chrome: { platform: 'macos', form: 'wide', toolbar: true, pointer: 'mouse' } });
  await wait(() => ui.exists('.h-section[data-section="all"]'));
  // Without the toolbar its query is not the page's.
  home.set({ chrome: { platform: 'android', form: 'phone', toolbar: false, pointer: 'touch', query: 'NOT' } });
  await wait(() => ui.exists('.h-bar'));
  expect(ui.exists('.h-results')).toBe(false);
});

test('no match says so; Escape closes search; Backspace in an empty field closes it, as does the close button', async () => {
  await show({ makes: [] });
  let input = await searchField();
  typeIn(input, 'zzz');
  await wait(() => ui.exists('.h-results') && ui.text('.h-results').includes('No apps match “zzz”'));
  press(input, 'Escape');
  await wait(() => !ui.exists('.h-search'));
  expect(ui.exists('.h-section[data-section="all"]')).toBeTruthy();
  input = await searchField();
  typeIn(input, 'r');
  press(input, 'Backspace');
  await new Promise((done) => setTimeout(done, 0));
  expect(ui.exists('.h-search')).toBe(true);
  typeIn(input, '');
  press(input, 'Backspace');
  await wait(() => !ui.exists('.h-search'));
  await searchField();
  await ui.click('[aria-label="Close search"]');
  await wait(() => !ui.exists('.h-search'));
});

test('a pull down from the top does not open search; only the ⌕ does', async () => {
  if (typeof Touch !== 'function') return;
  await show({ makes: [] });
  scrollTo(0, 0);
  const touch = (y) => new Touch({ identifier: 1, target: document.body, clientX: 200, clientY: y });
  const send = (type, y) => {
    const e = new TouchEvent(type, { bubbles: true, cancelable: true, touches: type === 'touchend' ? [] : [touch(y)], changedTouches: [touch(y)] });
    document.body.dispatchEvent(e);
    return e.defaultPrevented;
  };
  send('touchstart', 100);
  expect([send('touchmove', 160), send('touchmove', 400)]).toEqual([false, false]);
  send('touchend', 400);
  await new Promise((done) => setTimeout(done, 0));
  expect(ui.exists('.h-search')).toBe(false);
  expect(ui.exists('.h-search-ghost')).toBe(false);
  await searchField();
  expect(ui.exists('.h-search input')).toBe(true);
});

// Releases before this one wrote `bigTiles` and `big` and drew the apps
// opened most on 2×2 tiles; such a layout loads with every tile one cell.
test('a layout with bigTiles: true and big loads with every tile in one cell, and keeps both fields through a write', async () => {
  const ids = SAMPLE.apps.map((a) => a.id);
  const o = (n) => ({ n, last: '2026-10-16T10:00:00.000Z', days: { '2026-10-16': n } });
  const old = { v: 1, starred: [], order: ids, seen: ids, opens: { 'app-budget': o(9), 'app-mood': o(6), 'app-plants': o(5) }, bigTiles: true, big: ['app-budget', 'app-mood', 'app-plants'] };
  await show({ makes: [] }, { clock: manualClock(0, Date.UTC(2026, 9, 16, 12)), layout: old });
  await wait(() => section('all').length === ids.length);
  expect(section('all')).toEqual(ids);
  for (const t of document.querySelectorAll('.h-grid > .h-tile[data-id]')) {
    expect(t.style.gridColumn.endsWith('span 1')).toBe(true);
    expect(t.querySelector('.h-face').getBoundingClientRect().width).toBe(60);
  }
  await ui.click('[data-id="app-notes"]');
  await wait(async () => (await stored()).opens['app-notes']?.n === 1);
  const l = await stored();
  expect(l.bigTiles).toBe(true);
  expect(l.big).toEqual(['app-budget', 'app-mood', 'app-plants']);
});

test("the home screen takes the brand's colours", async () => {
  await show({ makes: [] });
  const html = document.documentElement;
  const theme = html.getAttribute('data-theme');
  html.setAttribute('data-theme', 'light');
  try {
    const probe = document.createElement('span');
    probe.style.color = 'var(--h-primary)';
    root().appendChild(probe);
    expect(getComputedStyle(probe).color).toBe('rgb(47, 111, 42)');
    probe.remove();
    expect(getComputedStyle(html).accentColor).toBe('rgb(47, 111, 42)');
  } finally {
    if (theme === null) html.removeAttribute('data-theme');
    else html.setAttribute('data-theme', theme);
  }
});

/** Runs `fn` with the page in light colours, whatever the system's. */
async function inLight(fn) {
  const html = document.documentElement;
  const theme = html.getAttribute('data-theme');
  html.setAttribute('data-theme', 'light');
  try { await fn(); } finally {
    if (theme === null) html.removeAttribute('data-theme');
    else html.setAttribute('data-theme', theme);
  }
}

test('every glyph is a filled Material Symbol: no Lucide strokes anywhere', async () => {
  await show({
    needsKey: true,
    makes: [
      { id: 'm1', request: 'A water reminder', state: 'offline', progress: null },
      { id: 'm2', request: 'A sleep log', state: 'waiting', progress: null },
      { id: 'm3', request: 'A chore chart', state: 'interrupted', progress: null },
      { id: 'm4', request: 'A tide table', state: 'failed', progress: null },
    ],
  });
  const svgs = [...document.querySelectorAll('.h-page svg:not(.clax-mark):not(.h-ring svg):not(.h-spinner svg)')];
  expect(svgs.length > 8).toBe(true);
  expect(svgs.filter((s) => s.hasAttribute('stroke-linecap') || s.getAttribute('viewBox') !== '0 -960 960 960').length).toBe(0);
  expect(document.querySelectorAll('[stroke-linecap]').length).toBe(0);
});

test('the bar shows the mark at 24 px and "Clax" in title-1, 56 px high', async () => {
  await show();
  const mark = document.querySelector('.h-bar .clax-mark');
  expect(!!mark).toBe(true);
  expect(Math.round(mark.getBoundingClientRect().width)).toBe(24);
  expect(ui.text('.h-bar .h-title')).toBe('Clax');
  const title = getComputedStyle(document.querySelector('.h-title'));
  expect([title.fontWeight, title.fontSize, title.lineHeight]).toEqual(['600', '22px', '28px']);
  const bar = document.querySelector('.h-bar');
  expect(Math.round(bar.getBoundingClientRect().height - parseFloat(getComputedStyle(bar).paddingTop))).toBe(56);
});

test("the bar's last button leaves 8 px for its focus ring", async () => {
  await show();
  const bar = document.querySelector('.h-bar').getBoundingClientRect();
  const last = [...document.querySelectorAll('.h-bar .h-action')].at(-1).getBoundingClientRect();
  expect(Math.round(bar.right - last.right)).toBe(8);
  const rules = [...document.styleSheets].flatMap((s) => { try { return [...s.cssRules]; } catch { return []; } });
  const rule = rules.find((r) => r.selectorText?.includes('.h-action:focus-visible'));
  expect(rule?.style.outlineStyle).toBe('solid');
  expect(parseFloat(rule.style.outlineOffset) + parseFloat(rule.style.outlineWidth)).toBe(4);
});

test('the inbox badge counts to 99, then reads 99+', async () => {
  const home = await show({ unread: 120 });
  expect(ui.text('.h-badge')).toBe('99+');
  expect(ui.exists('[aria-label="Inbox, 120 unread"]')).toBeTruthy();
  home.set({ unread: 99 });
  await wait(() => ui.text('.h-badge') === '99');
});

test('New app is an extended button on accent-tint with a mic', async () => {
  await show();
  await inLight(async () => {
    const fab = document.querySelector('.h-fab');
    const s = getComputedStyle(fab);
    expect([s.backgroundColor, s.color, s.borderRadius]).toEqual(['rgb(232, 241, 230)', 'rgb(47, 111, 42)', '16px']);
    expect(fab.textContent.trim()).toBe('New app');
    expect(fab.querySelector('svg').getAttribute('viewBox')).toBe('0 -960 960 960');
  });
});

test('the key banner is a banner on card, its key glyph leading, within the content width', async () => {
  await show({ needsKey: true });
  await inLight(async () => {
    const banner = document.querySelector('.h-key');
    expect(banner.classList.contains('h-banner')).toBe(true);
    const s = getComputedStyle(banner);
    expect([s.backgroundColor, s.borderRadius]).toEqual(['rgb(255, 255, 255)', '16px']);
    expect(banner.firstElementChild.classList.contains('h-banner-icon')).toBe(true);
    expect(banner.getBoundingClientRect().width <= 720).toBe(true);
  });
});

test("renaming a group selects all of its name, scrolled to its start, in a field raised at level 2 over the tile's name", async () => {
  const layout = groupLayout();
  layout.groups.g1.name = 'Groceries, errands and the weekly shop';
  await show({ makes: [] }, { layout });
  document.querySelector('.h-group').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => ui.exists('.h-group-menu'));
  await ui.click('Rename');
  await wait(() => nameField() && document.activeElement === nameField());
  const input = nameField();
  expect(input.scrollWidth > input.clientWidth).toBe(true);
  expect([input.selectionStart, input.selectionEnd]).toEqual([0, input.value.length]);
  expect(input.scrollLeft).toBe(0);
  const tile = document.querySelector('.h-group').getBoundingClientRect();
  const field = input.getBoundingClientRect();
  expect(Math.abs((field.left + field.right) / 2 - (tile.left + tile.right) / 2) <= 1).toBe(true);
  expect(field.top > tile.top + 40 && field.top < tile.bottom).toBe(true);
  await inLight(async () => {
    const f = getComputedStyle(input);
    expect(f.backgroundColor).toBe('rgb(255, 255, 255)');
    expect(f.boxShadow).toContain('rgba(20, 20, 15, 0.1) 0px 8px 24px');
  });
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  await wait(() => !ui.exists('.h-name'));
});

test('a notice spans the page less 16 px each side, at most 560 wide', async () => {
  const home = await show({ makes: [] });
  home.open = () => Promise.reject({ code: 'unavailable' });
  await ui.click('[data-id="app-run"]');
  await wait(() => ui.exists('.h-notice'));
  const r = document.querySelector('.h-notice').getBoundingClientRect();
  const width = document.documentElement.clientWidth;
  expect(Math.round(r.width)).toBe(Math.min(560, width - 32));
  expect(Math.abs(r.left - (width - r.right)) <= 1).toBe(true);
});
