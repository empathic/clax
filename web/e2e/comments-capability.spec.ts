import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { contentFrame, openArtifact, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const BOARD = readFileSync(new URL("./pages/board.html", import.meta.url), "utf8");

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a page button opens the composer anchored on its element`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Board ${mode}`, BOARD, { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await f.locator(".comment").click();
    await expect(f.locator("#status")).toHaveText(JSON.stringify({ opened: true }));
    const composer = page.locator(".composer");
    await expect(composer.locator(".composer-quote")).toContainText("Quarterly goals");
    await composer.locator("textarea").fill("Split this card in two.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    const card = page.locator(".section-open .thread-card").first();
    await expect(card).toContainText("Split this card in two.");
    const threads = await (await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`)).json();
    expect(threads.threads[0].anchor.selector).toBe("#goals");
    await f.locator(".note").click();
    await expect(f.locator("#status")).toHaveText("not_granted");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test(`${mode}: create asks once, then posts as the viewer`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Notes ${mode}`, BOARD, { comments: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await f.locator(".note").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect(f.locator("#status")).toHaveText("created string");
    await f.locator(".note").click();
    await expect(f.locator("#status")).toHaveText("created string");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".section-open .thread-card")).toHaveCount(2);

    // A second view of the same viewer, without the token (a LAN view): it
    // follows deletions through the event stream, and, unnamed, may not delete.
    const other = await page.context().newPage();
    const g = await openArtifact(other, d.base, artifact.id, 1, "sandbox", { lan: true });
    await expect(other.locator(".section-open .thread-card")).toHaveCount(2);
    const tid = await f.evaluate(async () => {
      const c = await (window as any).claude.use("comments");
      const r = await c.create({ anchor: await c.anchorFor(document.querySelector("h2")), text: "Temporary note." });
      await c.resolve(r.threadId, true);
      return r.threadId as string;
    });
    await expect(page.locator(".thread-card", { hasText: "Temporary note." })).toHaveCount(1);
    await expect(other.locator(".section-open .thread-card")).toHaveCount(2);
    const refused = await g.evaluate(async id => {
      const c = await (window as any).claude.use("comments");
      return c.delete(id).then(() => "deleted", (e: { code: string }) => e.code);
    }, tid);
    expect(refused).toBe("forbidden");
    await f.evaluate(async id => {
      const c = await (window as any).claude.use("comments");
      await c.resolve(id, false);
    }, tid);
    await expect(other.locator(".section-open .thread-card")).toHaveCount(3);
    await f.evaluate(async id => {
      const c = await (window as any).claude.use("comments");
      await c.delete(id);
    }, tid);
    await expect(page.locator(".thread-card", { hasText: "Temporary note." })).toHaveCount(0);
    await expect(other.locator(".section-open .thread-card")).toHaveCount(2);
    expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/threads/${tid}`)).status).toBe(404);
    await other.close();
  });

  test(`${mode}: a custom anchor thread survives a republish`, async ({ page }) => {
    const caps = { comments: { customAnchors: true } };
    const { artifact } = await publishWith(d.base, d.token, `Canvas ${mode}`, BOARD, caps);
    let f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect(f.locator("#status")).toHaveText("mode true");
    await expect(f.locator("artifax-overlay .o")).toBeHidden();
    await f.locator("#canvas").click({ position: { x: 50, y: 50 } });
    const composer = page.locator(".composer");
    await expect(composer.locator(".composer-quote")).toContainText("Red square");
    await composer.locator("textarea").fill("Make it blue.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    await expect(page.locator("button.thread-pin")).toHaveCount(1);
    // The pin follows the page's placement when the frame scrolls.
    const top = async () => (await page.locator("button.thread-pin").boundingBox())!.y;
    const before = await top();
    await f.evaluate(() => window.scrollBy(0, 40));
    await expect.poll(top).toBeLessThan(before - 30);
    await f.evaluate(() => window.scrollTo(0, 0));
    await expect.poll(top).toBeGreaterThan(before - 5);
    const stored = await (await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`)).json();
    expect(stored.threads[0].anchor).toMatchObject({ kind: "custom", custom_name: "shape-red" });
    const res = await fetch(`${d.base}/api/artifacts/${artifact.id}/versions`, {
      method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
      body: JSON.stringify({ if_version: 1, files: { "index.html": { content: BOARD, encoding: "utf8" } } }),
    });
    expect(res.status).toBe(201);
    await page.locator(".banner").getByRole("button", { name: "Reload" }).click();
    f = await contentFrame(page, artifact.id, 2);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect(f.locator("#status")).toHaveText("mode true");
    await expect(page.locator("button.thread-pin")).toHaveCount(1);
  });
}
