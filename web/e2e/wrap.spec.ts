import { test, expect, type Frame, type Page } from "@playwright/test";
import { openArtifact, publish, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const EARLY = `<!doctype html><html><head><title>Early</title><script>
  window.seen = typeof (window.claude && window.claude.use);
</script></head><body><p id="out"></p><script>document.getElementById("out").textContent = window.seen;</script></body></html>`;

// A page whose `<head>` blocks on a script, so the shell's welcome and anchor
// requests reach the bridge before `<body>` exists.
const SLOW = `<!doctype html><html><head><title>Slow</title><script src="slow.js"></script></head><body><main><h2>Quarterly goals</h2><p>Grow revenue.</p></main></body></html>`;

async function frameAt(page: Page, id: string, file: string): Promise<Frame> {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/1/${file.replace(".", "\\.")}|/c/${id}/v/1/${file.replace(".", "\\.")})$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 30_000 }).toBe(true);
  return page.frame({ url })!;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a <head> script sees window.claude.use`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Early ${mode}`, EARLY, {});
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#out")).toHaveText("function");
    // Exactly one bridge runs in the served document, first in <head>.
    expect(await f.evaluate(() => document.querySelectorAll("script[src^='/_artifax/bridge.js']").length)).toBe(1);
    expect(await f.evaluate(() => (document.head.firstElementChild as HTMLScriptElement).src)).toContain("/_artifax/bridge.js");
  });

  test(`${mode}: a sub page's <head> script sees window.claude.use`, async ({ page }) => {
    const index = `<main><a id="go" href="early.html">Early</a></main>`;
    const { artifact } = await publish(d.base, d.token, `Early sub ${mode}`, { "index.html": index, "early.html": EARLY });
    const home = await openArtifact(page, d.base, artifact.id, 1, mode);
    await home.locator("#go").click();
    const f = await frameAt(page, artifact.id, "early.html");
    await expect(f.locator("#out")).toHaveText("function");
    expect(await f.evaluate(() => (document.head.firstElementChild as HTMLScriptElement).dataset.file)).toBe("early.html");
  });

  test(`${mode}: anchors requested before <body> exists resolve once the page has parsed`, async ({ page, browser }) => {
    const { artifact } = await publish(d.base, d.token, `Slow ${mode}`, { "index.html": SLOW, "slow.js": "window.slow = 1;" });
    const id = artifact.id;
    const f = await openArtifact(page, d.base, id, 1, mode);
    const toggle = page.getByRole("button", { name: "Comment", exact: true });
    await toggle.click();
    await f.locator("h2").hover();
    await f.locator("h2").click();
    const composer = page.locator(".composer");
    await composer.locator("textarea").fill("Pin me.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    await expect(page.locator("button.thread-pin")).toHaveCount(1);

    // A fresh context (no cached slow.js) whose slow.js is held back.
    const ctx = await browser.newContext();
    try {
      const slow = await ctx.newPage();
      let released = false;
      await slow.route("**/slow.js", async r => { await new Promise(res => setTimeout(res, 2500)); released = true; await r.continue(); });
      const g = await openArtifact(slow, d.base, id, 1, mode);
      await expect(g.locator("h2")).toHaveText("Quarterly goals");
      expect(released).toBe(true);
      await expect(slow.locator("button.thread-pin")).toHaveCount(1);
    } finally {
      await ctx.close();
    }
  });
}
