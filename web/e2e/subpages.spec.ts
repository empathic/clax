import { test, expect, type Frame, type Page } from "@playwright/test";
import { api, contentFrame, openArtifact, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const INDEX = `<main><h1>Home</h1><p>Start here.</p><a id="to-about" href="about.html">About us</a> <a id="to-team" href="about.html#people">The people</a> <a id="self" href="index.html">Home</a></main>`;
const ABOUT = `<!doctype html><html><head><title>About</title></head><body><main><h2>Our team</h2><p>We build things.</p><a id="home" href="index.html">Home</a><div style="height:3000px"></div><h3 id="people">People</h3></main></body></html>`;

/** The content frame once it shows `about.html` of version 1. */
async function aboutFrame(page: Page, id: string): Promise<Frame> {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/1/about\\.html|/c/${id}/v/1/about\\.html)(#.*)?$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 15_000 }).toBe(true);
  return page.frame({ url })!;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a thread on a second page pins there, is labelled elsewhere, and opens its page`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Two pages ${mode}`, { "index.html": INDEX, "about.html": ABOUT });
    const id = artifact.id;
    const index = await openArtifact(page, d.base, id, 1, mode);
    if (mode === "sandbox") await expect(page.locator("iframe.frame")).toHaveAttribute("sandbox", /allow-scripts/);
    else await expect(page.locator("iframe.frame")).not.toHaveAttribute("sandbox", /.*/);
    await index.locator("#to-about").click();
    const about = await aboutFrame(page, id);
    await expect(about.locator("h2")).toHaveText("Our team");
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html`);

    const toggle = page.getByRole("button", { name: "Comment", exact: true });
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await about.locator("h2").hover();
    await about.locator("h2").click();
    const composer = page.locator(".composer");
    await expect(composer.locator(".composer-quote")).toContainText("Our team");
    await expect(composer.locator(".file-label")).toHaveText("on about.html");
    await composer.locator("textarea").fill("Name the team.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    const card = page.locator(".thread-card").filter({ hasText: "Name the team." });
    await expect(card).toHaveCount(1);
    await expect(card.locator(".file-label")).toHaveCount(0);
    await expect(page.locator("button.thread-pin")).toHaveCount(1);
    const tid = (await card.getAttribute("data-thread"))!;
    const t = await api(d.base, d.token, `/api/artifacts/${id}/threads/${tid}`);
    expect(t.thread.anchor).toMatchObject({ file: "about.html", selector: "body > main > h2", quote: "Our team" });
    await page.screenshot({ path: test.info().outputPath(`${mode}-about-pin.png`) });

    // Back on the index: no pin, and the thread is labelled with its page.
    // Comment mode came back after the post; a link is followed once it is off.
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await about.locator("#home").click();
    await expect(page).toHaveURL(`${d.base}/a/${id}`);
    await expect(page.locator("button.thread-pin")).toHaveCount(0);
    await expect(card.locator(".file-label")).toHaveText("on about.html");
    await expect(page.locator(".section-detached .thread-card")).toHaveCount(0);

    // Opening the thread takes the frame to its page and pins it there, as one history entry.
    const depth = await page.evaluate(() => history.length);
    await card.locator("button.card-head").click();
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html`);
    expect(await page.evaluate(() => history.length)).toBe(depth + 1);
    await expect((await aboutFrame(page, id)).locator("h2")).toHaveText("Our team");
    await expect(page.locator("button.thread-pin")).toHaveCount(1);
    await expect(card.locator(".file-label")).toHaveCount(0);
  });

  test(`${mode}: the address bar names the page, and back returns to it`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Address ${mode}`, { "index.html": INDEX, "about.html": ABOUT });
    const id = artifact.id;
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Our team", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "about.html" }));
    form.set("body", "about note");
    form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form })).status).toBe(201);
    if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    await page.goto(`${d.base}/a/${id}/about.html`);
    const about = await aboutFrame(page, id);
    await expect(about.locator("h2")).toHaveText("Our team");
    await expect(page.locator("button.thread-pin")).toHaveCount(1);
    await about.locator("#home").click();
    await expect(page).toHaveURL(`${d.base}/a/${id}`);
    await expect(page.locator("button.thread-pin")).toHaveCount(0);
    await page.evaluate(() => history.back());
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html`);
    await expect((await aboutFrame(page, id)).locator("h2")).toHaveText("Our team");
    await expect(page.locator("button.thread-pin")).toHaveCount(1);
  });

  test(`${mode}: a link's fragment is kept, and a link to the same page stays with the browser`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Fragments ${mode}`, { "index.html": INDEX, "about.html": ABOUT });
    const id = artifact.id;
    const index = await openArtifact(page, d.base, id, 1, mode);
    const depth = await page.evaluate(() => history.length);
    await index.locator("#self").click();
    await page.waitForTimeout(500);
    expect(await page.evaluate(() => history.length)).toBe(depth);
    await expect(page).toHaveURL(`${d.base}/a/${id}`);
    const home = await contentFrame(page, id, 1);
    await home.locator("#to-team").click();
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html#people`);
    expect(await page.evaluate(() => history.length)).toBe(depth + 1);
    const about = await aboutFrame(page, id);
    await expect(about.locator("#people")).toBeInViewport();
    await expect(about.locator("h2")).not.toBeInViewport();
  });

  test(`${mode}: the address bar and the frame share the page's fragment`, async ({ page }) => {
    const HASH = `<!doctype html><html><head><title>Hash</title></head><body><p id="h"></p><script>const show = () => { document.getElementById("h").textContent = location.hash; }; show(); addEventListener("hashchange", show);</script></body></html>`;
    const { artifact } = await publish(d.base, d.token, `Hash ${mode}`, { "index.html": INDEX, "hash.html": HASH });
    const id = artifact.id;
    if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    await page.goto(`${d.base}/a/${id}/hash.html#docs%2Fcontract.md`);
    const url = new RegExp(`/v/1/hash\\.html#`);
    await expect.poll(() => page.frames().some(f => f !== page.mainFrame() && url.test(f.url()))).toBe(true);
    const frame = page.frames().find(f => f !== page.mainFrame() && url.test(f.url()))!;
    await expect(frame.locator("#h")).toHaveText("#docs%2Fcontract.md");
    await frame.evaluate(() => { location.hash = "#crates%2Fa.rs"; });
    await expect(page).toHaveURL(`${d.base}/a/${id}/hash.html#crates%2Fa.rs`);
    await expect(frame.locator("#h")).toHaveText("#crates%2Fa.rs");
    await page.evaluate(() => history.back());
    await expect(frame.locator("#h")).toHaveText("#docs%2Fcontract.md");
    await expect(page).toHaveURL(`${d.base}/a/${id}/hash.html#docs%2Fcontract.md`);
  });

  test(`${mode}: one link inside the frame is one history entry`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `History ${mode}`, { "index.html": INDEX, "about.html": ABOUT });
    const id = artifact.id;
    await page.goto(`${d.base}/`);
    const index = await openArtifact(page, d.base, id, 1, mode);
    const step = (fn: () => void) => page.evaluate(fn).catch(() => { /* the document may be replaced */ });
    const depth = await page.evaluate(() => history.length);
    await index.locator("#to-about").click();
    await expect((await aboutFrame(page, id)).locator("h2")).toHaveText("Our team");
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html`);
    expect(await page.evaluate(() => history.length)).toBe(depth + 1);
    await step(() => history.back());
    await expect(page).toHaveURL(`${d.base}/a/${id}`);
    await expect.poll(() => page.frames().some(f => f !== page.mainFrame() && /\/v\/1\/$/.test(f.url()))).toBe(true);
    await step(() => history.back());
    await expect(page).toHaveURL(`${d.base}/`);
    await step(() => history.forward());
    await expect(page).toHaveURL(`${d.base}/a/${id}`);
    await expect.poll(() => page.frames().some(f => f !== page.mainFrame() && /\/v\/1\/$/.test(f.url()))).toBe(true);
    await step(() => history.forward());
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html`);
    await expect((await aboutFrame(page, id)).locator("h2")).toHaveText("Our team");
  });
}

test("a page the version does not hold gets a message instead of a frame", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Missing page", { "index.html": INDEX });
  await page.goto(`${d.base}/a/${artifact.id}/nope.html`);
  await expect(page.locator(".stage .empty")).toContainText("v1 has no page nope.html");
  await expect(page.locator("iframe.frame")).toHaveCount(0);
});
