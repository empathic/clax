import { readFileSync } from "node:fs";
import { test, expect, type Frame, type Page } from "@playwright/test";
import { contentFrame, openArtifact, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const pageHtml = (name: string) => readFileSync(new URL(`./pages/${name}`, import.meta.url), "utf8");

async function current(id: string): Promise<number> {
  return (await (await fetch(`${d.base}/api/artifacts/${id}`)).json()).artifact.current_version;
}

/** The frame showing `file` of version `n`. */
async function fileFrame(page: Page, id: string, n: number, file: string): Promise<Frame> {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)${file.replace(/[.]/g, "\\.")}$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 15_000 }).toBe(true);
  return page.frame({ url })!;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a vote republishes the page and every open view reloads to it`, async ({ browser }) => {
    const { artifact } = await publishWith(d.base, d.token, `Poll ${mode}`, pageHtml("poll.html"), { artifact: {} });
    const ctx = await browser.newContext();
    const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
    const a = await openArtifact(pa, d.base, artifact.id, 1, mode);
    await openArtifact(pb, d.base, artifact.id, 1, mode);
    await expect(a.locator("#count")).toHaveText("0");
    await a.locator("#vote").click();
    for (const p of [pa, pb]) {
      const f = await contentFrame(p, artifact.id, 2);
      await expect(f.locator("#count")).toHaveText("1");
      await expect(p.locator(".banner")).toHaveCount(0);
    }
    expect(await current(artifact.id)).toBe(2);
    const stored = await (await fetch(`${d.base}/api/artifacts/${artifact.id}/versions/2/files/index.html`)).text();
    expect(stored.startsWith("<!doctype html>")).toBe(true);
    expect(stored).not.toContain("/_artifax/bridge.js");
    const served = await (await fetch(`${d.base}/c/${artifact.id}/v/2/`)).text();
    expect(served.match(/\/_artifax\/bridge\.js/g)?.length).toBe(1);
    expect(served.startsWith("<!doctype html>\n<html"), "served as the full document, not wrapped again").toBe(true);
    await ctx.close();
  });

  test(`${mode}: concurrent publishes: one wins, the other gets conflict`, async ({ browser }) => {
    const { artifact } = await publishWith(d.base, d.token, `Race ${mode}`, pageHtml("poll.html"), { artifact: {} });
    const ctx = await browser.newContext();
    const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
    // What each page hears back for its publish, from inside the frame.
    const heard: unknown[] = [];
    for (const p of [pa, pb]) {
      await p.exposeFunction("artifaxHeard", (m: unknown) => { heard.push(m); });
      await p.addInitScript(() => {
        addEventListener("message", e => { if (e.data?.type === "artifax:call-result") (window as unknown as { artifaxHeard(m: unknown): void }).artifaxHeard(e.data); });
      });
    }
    const a = await openArtifact(pa, d.base, artifact.id, 1, mode);
    const b = await openArtifact(pb, d.base, artifact.id, 1, mode);
    const isPublish = (r: import("@playwright/test").Response) => r.url().endsWith(`/api/artifacts/${artifact.id}/versions`) && r.request().method() === "POST";
    // Both publishes are held until both are in flight, so they race at the daemon.
    let arrived = 0;
    let bothArrived!: () => void;
    const both = new Promise<void>(r => { bothArrived = r; });
    for (const p of [pa, pb]) {
      await p.route(`**/api/artifacts/${artifact.id}/versions`, async route => {
        if (route.request().method() !== "POST") return route.continue();
        if (++arrived === 2) bothArrived();
        await both;
        await route.continue();
      });
    }
    const [ra, rb] = await Promise.all([pa.waitForResponse(isPublish), pb.waitForResponse(isPublish), a.locator("#vote").click(), b.locator("#vote").click()]);
    expect([ra.status(), rb.status()].sort()).toEqual([201, 409]);
    await expect.poll(() => heard.length).toBe(2);
    expect(heard).toEqual(expect.arrayContaining([
      expect.objectContaining({ ok: true, value: { version: "2" } }),
      expect.objectContaining({ ok: false, error: expect.objectContaining({ code: "conflict", live: "2" }) }),
    ]));
    for (const p of [pa, pb]) await expect((await contentFrame(p, artifact.id, 2)).locator("#count")).toHaveText("1");
    expect(await current(artifact.id)).toBe(2);
    await ctx.close();
  });

  test(`${mode}: a sub page's vote replaces that page only, and every view stays on it`, async ({ browser }) => {
    const index = "<!doctype html><html><body><h1>Home</h1><a href=\"votes/poll.html\">poll</a></body></html>";
    const res = await fetch(`${d.base}/api/artifacts`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
      body: JSON.stringify({ title: `Sub poll ${mode}`, capabilities: { artifact: {} }, files: { "index.html": { content: index, encoding: "utf8" }, "votes/poll.html": { content: pageHtml("poll.html"), encoding: "utf8" }, "data.json": { content: "{\"a\":1}", encoding: "utf8" } } }),
    });
    const { artifact } = (await res.json()) as { artifact: { id: string } };
    const ctx = await browser.newContext();
    const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
    for (const p of [pa, pb]) {
      if (mode === "sandbox") await p.addInitScript(() => { try { sessionStorage.setItem("artifax.origin-ok", "0"); } catch { /* storage unavailable */ } });
      await p.goto(`${d.base}/a/${artifact.id}/votes/poll.html`);
    }
    const a = await fileFrame(pa, artifact.id, 1, "votes/poll.html");
    await expect(a.locator("#count")).toHaveText("0");
    await a.locator("#vote").click();
    for (const p of [pa, pb]) {
      await expect((await fileFrame(p, artifact.id, 2, "votes/poll.html")).locator("#count")).toHaveText("1");
      await expect(p).toHaveURL(new RegExp(`/a/${artifact.id}/votes/poll\\.html$`));
      await expect(p.locator(".banner")).toHaveCount(0);
    }
    expect(await current(artifact.id)).toBe(2);
    const file = async (n: number, f: string) => (await fetch(`${d.base}/api/artifacts/${artifact.id}/versions/${n}/files/${f}`)).text();
    expect(await file(2, "index.html")).toBe(index);
    expect(await file(2, "data.json")).toBe("{\"a\":1}");
    expect(await file(2, "votes/poll.html")).toContain("data-votes=\"1\"");
    const v2 = (await (await fetch(`${d.base}/api/artifacts/${artifact.id}/versions/2`)).json()).version;
    expect(Object.keys(v2.files).sort()).toEqual(["data.json", "index.html", "votes/poll.html"]);
    await ctx.close();
  });
}

test("LAN: a view without the token is read-only", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Poll LAN", pageHtml("poll.html"), { artifact: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await f.locator("#vote").click();
  await expect(f.locator("#status")).toHaveText("This view is read-only.");
  expect(await current(artifact.id)).toBe(1);
});

test("an agent publish still offers the banner instead of reloading", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Poll agent", pageHtml("poll.html"), { artifact: {} });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const res = await fetch(`${d.base}/api/artifacts/${artifact.id}/versions`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
    body: JSON.stringify({ if_version: 1, files: { "index.html": { content: "<p>agent</p>", encoding: "utf8" } } }),
  });
  expect(res.status).toBe(201);
  await expect(page.locator(".banner")).toContainText("v2 published");
  await contentFrame(page, artifact.id, 1);
});

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: downloads asks, saves the file under the sanitized name, and refuses unlisted types`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Export ${mode}`, pageHtml("downloads.html"), { downloads: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await f.locator("#csv").click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("q3 report.csv (23 bytes)");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    const [download] = await Promise.all([page.waitForEvent("download"), dialog.getByRole("button", { name: "Save" }).click()]);
    expect(download.suggestedFilename()).toBe("q3 report.csv");
    expect(readFileSync(await download.path(), "utf8")).toBe("quarter,revenue\nQ3,120\n");
    await expect(f.locator("#status")).toHaveText("saved");
    await f.locator("#csv").click();
    await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();
    await expect(f.locator("#status")).toHaveText("declined");
    await f.locator("#exe").click();
    await expect(f.locator("#status")).toHaveText("rejected_extension");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
}
