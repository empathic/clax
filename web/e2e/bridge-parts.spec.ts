import { test, expect } from "@playwright/test";
import { type FrameMode, openArtifact, publish, startDaemon } from "./fixtures";

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
