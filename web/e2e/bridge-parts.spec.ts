import { test, expect } from "@playwright/test";
import { type FrameMode, openArtifact, publish, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const partsLoaded = (frame: import("@playwright/test").Frame) =>
  frame.evaluate(() => performance.getEntriesByType("resource").map(e => e.name).filter(n => n.includes("/_clax/bridge/")));

for (const mode of ["subdomain", "sandbox"] as FrameMode[]) {
  test(`loads comment mode lazily and says so when the page's CSP blocks it (${mode})`, async ({ page }) => {
    const ok = await publish(d.base, d.token, "Parts", { "index.html": "<h1 id=h>Parts</h1>" });
    const frame = await openArtifact(page, d.base, ok.artifact.id, 1, mode);
    // The comment part loads from the daemon's /_clax/bridge/, once the page has greeted.
    await expect.poll(async () => (await partsLoaded(frame)).some(n => /\/_clax\/bridge\/comment-[^/]+\.js$/.test(n))).toBe(true);
    const host = new URL(frame.url()).host;
    for (const n of await partsLoaded(frame)) expect(new URL(n).host).toBe(host);
    await page.getByRole("button", { name: "Comment" }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    // Comment mode turned on starts the clip part.
    await expect.poll(async () => (await partsLoaded(frame)).some(n => /\/_clax\/bridge\/clip-[^/]+\.js$/.test(n))).toBe(true);

    // A full document, so its CSP is in its <head> (a fragment's is moved into
    // the skeleton's <body>, where a browser ignores it). The bridge comes
    // before it, so only the lazy parts are blocked.
    const blocked = await publish(d.base, d.token, "Strict", { "index.html": `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="script-src 'unsafe-inline'"></head><body><h1>Strict</h1></body></html>` });
    const strict = await openArtifact(page, d.base, blocked.artifact.id, 1, mode);
    const alert = page.getByRole("alert");
    await expect(alert).toContainText("Comment mode could not load in this page");
    // Pressing Comment there keeps comment mode off and says why again, even
    // once the notice was dismissed.
    await alert.getByRole("button", { name: "Dismiss" }).click();
    await expect(alert).toHaveCount(0);
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    await comment.click();
    await expect(alert).toContainText("Comment mode could not load in this page");
    await expect(comment).toHaveAttribute("aria-pressed", "false");
    expect(await strict.evaluate(() => document.documentElement.style.cursor)).not.toBe("crosshair");
  });

  test(`recovers from a comment part that failed once: capabilities load, and a later need loads comment mode again (${mode})`, async ({ page }) => {
    const html = `<!doctype html><html><head><title>Flaky</title></head><body><h1 id="t">Flaky</h1><button id="use">Use</button><p id="out">waiting</p>
<script>document.getElementById("use").onclick = async () => { const c = await claude.use("comments"); document.getElementById("out").textContent = c ? "ready" : "null"; };</script></body></html>`;
    const { artifact } = await publishWith(d.base, d.token, "Flaky", html, { comments: {} });
    // The comment part's first request fails (a daemon restarted under the
    // tab, say); its retry, and every other part, load.
    const seen: string[] = [];
    await page.route(/\/_clax\/bridge\/[^/]+\.js/, async route => {
      const u = new URL(route.request().url());
      seen.push(u.pathname.replace(/^.*\//, "").replace(/-[^.]+\.js$/, ".js") + u.search);
      if (/\/comment[-.]/.test(u.pathname) && !u.search && seen.filter(s => s === "comment.js").length === 1) {
        await route.fulfill({ status: 404, headers: { "access-control-allow-origin": "*" }, body: "gone" });
      } else await route.continue();
    });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.getByRole("alert")).toContainText("Comment mode could not load in this page");
    // The capability members load on their own, with no request for the comment part's file.
    await frame.locator("#use").click();
    await expect(frame.locator("#out")).toHaveText("ready");
    expect(seen).toEqual(["comment.js", "caps.js"]);
    // After the backoff, a thread arriving makes the shell resolve anchors:
    // the comment part loads again under a retry query, and the pin shows.
    await page.waitForTimeout(2_200);
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "#t", quote: "Flaky", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "Retry me"); form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`, { method: "POST", body: form })).status).toBe(201);
    await expect(page.locator(".thread-pin")).toHaveCount(1);
    expect(seen).toContain("comment.js?retry=1");
  });

  // A smoke test only: Chromium honours an import map added after a module
  // load, so this passes with or without the bridge's wait for the parse.
  // bridge-parse-gate.test.ts guards that wait.
  test(`smoke: a page's own import map and comment mode work together (${mode})`, async ({ page }) => {
    const html = `<!doctype html><script type="importmap">{"imports":{"greeting":"data:text/javascript,export default 'mapped'"}}</script>`
      + `<script type="module">import g from "greeting"; document.getElementById("out").textContent = g;</script><p id="out">waiting</p>`;
    const ok = await publish(d.base, d.token, "Import map", { "index.html": html });
    const frame = await openArtifact(page, d.base, ok.artifact.id, 1, mode);
    await expect(frame.locator("#out")).toHaveText("mapped");
    await page.getByRole("button", { name: "Comment" }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  });
}
