// Renders of the Clax app's web pieces, for a person to look at: the home
// app over the fake `home` capability (apps/clax_app/assets/kit/fake-home.js),
// the starter kit's example app, and an artifact in app mode with the
// app's side played by ./app-host.ts, in comment mode over a thread's pin.
// Each scene is rendered at the phone (412×915, touch, 2.625×), Android
// wide (1280×800, touch, 2×) and the Mac (1200×800, 2×), light and dark,
// with reduced motion, into $CLAX_SHOTS_DIR/<scene>-<form>-<theme>.png.
// A page that scrolls sideways fails. Skipped without CLAX_SHOTS_DIR; the
// `shots` project is run on demand only, by //web:e2e_shots:
//
//   SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
//   bazelisk test //web:e2e_shots --nocache_test_results \
//     --test_env=CLAX_SHOTS_DIR="$SHOTS" --sandbox_writable_path="$SHOTS" \
//     --test_arg=--grep --test_arg=home
import { mkdirSync, readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Frame, Page } from "@playwright/test";
import { hostChannel, host, initHost, ofType } from "./app-host";
import { test, expect, api, contentFrame, postThread, registerSession } from "./fixtures";

const OUT = process.env.CLAX_SHOTS_DIR || "";
test.skip(!OUT, "set CLAX_SHOTS_DIR to the directory the renders go to");

const repo = fileURLToPath(new URL("../..", import.meta.url));
const homeSrc = join(repo, "apps/clax_app/home");
const kitSrc = join(repo, "apps/clax_app/assets/kit");

const FORMS = {
  phone: { viewport: { width: 412, height: 915 }, deviceScaleFactor: 2.625, isMobile: true, hasTouch: true },
  wide: { viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2, isMobile: false, hasTouch: true },
  mac: { viewport: { width: 1200, height: 800 }, deviceScaleFactor: 2, isMobile: false, hasTouch: false },
} as const;
type Form = keyof typeof FORMS;

/** Every file under `dir`, by its path relative to `dir`. */
function tree(dir: string): string[] {
  return readdirSync(dir).flatMap(n => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? tree(p).map(f => `${n}/${f}`) : [n];
  });
}

/** The kit's files under kit/, as an app made from it carries them. */
function kitFiles(): Record<string, string> {
  const files: Record<string, string> = {};
  for (const f of tree(kitSrc)) if (/\.(js|css)$/.test(f) && !f.startsWith("example/")) files[`kit/${f}`] = readFileSync(join(kitSrc, f), "utf8");
  return files;
}

/** The home app's files as its bundle carries them: the source (less its
 * manifest) and the kit files the manifest names, under kit/. */
function homeFiles(): Record<string, string> {
  const manifest = JSON.parse(readFileSync(join(homeSrc, "manifest.json"), "utf8")) as { kit: string[] };
  const files: Record<string, string> = {};
  for (const f of tree(homeSrc)) if (f !== "manifest.json") files[f] = readFileSync(join(homeSrc, f), "utf8");
  for (const f of manifest.kit) files[`kit/${f}`] = readFileSync(join(kitSrc, f), "utf8");
  return files;
}

/** The kit's example app: its page and the kit beside it. */
const exampleFiles = () => ({ "index.html": readFileSync(join(kitSrc, "example/index.html"), "utf8"), ...kitFiles() });

const ORIGIN = "http://shots.test";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

/** Serves `files` at ORIGIN and opens its index.html. With `home`, the page
 * has the fake `home` capability over that state (kept as `window.fakeHome`),
 * and a fake `db` seeded with `home.docs`. */
async function serve(page: Page, files: Record<string, string>, home?: Record<string, unknown>) {
  await page.route(`${ORIGIN}/**`, route => {
    const path = new URL(route.request().url()).pathname.replace(/^\//, "") || "index.html";
    const body = files[path];
    if (body === undefined) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ status: 200, contentType: TYPES[extname(path)] ?? "application/octet-stream", body });
  });
  if (home) {
    await page.addInitScript((state) => {
      const w = window as unknown as Record<string, unknown>;
      w.claude = {
        use: async (name: string) => {
          const { fakeHome, fakeDb } = await import(`${location.origin}/kit/fake-home.js`);
          if (name === "db") return (w.fakeDb ??= fakeDb((state as { docs?: Record<string, unknown> }).docs ?? {}));
          if (name !== "home") return null;
          return (w.fakeHome = fakeHome(state));
        },
      };
    }, home);
  }
  await page.goto(`${ORIGIN}/index.html`);
}

/** Waits for fonts and images, then writes the render and checks that
 * neither the page nor `frame` scrolls sideways. */
async function take(page: Page, scene: string, form: Form, theme: string, frame?: Frame) {
  await page.evaluate(() => document.fonts.ready);
  for (const f of [page.mainFrame(), ...(frame ? [frame] : [])]) {
    await f.evaluate(() => Promise.all([...document.images].map(i => i.decode().catch(() => {}))));
  }
  mkdirSync(OUT, { recursive: true });
  await page.screenshot({ path: join(OUT, `${scene}-${form}-${theme}.png`), animations: "disabled" });
  for (const f of [page.mainFrame(), ...(frame ? [frame] : [])]) {
    const { scroll, inner } = await f.evaluate(() => ({ scroll: document.documentElement.scrollWidth, inner: innerWidth }));
    expect(scroll, `${f.url()} scrolls sideways`).toBeLessThanOrEqual(inner);
  }
}

/** The sample's apps by ID, and opens today, for the arranged scenes. */
const IDS = ["app-run", "app-groceries", "app-habits", "app-recipes", "app-budget", "app-plants", "app-mood", "app-notes"];
const opens = (n: number) => ({ n, last: new Date().toISOString(), days: { [new Date().toISOString().slice(0, 10)]: n } });
const GROUPED = {
  v: 1, starred: ["app-run", "app-notes"], order: ["g:g1", "g:g2"], seen: IDS,
  groups: { g1: { name: "Lists", look: "folder", apps: ["app-groceries", "app-budget", "app-plants"] }, g2: { name: "Fun", look: "pile", apps: ["app-habits", "app-recipes", "app-mood"] } },
};
/** Home scenes over a stored arrangement: two starred apps, a folder and a
 * pile (then the folder open, the ⋮ menu open, a search for "re"); the
 * three apps opened most on big tiles; and a tile under the mouse. */
const ARRANGED: Record<string, unknown> = {
  "home-groups": GROUPED,
  "home-folder": GROUPED,
  "home-menu": GROUPED,
  "home-search": GROUPED,
  "home-big": { v: 1, starred: [], order: IDS, seen: IDS, opens: { "app-run": opens(9), "app-habits": opens(7), "app-plants": opens(6) } },
  "home-hover": { v: 1, starred: [], order: IDS, seen: IDS },
};

for (const form of Object.keys(FORMS) as Form[]) {
  for (const theme of ["light", "dark"] as const) {
    test.describe(`${form} ${theme}`, () => {
      test.use({ ...FORMS[form], colorScheme: theme, reducedMotion: "reduce" });

      test(`home ${form} ${theme}`, async ({ page }) => {
        await serve(page, homeFiles(), {});
        await expect(page.locator(".h-tile")).toHaveCount(11);
        await take(page, "home", form, theme);
      });

      test(`home-empty ${form} ${theme}`, async ({ page }) => {
        await serve(page, homeFiles(), { apps: [], makes: [] });
        await expect(page.locator(".h-message .clax-mark")).toBeVisible();
        await take(page, "home-empty", form, theme);
      });

      test(`home-banner ${form} ${theme}`, async ({ page }) => {
        // The key banner, makes in each state, a build, 120 unread and a notice.
        await serve(page, homeFiles(), {
          needsKey: true, unread: 120,
          makes: [
            { id: "m1", request: "A step counter", state: "making", progress: { done: 3, total: 7 } },
            { id: "m2", request: "A tide table", state: "offline", progress: null },
            { id: "m3", request: "A chore chart", state: "failed", progress: null },
          ],
        });
        await expect(page.locator(".h-key")).toBeVisible();
        await page.evaluate(() => { (window as unknown as { fakeHome: { open: () => Promise<never> } }).fakeHome.open = () => Promise.reject({ code: "unavailable" }); });
        await page.locator('[data-id="app-run"]').click();
        await expect(page.locator(".h-notice")).toBeVisible();
        await page.mouse.move(1, 1);
        await take(page, "home-banner", form, theme);
      });

      test(`home-error ${form} ${theme}`, async ({ page }) => {
        await serve(page, homeFiles(), {});
        await expect(page.locator(".h-tile").first()).toBeVisible();
        // Mounted again over a home whose list fails.
        await page.evaluate(async () => {
          const { mount } = await import(`${location.origin}/home.js`);
          const { fakeHome } = await import(`${location.origin}/kit/fake-home.js`);
          const home = fakeHome({});
          home.apps = () => Promise.reject({ code: "internal" });
          mount(document.getElementById("app"), home);
        });
        await expect(page.locator(".h-message")).toContainText("Couldn’t show your apps");
        await take(page, "home-error", form, theme);
      });

      for (const [scene, layout] of Object.entries(ARRANGED)) {
        test(`${scene} ${form} ${theme}`, async ({ page }) => {
          await serve(page, homeFiles(), { makes: [], docs: { "layout/main": layout } });
          await expect(page.locator(".h-tile[data-key]").first()).toBeVisible();
          if (scene === "home-folder") await page.locator('.h-group[data-group="g1"]').click();
          if (scene === "home-menu") await page.getByRole("button", { name: "More" }).click();
          if (scene === "home-search") {
            await page.getByRole("button", { name: "Search", exact: true }).click();
            await page.keyboard.type("re");
            await expect(page.locator(".h-results .h-tile[data-id]").first()).toBeVisible();
          }
          if (scene === "home-hover" && !FORMS[form].hasTouch) await page.locator('[data-id="app-groceries"]').hover();
          else await page.mouse.move(1, 1);
          await take(page, scene, form, theme);
        });
      }

      test(`kit ${form} ${theme}`, async ({ page }) => {
        await serve(page, exampleFiles());
        await expect(page.locator(".k-stat").first()).toBeVisible();
        await take(page, "kit", form, theme);
      });

      test(`app-comment ${form} ${theme}`, async ({ page, daemon: d }) => {
        const s = await registerSession(d.base, d.token, "app");
        const files = Object.fromEntries(Object.entries(exampleFiles()).map(([k, v]) => [k, { content: v, encoding: "utf8" }]));
        const { artifact } = await api(d.base, d.token, "/api/artifacts", { method: "POST", session: s.id, body: JSON.stringify({ title: "Pocket Budget", capabilities: { db: {} }, files }) }) as { artifact: { id: string } };
        await postThread(d.base, artifact.id, "Show the budget left first", ".k-stat");
        await hostChannel(page);
        await page.goto(`${d.base}/a/${artifact.id}?app=1`);
        const frame = await contentFrame(page, artifact.id, 1);
        await initHost(page);
        await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
        await expect(frame.locator(".k-stat").first()).toBeVisible();
        // Comment mode: the page framed in the pin colour and its threads'
        // pins shown. A pick would end it, the app's composer taking over.
        await host(page, "enterCommentMode()");
        await expect.poll(() => page.locator(".stage").evaluate(el => getComputedStyle(el, "::after").boxShadow !== "none")).toBe(true);
        await expect(page.locator(".thread-pin").first()).toBeVisible();
        await take(page, "app-comment", form, theme, frame);
      });
    });
  }
}
