import { readFileSync } from "node:fs";
import { type Frame, type Page } from "@playwright/test";
import { test, expect, type Daemon, reach, contentFrame, openArtifact, publishWith, streamLive } from "./fixtures";
import { guestOn, hostedDaemon, ownerOf, shareByLink } from "./hosted-helpers";
import { advance, settle } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const pageHtml = (name: string) => readFileSync(new URL(`./pages/${name}`, import.meta.url), "utf8");

async function current(id: string): Promise<number> {
  return (await (await fetch(`${d.base}/api/artifacts/${id}`)).json()).artifact.current_version;
}

/** The frame showing `file` of version `n`. */
async function fileFrame(page: Page, id: string, n: number, file: string): Promise<Frame> {
  const url = new RegExp(`(${id}\\.localhost:\\d+/_clax/v/${n}/)${file.replace(/[.]/g, "\\.")}$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 15_000 }).toBe(true);
  return page.frame({ url })!;
}

test(`a vote republishes the page and every open view reloads to it`, async ({ browser }) => {
  const { artifact } = await publishWith(d.base, d.token, `Poll`, pageHtml("poll.html"), { artifact: {} });
  const ctx = await browser.newContext();
  const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
  const a = await openArtifact(pa, d.base, artifact.id, 1);
  await openArtifact(pb, d.base, artifact.id, 1);
  for (const p of [pa, pb]) await streamLive(p);
  await expect(a.locator("#count")).toHaveText("0");
  await a.locator("#vote").click();
  for (const p of [pa, pb]) {
    const f = await contentFrame(p, artifact.id, 2);
    await expect(f.locator("#count")).toHaveText("1");
    await expect(p.locator(".topbar button.reload")).toHaveCount(0);
  }
  expect(await current(artifact.id)).toBe(2);
  const stored = await (await fetch(`${d.base}/api/artifacts/${artifact.id}/versions/2/files/index.html`)).text();
  expect(stored.startsWith("<!doctype html>")).toBe(true);
  expect(stored).not.toContain("/_clax/bridge.js");
  // As served on the site, read inside the frame with its site session.
  const v2 = await contentFrame(pa, artifact.id, 2);
  const served = await v2.evaluate(() => fetch(location.href.split("#")[0]).then(r => r.text()));
  expect(served.match(/\/_clax\/bridge\.js/g)?.length).toBe(1);
  // The bridge goes right after the doctype and its whitespace; the page
  // follows as stored.
  expect(served, "served as the full document, not wrapped again").toMatch(/^<!doctype html>\n<script src="\/_clax\/bridge\.js[^>]*><\/script><html/);
  await ctx.close();
});

test(`concurrent publishes: one wins, the other gets conflict`, async ({ browser }) => {
  const { artifact } = await publishWith(d.base, d.token, `Race`, pageHtml("poll.html"), { artifact: {} });
  const ctx = await browser.newContext();
  const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
  // What each page hears back for its publish, from inside the frame.
  const heard: unknown[] = [];
  for (const p of [pa, pb]) {
    await p.exposeFunction("claxHeard", (m: unknown) => { heard.push(m); });
    await p.addInitScript(() => {
      addEventListener("message", e => { if (e.data?.type === "clax:call-result") (window as unknown as { claxHeard(m: unknown): void }).claxHeard(e.data); });
    });
  }
  const a = await openArtifact(pa, d.base, artifact.id, 1);
  const b = await openArtifact(pb, d.base, artifact.id, 1);
  for (const p of [pa, pb]) await streamLive(p);
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

test(`a sub page's vote replaces that page only, and every view stays on it`, async ({ browser }) => {
  const index = "<!doctype html><html><body><h1>Home</h1><a href=\"votes/poll.html\">poll</a></body></html>";
  const res = await fetch(`${d.base}/api/artifacts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
    body: JSON.stringify({ title: `Sub poll`, capabilities: { artifact: {} }, files: { "index.html": { content: index, encoding: "utf8" }, "votes/poll.html": { content: pageHtml("poll.html"), encoding: "utf8" }, "data.json": { content: "{\"a\":1}", encoding: "utf8" } } }),
  });
  const { artifact } = (await res.json()) as { artifact: { id: string } };
  const ctx = await browser.newContext();
  const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
  for (const p of [pa, pb]) {
    await p.goto(`${d.base}/a/${artifact.id}/votes/poll.html`);
  }
  const a = await fileFrame(pa, artifact.id, 1, "votes/poll.html");
  await fileFrame(pb, artifact.id, 1, "votes/poll.html");
  for (const p of [pa, pb]) await streamLive(p);
  await expect(a.locator("#count")).toHaveText("0");
  await a.locator("#vote").click();
  for (const p of [pa, pb]) {
    await expect((await fileFrame(p, artifact.id, 2, "votes/poll.html")).locator("#count")).toHaveText("1");
    await expect(p).toHaveURL(new RegExp(`/a/${artifact.id}/votes/poll\\.html$`));
    await expect(p.locator(".topbar button.reload")).toHaveCount(0);
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

/** Clicks the poll's vote button in `f`, 2 s (of the shell's clock) after the view showed it. */
async function voteSoon(page: Page, f: Frame) {
  await advance(page, 2_000);
  await f.locator("#vote").click();
}

// A shell load is not input to the shell: a vote soon after one publishes.
test(`a vote 2 s after opening the page publishes`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Vote soon`, pageHtml("poll.html"), { artifact: {} });
  await voteSoon(page, await openArtifact(page, d.base, artifact.id, 1));
  await expect((await contentFrame(page, artifact.id, 2)).locator("#count")).toHaveText("1");
  expect(await current(artifact.id)).toBe(2);
});

test(`a vote 2 s after the reload the viewer's own vote caused publishes`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Vote again`, pageHtml("poll.html"), { artifact: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  await advance(page, 6_000);
  await f.locator("#vote").click();
  await voteSoon(page, await contentFrame(page, artifact.id, 2));
  await expect((await contentFrame(page, artifact.id, 3)).locator("#count")).toHaveText("2");
  expect(await current(artifact.id)).toBe(3);
});

test(`a second viewer's vote 2 s after another viewer's vote reloaded it publishes`, async ({ browser }) => {
  const { artifact } = await publishWith(d.base, d.token, `Vote two`, pageHtml("poll.html"), { artifact: {} });
  const ctx = await browser.newContext();
  const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
  const a = await openArtifact(pa, d.base, artifact.id, 1);
  await openArtifact(pb, d.base, artifact.id, 1);
  for (const p of [pa, pb]) await streamLive(p);
  await advance(pa, 6_000);
  await a.locator("#vote").click();
  await voteSoon(pb, await contentFrame(pb, artifact.id, 2));
  await expect((await contentFrame(pb, artifact.id, 3)).locator("#count")).toHaveText("2");
  expect(await current(artifact.id)).toBe(3);
  await ctx.close();
});

test(`a click inside the frame gives the shell window sticky activation`, async ({ page }) => {
  // Playwright's evaluate runs as a user gesture, so the shell samples its own
  // activation, reports its first true sample through a binding (which is no
  // gesture), and the click is a raw mouse event inside the frame's box.
  const act: { seen: { falseSamples: number; firstActive: number } | null } = { seen: null };
  await page.exposeFunction("claxActivated", (falseSamples: number, firstActive: number) => { act.seen ??= { falseSamples, firstActive }; });
  await page.addInitScript(() => {
    if (window !== window.top) { addEventListener("click", () => { (window as unknown as { clicked: boolean }).clicked = true; }); return; }
    let falseSamples = 0;
    const tick = () => {
      if (!navigator.userActivation.hasBeenActive) { falseSamples++; setTimeout(tick, 10); return; }
      (window as unknown as { claxActivated: (n: number, t: number) => void }).claxActivated(falseSamples, Date.now());
    };
    tick();
  });
  const { artifact } = await publishWith(d.base, d.token, `Activation`, pageHtml("poll.html"), { artifact: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  await f.waitForLoadState();
  await settle(page);
  expect(act.seen, "the shell is not active before the click").toBeNull();
  const before = Date.now();
  await page.mouse.click(300, 400);
  await expect.poll(() => act.seen, { timeout: 20_000 }).not.toBeNull();
  const seen = act.seen!;
  console.log(`shell activation samples before the frame click: ${seen.falseSamples} x false; first true ${seen.firstActive - before} ms after the click started`);
  expect(await f.evaluate(() => (window as unknown as { clicked?: boolean }).clicked), "the click landed inside the frame").toBe(true);
  expect(seen.falseSamples).toBeGreaterThanOrEqual(1);
  expect(seen.firstActive).toBeGreaterThanOrEqual(before);
});

test(`a page that publishes on load publishes nothing`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `On load`, pageHtml("publish-on-load.html"), { artifact: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  await expect(f.locator("#status")).toHaveText("rate_limited: publish from the viewer's own input in the page, never on load or a timer");
  await settle(page);
  expect(await current(artifact.id)).toBe(1);
});

test(`twelve publishes from one click: one lands, the rest are rate_limited`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Burst`, pageHtml("publish-burst.html"), { artifact: {} });
  const heard: { ok: boolean; error?: { code: string } }[] = [];
  await page.exposeFunction("claxHeard", (m: { ok: boolean; error?: { code: string } }) => { heard.push(m); });
  await page.addInitScript(() => {
    addEventListener("message", e => { if (e.data?.type === "clax:call-result") (window as unknown as { claxHeard(m: unknown): void }).claxHeard(e.data); });
  });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  await f.locator("#burst").click();
  await expect.poll(() => heard.length).toBe(12);
  expect(heard.filter(m => m.ok)).toHaveLength(1);
  expect(heard.filter(m => !m.ok).map(m => m.error?.code)).toEqual(Array(11).fill("rate_limited"));
  await expect((await contentFrame(page, artifact.id, 2)).locator("#done")).toHaveText("republished");
  expect(await current(artifact.id)).toBe(2);
});

// A viewer who is not the owner: a hosted daemon's guest on a page shared by
// link (a local daemon frames pages for the owner alone).
test("a guest's view, without the token, is read-only", async ({ browser }) => {
  const { d: h, origin } = await hostedDaemon();
  try {
    const { artifact } = await publishWith(h.base, h.token, "Poll guest", pageHtml("poll.html"), { artifact: {} });
    await shareByLink(await ownerOf(browser, origin), artifact.id);
    const { frame: f } = await guestOn(browser, origin, artifact.id);
    await f.locator("#vote").click();
    await expect(f.locator("#status")).toHaveText("This view is read-only.");
    const live = await (await fetch(`${h.base}/api/artifacts/${artifact.id}`, { headers: { authorization: `Bearer ${h.token}` } })).json();
    expect(live.artifact.current_version).toBe(1);
  } finally {
    await h.stop();
  }
});

test("an agent publish still offers Reload in the top bar instead of reloading", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Poll agent", pageHtml("poll.html"), { artifact: {} });
  await openArtifact(page, d.base, artifact.id, 1);
  const res = await fetch(`${d.base}/api/artifacts/${artifact.id}/versions`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
    body: JSON.stringify({ if_version: 1, files: { "index.html": { content: "<p>agent</p>", encoding: "utf8" } } }),
  });
  expect(res.status).toBe(201);
  await expect(page.locator(".who .sum b.l1")).toHaveText("v2 published");
  await expect(page.locator(".topbar button.reload")).toBeVisible();
  await contentFrame(page, artifact.id, 1);
});

test(`downloads asks, saves the file under the sanitized name, and refuses unlisted types`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Export`, pageHtml("downloads.html"), { downloads: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  await f.locator("#csv").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("q3 report.csv (23 bytes)");
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  const [download] = await Promise.all([page.waitForEvent("download"), dialog.getByRole("button", { name: "Save" }).click()]);
  expect(download.suggestedFilename()).toBe("q3 report.csv");
  expect(readFileSync(await download.path(), "utf8")).toBe("quarter,revenue\nQ3,120\n");
  await expect(f.locator("#status")).toHaveText("saved");
  await reach(page, f.locator("#csv"));
  await f.locator("#csv").click();
  await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();
  await expect(f.locator("#status")).toHaveText("declined");
  await reach(page, f.locator("#exe"));
  await f.locator("#exe").click();
  await expect(f.locator("#status")).toHaveText("rejected_extension");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
