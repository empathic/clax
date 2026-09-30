import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { reach, contentFrame, openArtifact, publishWith, startDaemon } from "./fixtures";

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
    await reach(page, f.locator(".note"));
    await f.locator(".note").click();
    await expect(f.locator("#status")).toHaveText("not_granted");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test(`${mode}: a page cannot open the composer without the viewer's gesture`, async ({ page }) => {
    const onLoad = `<!doctype html><html><head><title>On load</title></head><body><h2 id="t">Title</h2><p id="status">waiting</p>
<script>(async () => { const c = await claude.use("comments"); document.getElementById("status").textContent = JSON.stringify(await c.openComposer({ element: document.getElementById("t") })); })();</script></body></html>`;
    const { artifact } = await publishWith(d.base, d.token, `On load ${mode}`, onLoad, { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#status")).toHaveText(JSON.stringify({ opened: false }));
    await expect(page.locator(".composer")).toHaveCount(0);
  });

  test(`${mode}: a page polling on a timer cannot ride the viewer's input to the shell`, async ({ page }) => {
    const poller = `<!doctype html><html><head><title>Poller</title></head><body><h2 id="t">Title</h2><p id="status">0</p>
<script>(async () => { const c = await claude.use("comments"); let n = 0; const s = document.getElementById("status");
setInterval(async () => { const r = await c.openComposer({ element: document.getElementById("t") }).catch(e => ({ code: e.code })); n++; s.textContent = n + " " + JSON.stringify(r); }, 150); })();</script></body></html>`;
    const { artifact } = await publishWith(d.base, d.token, `Poller ${mode}`, poller, { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#status")).toContainText("opened");
    // The viewer types in the shell while the page polls.
    const name = page.getByRole("textbox", { name: "Your name" });
    await name.click();
    await name.pressSequentially("Sam", { delay: 120 });
    const seen = Number((await f.locator("#status").textContent())!.split(" ")[0]);
    await expect.poll(async () => Number((await f.locator("#status").textContent())!.split(" ")[0])).toBeGreaterThan(seen + 2);
    await expect(page.locator(".composer")).toHaveCount(0);
  });

  test(`${mode}: create asks once, then posts as the viewer`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Notes ${mode}`, BOARD, { comments: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await f.locator(".note").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect(f.locator("#status")).toHaveText("created string");
    await reach(page, f.locator(".note"));
    await f.locator(".note").click();
    await expect(f.locator("#status")).toHaveText("created string");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".section-open .thread-card")).toHaveCount(2);

    await expect(page.locator(".section-open .thread-card .via-page")).toHaveCount(2);
    const storeIds = async () => ((await (await fetch(`${d.base}/api/artifacts/${artifact.id}/threads?include_resolved=true`)).json()).threads as { id: string }[]).map(t => t.id);

    // The page reads every message the shell posts to it: the thread anchors it
    // is sent carry opaque handles, and neither they nor store IDs let it act.
    await f.evaluate(() => {
      (window as any).seen = [];
      addEventListener("message", e => { if (e.data?.type === "artifax:resolve-anchors") (window as any).seen.push(...e.data.anchors.map((a: { id: string }) => a.id)); });
    });
    const tid = await f.evaluate(async () => {
      const c = await (window as any).claude.use("comments");
      const r = await c.create({ anchor: await c.anchorFor(document.querySelector("h2")), text: "Temporary note." });
      await c.resolve(r.threadId, true);
      return r.threadId as string;
    });
    await expect(page.locator(".thread-card", { hasText: "Temporary note." })).toHaveCount(1);
    const ids = await storeIds();
    expect(ids).toHaveLength(3);
    expect(ids).not.toContain(tid);
    const seen = await f.evaluate(async () => { for (;;) { if ((window as any).seen.length) return (window as any).seen as string[]; await new Promise(r => setTimeout(r, 50)); } });
    for (const id of seen) expect(ids).not.toContain(id);
    const codes = await f.evaluate(async cands => {
      const c = await (window as any).claude.use("comments");
      return Promise.all(cands.map((id: string) => c.delete(id).then(() => "deleted", (e: { code: string }) => e.code)));
    }, [...seen, ...ids]);
    expect(new Set(codes)).toEqual(new Set(["not_found"]));
    expect(await storeIds()).toHaveLength(3);

    // A second view of the same viewer, without the token (a LAN view): it
    // follows changes through the event stream and, unnamed, may not delete
    // even a thread its own page wrote.
    const other = await page.context().newPage();
    const g = await openArtifact(other, d.base, artifact.id, 1, "sandbox", { lan: true });
    await expect(other.locator(".section-open .thread-card")).toHaveCount(2);
    const refused = await g.evaluate(async () => {
      const c = await (window as any).claude.use("comments");
      const r = await c.create({ anchor: await c.anchorFor(document.querySelector("h2")), text: "From the LAN." });
      return c.delete(r.threadId).then(() => "deleted", (e: { code: string }) => e.code);
    });
    expect(refused).toBe("forbidden");
    await expect(other.locator(".section-open .thread-card")).toHaveCount(3);
    await f.evaluate(async id => {
      const c = await (window as any).claude.use("comments");
      await c.resolve(id, false);
    }, tid);
    await expect(other.locator(".section-open .thread-card")).toHaveCount(4);
    const before = await storeIds();
    await f.evaluate(async id => {
      const c = await (window as any).claude.use("comments");
      await c.delete(id);
    }, tid);
    await expect(page.locator(".thread-card", { hasText: "Temporary note." })).toHaveCount(0);
    await expect(other.locator(".section-open .thread-card")).toHaveCount(3);
    const after = await storeIds();
    expect(after).toHaveLength(before.length - 1);
    const gone = before.find(id => !after.includes(id))!;
    expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/threads/${gone}`)).status).toBe(404);
    await other.close();
  });

  test(`${mode}: a custom anchor thread survives a republish`, async ({ page }) => {
    const caps = { comments: { customAnchors: true } };
    const { artifact } = await publishWith(d.base, d.token, `Canvas ${mode}`, BOARD, caps);
    let f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect(f.locator("#status")).toHaveText("mode true");
    // The bridge's own hover outline stands down while the page anchors.
    await f.locator("#goals h2").hover();
    await f.locator("#goals p").hover();
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
    // Out of comment mode the pin stays and keeps following the page.
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect(f.locator("#status")).toHaveText("mode false");
    await f.evaluate(() => window.scrollBy(0, 40));
    await expect.poll(top).toBeLessThan(before - 30);
    await f.evaluate(() => window.scrollTo(0, 0));
    await expect.poll(top).toBeGreaterThan(before - 5);
    const stored = await (await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`)).json();
    expect(stored.threads[0].anchor).toMatchObject({ kind: "custom", custom_name: "shape-red", quote: null });
    expect(stored.threads[0].comments[0].via_page).toBe(false);
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
