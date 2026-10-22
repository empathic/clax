import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect, type Daemon, contentFrame, openArtifact, publish } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const PAGE = `<h1>Top</h1><p style="height:3000px">long</p><h2 id="part-2">Part 2</h2>`;

test("the frame in the HTML opens through its site's handshake, at the link's fragment, and comments at once", async ({ browser }) => {
  const { artifact } = await publish(d.base, d.token, "Boot", { "index.html": PAGE });
  const ctx = await browser.newContext();
  const html = await (await ctx.request.get(`${d.base}/a/${artifact.id}`)).text();
  expect(html).toContain(`<iframe class="frame"`);
  expect(html).toContain(`/_clax/session/start?next=%2F_clax%2Fv%2F1%2F`);
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
  const frame = await contentFrame(page, artifact.id, 1);
  await expect.poll(() => frame.evaluate(() => location.hash)).toBe("#part-2");
  expect(new URL(page.url()).hash).toBe("#part-2");
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  expect(await page.evaluate(() => (window as unknown as { claxFrames: number }).claxFrames)).toBe(1);
  const el = page.locator(".stage > iframe.frame");
  await expect(el).toHaveCount(1);
  expect(await el.getAttribute("sandbox")).toBeNull();
  expect(await el.getAttribute("allow")).toBe("clipboard-write; fullscreen; storage-access");
  await ctx.close();
});

test("the shell frames the content on its site origin, and the artifact's pages frame each other", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Nested", {
    "index.html": `<p id="top">top</p><iframe src="about.html"></iframe>`,
    "about.html": `<p id="about">about</p>`,
  });
  const frame = await openArtifact(page, d.base, artifact.id, 1);
  expect(new URL(frame.url()).hostname).toBe(`${artifact.id}.localhost`);
  expect(await frame.evaluate(() => origin)).toBe(new URL(frame.url()).origin);
  await expect(frame.locator("#top")).toHaveText("top");
  await expect.poll(() => frame.childFrames().length).toBe(1);
  await expect(frame.childFrames()[0].locator("#about")).toHaveText("about");
});

test("a hostile title is shown as text", async ({ browser }) => {
  const ctx = await browser.newContext();
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

test("a page reached under a rebound host name gets no bootstrap and no frame, as the API refuses it", async ({ playwright }) => {
  const { artifact } = await publish(d.base, d.token, "Rebound", { "index.html": "<p>x</p>" });
  const port = new URL(d.base).port;
  const rebound = await playwright.request.newContext({ extraHTTPHeaders: { host: `rebind.example:${port}` } });
  try {
    expect((await rebound.get(`${d.base}/api/artifacts/${artifact.id}`)).status()).toBe(403);
    const res = await rebound.get(`${d.base}/a/${artifact.id}`);
    expect(res.status()).toBe(200);
    const html = await res.text();
    expect(html).not.toContain(`id="clax-boot"`);
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain("Rebound");
    // Nor does a rebound name read the artifact's content: there is no
    // content on the public host.
    expect((await rebound.get(`${d.base}/c/${artifact.id}/v/1/`)).status()).toBe(404);
  } finally { await rebound.dispose(); }
  const html = await (await fetch(`${d.base}/a/${artifact.id}`)).text();
  expect(html).toContain(`id="clax-boot"`);
  expect(html).toContain("<iframe");
});

test("no other site can frame the shell, or a page on an artifact's site", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Framed", { "index.html": `<p id="hi">hi</p>` });
  const port = new URL(d.base).port;
  const site = `http://${artifact.id}.localhost:${port}`;
  const refused = [`${d.base}/`, `${d.base}/a/${artifact.id}`, `${site}/_clax/v/1/`, `${site}/`, `${site}/_clax/session/start?next=%2F`];
  // A page of the other site itself, which it may frame: it shows that the
  // other site's frames load at all.
  const shown = "/shown";
  const body = [...refused, shown].map(t => `<iframe src="${t}"></iframe>`).join("");
  // Another site on this machine, so the browser's local network checks allow its frames.
  const other = createServer((req, res) => { res.writeHead(200, { "content-type": "text/html" }); res.end(req.url === shown ? `<p id="hi">hi</p>` : body); });
  await new Promise<void>(r => other.listen(0, "127.0.0.1", r));
  try {
    await page.goto(`http://127.0.0.1:${(other.address() as AddressInfo).port}/`);
    await expect.poll(() => page.frames().length).toBe(2 + refused.length);
    const frames = page.frames().slice(1);
    await expect(frames[refused.length].locator("#hi")).toHaveText("hi");
    for (const [i, url] of refused.entries()) await expect.poll(() => frames[i].url(), url).toBe("chrome-error://chromewebdata/");
  } finally { other.close(); }
});
