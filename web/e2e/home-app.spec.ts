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
    expect(tests.passed).toBe(13);
  });
});

const ORIGIN = "http://home.test";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".txt": "text/plain" };

/** Serves the home app at ORIGIN; with `fake`, `claude.use("home")`
 * resolves the fake capability over that state (`window.fakeHomeCalls`
 * keeps its calls), else the page has no `claude` at all. */
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
          if (name !== "home") return null;
          const { fakeHome } = await import(`${location.origin}/kit/fake-home.js`);
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
      })] },
      { method: "openInbox", args: [] },
    ]);
    // Four to a row on a phone, cells at most 112 px.
    const boxes = await page.locator(".h-tile").evaluateAll(els => els.slice(0, 5).map(e => e.getBoundingClientRect()).map(r => ({ x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width) })));
    expect(new Set(boxes.slice(0, 4).map(b => b.y)).size).toBe(1);
    expect(boxes[4].y).toBeGreaterThan(boxes[0].y);
    expect(boxes[0].w).toBeLessThan(113);
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
            mount(document.getElementById("app"), fakeHome({
              needsKey: true, unread: 12,
              makes: [
                { id: "m1", request: "A step counter", state: "making", progress: { done: 3, total: 7 } },
                { id: "m2", request: "A tide table", state: "offline", progress: null },
                { id: "m3", request: "A chore chart", state: "failed", progress: null },
              ],
              apps: SAMPLE.apps.map((a: { id: string }) => (a.id === "app-notes" ? { ...a, build: { done: 2, total: 5 } } : a)),
            }));
          });
          await expect(page.locator(".h-key")).toBeVisible();
          await page.evaluate(() => Promise.all([...document.images].map(i => i.decode().catch(() => {}))));
          await page.screenshot({ path: join(shots, `home-states-${size}-${scheme}.png`), animations: "disabled" });
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
