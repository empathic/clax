import { test, expect, type Frame, type Page } from "@playwright/test";
import { contentFrame, openArtifact, publish, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const EARLY = `<!doctype html><html lang="en"><head><title>Early</title><script>
  window.seen = typeof (window.claude && window.claude.use);
</script></head><body><p id="out"></p><script>document.getElementById("out").textContent = window.seen;</script></body></html>`;

// No <head> tag: the head is implied, and its script runs before <body>.
const EARLY_NO_HEAD = `<!doctype html><html lang="fr"><meta charset="utf-8"><title>Early</title><script>
  window.seen = typeof (window.claude && window.claude.use);
</script><body><p id="out"></p><script>document.getElementById("out").textContent = window.seen;</script></body></html>`;

// Republishes its served DOM on a click.
const ROUND_TRIP = `<!doctype html><html lang="en"><head><title>Round trip</title><script>
  window.seen = typeof (window.claude && window.claude.use);
</script></head><body><p id="out"></p><button id="go">Republish</button><script>
  document.getElementById("out").textContent = window.seen;
  document.getElementById("go").onclick = async () => {
    const artifact = await claude.use("artifact");
    await artifact.publish("<!doctype html>\\n" + document.documentElement.outerHTML);
  };
</script></body></html>`;

// Renders its anchored content from a module script, which runs after the
// parse ends; the blocking head script makes the shell's anchor request arrive
// while the page is still loading.
const LATE = `<!doctype html><html><head><title>Late</title><script src="slow.js"></script><script type="module">
  document.getElementById("app").innerHTML = "<h2>Quarterly goals</h2><p>Grow revenue.</p>";
</script></head><body><main id="app"></main></body></html>`;

const bridges = (f: Frame) => f.evaluate(() => Array.from(document.querySelectorAll("script[src^='/_artifax/bridge.js']")).map(s => ({ first: s === document.head.firstElementChild, version: (s as HTMLScriptElement).dataset.version })));

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
    // Exactly one bridge runs in the served document, first in the head the
    // parser builds after the doctype, and
    // the page's <html lang> still applies.
    expect(await bridges(f)).toEqual([{ first: true, version: "1" }]);
    expect(await f.evaluate(() => document.documentElement.lang)).toBe("en");
  });

  test(`${mode}: a script in an implied <head> sees window.claude.use`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Early no head ${mode}`, EARLY_NO_HEAD, {});
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#out")).toHaveText("function");
    expect(await bridges(f)).toEqual([{ first: true, version: "1" }]);
    expect(await f.evaluate(() => document.documentElement.lang)).toBe("fr");
  });

  test(`${mode}: a page republished from its served DOM runs one bridge, for the new version`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Round trip ${mode}`, ROUND_TRIP, { artifact: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#out")).toHaveText("function");
    const headOf = (fr: Frame) => fr.evaluate(() => {
      const h = document.head.cloneNode(true) as HTMLHeadElement;
      h.querySelectorAll("script[src^='/_artifax/bridge.js']").forEach(s => s.remove());
      return h.innerHTML;
    });
    const head1 = await headOf(f);
    await f.locator("#go").click();
    const g = await contentFrame(page, artifact.id, 2);
    await expect(g.locator("#out")).toHaveText("function");
    // The stored page carries the version 1 tag the daemon strips when serving.
    const stored = await (await fetch(`${d.base}/api/artifacts/${artifact.id}/versions/2/files/index.html`)).text();
    expect(stored).toContain('data-version="1"');
    expect(await bridges(g)).toEqual([{ first: true, version: "2" }]);
    expect(await g.evaluate(() => document.documentElement.lang)).toBe("en");
    // Republishing the served DOM again and again leaves <head> as it was.
    expect(await headOf(g)).toBe(head1);
    let prev = g;
    for (const n of [3, 4]) {
      await page.waitForTimeout(2_100); // the shell's gap between publishes
      await prev.locator("#go").click();
      const next = await contentFrame(page, artifact.id, n);
      await expect(next.locator("#out")).toHaveText("function");
      expect(await headOf(next)).toBe(head1);
      prev = next;
    }
  });

  test(`${mode}: a sub page's <head> script sees window.claude.use`, async ({ page }) => {
    const index = `<main><a id="go" href="early.html">Early</a></main>`;
    const { artifact } = await publish(d.base, d.token, `Early sub ${mode}`, { "index.html": index, "early.html": EARLY });
    const home = await openArtifact(page, d.base, artifact.id, 1, mode);
    await home.locator("#go").click();
    const f = await frameAt(page, artifact.id, "early.html");
    await expect(f.locator("#out")).toHaveText("function");
    expect(await f.evaluate(() => (document.head.firstElementChild as HTMLScriptElement).dataset.file)).toBe("early.html");
    expect(await bridges(f)).toEqual([{ first: true, version: "1" }]);
  });

  test(`${mode}: a thread on content a module script renders pins without a scroll`, async ({ page, browser }) => {
    const { artifact } = await publish(d.base, d.token, `Late ${mode}`, { "index.html": LATE, "slow.js": "window.slow = 1;" });
    const id = artifact.id;
    const f = await openArtifact(page, d.base, id, 1, mode);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await f.locator("h2").hover();
    await f.locator("h2").click();
    const composer = page.locator(".composer");
    await composer.locator("textarea").fill("Pin me late.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    await expect(page.locator("button.thread-pin")).toHaveCount(1);

    const ctx = await browser.newContext();
    try {
      const slow = await ctx.newPage();
      await slow.route("**/slow.js", async r => { await new Promise(res => setTimeout(res, 2000)); await r.continue(); });
      const g = await openArtifact(slow, d.base, id, 1, mode);
      await expect(g.locator("h2")).toHaveText("Quarterly goals");
      await expect(slow.locator("button.thread-pin")).toHaveCount(1);
      expect(await g.evaluate(() => scrollY)).toBe(0);
    } finally {
      await ctx.close();
    }
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
