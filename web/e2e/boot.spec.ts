import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect } from "@playwright/test";
import { FRAME_SANDBOX } from "../shell/src/view/frame-host";
import { type FrameMode, contentFrame, openArtifact, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = `<h1>Top</h1><p style="height:3000px">long</p><h2 id="part-2">Part 2</h2>`;
const sandboxed = () => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } };

for (const mode of ["subdomain", "sandbox"] as FrameMode[]) {
  test(`a second visit gets the frame in the HTML, opens at the link's fragment, and comments at once (${mode})`, async ({ browser }) => {
    const { artifact } = await publish(d.base, d.token, "Boot", { "index.html": PAGE });
    const ctx = await browser.newContext();
    if (mode === "sandbox") await ctx.addInitScript(sandboxed);
    const first = await ctx.newPage();
    await first.goto(`${d.base}/a/${artifact.id}`);
    await contentFrame(first, artifact.id, 1);
    await first.close();
    expect((await ctx.cookies(d.base)).find(c => c.name === "clax_frame")?.value).toBe(mode);
    const html = await (await ctx.request.get(`${d.base}/a/${artifact.id}`)).text();
    expect(html).toContain(`<iframe class="frame"`);
    expect(html).not.toContain(d.token);
    const page = await ctx.newPage();
    // The frame in the HTML is the one the page keeps: never replaced.
    await page.addInitScript(() => {
      const w = window as unknown as { claxFrames: number };
      w.claxFrames = 0;
      new MutationObserver(rs => { for (const r of rs) for (const n of r.addedNodes) if ((n as Element).localName === "iframe") w.claxFrames++; })
        .observe(document, { childList: true, subtree: true });
    });
    await page.goto(`${d.base}/a/${artifact.id}#part-2`);
    // The frame's URL carries the fragment.
    const url = new RegExp(`(${artifact.id}\\.localhost:\\d+/v/1/|/c/${artifact.id}/v/1/)(#part-2)?$`);
    await expect.poll(() => page.frame({ url }) !== null, { timeout: 30_000 }).toBe(true);
    const frame = page.frame({ url })!;
    await expect.poll(() => frame.evaluate(() => location.hash)).toBe("#part-2");
    expect(new URL(page.url()).hash).toBe("#part-2");
    await page.getByRole("button", { name: "Comment" }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    expect(await page.evaluate(() => (window as unknown as { claxFrames: number }).claxFrames)).toBe(1);
    const el = page.locator(".stage > iframe.frame");
    await expect(el).toHaveCount(1);
    expect(await el.getAttribute("sandbox")).toBe(mode === "sandbox" ? FRAME_SANDBOX : null);
    expect(await el.getAttribute("allow")).toBe("clipboard-write; fullscreen");
    await ctx.close();
  });

  test(`the shell frames the content, and the artifact's pages frame each other (${mode})`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, "Nested", {
      "index.html": `<p id="top">top</p><iframe src="about.html"></iframe>`,
      "about.html": `<p id="about">about</p>`,
    });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    expect(new URL(frame.url()).hostname).toBe(mode === "subdomain" ? `${artifact.id}.localhost` : "localhost");
    await expect(frame.locator("#top")).toHaveText("top");
    await expect.poll(() => frame.childFrames().length).toBe(1);
    await expect(frame.childFrames()[0].locator("#about")).toHaveText("about");
  });

  test(`a hostile title is shown as text (${mode})`, async ({ browser }) => {
    const ctx = await browser.newContext();
    if (mode === "sandbox") await ctx.addInitScript(sandboxed);
    const title = `</script><img src=x onerror="window.pwned=1"><!--clax:frame-->`;
    const { artifact } = await publish(d.base, d.token, title, { "index.html": "<p>x</p>" });
    for (let visit = 0; visit < 2; visit++) {
      const page = await ctx.newPage();
      await page.goto(`${d.base}/a/${artifact.id}`);
      await contentFrame(page, artifact.id, 1);
      await expect(page.locator(".topbar h1")).toHaveText(title);
      expect(await page.evaluate(() => (window as { pwned?: number }).pwned)).toBeUndefined();
      expect(await page.locator("img").count()).toBe(0);
      await page.close();
    }
    await ctx.close();
  });
}

test("a cookie naming the wrong frame mode never weakens the frame: a sandbox tab gets the sandboxed frame", async ({ browser }) => {
  const { artifact } = await publish(d.base, d.token, "Wrong cookie", { "index.html": "<p id=\"hi\">hi</p>" });
  const ctx = await browser.newContext();
  await ctx.addInitScript(sandboxed);
  await ctx.addCookies([{ name: "clax_frame", value: "subdomain", url: d.base }]);
  const page = await ctx.newPage();
  const subdomainDocs: string[] = [];
  page.on("framenavigated", f => { if (new URL(f.url()).hostname === `${artifact.id}.localhost`) subdomainDocs.push(f.url()); });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const frame = await contentFrame(page, artifact.id, 1);
  await expect(frame.locator("#hi")).toHaveText("hi");
  const el = page.locator(".stage > iframe.frame");
  await expect(el).toHaveCount(1);
  expect(await el.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
  expect(new URL(frame.url()).pathname).toBe(`/c/${artifact.id}/v/1/`);
  expect(subdomainDocs).toEqual([]);
  expect((await ctx.cookies(d.base)).find(c => c.name === "clax_frame")?.value).toBe("sandbox");
  await ctx.close();
});

test("a page reached under a rebound host name gets no bootstrap and no frame, as the API refuses it", async ({ playwright }) => {
  const { artifact } = await publish(d.base, d.token, "Rebound", { "index.html": "<p>x</p>" });
  const port = new URL(d.base).port;
  const rebound = await playwright.request.newContext({ extraHTTPHeaders: { host: `rebind.example:${port}`, cookie: "clax_frame=sandbox" } });
  try {
    expect((await rebound.get(`${d.base}/api/artifacts/${artifact.id}`)).status()).toBe(403);
    const res = await rebound.get(`${d.base}/a/${artifact.id}`);
    expect(res.status()).toBe(200);
    const html = await res.text();
    expect(html).not.toContain(`id="clax-boot"`);
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain("Rebound");
  } finally { await rebound.dispose(); }
  const html = await (await fetch(`${d.base}/a/${artifact.id}`, { headers: { cookie: "clax_frame=sandbox" } })).text();
  expect(html).toContain(`id="clax-boot"`);
  expect(html).toContain("<iframe");
});

test("no other site can frame the shell, or the content on an artifact origin", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Framed", { "index.html": `<p id="hi">hi</p>` });
  const port = new URL(d.base).port;
  const refused = [`${d.base}/`, `${d.base}/a/${artifact.id}`, `http://${artifact.id}.localhost:${port}/v/1/`];
  // Sandboxed content names no frame ancestors (its opaque origin would match
  // none); it shows that the other site can frame this daemon at all.
  const shown = `${d.base}/c/${artifact.id}/v/1/`;
  const body = [...refused, shown].map(t => `<iframe src="${t}"></iframe>`).join("");
  // Another site on this machine, so the browser's local network checks allow its frames.
  const other = createServer((_, res) => { res.writeHead(200, { "content-type": "text/html" }); res.end(body); });
  await new Promise<void>(r => other.listen(0, "127.0.0.1", r));
  try {
    await page.goto(`http://127.0.0.1:${(other.address() as AddressInfo).port}/`);
    await expect.poll(() => page.frames().length).toBe(2 + refused.length);
    const frames = page.frames().slice(1);
    await expect(frames[refused.length].locator("#hi")).toHaveText("hi");
    for (const [i, url] of refused.entries()) await expect.poll(() => frames[i].url(), url).toBe("chrome-error://chromewebdata/");
  } finally { other.close(); }
});
