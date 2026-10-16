// The home screen's tests, run by the inspect view's harness (test, expect,
// ui). Each test mounts the page over its own fake `home` capability.
import { mount } from '../home.js';
import { fakeHome, SAMPLE } from '../kit/fake-home.js';

const root = () => document.getElementById('app');
const tiles = () => [...document.querySelectorAll('.h-tile')];
const labels = () => tiles().map((t) => t.querySelector('.h-label').textContent.trim());

async function show(state) {
  const home = fakeHome(state);
  mount(root(), home);
  await ui.waitFor(() => document.querySelector('.h-grid, .h-message'));
  return home;
}

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
  const home = await show({ makes: [] });
  const tile = document.querySelector('[data-id="app-habits"]');
  tile.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 5, clientY: 5 }));
  await wait(() => home.calls.length === 1);
  const [first] = home.calls;
  expect(first.method).toBe('menu');
  expect(first.args[0]).toBe('app-habits');
  const r = tile.querySelector('.h-icon').getBoundingClientRect();
  const rect = { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
  expect(first.args[1]).toEqual(rect);
  expect(rect.w).toBe(60);

  const opts = { bubbles: true, cancelable: true, pointerId: 7, isPrimary: true, pointerType: 'touch', clientX: r.x + 10, clientY: r.y + 10 };
  tile.dispatchEvent(new PointerEvent('pointerdown', opts));
  // The WebView's own long press comes while the finger is down: the release answers it.
  tile.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  await wait(() => 'held' in tile.dataset);
  expect(home.calls.length).toBe(1);
  tile.dispatchEvent(new PointerEvent('pointerup', opts));
  await wait(() => home.calls.length === 2);
  expect(home.calls[1]).toEqual({ method: 'menu', args: ['app-habits', rect] });
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
  mount(root(), null);
  await ui.waitFor(() => ui.exists('.h-message'));
  expect(ui.text()).toContain('This is a Clax home screen. Open it in the Clax app.');
  expect(ui.exists('.h-grid')).toBe(false);
  expect(ui.exists('.h-fab')).toBe(false);
});

test('a refused capability reads as unavailable; another failure offers Try again', async () => {
  const refused = { ...fakeHome(), apps: () => Promise.reject(Object.assign(new Error('refused'), { code: 'refused' })) };
  mount(root(), refused);
  await wait(() => ui.text().includes('This is a Clax home screen.'));
  let fail = true;
  const flaky = fakeHome();
  const apps = flaky.apps;
  flaky.apps = () => (fail ? Promise.reject(new Error('offline')) : apps());
  mount(root(), flaky);
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
  mount(root(), home);
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
