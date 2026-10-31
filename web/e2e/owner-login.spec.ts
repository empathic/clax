// A local owner's browser session in a real browser (docs/contract.md,
// "The owner and other viewers", "Browser login", "Get in Clax"): a login
// link opened in a signed-in browser keeps that browser's new session and
// retires the one it held there, never another browser's; and the Get in
// Clax page, whose page load carries the cookie without its proof, loads
// the app's page with the proof (crates/clax-server/src/shell/get_page.rs,
// BARE_SCRIPT), follows its redirect, saves Download .clax, says when a
// download failed, and shows how to open Clax without a proof.
import { type Page } from "@playwright/test";
import { type Daemon, api, expect, publish, test } from "./fixtures";

const MIME = "application/vnd.empathic.clax+zip";

/** The status `page`'s own `fetch` (with the session's proofs) gets for `GET /api/token`. */
const tokenStatus = (page: Page) => page.evaluate(() => fetch("/api/token").then(r => r.status));

/** The proof keys `page`'s origin keeps. */
const proofKeys = (page: Page) => page.evaluate(() => Object.keys(localStorage).filter(k => k === "clax.owner_proof" || k.startsWith("clax.owner_proof.")));

/** An app on `d`: an artifact titled `title` whose version 1 carries a bundle. */
async function app(d: Daemon, title: string): Promise<string> {
  const { artifact } = await publish(d.base, d.token, title, { "index.html": "<h1>app</h1>" });
  const v = await api(d.base, d.token, `/api/artifacts/${artifact.id}/versions/1`);
  const res = await fetch(`${d.base}/api/artifacts/${artifact.id}/versions/1/bundle`, {
    method: "PUT",
    headers: {
      authorization: `Bearer ${d.token}`,
      "content-type": MIME,
      "clax-bundle-lineage": "l_abcdefghijklmnopqrstuvwxyz",
      "clax-bundle-version": "2",
      "clax-bundle-content-sha256": v.version.content_sha256,
    },
    body: new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0x20, 0x61]),
  });
  expect(res.status).toBe(201);
  return artifact.id;
}

test(`a login link opened in a signed-in browser keeps its new session there and leaves every other browser's live`, async ({ page, browser, freshDaemon: d }) => {
  await page.goto(`${d.base}/`);
  await expect.poll(() => tokenStatus(page)).toBe(200);
  // Twice, as `clax open` and then an agent's `open` would: the second
  // login's page retires the first's session and drops its proof.
  const seen: string[] = [];
  for (let i = 0; i < 2; i++) {
    const { path } = await api(d.base, d.token, "/api/browser-login", { method: "POST", body: JSON.stringify({ next: "/" }) });
    await page.goto(`${d.base}${path}`);
    await page.waitForURL(`${d.base}/`);
    // The login's own proof, under a key of its own, alone.
    await expect.poll(() => proofKeys(page)).toHaveLength(1);
    const [key] = await proofKeys(page);
    expect(key).toMatch(/^clax\.owner_proof\.\d{15}\.[0-9a-f]{16}$/);
    seen.push(key);
    expect(await tokenStatus(page)).toBe(200);
  }
  expect(seen[1]).not.toBe(seen[0]);
  // Another browser, holding the session every test browser shares, is
  // still the owner's.
  const other = await browser.newPage();
  await other.goto(`${d.base}/`);
  expect(await tokenStatus(other)).toBe(200);
  await other.close();
});

test(`the Get in Clax page loads the app's page with the proof, and saves Download .clax from a fetch`, async ({ page, daemon: d }) => {
  const id = await app(d, "Pocket Calculator");
  await page.goto(`${d.base}/a/${id}/get`);
  await expect(page.getByRole("heading", { name: "Pocket Calculator" })).toBeVisible();
  await expect(page).toHaveTitle(/Pocket Calculator/);
  // A button, not a link a middle-click would open without the proof.
  await expect(page.getByRole("link", { name: "Download .clax" })).toHaveCount(0);
  const button = page.getByRole("button", { name: "Download .clax" });
  const download = page.waitForEvent("download");
  await button.click();
  expect((await download).suggestedFilename()).toMatch(/\.clax$/);
  // A failed fetch says so under the button.
  await page.route(`**/api/artifacts/${id}/versions/1/bundle`, r => r.fulfill({ status: 500, body: "{}" }));
  await button.click();
  await expect(page.getByRole("alert")).toHaveText("The download failed (500). Try again.");
});

test(`the Get in Clax page follows the address's own page, and without a proof shows how to open Clax`, async ({ page, browser, daemon: d }) => {
  const { artifact } = await publish(d.base, d.token, "Has its own get", { "index.html": "<p>x</p>", "get/index.html": "<p>its own get page</p>" });
  await page.goto(`${d.base}/a/${artifact.id}/get`);
  await page.waitForURL(`${d.base}/a/${artifact.id}/get/index.html`);
  const id = await app(d, "Signed-out Calculator");
  // A browser holding the owner cookie but no proof in its storage.
  const ctx = await browser.newContext();
  await ctx.addInitScript(() => { try { for (const k of Object.keys(localStorage)) if (k.startsWith("clax.owner_proof")) localStorage.removeItem(k); } catch { /* storage refused */ } });
  const cold = await ctx.newPage();
  await cold.goto(`${d.base}/a/${id}/get`);
  await expect(cold.getByRole("heading", { name: "Open Clax from your terminal" })).toBeVisible();
  await expect(cold.getByText("Signed-out Calculator")).toHaveCount(0);
  await ctx.close();
});
