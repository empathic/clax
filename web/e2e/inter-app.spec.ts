// One app reads another's documents in the Clax app's view (`/a/<id>?app=1`,
// a phone's screen, no chrome): the viewer taps something in app A that
// reads app B's data, the shell asks the owner once, and after Allow the data
// shows, and stays readable without asking after a reload. A share in B's
// declaration is read without asking. The refusals, the live follow and the
// revoke are web/e2e/db-artifact.spec.ts's.
import type { Frame, Page } from "@playwright/test";
import { hostChannel, initHost, noFrames, ofType } from "./app-host";
import { test, expect, type Daemon, api, contentFrame, publishWith } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const SOURCE = `<!doctype html><html><head><title>Runs</title></head><body><p>runs</p></body></html>`;

/** App A: Show reads B's `runs/r1` and shows its distance, or the error. */
const reader = (b: string) => `<!doctype html><html><head><title>Dashboard</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:24px}button{font:inherit;padding:12px 24px}</style></head>
<body><main><button id="show">Show runs</button><p id="out">idle</p></main>
<script>
(async () => {
  const db = await claude.use("db");
  document.getElementById("show").onclick = () => db.artifact("${b}").doc("runs/r1").get().then(
    s => { document.getElementById("out").textContent = s.data().km + " km"; },
    e => { document.getElementById("out").textContent = e.code; });
  window.ready = true;
})();
</script></body></html>`;

/** Publishes B with one run and A declaring the read of B; `share` adds a
 * share of B's documents with A to B's declaration. */
async function apps(tag: string, share = false) {
  const b = (await publishWith(d.base, d.token, `Runs ${tag}`, SOURCE, { db: {} })).artifact.id;
  await api(d.base, d.token, `/api/artifacts/${b}/docs/runs/r1`, { method: "PUT", body: JSON.stringify({ data: { km: 5 }, lww: true }) });
  const a = (await publishWith(d.base, d.token, `Dashboard ${tag}`, reader(b), { db: { read: [b] } })).artifact.id;
  if (share) await api(d.base, d.token, `/api/artifacts/${b}`, { method: "PATCH", body: JSON.stringify({ capabilities: { db: { share: { read: [a] } } } }) });
  return { a, b };
}

/** Waits for A's frame in the app's view, as loaded now, to be ready. */
async function appFrame(page: Page, a: string): Promise<Frame> {
  const f = await contentFrame(page, a, 1);
  await initHost(page);
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await expect.poll(() => f.evaluate(() => (window as unknown as { ready?: boolean }).ready === true)).toBe(true);
  return f;
}

/** Opens A as the Clax app does, in `mode`. */
async function openApp(page: Page, a: string, mode: "subdomain" | "sandbox"): Promise<Frame> {
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  await hostChannel(page);
  await page.goto(`${d.base}/a/${a}?app=1`);
  return appFrame(page, a);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a tap that reads another app asks once; after Allow the data shows, and a reload reads it without asking`, async ({ page }) => {
    const { a, b } = await apps(`allow ${mode}`);
    const f = await openApp(page, a, mode);
    await f.locator("#show").tap();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText(`Let “Dashboard allow ${mode}” read “Runs allow ${mode}”'s data?`);
    // The app's view has no chrome, but the question shows on the phone's
    // screen and its answers can be tapped.
    const allow = dialog.getByRole("button", { name: "Allow", exact: true });
    await expect(allow).toBeInViewport();
    await allow.tap();
    await expect(f.locator("#out")).toHaveText("5 km");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const grants = await api(d.base, d.token, `/api/artifacts/${a}/db-grants`) as { grants: { source_id: string }[] };
    expect(grants.grants.map(g => g.source_id)).toEqual([b]);

    await page.reload();
    const g = await appFrame(page, a);
    await g.locator("#show").tap();
    await expect(g.locator("#out")).toHaveText("5 km");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
}

test("a share in the other app's declaration is read without asking", async ({ page }) => {
  const { a } = await apps("share", true);
  const f = await openApp(page, a, "subdomain");
  await f.locator("#show").tap();
  await expect(f.locator("#out")).toHaveText("5 km");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const grants = await api(d.base, d.token, `/api/artifacts/${a}/db-grants`) as { grants: unknown[] };
  expect(grants.grants).toEqual([]);
});

test("with no animation frames (the macOS app's WebView), the consent dialog's Allow still arms and the read succeeds", async ({ page }) => {
  await noFrames(page);
  const { a } = await apps("no frames");
  const f = await openApp(page, a, "subdomain");
  await f.locator("#show").tap();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Let “Dashboard no frames” read “Runs no frames”'s data?");
  const allow = dialog.getByRole("button", { name: "Allow", exact: true });
  await expect(allow).toBeEnabled();
  await allow.tap();
  await expect(f.locator("#out")).toHaveText("5 km");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
