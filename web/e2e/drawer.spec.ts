// The thread drawer wider than a phone (web/shell/src/ui/DrawerChrome.svelte):
// docked right, left or at the bottom with an edge that resizes it, or
// floating over the page, moved by its bar and resized by its edges, each
// remembered by the browser; at phone width the threads stay the full-screen
// sheet. With CLAX_DRAWER_SHOTS=<dir>, it photographs each layout in light
// and dark, and the phone's sheet, into that directory.
import { mkdirSync } from "node:fs";
import type { Frame, Locator, Page } from "@playwright/test";
import { test, expect, type Daemon, openArtifact, postThread, publishAs, registerSession } from "./fixtures";
import { settle } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const DESKTOP = { width: 1440, height: 900 };
const PAGE = "<main><h2>Goals</h2><p>Ship the drawer that moves.</p><h2>Risks</h2><p>None we know of.</p></main>";

async function open(page: Page, name: string, threads = 1): Promise<{ aid: string; frame: Frame }> {
  const s = await registerSession(d.base, d.token, "claude", `drawer-${name}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, `Drawer ${name}`, { "index.html": PAGE });
  for (let i = 0; i < threads; i++) await postThread(d.base, artifact.id, `Thread ${i + 1} on the goals`);
  const frame = await openArtifact(page, d.base, artifact.id, 1);
  await expect(page.locator(".drawer .drawer-bar")).toBeVisible();
  return { aid: artifact.id, frame };
}

const box = async (l: Locator) => (await l.boundingBox())!;
const drawer = (page: Page) => page.locator("div.drawer");
const stage = (page: Page) => page.locator(".stage");

/** Drags from the middle of `l` by (`dx`, `dy`), through the page under it. */
async function drag(page: Page, l: Locator, dx: number, dy: number) {
  const b = await box(l);
  const [x, y] = [b.x + b.width / 2, b.y + b.height / 2];
  await page.mouse.move(x, y, { steps: 2 });
  await page.mouse.down();
  await page.mouse.move(x + dx / 2, y + dy / 2, { steps: 4 });
  await page.mouse.move(x + dx, y + dy, { steps: 4 });
  await page.mouse.up();
}

/** The pin keeps its place on its heading in the frame (the heading's top
 * right, kept off the stage's right edge) wherever the stage is: its offset
 * from the heading's top right is the same at every call in a test, and
 * near that corner. */
const pinOffsets = new WeakMap<Page, string>();
async function pinOnHeading(page: Page, frame: Frame) {
  const offset = async () => {
    const h = await frame.locator("h2").first().boundingBox();
    const p = await page.locator("button.thread-pin").first().boundingBox();
    if (!h || !p) return "";
    const [dx, dy] = [Math.round(p.x + p.width / 2 - (h.x + h.width)), Math.round(p.y + p.height / 2 - h.y)];
    return Math.abs(dx) < 40 && Math.abs(dy) < 24 ? `${dx},${dy}` : `far: ${dx},${dy}`;
  };
  const known = pinOffsets.get(page);
  if (known) return expect.poll(offset).toBe(known);
  await expect.poll(offset).toMatch(/^-?\d+,-?\d+$/);
  pinOffsets.set(page, await offset());
}

const stored = (page: Page) => page.evaluate(() => JSON.parse(localStorage.getItem("clax.drawer") ?? "null"));

test("docked right by default, its edge drags and keys resize it, a double click resets it, and a reload keeps it", async ({ page }) => {
  await page.setViewportSize(DESKTOP);
  const { frame } = await open(page, "right");
  const edge = page.getByRole("separator", { name: "Resize threads width" });
  expect((await box(drawer(page))).width).toBe(392);
  expect(await page.getByRole("button", { name: "Dock threads right" }).getAttribute("aria-pressed")).toBe("true");
  await pinOnHeading(page, frame);
  // Dragged over the page, which takes no pointer while the drag lasts.
  await drag(page, edge, -100, 0);
  expect((await box(drawer(page))).width).toBe(492);
  expect((await box(stage(page))).width).toBe(1440 - 492);
  await expect(edge).toHaveAttribute("aria-valuenow", "492");
  await expect(page.locator(".viewer.drawer-drag")).toHaveCount(0);
  await pinOnHeading(page, frame);
  expect(await stored(page)).toMatchObject({ layout: "right", w: 492 });
  await page.reload();
  await expect(page.locator(".drawer .drawer-bar")).toBeVisible();
  expect((await box(drawer(page))).width).toBe(492);
  // Arrow keys move the edge 16 px, Home and End to its least and greatest width.
  await edge.focus();
  await page.keyboard.press("ArrowLeft");
  await expect(edge).toHaveAttribute("aria-valuenow", "508");
  await page.keyboard.press("ArrowRight");
  await expect(edge).toHaveAttribute("aria-valuenow", "492");
  await page.keyboard.press("Home");
  await expect(edge).toHaveAttribute("aria-valuenow", "280");
  await page.keyboard.press("End");
  await expect(edge).toHaveAttribute("aria-valuenow", "720");
  await expect(edge).toHaveAttribute("aria-valuemax", "720");
  await edge.dblclick();
  expect((await box(drawer(page))).width).toBe(392);
  expect(await stored(page)).toMatchObject({ w: 392 });
  // Escape cancels a drag: the drawer goes back, the page takes the pointer again, and nothing is remembered.
  const e = await box(edge);
  await page.mouse.move(e.x + e.width / 2, e.y + e.height / 2, { steps: 2 });
  await page.mouse.down();
  await page.mouse.move(e.x + e.width / 2 - 150, e.y + e.height / 2, { steps: 4 });
  expect((await box(drawer(page))).width).toBe(542);
  await expect(page.locator(".viewer.drawer-drag")).toHaveCount(1);
  await page.keyboard.press("Escape");
  expect((await box(drawer(page))).width).toBe(392);
  await expect(page.locator(".viewer.drawer-drag")).toHaveCount(0);
  await page.mouse.move(e.x - 250, e.y + e.height / 2, { steps: 2 });
  await page.mouse.up();
  expect((await box(drawer(page))).width).toBe(392);
  expect(await stored(page)).toMatchObject({ w: 392 });
  // A handle that leaves the page mid-drag loses the pointer's capture at the
  // document, which ends the drag where it was: the page takes the pointer again.
  await page.mouse.move(e.x + e.width / 2, e.y + e.height / 2, { steps: 2 });
  await page.mouse.down();
  await page.mouse.move(e.x + e.width / 2 - 50, e.y + e.height / 2, { steps: 2 });
  await expect(page.locator(".viewer.drawer-drag")).toHaveCount(1);
  await edge.evaluate(el => el.remove());
  await expect(page.locator(".viewer.drawer-drag")).toHaveCount(0);
  await page.mouse.up();
  expect(await stored(page)).toMatchObject({ w: 442 });
  await page.reload();
  await expect(page.locator(".drawer .drawer-bar")).toBeVisible();
  expect((await box(drawer(page))).width).toBe(442);
  // Hide threads, from the keyboard, closes the drawer and leaves focus on
  // the top bar's Threads, which opens it again.
  await page.getByRole("button", { name: "Hide threads" }).focus();
  await page.keyboard.press("Enter");
  await expect(drawer(page)).toHaveCount(0);
  expect((await box(stage(page))).width).toBe(1440);
  const toggle = page.locator(".topbar button.threads");
  await expect(toggle).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  expect((await box(drawer(page))).width).toBe(442);
});

test("docked left, then at the bottom, each resized by its edge and kept across a reload, holding its place before its code loads", async ({ page }) => {
  await page.setViewportSize(DESKTOP);
  const { frame } = await open(page, "left-bottom");
  // Tab from the top bar's last control: the page first while docked right.
  const nextAfterTopbar = async () => {
    await page.locator(".topbar").getByRole("button").last().focus();
    await page.keyboard.press("Tab");
    return page.evaluate(() => (document.activeElement?.closest(".drawer") ? "drawer" : "page"));
  };
  expect(await nextAfterTopbar()).toBe("page");
  const left = page.getByRole("button", { name: "Dock threads left" });
  await left.focus();
  await page.keyboard.press("Enter");
  // Docked left, the threads come before the page in the page and in Tab order, and the switch keeps focus.
  await expect(left).toBeFocused();
  expect(await page.evaluate(() => !!(document.querySelector("div.drawer")!.compareDocumentPosition(document.querySelector(".stage")!) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
  expect(await nextAfterTopbar()).toBe("drawer");
  const viewer = await box(page.locator(".viewer"));
  expect((await box(drawer(page))).x).toBe(viewer.x);
  expect((await box(stage(page))).x).toBe(viewer.x + 392);
  await pinOnHeading(page, frame);
  await drag(page, page.getByRole("separator", { name: "Resize threads width" }), 60, 0);
  expect((await box(drawer(page))).width).toBe(452);
  await pinOnHeading(page, frame);

  await page.getByRole("button", { name: "Dock threads at the bottom" }).click();
  expect(await nextAfterTopbar()).toBe("page");
  const edge =page.getByRole("separator", { name: "Resize threads height" });
  await expect(edge).toHaveAttribute("aria-orientation", "horizontal");
  let b = await box(drawer(page));
  expect([b.x, b.width, b.height]).toEqual([viewer.x, viewer.width, 280]);
  expect(b.y + b.height).toBe(viewer.y + viewer.height);
  expect((await box(stage(page))).height).toBe(viewer.height - 280);
  await drag(page, edge, 0, -100);
  expect((await box(drawer(page))).height).toBe(380);
  await edge.focus();
  await page.keyboard.press("ArrowDown");
  await expect(edge).toHaveAttribute("aria-valuenow", "364");
  await pinOnHeading(page, frame);
  expect(await stored(page)).toMatchObject({ layout: "bottom", w: 452, h: 364 });

  // On a reload the drawer holds its remembered place and size before its
  // code arrives, so the frame is never laid out beside a drawer of the
  // default place or size: only at the whole viewer (before the threads
  // show) and at the remembered size.
  let release = () => {};
  const held = new Promise<void>(r => { release = r; });
  await page.route("**/ShellSidebar-*.js", async r => { await held; await r.continue(); });
  await page.addInitScript(() => {
    const sizes: string[] = [];
    (window as unknown as { frameSizes: string[] }).frameSizes = sizes;
    new MutationObserver((_, o) => {
      const f = document.querySelector("iframe.frame");
      if (!f) return;
      o.disconnect();
      new ResizeObserver(() => { const r = f.getBoundingClientRect(); sizes.push(`${Math.round(r.width)}x${Math.round(r.height)}`); }).observe(f);
    }).observe(document, { childList: true, subtree: true });
  });
  await page.reload();
  const placeholder = page.locator('aside.sidebar.drawer[aria-busy="true"]');
  await expect(placeholder).toBeVisible();
  b = await box(placeholder);
  expect([b.x, b.width, b.height, b.y + b.height]).toEqual([viewer.x, viewer.width, 364, viewer.y + viewer.height]);
  release();
  await expect(page.locator(".drawer .drawer-bar")).toBeVisible();
  b = await box(drawer(page));
  expect(b.height).toBe(364);
  const full = `${viewer.width}x${viewer.height}`;
  const final = `${viewer.width}x${viewer.height - 364}`;
  expect((await page.evaluate(() => (window as unknown as { frameSizes: string[] }).frameSizes)).filter(s => s !== full)).toEqual([final]);
  await expect(page.getByRole("button", { name: "Dock threads at the bottom" })).toHaveAttribute("aria-pressed", "true");
});

test("floating over the page, moved by its bar and resized by its edges, kept across a reload and inside a smaller window", async ({ page }) => {
  await page.setViewportSize(DESKTOP);
  const { frame } = await open(page, "float");
  await page.getByRole("button", { name: "Float threads over the page" }).click();
  const viewer = await box(page.locator(".viewer"));
  // The page keeps its whole width; the window sits at the top right.
  expect((await box(stage(page))).width).toBe(1440);
  await pinOnHeading(page, frame);
  let b = await box(drawer(page));
  expect([b.x + b.width, b.y, b.height]).toEqual([viewer.x + viewer.width - 48, viewer.y + 16, viewer.height - 32]);
  await drag(page, page.locator(".drawer-title"), -500, 10);
  b = await box(drawer(page));
  expect([b.x, b.y]).toEqual([viewer.x + viewer.width - 48 - 392 - 500, viewer.y + 26]);
  // Moved past the viewer's edge, it stops there.
  await drag(page, page.locator(".drawer-title"), -2000, -400);
  b = await box(drawer(page));
  expect([b.x, b.y]).toEqual([viewer.x, viewer.y]);
  await drag(page, page.getByRole("separator", { name: "Resize threads window height" }), 0, -300);
  await drag(page, page.getByRole("separator", { name: "Resize threads window width" }), 100, 0);
  b = await box(drawer(page));
  expect([b.width, b.height]).toEqual([492, viewer.height - 32 - 300]);
  await drag(page, page.locator('[data-grip="corner"]'), 20, 20);
  b = await box(drawer(page));
  expect([b.width, b.height]).toEqual([512, viewer.height - 32 - 280]);
  // The move button moves it by the keys.
  await page.getByRole("button", { name: "Move the threads window" }).focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Shift+ArrowDown");
  b = await box(drawer(page));
  expect([b.x, b.y]).toEqual([viewer.x + 16, viewer.y + 64]);
  expect(await stored(page)).toEqual({ layout: "float", w: 392, h: 280, x: 16, y: 64, fw: 512, fh: viewer.height - 32 - 280 });

  await page.reload();
  await expect(page.locator(".drawer .drawer-bar")).toBeVisible();
  expect(await box(drawer(page))).toEqual(b);
  // A smaller window keeps it whole: moved and shrunk to fit, its place remembered as it was.
  await page.setViewportSize({ width: 800, height: 420 });
  await expect.poll(async () => {
    const v = await box(page.locator(".viewer"));
    const w = await box(drawer(page));
    return w.x >= v.x && w.y >= v.y && w.x + w.width <= v.x + v.width && w.y + w.height <= v.y + v.height;
  }).toBe(true);
  await page.setViewportSize(DESKTOP);
  await expect.poll(async () => box(drawer(page))).toEqual(b);
  // A double click on an edge puts the window back at its default place and size.
  await page.getByRole("separator", { name: "Resize threads window width" }).dblclick();
  b = await box(drawer(page));
  expect([b.x + b.width, b.y, b.width]).toEqual([viewer.x + viewer.width - 48, viewer.y + 16, 392]);
});

const overlaps = (a: { x: number; y: number; width: number; height: number }, b: typeof a) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
/** Whether the element topmost at the middle of `l` is in `l`. */
const onTop = (l: Locator) => l.evaluate(e => { const r = e.getBoundingClientRect(); return e.contains(document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2)); });

test("floating, the window leaves the pins clear by default, and the composer keeps beside it with the window's grip reachable", async ({ page }) => {
  await page.setViewportSize(DESKTOP);
  const { frame } = await open(page, "float-pins");
  await page.getByRole("button", { name: "Float threads over the page" }).click();
  const pin = page.locator("button.thread-pin").first();
  await expect(pin).toBeVisible();
  // The pin is placed again once the stage has its floating width.
  await expect.poll(async () => overlaps(await box(pin), await box(drawer(page)))).toBe(false);
  // Comment on the second heading: the composer opens beside the window, which stays above it.
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  await frame.locator("h2").nth(1).hover();
  await frame.locator("h2").nth(1).click();
  const composer = page.locator(".composer");
  await expect(composer).toBeVisible();
  expect(overlaps(await box(composer), await box(drawer(page)))).toBe(false);
  expect(await onTop(page.locator('[data-grip="corner"]'))).toBe(true);
  expect(await onTop(composer.locator("textarea"))).toBe(true);
  // Moved to the left, the window takes the left's room and the composer goes back to the right.
  await drag(page, page.locator(".drawer-title"), -900, 0);
  const w = await box(drawer(page));
  expect(w.x).toBeLessThan(400);
  await expect.poll(async () => overlaps(await box(composer), await box(drawer(page)))).toBe(false);
  expect((await box(composer)).x + (await box(composer)).width).toBe(DESKTOP.width - 16);
});

test("at phone width the threads are the full-screen sheet with the Page | Threads tabs, whatever the drawer's layout", async ({ page }) => {
  await page.setViewportSize(DESKTOP);
  await open(page, "phone");
  await page.getByRole("button", { name: "Float threads over the page" }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  const tabs = page.getByRole("group", { name: "Page or threads" });
  await expect(tabs).toBeVisible();
  await tabs.getByRole("button", { name: /^Threads/ }).click();
  const sheet = page.locator("aside.sidebar");
  await expect(sheet.locator(".gh")).toHaveCount(3);
  const s = await box(sheet);
  const v = await box(page.locator(".viewer"));
  expect([s.x, s.y, s.width, s.height]).toEqual([v.x, v.y, v.width, v.height]);
  await expect(page.locator(".drawer-bar")).toBeHidden();
  await expect(page.getByRole("separator")).toHaveCount(0);
  expect(await page.evaluate(() => getComputedStyle(document.querySelector("div.drawer")!).display)).toBe("contents");
  await tabs.getByRole("button", { name: "Page" }).click();
  await expect(sheet).toHaveCount(0);
});

/** `l` is focused from the keyboard and its edge line is drawn in --focus. */
async function expectFocusRing(page: Page, l: Locator) {
  await expect(l).toBeFocused();
  expect(await l.evaluate(e => e.matches(":focus-visible"))).toBe(true);
  const focus = await page.evaluate(() => { const p = document.body.appendChild(document.createElement("i")); p.style.color = "var(--focus)"; const c = getComputedStyle(p).color; p.remove(); return c; });
  await expect.poll(() => l.evaluate(e => getComputedStyle(e, "::after").backgroundColor)).toBe(focus);
  await settle(page);
}

test("an edge reached from the keyboard shows its focus line in --focus", async ({ page }) => {
  await page.setViewportSize(DESKTOP);
  await open(page, "focus");
  await page.getByRole("button", { name: "Hide threads" }).focus();
  await page.keyboard.press("Tab");
  await expectFocusRing(page, page.getByRole("separator", { name: "Resize threads width" }));
});

const SHOTS = process.env.CLAX_DRAWER_SHOTS;
test.describe("screenshots", () => {
  test.skip(!SHOTS, "set CLAX_DRAWER_SHOTS=<dir> to photograph the drawer's layouts");
  for (const theme of ["light", "dark"] as const) {
    test(`each layout and the phone's sheet in ${theme}`, async ({ page }) => {
      mkdirSync(SHOTS!, { recursive: true });
      await page.setViewportSize(DESKTOP);
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await open(page, `shots-${theme}`, 3);
      for (const [layout, name] of [["right", "Dock threads right"], ["left", "Dock threads left"], ["bottom", "Dock threads at the bottom"], ["float", "Float threads over the page"]] as const) {
        await page.getByRole("button", { name }).click();
        await page.mouse.move(2, 880);
        await settle(page);
        await page.screenshot({ path: `${SHOTS}/${theme}-desktop-${layout}.png` });
      }
      // Reached from the keyboard, so :focus-visible draws the edge's ring.
      await page.getByRole("button", { name: "Hide threads" }).focus();
      await page.keyboard.press("Tab");
      const fw = page.getByRole("separator", { name: "Resize threads window width" });
      await expect(fw).toBeFocused();
      await expectFocusRing(page, fw);
      await page.screenshot({ path: `${SHOTS}/${theme}-desktop-float-focus.png` });
      await page.keyboard.press("Tab");
      await expectFocusRing(page, page.getByRole("separator", { name: "Resize threads window height" }));
      await page.screenshot({ path: `${SHOTS}/${theme}-desktop-float-focus-height.png` });
      await page.getByRole("button", { name: "Dock threads right" }).click();
      await page.getByRole("button", { name: "Hide threads" }).focus();
      await page.keyboard.press("Tab");
      await expectFocusRing(page, page.getByRole("separator", { name: "Resize threads width" }));
      await page.screenshot({ path: `${SHOTS}/${theme}-desktop-right-focus.png` });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.reload();
      await page.getByRole("group", { name: "Page or threads" }).getByRole("button", { name: /^Threads/ }).click();
      await expect(page.locator("aside.sidebar .gh")).toHaveCount(3);
      await settle(page);
      await page.screenshot({ path: `${SHOTS}/${theme}-phone-threads.png` });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    });
  }
});
