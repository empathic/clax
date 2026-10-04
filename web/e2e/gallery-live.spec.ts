import { test, expect, type Page } from "@playwright/test";
import { publishAs, publishNext, registerSession, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

/** Opens a thread on `aid` as this page's viewer; answers its ID. */
async function comment(page: Page, aid: string, body: string): Promise<string> {
  return page.evaluate(async ([id, text]) => {
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", text); f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: f })).json()).thread.id;
  }, [aid, body] as const);
}

test("a new version updates only its own card, in place, without a full attention fetch", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "gallery-live");
  const a = (await publishAs(d.base, d.token, s.id, "Live roadmap", { "index.html": PAGE })).artifact.id;
  const b = (await publishAs(d.base, d.token, s.id, "Live budget", { "index.html": PAGE })).artifact.id;
  await page.goto(`${d.base}/`);
  await page.evaluate(() => fetch("/api/viewers/me"));
  const tid = await comment(page, a, "Two columns");
  await comment(page, b, "Which quarter?");
  await page.evaluate(id => fetch("/api/viewers/me/seen", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: id, version: 1 }) }), a);
  await publishNext(d.base, d.token, s.id, a, 1, {});

  const gets: string[] = [];
  page.on("request", r => { if (r.method() === "GET") gets.push(new URL(r.url()).pathname + new URL(r.url()).search); });
  await page.reload();
  const cardA = page.locator(".grp.needs .card-wrap", { hasText: "Live roadmap" });
  const cardB = page.locator(".grp.rest .card-wrap", { hasText: "Live budget" });
  await expect(cardA.locator(".chip.new")).toHaveText("v2 new");
  await expect(cardB.locator(".chip.oth")).toHaveText("1 open");
  // The first load, then the stream's `ready`: two full fetches of each.
  const full = (path: string) => gets.filter(u => u === path).length;
  await expect.poll(() => full("/api/viewers/me/attention")).toBe(2);
  await expect.poll(() => full("/api/artifacts")).toBe(2);
  await page.waitForTimeout(300);
  const before = gets.length;
  // Mark both cards' nodes: an update in place keeps them.
  await cardA.evaluate(e => { e.dataset.mark = "a"; });
  await cardB.evaluate(e => { e.dataset.mark = "b"; });

  await publishNext(d.base, d.token, s.id, a, 2, { addresses: [tid] });
  await expect(cardA.locator(".chip.you")).toHaveText("1 addressed in v3");
  await expect(cardA.locator(".chip.new")).toHaveText("v3 new");
  await expect(cardA.locator(".v")).toHaveText("v3");
  await page.waitForTimeout(1500);

  const after = gets.slice(before).filter(u => u.startsWith("/api/"));
  expect(after.filter(u => u === "/api/viewers/me/attention" || u === "/api/artifacts"), "no full refetch").toEqual([]);
  expect(after).toContain(`/api/viewers/me/attention?artifact=${a}`);
  expect(after).toContain(`/api/artifacts?artifact=${a}`);
  expect(after.filter(u => u.includes(b)), "nothing about the other card").toEqual([]);
  await expect(page.locator(".card-wrap[data-mark=a]")).toHaveCount(1);
  await expect(page.locator(".card-wrap[data-mark=a] .v")).toHaveText("v3");
  await expect(page.locator(".card-wrap[data-mark=b] .chip.oth")).toHaveText("1 open");
  await expect(page.locator(".card-wrap[data-mark=b] .v")).toHaveText("v1");
});
