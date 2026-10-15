// The kit's map (apps/clax_app/assets/kit/map.js) in the Clax app's view:
// its tiles go through the `net` capability, so the first one asks the owner
// for the tile host, and after Allow the tiles show as blob: images. The tile
// host is under `.invalid`, so the daemon's own fetch fails
// (`upstream_error`); the test answers those fetches itself, at the shell's
// `net:fetch` request, with a PNG per tile, while the consent refusals come
// from the daemon unchanged.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { crc32, deflateSync } from "node:zlib";
import type { Frame, Page } from "@playwright/test";
import { host, hostChannel, initHost, ofType, type HostMessage } from "./app-host";
import { test, expect, type Daemon, api, contentFrame } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const KIT = new URL("../../apps/clax_app/assets/kit/", import.meta.url);
const kitFiles = Object.fromEntries(["preact.js", "icons.js", "map.js", "kit.css"]
  .map(n => [`kit/${n}`, { content: readFileSync(new URL(n, KIT), "utf8"), encoding: "utf8" }]));

const HOST = "tiles.clax.invalid";

/** A 256 by 256 PNG, one colour per tile, with a darker edge. */
function png(z: number, x: number, y: number): Buffer {
  const chunk = (type: string, data: Buffer) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const hdr = Buffer.alloc(13);
  hdr.writeUInt32BE(256, 0);
  hdr.writeUInt32BE(256, 4);
  hdr.set([8, 2, 0, 0, 0], 8);
  const base = [(x * 53 + z * 31) % 200 + 40, (y * 71) % 200 + 40, ((x + y) % 2) * 120 + 90];
  const rows: Buffer[] = [];
  for (let r = 0; r < 256; r++) {
    const row = Buffer.alloc(1 + 256 * 3);
    for (let c = 0; c < 256; c++) {
      const edge = r < 2 || c < 2;
      row.set(base.map(v => (edge ? v >> 1 : v)), 1 + c * 3);
    }
    rows.push(row);
  }
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", hdr), chunk("IDAT", deflateSync(Buffer.concat(rows))), chunk("IEND", Buffer.alloc(0))]);
}

const PAGE = (props: string, extraMarkers = "") => `<!doctype html><html><head><title>Places</title>
<link rel="stylesheet" href="./kit/kit.css"></head><body class="k-section">
<div id="app"></div>
<script type="module">
import { html, render } from './kit/preact.js';
import { MapView } from './kit/map.js';
window.clicked = [];
const markers = [{ lat: 48.8566, lng: 2.3522, label: 'Office', onClick: m => clicked.push(m.label) }, ${extraMarkers}];
render(html\`<\${MapView} center=\${[48.8566, 2.3522]} zoom=\${12} height=\${360} markers=\${markers}
  here=\${{ lat: 48.86, lon: 2.37, accuracy: 400 }} ${props} />\`, document.getElementById('app'));
</script></body></html>`;

/** Answers the shell's tile fetches the daemon cannot make; keeps each tile URL served.
 * With `failFirst`, each tile's first fetch keeps the daemon's `upstream_error`. */
async function fakeTiles(page: Page, failFirst = false, failed: string[] = []): Promise<string[]> {
  const served: string[] = [];
  await page.route("**/net:fetch", async route => {
    const res = await route.fetch();
    if (res.status() !== 502) return route.fulfill({ response: res });
    const { url } = route.request().postDataJSON() as { url: string };
    const m = /\/(\d+)\/(\d+)\/(\d+)\.png$/.exec(url);
    if (!m) return route.fulfill({ response: res });
    if (failFirst && !failed.includes(url)) {
      failed.push(url);
      return route.fulfill({ response: res });
    }
    served.push(url);
    const [z, x, y] = m.slice(1).map(Number);
    return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ url, status: 200, headers: { "content-type": "image/png" }, body_base64: png(z, x, y).toString("base64") }) });
  });
  return served;
}

async function openMap(page: Page, title: string, props: string, capabilities: object, { query = "app=1", extraMarkers = "" } = {}): Promise<Frame> {
  const { artifact } = await api(d.base, d.token, "/api/artifacts", { method: "POST", body: JSON.stringify({ title, capabilities, files: { "index.html": { content: PAGE(props, extraMarkers), encoding: "utf8" }, ...kitFiles } }) }) as { artifact: { id: string } };
  await hostChannel(page);
  await page.goto(`${d.base}/a/${artifact.id}?${query}`);
  const frame = await contentFrame(page, artifact.id, 1);
  await initHost(page);
  return frame;
}

const tiles = (f: Frame) => f.evaluate(() => [...document.querySelectorAll<HTMLImageElement>(".k-map > img.k-map-tile")].map(i => ({ src: i.src, ok: i.complete && i.naturalWidth === 256 })));
const custom = `tiles="https://${HOST}/{z}/{x}/{y}.png" attribution="© Test tiles"`;

test("the map asks for its tile host, then shows tiles as blob: images; it pans, zooms and its markers click", async ({ page }) => {
  const served = await fakeTiles(page);
  const frame = await openMap(page, "Places allow", custom, { net: { hosts: [HOST] } });
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText(HOST);
  expect(await tiles(frame)).toEqual([]);
  await dialog.getByRole("button", { name: "Allow", exact: true }).click();

  await expect.poll(async () => (await tiles(frame)).filter(t => t.ok).length).toBeGreaterThan(3);
  const shown = await tiles(frame);
  expect(shown.every(t => t.src.startsWith("blob:"))).toBe(true);
  expect(served.every(u => u.startsWith(`https://${HOST}/12/`))).toBe(true);
  await expect(frame.getByText("© Test tiles")).toBeVisible();
  await expect(frame.getByRole("img", { name: "You are here" })).toBeVisible();

  // A drag to the left brings in tiles further east.
  const maxX = (urls: string[]) => Math.max(...urls.map(u => Number(/\/12\/(\d+)\//.exec(u)?.[1] ?? 0)));
  const east = maxX(served);
  const before = served.length;
  // (A frame's box is in the page's coordinates.)
  const map = (await frame.locator(".k-map").boundingBox())!;
  const [x, y] = [map.x + map.width / 2, map.y + map.height - 80]; // below the marker
  await page.mouse.move(x, y);
  await page.mouse.down();
  for (let i = 1; i <= 6; i++) await page.mouse.move(x - i * 100, y);
  await page.mouse.up();
  await expect.poll(() => maxX(served)).toBeGreaterThan(east);
  expect(new Set(served.slice(before)).size).toBe(served.length - before);
  await expect.poll(async () => (await tiles(frame)).every(t => t.src.startsWith("blob:"))).toBe(true);

  // The keyboard zooms in a level.
  await frame.locator(".k-map").focus();
  await page.keyboard.press("+");
  await expect.poll(() => served.some(u => u.includes("/13/"))).toBe(true);

  // The marker was dragged out of view; the arrow keys bring it back, and it clicks.
  await frame.getByRole("button", { name: "Zoom out" }).click();
  for (let i = 0; i < 6; i++) await page.keyboard.press("ArrowLeft");
  await frame.getByRole("button", { name: "Office" }).click();
  expect(await frame.evaluate(() => (window as unknown as { clicked: string[] }).clicked)).toEqual(["Office"]);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("without the owner's consent the map says why it is empty and fetches no more", async ({ page }) => {
  const served = await fakeTiles(page);
  const frame = await openMap(page, "Places deny", custom, { net: { hosts: [HOST] } });
  await page.getByRole("dialog").getByRole("button", { name: "Don't allow", exact: true }).click();
  await expect(frame.getByRole("status")).toContainText("Map tiles need permission");
  await expect(frame.getByRole("status")).toContainText(HOST);
  // Outside the inspect view the note stays; the stand-in is the inspect view's.
  await expect(frame.locator(".k-map-standin")).toHaveCount(0);
  await expect(frame.getByText("© Test tiles")).toBeVisible();
  expect([served.length, (await tiles(frame)).length]).toEqual([0, 0]);
});

test("a map without net shows OpenStreetMap's attribution and says it cannot load tiles", async ({ page }) => {
  const frame = await openMap(page, "Places bare", "", {});
  await expect(frame.getByRole("status")).toContainText("Map unavailable");
  await expect(frame.getByText("© OpenStreetMap contributors")).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("a marker's color and icon from data never become markup; a valid color fills the pin", async ({ page }) => {
  const hostile = `'red" /><img src=x onerror="window.pwned=1"><path fill="'`;
  const frame = await openMap(page, "Places hostile", "", {}, {
    extraMarkers: `{ lat: 48.86, lng: 2.34, label: 'Bad', color: ${JSON.stringify(hostile)}, icon: '<img src=x onerror="window.pwned=2">' },`
      + ` { lat: 48.85, lng: 2.36, label: 'Teal', color: 'rgb(1, 128, 128)', icon: 'star' }`,
  });
  await expect(frame.getByRole("button", { name: "Bad" })).toBeVisible();
  const pins = await frame.evaluate(() => [...document.querySelectorAll(".k-map-pin")].map(b => ({
    label: b.getAttribute("aria-label"),
    imgs: b.querySelectorAll("img").length,
    paths: b.querySelectorAll("span:first-child path").length,
    fill: getComputedStyle(b.querySelector("span:first-child path")!).fill,
    glyph: b.querySelector(".k-map-pin-glyph")!.innerHTML.includes("<circle") ? "dot" : "icon",
  })));
  expect(pins.find(p => p.label === "Teal")).toMatchObject({ imgs: 0, paths: 1, fill: "rgb(1, 128, 128)", glyph: "icon" });
  expect(pins.find(p => p.label === "Bad")).toMatchObject({ imgs: 0, paths: 1, glyph: "dot" });
  expect(await frame.evaluate(() => (window as unknown as { pwned?: number }).pwned)).toBeUndefined();
});

test("tiles that fail are tried again on a timer, without the map moving", async ({ page }) => {
  const failed: string[] = [];
  const served = await fakeTiles(page, true, failed);
  const frame = await openMap(page, "Places retry", custom, { net: { hosts: [HOST] } });
  await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
  await expect.poll(() => failed.length).toBeGreaterThan(3);
  await expect.poll(async () => (await tiles(frame)).filter(t => t.ok).length).toBeGreaterThan(3);
  expect(served.every(u => failed.includes(u))).toBe(true);
  await expect(frame.getByText("Map tiles did not load; retrying.")).toHaveCount(0);
});

const SHOTS = new URL("../../target/flutter-app/m2/", import.meta.url);

for (const scheme of ["light", "dark"] as const) {
  test(`${scheme}: in the inspect view a map without its grant draws a stand-in grid under its pins, controls and attribution`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    const frame = await openMap(page, `Places preview ${scheme}`, "", { net: { hosts: ["tile.openstreetmap.org"] } }, { query: "app=1&inspect=1" });
    await expect(frame.locator(".k-map.k-map-standin")).toHaveCount(1);
    await expect(frame.getByText("Map preview")).toBeVisible();
    await expect(frame.getByRole("button", { name: "Office" })).toBeVisible();
    await expect(frame.getByRole("button", { name: "Zoom in" })).toBeVisible();
    await expect(frame.getByText("© OpenStreetMap contributors")).toBeVisible();
    await expect(frame.getByRole("status")).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await tiles(frame)).toEqual([]);
    // What the agent's render sees: the inspection's PNG.
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    await host(page, "inspect({ settleMs: 300 })");
    await expect.poll(async () => (await ofType(page, "inspection")).length, { timeout: 20_000 }).toBeGreaterThan(0);
    const r = (await ofType(page, "inspection")).at(-1) as HostMessage & { png?: string; error?: string };
    expect(r.error).toBeUndefined();
    mkdirSync(SHOTS, { recursive: true });
    writeFileSync(new URL(`standin-inspection-${scheme}.png`, SHOTS), Buffer.from(r.png!, "base64"));
    await page.screenshot({ path: new URL(`standin-${scheme}.png`, SHOTS).pathname });
  });
}
