// Each artifact framed on its own site origin (docs/superpowers/specs/
// 2026-10-21-clax-run-site-origins-design.md, "The shell and the bridge"):
// the frame opens through the site's handshake, unsandboxed, so each
// artifact's storage is its own; no other site may frame a site page; and
// when the frame cannot sign in, the stage offers the page in a tab.
import { createServer, request as httpRequest } from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect, type Daemon, contentFrame, openArtifact, publish } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

test("the frame loads the site origin unsandboxed and its localStorage is the artifact's own", async ({ page }) => {
  const page1 = `<p id="v"></p><script>localStorage.setItem("x", "one"); document.getElementById("v").textContent = localStorage.getItem("x");</script>`;
  const page2 = `<p id="v"></p><script>document.getElementById("v").textContent = String(localStorage.getItem("x")); localStorage.setItem("x", "two");</script>`;
  const a = (await publish(d.base, d.token, "Store one", { "index.html": page1 })).artifact;
  const b = (await publish(d.base, d.token, "Store two", { "index.html": page2 })).artifact;
  const fa = await openArtifact(page, d.base, a.id, 1);
  expect(await page.locator(".stage > iframe.frame").getAttribute("sandbox")).toBeNull();
  expect(await fa.evaluate(() => origin)).toBe(new URL(fa.url()).origin);
  await expect(fa.locator("#v")).toHaveText("one");
  const fb = await openArtifact(page, d.base, b.id, 1);
  // The other artifact's page found nothing of the first's, and wrote its own.
  await expect(fb.locator("#v")).toHaveText("null");
  expect(await fb.evaluate(() => localStorage.getItem("x"))).toBe("two");
  const again = await openArtifact(page, d.base, a.id, 1);
  expect(await again.evaluate(() => localStorage.getItem("x"))).toBe("one");
});

test("no other origin can frame a site page", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Not yours", { "index.html": `<p id="hi">hi</p>` });
  const site = `http://${artifact.id}.localhost:${new URL(d.base).port}`;
  // In the shell's frame the site's page loads, framed only by its own
  // pages and the shell's loopback names…
  const served = page.waitForResponse(r => r.url() === `${site}/_clax/v/1/` && r.status() === 200);
  await page.goto(`${d.base}/a/${artifact.id}`);
  await contentFrame(page, artifact.id, 1);
  const port = new URL(d.base).port;
  expect((await served).headers()["content-security-policy"]).toBe(`frame-ancestors 'self' http://localhost:${port} http://127.0.0.1:${port}`);
  // …but another site's frame of them is refused, whatever it asks for.
  const refused = [`${site}/`, `${site}/_clax/v/1/`, `${site}/_clax/session/start?next=%2F`];
  const shown = "/shown";
  const body = [...refused, shown].map(t => `<iframe src="${t}"></iframe>`).join("");
  const other = createServer((req, r) => { r.writeHead(200, { "content-type": "text/html" }); r.end(req.url === shown ? `<p id="hi">hi</p>` : body); });
  await new Promise<void>(r => other.listen(0, "127.0.0.1", r));
  try {
    const p = await page.context().newPage();
    await p.goto(`http://127.0.0.1:${(other.address() as AddressInfo).port}/`);
    await expect.poll(() => p.frames().length).toBe(2 + refused.length);
    const frames = p.frames().slice(1);
    await expect(frames[refused.length].locator("#hi")).toHaveText("hi");
    for (const [i, url] of refused.entries()) await expect.poll(() => frames[i].url(), url).toBe("chrome-error://chromewebdata/");
  } finally { other.close(); }
});

test("a frame whose code is refused offers the page in a tab", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Refused", { "index.html": `<p id="hi">hi</p>` });
  await page.route(`**/api/artifacts/${artifact.id}/site-code`, r => r.fulfill({ status: 404, contentType: "application/json", body: JSON.stringify({ error: { code: "not_found", message: "not found" } }) }));
  await page.goto(`${d.base}/a/${artifact.id}`);
  const link = page.getByRole("link", { name: "Open in a tab" });
  await expect(link).toHaveAttribute("href", `http://${artifact.id}.localhost:${new URL(d.base).port}/_clax/v/1/`);
  await expect(page.locator(".stage > iframe.frame")).toHaveCount(0);
});

test("a frame whose browser keeps its cookies from the site is told so, asked for storage access, tried once more, then offered in a tab", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Cookies refused", { "index.html": `<p id="hi">hi</p>` });
  // As where the browser refuses the frame's cookies: the site's answers
  // reach the frame without their cookies (fetched outside the browser, so
  // its cookie store never sees them), and its requests carry none, so the
  // redemption fails and the site answers its no-cookie page.
  let redemptions = 0;
  await page.route(`http://${artifact.id}.localhost:*/**`, async r => {
    const u = new URL(r.request().url());
    if (u.pathname === "/_clax/session") redemptions++;
    // The browser adds its Sec-Fetch headers after interception: a frame's
    // navigation says what it is, as Chromium's does.
    const nav: Record<string, string> = r.request().isNavigationRequest() ? { "sec-fetch-mode": "navigate", "sec-fetch-dest": "iframe" } : {};
    const headers: Record<string, string> = { ...(await r.request().allHeaders()), ...nav, host: u.host };
    delete headers.cookie;
    const res = await new Promise<{ status: number; headers: Record<string, string>; body: Buffer }>((resolve, reject) => {
      const req = httpRequest({ host: "127.0.0.1", port: u.port, path: u.pathname + u.search, method: r.request().method(), headers }, answer => {
        const chunks: Buffer[] = [];
        answer.on("data", (c: Buffer) => { chunks.push(c); });
        answer.on("end", () => {
          const h: Record<string, string> = {};
          for (const [k, v] of Object.entries(answer.headers)) if (k !== "set-cookie" && v !== undefined) h[k] = String(v);
          resolve({ status: answer.statusCode ?? 502, headers: h, body: Buffer.concat(chunks) });
        });
      });
      req.on("error", reject);
      req.end(r.request().postDataBuffer() ?? undefined);
    });
    await r.fulfill(res);
  });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await expect(page.getByRole("link", { name: "Open in a tab" })).toBeVisible();
  expect(redemptions).toBe(2);
  await expect(page.locator(".stage > iframe.frame")).toHaveCount(0);
});
