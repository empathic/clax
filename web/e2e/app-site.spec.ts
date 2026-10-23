// A page's own site API in the Clax app (./app-scheme.ts plays the app's
// side against an embed daemon): the app's scheme answers reads alone, so a
// page's writes go by its shell, which relays them to the page's own site
// with that site's session and origin.
import type { Frame, Page } from "@playwright/test";
import { hostChannel } from "./app-host";
import { SHELL, content, startEmbedDaemon } from "./app-scheme";
import { test as base, expect, type Daemon, publishWith } from "./fixtures";

const test = base.extend<object, { embed: Daemon }>({
  // oxlint-disable-next-line no-empty-pattern
  embed: [async ({}, use) => {
    const d = await startEmbedDaemon();
    await use(d);
    await d.stop();
  }, { scope: "worker", timeout: 30_000 }],
});

let d: Daemon;
test.beforeEach(({ embed }) => { d = embed; });

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The content frame showing version 1 of `id`, from its app origin, once parsed. */
async function appFrame(page: Page, id: string): Promise<Frame> {
  const url = new RegExp(`^${escape(content(id))}/_clax/v/1/`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 10_000 }).toBe(true);
  const frame = page.frame({ url })!;
  await frame.waitForLoadState("domcontentloaded");
  return frame;
}

/** A page that writes a document, reads it back, updates, deletes and
 * lists, and says what it saw in #out. */
const NOTES = `<!doctype html><html><head><title>Notes</title></head><body><p id="out">waiting</p>
<script>(async () => {
  const out = document.getElementById("out");
  try {
    const db = await claude.use("db");
    if (!db) { out.textContent = "no db"; return; }
    const n1 = db.doc("notes/n1");
    await n1.set({ text: "hello" });
    const one = (await n1.get()).data();
    await n1.update({ more: 1 });
    const two = (await n1.get()).data();
    const n2 = db.doc("notes/n2");
    await n2.set({ text: "bye" });
    await n2.delete();
    const all = await db.collection("notes").get();
    out.textContent = JSON.stringify({ one, two, ids: all.docs.map(d => d.id) });
  } catch (e) { out.textContent = "error " + (e.code || "") + " " + e.message; }
})();</script></body></html>`;

test("a page in the app writes its documents through its shell and reads them back from its site", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Site notes", NOTES, { db: {} });
  const id = artifact.id;
  const h = await hostChannel(page, { relay: d });
  await page.goto(`${SHELL}/a/${id}?app=1`);
  const frame = await appFrame(page, id);
  await expect(frame.locator("#out")).toHaveText(JSON.stringify({ one: { text: "hello" }, two: { more: 1, text: "hello" }, ids: ["n1"] }));
  // Each write went to the app as a site call for this artifact alone; no
  // write reached the scheme, which answers reads only.
  const writes = h.calls.filter(c => c.site !== undefined);
  expect(writes.map(c => [c.method, c.path.split("?")[0]])).toEqual([
    ["PUT", "/_clax/api/docs/notes/n1"],
    ["PATCH", "/_clax/api/docs/notes/n1"],
    ["PUT", "/_clax/api/docs/notes/n2"],
    ["DELETE", "/_clax/api/docs/notes/n2"],
  ]);
  expect(writes.every(c => c.site === id && c.answered?.status === 200)).toBe(true);
  expect(h.scheme.filter(s => s.status === 405)).toEqual([]);
  // The owner reads what the page wrote, from the same store.
  const res = await fetch(`${d.base}/api/artifacts/${id}/docs/notes/n1`, { headers: { authorization: `Bearer ${d.token}` } });
  expect((await res.json()).doc.data).toEqual({ text: "hello", more: 1 });
});

test("the scheme still refuses a page's own write: only its shell's relay writes", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Site bounds", "<!doctype html><title>B</title><p>b</p>", { db: {} });
  const id = artifact.id;
  const h = await hostChannel(page, { relay: d });
  await page.goto(`${SHELL}/a/${id}?app=1`);
  const frame = await appFrame(page, id);
  const direct = await frame.evaluate(async () => (await fetch("/_clax/api/docs/notes/x", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ data: {} }) })).status);
  expect(direct).toBe(405);
  expect(h.calls.filter(c => c.site !== undefined)).toEqual([]);
  const res = await fetch(`${d.base}/api/artifacts/${id}/docs/notes/x`, { headers: { authorization: `Bearer ${d.token}` } });
  expect(res.status).toBe(404);
});
