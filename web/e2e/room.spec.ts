import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { openArtifact, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = readFileSync(new URL("./pages/room.html", import.meta.url), "utf8");
const CAPS = { room: { topics: { reaction: "interact" } } };

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: two tabs see each other's presence, events, and named rooms`, async ({ context }) => {
    const { artifact } = await publishWith(d.base, d.token, `Room ${mode}`, PAGE, CAPS);
    const a = await context.newPage();
    const b = await context.newPage();
    const fa = await openArtifact(a, d.base, artifact.id, 1, mode);
    const fb = await openArtifact(b, d.base, artifact.id, 1, mode);
    await expect(fa.locator("#status")).toHaveText("connected");
    await expect(fb.locator("#status")).toHaveText("connected");
    await expect(fa.locator("#count")).toHaveText("2");
    await expect(fb.locator("#count")).toHaveText("2");

    await fa.locator("#pick-b").click();
    await expect(fa.locator("#peers")).toContainText("this tab: B");
    await expect(fb.locator("#peers")).toContainText("your other tab: B");

    await fb.locator("#wave").click();
    await expect(fa.locator("#log")).toContainText("wave from your other tab");
    await expect(fb.locator("#log")).toContainText("wave from this tab");

    await fa.locator("#join").click();
    await expect(fa.locator("#table-count")).toHaveText("1");
    await fb.locator("#join").click();
    await expect(fa.locator("#table-count")).toHaveText("2");
    await expect(fb.locator("#table-count")).toHaveText("2");
    await fa.locator("#leave").click();
    await expect(fb.locator("#table-count")).toHaveText("1");

    await b.close();
    await expect(fa.locator("#count")).toHaveText("1");
  });

  test(`${mode}: a reload is a new peer, and the old one leaves`, async ({ context }) => {
    const { artifact } = await publishWith(d.base, d.token, `Room reload ${mode}`, PAGE, CAPS);
    const a = await context.newPage();
    const b = await context.newPage();
    const fa = await openArtifact(a, d.base, artifact.id, 1, mode);
    await openArtifact(b, d.base, artifact.id, 1, mode);
    await expect(fa.locator("#count")).toHaveText("2");
    const before = await fa.locator("#peers li").evaluateAll(els => els.map(e => (e as HTMLElement).dataset.peer));
    await b.reload();
    await expect.poll(async () => {
      const now = await fa.locator("#peers li").evaluateAll(els => els.map(e => (e as HTMLElement).dataset.peer));
      return now.length === 2 && now.some(p => !before.includes(p));
    }, { timeout: 15_000 }).toBe(true);
  });
}

/** A foreign document that greets nobody and keeps every message it receives. */
const RECORDER = "<!doctype html><title>Elsewhere</title><script>window.got = []; addEventListener(\"message\", e => window.got.push(e.data));</script><p>elsewhere</p>";

test("sandbox: a document that leaves takes its peer with it, and nothing of the room reaches the next one", async ({ context }) => {
  const { artifact } = await publishWith(d.base, d.token, "Room leave", PAGE, CAPS);
  const a = await context.newPage();
  const b = await context.newPage();
  await a.route("http://foreign.test/**", r => r.fulfill({ contentType: "text/html", body: RECORDER }));
  const fa = await openArtifact(a, d.base, artifact.id, 1, "sandbox");
  const fb = await openArtifact(b, d.base, artifact.id, 1, "sandbox");
  await expect(fa.locator("#count")).toHaveText("2");
  await expect(fb.locator("#count")).toHaveText("2");
  // The page sends its own frame elsewhere; B stays quiet until A's peer has left.
  await fa.evaluate(() => { location.href = "http://foreign.test/elsewhere"; });
  await expect(fb.locator("#count")).toHaveText("1");
  await fb.locator("#pick-b").click();
  await fb.locator("#wave").click();
  await expect(fb.locator("#log")).toContainText("wave from this tab");
  await expect.poll(() => a.frame({ url: /foreign\.test/ }) !== null).toBe(true);
  const rec = a.frame({ url: /foreign\.test/ })!;
  // Long enough for B's presence and wave to have reached A's shell, were its socket still open.
  await a.waitForTimeout(1000);
  const got = await rec.evaluate(() => (window as unknown as { got: { type?: string }[] }).got);
  expect(got.filter(m => m && typeof m === "object" && String(m.type).startsWith("clax:"))).toEqual([]);
});

test("lan: naming yourself reconnects the room at the new level, so an interact topic opens", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Room lan", PAGE, CAPS);
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await expect(f.locator("#status")).toHaveText("connected");
  await f.locator("#wave").click();
  await expect(f.locator("#log")).toContainText("refused: not_permitted");
  // The name field's place is Echo's (align with Echo at merge): on main it is in the top bar.
  await page.getByLabel("Your name").fill("Ben");
  await page.getByLabel("Your name").press("Enter");
  await expect(async () => {
    await f.locator("#wave").click();
    await expect(f.locator("#log")).toContainText("wave from this tab", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(f.locator("#count")).toHaveText("1");
});
