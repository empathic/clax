// The home app (apps/clax_app/home, shipped as apps/clax_app/assets/home/
// home.clax): its own tests (tests/home.test.js) run by the inspect view's
// harness over a draft of it, as the agent's render runs them; and the page
// over a fake `home` capability (apps/clax_app/assets/kit/fake-home.js) in
// a plain browser: it lists, opens, starts a make and opens the menu, and
// with no capability it says to open it in Clax. Each run writes renders at
// phone and desktop sizes, light and dark, to target/flutter-app/home/.
import { mkdirSync, readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { host, hostChannel, initHost, ofType, type HostMessage } from "./app-host";
import { test, expect, api, contentFrame } from "./fixtures";
import { activationLapsed } from "./time";

const repo = fileURLToPath(new URL("../..", import.meta.url));
const source = join(repo, "apps/clax_app/home");
const kit = join(repo, "apps/clax_app/assets/kit");
const shots = join(repo, "target/flutter-app/home");

/** Every file under `dir`, by its path relative to `dir`. */
function tree(dir: string): string[] {
  return readdirSync(dir).flatMap(n => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? tree(p).map(f => `${n}/${f}`) : [n];
  });
}

/** The home app's files as its bundle carries them: the source (less its
 * manifest) and the kit files the manifest names, under kit/. */
function homeFiles(): Record<string, string> {
  const manifest = JSON.parse(readFileSync(join(source, "manifest.json"), "utf8")) as { kit: string[] };
  const files: Record<string, string> = {};
  for (const f of tree(source)) if (f !== "manifest.json") files[f] = readFileSync(join(source, f), "utf8");
  for (const f of manifest.kit) files[`kit/${f}`] = readFileSync(join(kit, f), "utf8");
  return files;
}

test.describe("its tests in the inspect view", () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 412, height: 915 } });

  test("tests/home.test.js passes over the fake home capability", async ({ page, daemon: d }) => {
    const files = homeFiles();
    const r = await api(d.base, d.token, "/api/drafts", { method: "POST", body: JSON.stringify({ title: "Home", capabilities: { db: {} }, files }) }) as { draft: { id: string } };
    const id = r.draft.id;
    await hostChannel(page);
    await page.goto(`${d.base}/a/${id}?app=1&inspect=1`);
    await contentFrame(page, id, 1);
    await initHost(page);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    await host(page, `runTests({ files: ["tests/home.test.js"], testMs: 10000, totalMs: 60000 })`);
    await expect.poll(async () => (await ofType(page, "tests")).length, { timeout: 60_000 }).toBeGreaterThan(0);
    const tests = (await ofType(page, "tests"))[0] as HostMessage & { passed: number; failed: unknown[]; skipped: number };
    expect(tests.failed).toEqual([]);
    expect(tests.skipped).toBe(0);
    expect(tests.passed).toBe(46);
  });
});

type DeviceCall = HostMessage & { id: string; cap: string; method: string; args: unknown[] };

test.describe("in app mode, through the shell", () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 412, height: 915 } });

  // The app's answers to the home app's calls, as HomeHost gives them for
  // its recorded home app; every call is answered once, and kept.
  const SAMPLE_APPS = [
    { id: "app-run", title: "Run tracker", updatedAt: "2026-10-16T11:00:00Z", build: null, icon: { monogram: "RT", hue: 20 } },
    { id: "app-groceries", title: "Groceries", updatedAt: "2026-10-16T10:00:00Z", build: null, icon: { word: "groceries" } },
  ];
  const answer = (m: string): unknown => ({ available: true, apps: SAMPLE_APPS, makes: [], unread: 0, needsKey: false })[m] ?? null;

  test("a touch long press arms the tile, and its release asks the app for the menu, which the shell accepts", async ({ page, daemon: d }) => {
    const files = Object.fromEntries(Object.entries(homeFiles()).map(([k, v]) => [k, { content: v, encoding: "utf8" }]));
    const { artifact } = await api(d.base, d.token, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "Home", capabilities: { home: {}, db: {} }, files }) }) as { artifact: { id: string } };
    await hostChannel(page);
    await page.goto(`${d.base}/a/${artifact.id}?app=1`);
    const frame = await contentFrame(page, artifact.id, 1);
    await initHost(page);
    const answered = new Set<string>();
    const calls: DeviceCall[] = [];
    const serveCalls = async () => {
      for (const c of (await ofType(page, "device")) as DeviceCall[]) {
        if (c.cap !== "home" || answered.has(c.id)) continue;
        answered.add(c.id);
        calls.push(c);
        await host(page, `deviceResult(${JSON.stringify({ id: c.id, ok: true, value: answer(c.method) })})`);
      }
      return calls.map(c => c.method);
    };
    await expect.poll(async () => { await serveCalls(); return frame.locator(".h-tile").count(); }).toBe(2);
    // Once the grid is drawn with its apps, the page says so: Clax lifts
    // its own home screen on that, not on the frame's ready.
    await expect.poll(serveCalls).toContain("shown");
    const order = calls.map(c => c.method);
    expect(order.indexOf("shown")).toBeGreaterThan(order.indexOf("apps"));
    expect(order.filter(m => m === "shown")).toHaveLength(1);

    const box = (await frame.locator('[data-id="app-groceries"] .h-icon').boundingBox())!;
    const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
    const cdp = await page.context().newCDPSession(page);
    const touch = (type: "touchStart" | "touchEnd") => cdp.send("Input.dispatchTouchEvent", { type, touchPoints: type === "touchEnd" ? [] : [{ x, y, id: 1 }] });
    // The frame's own document, read without granting user activation.
    const origin = new URL(frame.url()).origin;
    const contexts: { id: number; origin: string; auxData?: { isDefault?: boolean } }[] = [];
    cdp.on("Runtime.executionContextCreated", e => { contexts.push(e.context as (typeof contexts)[number]); });
    await cdp.send("Runtime.enable");
    const ctx = contexts.find(c => c.origin === origin && c.auxData?.isDefault)!;
    expect(ctx).toBeTruthy();
    const held = async () => ((await cdp.send("Runtime.evaluate", { contextId: ctx.id, expression: `'held' in document.querySelector('[data-id="app-groceries"]').dataset`, returnByValue: true, userGesture: false })).result.value) as boolean;
    // No tap in the last 5 s, as on a phone left on the home screen a while.
    await activationLapsed(page);
    // The finger goes down, as the app attests it, and stays down until the tile arms.
    await host(page, `attestPointer({ pointer: 1, phase: "down", x: ${x}, y: ${y} })`);
    await touch("touchStart");
    await expect.poll(held).toBe(true);
    expect(await serveCalls()).not.toContain("menu");
    await touch("touchEnd");
    await host(page, `attestPointer({ pointer: 1, phase: "up", x: ${x}, y: ${y} })`);
    await expect.poll(serveCalls).toContain("menu");
    const menu = calls.find(c => c.method === "menu")!;
    expect(menu.args).toEqual([{ id: "app-groceries", rect: { x: expect.any(Number), y: expect.any(Number), w: 60, h: 60 }, items: [{ id: "star", label: "Star" }] }]);
    // The release's click opens nothing, and nothing says the call failed.
    expect((await serveCalls()).filter(m => m === "open")).toEqual([]);
    await expect(frame.locator(".h-notice")).toHaveCount(0);
  });
});

const ORIGIN = "http://home.test";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain" };

/** Serves the home app at ORIGIN; with `fake`, `claude.use("home")`
 * resolves the fake capability over that state (`window.fakeHome` keeps
 * its calls) and `claude.use("db")` a fake `db` seeded with `fake.docs`
 * and kept across reloads, else the page has no `claude` at all. */
async function serve(page: Page, fake?: Record<string, unknown>) {
  const files = homeFiles();
  await page.route(`${ORIGIN}/**`, route => {
    const path = new URL(route.request().url()).pathname.replace(/^\//, "") || "index.html";
    const body = files[path];
    if (body === undefined) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ status: 200, contentType: TYPES[extname(path)] ?? "application/octet-stream", body });
  });
  if (fake) {
    await page.addInitScript((state) => {
      const w = window as unknown as Record<string, unknown>;
      w.claude = {
        use: async (name: string) => {
          const { fakeHome, fakeDb } = await import(`${location.origin}/kit/fake-home.js`);
          if (name === "db") return (w.fakeDb ??= fakeDb((state as { docs?: Record<string, unknown> }).docs ?? {}, { persist: "clax-home-db" }));
          if (name !== "home") return null;
          const home = fakeHome(state);
          w.fakeHome = home;
          return home;
        },
      };
    }, fake);
  }
  await page.goto(`${ORIGIN}/index.html`);
}

const calls = (page: Page) => page.evaluate(() => (window as unknown as { fakeHome: { calls: unknown[] } }).fakeHome.calls);

test.describe("over a fake host", () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 412, height: 915 } });

  test("lists, opens, starts a make and opens the menu", async ({ page }) => {
    await serve(page, { unread: 3 });
    await expect(page.locator(".h-tile")).toHaveCount(11);
    await expect(page.locator(".h-label").first()).toHaveText("A packing list for a weekend away");
    await page.locator('[data-id="app-groceries"]').tap();
    await page.getByRole("button", { name: "New app" }).tap();
    await page.locator('[data-id="app-habits"]').click({ button: "right" });
    await page.getByRole("button", { name: "Inbox, 3 unread" }).tap();
    await expect.poll(() => calls(page)).toEqual([
      { method: "open", args: ["app-groceries"] },
      { method: "newApp", args: [] },
      { method: "menu", args: ["app-habits", await page.locator('[data-id="app-habits"] .h-face').evaluate(el => {
        const r = el.getBoundingClientRect();
        return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
      }), { items: [{ id: "star", label: "Star" }] }] },
      { method: "openInbox", args: [] },
    ]);
    // Four to a row on a phone, cells at most 112 px.
    const boxes = await page.locator(".h-tile").evaluateAll(els => els.slice(0, 5).map(e => e.getBoundingClientRect()).map(r => ({ x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width) })));
    expect(new Set(boxes.slice(0, 4).map(b => b.y)).size).toBe(1);
    expect(boxes[4].y).toBeGreaterThan(boxes[0].y);
    expect(boxes[0].w).toBeLessThan(113);
  });

  test("stars from the menu, marks a new app, and keeps the arrangement across a reload", async ({ page }) => {
    const seen = ["app-run", "app-groceries", "app-habits", "app-recipes", "app-budget", "app-plants", "app-mood"];
    await serve(page, { makes: [], docs: { "layout/main": { v: 1, starred: [], order: [], seen } } });
    await expect(page.locator('[data-id="app-notes"][data-new]')).toHaveCount(1);
    await page.evaluate(() => { (window as unknown as { fakeHome: { menuPick: string } }).fakeHome.menuPick = "star"; });
    await page.locator('[data-id="app-plants"]').click({ button: "right" });
    await expect(page.locator('.h-section[data-section="starred"] [data-id="app-plants"]')).toHaveCount(1);
    await page.reload();
    await expect(page.locator('.h-section[data-section="starred"] [data-id="app-plants"]')).toHaveCount(1);
    await expect(page.locator('[data-id="app-notes"][data-new]')).toHaveCount(1);
    await page.locator('[data-id="app-notes"]').tap();
    await expect(page.locator('[data-id="app-notes"][data-new]')).toHaveCount(0);
    await page.reload();
    await expect(page.locator(".h-tile[data-id]")).toHaveCount(8);
    await expect(page.locator("[data-new]")).toHaveCount(0);
  });

  test("a touch hold then drag reorders, and a mouse drag to Starred stars", async ({ page }) => {
    await page.clock.install();
    await serve(page, { makes: [] });
    await expect(page.locator(".h-tile[data-id]")).toHaveCount(8);
    const box = async (sel: string) => (await page.locator(sel).boundingBox())!;
    const from = await box('[data-id="app-run"]');
    const to = await box('[data-id="app-budget"]');
    const cdp = await page.context().newCDPSession(page);
    const touch = (type: "touchStart" | "touchMove" | "touchEnd", x: number, y: number) => cdp.send("Input.dispatchTouchEvent", { type, touchPoints: type === "touchEnd" ? [] : [{ x, y, id: 1 }] });
    const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
    const [x1, y1] = [to.x + to.width / 2, to.y + to.height / 2];
    await touch("touchStart", x0, y0);
    await page.clock.runFor(450);
    for (let i = 1; i <= 8; i++) { await touch("touchMove", x0 + (x1 - x0) * i / 8, y0 + (y1 - y0) * i / 8); await page.clock.runFor(30); }
    await page.clock.runFor(150);
    await touch("touchEnd", x1, y1);
    await page.clock.runFor(1500);
    await expect.poll(() => page.locator('.h-section[data-section="all"] .h-tile[data-id]').evaluateAll(els => els.map(e => (e as HTMLElement).dataset.id).indexOf("app-run"))).toBe(4);
    await expect.poll(() => page.locator(".h-tile[data-id]").evaluateAll(els => els.every(e => !(e as HTMLElement).style.transform))).toBe(true);
    // The release opened nothing.
    expect((await calls(page)).filter(c => (c as { method: string }).method === "open")).toEqual([]);
    const notes = await box('[data-id="app-notes"]');
    await page.mouse.move(notes.x + 30, notes.y + 30);
    await page.mouse.down();
    await page.mouse.move(notes.x + 30, notes.y + 10, { steps: 2 });
    await expect(page.locator(".h-drop")).toBeVisible();
    // The zone opens above the grid without moving the tiles under the
    // pointer; held near the top edge, the page scrolls it into view.
    const bar = await box(".h-bar");
    await page.mouse.move(notes.x + 30, bar.y + bar.height + 8, { steps: 4 });
    await page.clock.runFor(1000);
    const zone = await box(".h-drop");
    expect(zone.y).toBeGreaterThanOrEqual(bar.y + bar.height);
    await page.mouse.move(zone.x + 40, zone.y + 40, { steps: 6 });
    await page.mouse.up();
    await page.clock.runFor(1500);
    await expect(page.locator('.h-section[data-section="starred"] [data-id="app-notes"]')).toHaveCount(1);
  });

  test("without the home capability it says to open it in Clax", async ({ page }) => {
    await serve(page);
    await expect(page.locator(".h-message")).toContainText("This is a Clax home screen.Open it in the Clax app.");
    await expect(page.locator(".h-grid")).toHaveCount(0);
  });
});

test.describe("renders", () => {
  const sizes = { phone: { width: 412, height: 915 }, desktop: { width: 1280, height: 800 } } as const;
  for (const [size, viewport] of Object.entries(sizes)) {
    for (const scheme of ["light", "dark"] as const) {
      test.describe(`${size} ${scheme}`, () => {
        test.use({ viewport, colorScheme: scheme, deviceScaleFactor: 2, isMobile: size === "phone", hasTouch: size === "phone" });
        test(`home ${size} ${scheme}`, async ({ page }) => {
          mkdirSync(shots, { recursive: true });
          await serve(page, {});
          await expect(page.locator(".h-tile")).toHaveCount(11);
          await page.evaluate(() => Promise.all([...document.images].map(i => i.decode().catch(() => {}))));
          await page.screenshot({ path: join(shots, `home-${size}-${scheme}.png`), animations: "disabled" });
          await page.evaluate(async () => {
            const { mount } = await import(`${location.origin}/home.js`);
            const { fakeHome, SAMPLE } = await import(`${location.origin}/kit/fake-home.js`);
            const home = fakeHome({
              needsKey: true, unread: 12,
              makes: [
                { id: "m1", request: "A step counter", state: "making", progress: { done: 3, total: 7 } },
                { id: "m2", request: "A tide table", state: "offline", progress: null },
                { id: "m3", request: "A chore chart", state: "failed", progress: null },
              ],
              apps: SAMPLE.apps.map((a: { id: string }) => (a.id === "app-notes" ? { ...a, build: { done: 2, total: 5 } } : a)),
            });
            home.open = () => Promise.reject({ code: "no_gesture", message: "tap" });
            mount(document.getElementById("app"), home);
          });
          await expect(page.locator(".h-key")).toBeVisible();
          await page.locator('[data-id="app-run"]').click();
          await expect(page.locator(".h-notice")).toHaveText("Tap again to open Run tracker");
          await page.evaluate(() => Promise.all([...document.images].map(i => i.decode().catch(() => {}))));
          await page.screenshot({ path: join(shots, `home-states-${size}-${scheme}.png`), animations: "disabled" });
          await page.evaluate(async () => {
            const { mount } = await import(`${location.origin}/home.js`);
            const { fakeHome, fakeDb, SAMPLE } = await import(`${location.origin}/kit/fake-home.js`);
            const ids = SAMPLE.apps.map((a: { id: string }) => a.id);
            const db = fakeDb({ "layout/main": { v: 1, starred: ["app-run", "app-notes"], order: ids.filter((id: string) => !["app-run", "app-notes", "app-mood"].includes(id)), seen: ids.filter((id: string) => id !== "app-mood") } });
            mount(document.getElementById("app"), fakeHome({ makes: [] }), { db });
          });
          await expect(page.locator(".h-heading")).toHaveCount(2);
          await page.evaluate(() => Promise.all([...document.images].map(i => i.decode().catch(() => {}))));
          await page.screenshot({ path: join(shots, `home-starred-${size}-${scheme}.png`), animations: "disabled" });
          // A drag caught midway: the tile lifted under the pointer, its
          // neighbours moved aside to show where it lands.
          const habits = (await page.locator('[data-id="app-habits"]').boundingBox())!;
          await page.mouse.move(habits.x + habits.width / 2, habits.y + 40);
          await page.mouse.down();
          await page.mouse.move(habits.x + habits.width / 2 + 60, habits.y + 80, { steps: 4 });
          await expect(page.locator(".h-page[data-dragging]")).toHaveCount(1);
          await expect.poll(() => page.locator(".h-tile[data-id]:not([data-dragging])").evaluateAll(els => els.every(e => !(e as HTMLElement).style.transform))).toBe(true);
          await page.screenshot({ path: join(shots, `home-drag-${size}-${scheme}.png`), animations: "disabled" });
          await page.mouse.up();
          // With nothing starred, a drag opens the zone to star by dropping:
          // scrolled into view, with the pointer over it.
          await page.evaluate(async () => {
            const { mount } = await import(`${location.origin}/home.js`);
            const { fakeHome, fakeDb, SAMPLE } = await import(`${location.origin}/kit/fake-home.js`);
            const ids = SAMPLE.apps.map((a: { id: string }) => a.id);
            mount(document.getElementById("app"), fakeHome({ makes: [] }), { db: fakeDb({ "layout/main": { v: 1, starred: [], order: ids, seen: ids } }) });
          });
          await expect(page.locator(".h-tile[data-id]")).toHaveCount(8);
          const plants = (await page.locator('[data-id="app-plants"]').boundingBox())!;
          await page.mouse.move(plants.x + plants.width / 2, plants.y + 40);
          await page.mouse.down();
          await page.mouse.move(plants.x + plants.width / 2, plants.y + 20, { steps: 2 });
          await expect(page.locator(".h-drop")).toHaveCount(1);
          await page.evaluate(() => scrollTo(0, 0));
          const drop = (await page.locator(".h-drop").boundingBox())!;
          await page.mouse.move(drop.x + drop.width / 2, drop.y + drop.height / 2, { steps: 4 });
          await expect(page.locator(".h-drop[data-over]")).toHaveCount(1);
          await expect.poll(() => page.locator(".h-tile[data-id]:not([data-dragging])").evaluateAll(els => els.every(e => !(e as HTMLElement).style.transform))).toBe(true);
          await page.screenshot({ path: join(shots, `home-drop-${size}-${scheme}.png`), animations: "disabled" });
          await page.mouse.up();
          await page.evaluate(async () => {
            const { mount } = await import(`${location.origin}/home.js`);
            const { fakeHome } = await import(`${location.origin}/kit/fake-home.js`);
            mount(document.getElementById("app"), fakeHome({ apps: [], makes: [] }));
          });
          await expect(page.locator(".h-message")).toBeVisible();
          await page.screenshot({ path: join(shots, `home-empty-${size}-${scheme}.png`) });
          expect(relative(repo, shots)).toBe("target/flutter-app/home");
        });
      });
    }
  }
});
