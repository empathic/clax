import { createServer, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { type Frame, type Page } from "@playwright/test";
import { test, expect, type Daemon, api, commentModeIn, contentFrame, openArtifact, publish } from "./fixtures";
import { settle } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

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
    await commentModeIn(about);
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
    await card.getByRole("button", { name: "Go to page about.html" }).click();
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
    await settle(page);
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

  test(`${mode}: the address bar's fragment moves the page's, which goes on hearing the shell`, async ({ page }) => {
    const HASH = `<!doctype html><html><head><title>Hash</title></head><body><p id="h"></p><script>const show = () => { document.getElementById("h").textContent = location.hash; }; show(); addEventListener("hashchange", show);</script></body></html>`;
    const { artifact } = await publish(d.base, d.token, `Address ${mode}`, { "index.html": INDEX, "hash.html": HASH });
    const id = artifact.id;
    if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    await page.goto(`${d.base}/a/${id}/hash.html#first`);
    const url = new RegExp(`/v/1/hash\\.html#`);
    await expect.poll(() => page.frames().some(f => f !== page.mainFrame() && url.test(f.url()))).toBe(true);
    const frame = page.frames().find(f => f !== page.mainFrame() && url.test(f.url()))!;
    await expect(frame.locator("#h")).toHaveText("#first");
    // The page moves itself to the fragment (clax:hash): a fragment
    // navigation the shell started would fire a load event at the frame
    // element, which the shell takes for a document that never greeted.
    await page.evaluate(() => { location.hash = "#viewer"; });
    await expect(frame.locator("#h")).toHaveText("#viewer");
    await expect(page).toHaveURL(`${d.base}/a/${id}/hash.html#viewer`);
    await page.getByRole("button", { name: "Comment" }).click();
    await commentModeIn(frame);
  });

  test(`${mode}: back across an in-page link, the page is asked to greet again and goes on hearing the shell`, async ({ page }) => {
    const LONG = `<!doctype html><html><head><title>Long</title></head><body><a id="jump" href="#end">To the end</a><div style="height:3000px"></div><h2 id="end">End</h2></body></html>`;
    const { artifact } = await publish(d.base, d.token, `Back ${mode}`, { "index.html": LONG });
    const id = artifact.id;
    // Counts the shell's requests to greet again, in every frame.
    await page.addInitScript(() => addEventListener("message", e => { if (e.data && e.data.type === "clax:greet") (window as unknown as { greets: number }).greets = ((window as unknown as { greets?: number }).greets ?? 0) + 1; }));
    const frame = await openArtifact(page, d.base, id, 1, mode);
    // The link stays with the browser: a fragment entry of the frame's own.
    await frame.locator("#jump").click();
    await expect(page).toHaveURL(`${d.base}/a/${id}#end`);
    // Back across that entry: Chromium fires a load at the frame element with
    // no new document, which closes the gate; the shell asks the page to
    // greet, and its hello reopens it.
    await page.evaluate(() => history.back());
    await expect.poll(() => frame.evaluate(() => location.hash)).toBe("");
    await expect.poll(() => frame.evaluate(() => (window as unknown as { greets?: number }).greets ?? 0)).toBe(1);
    await page.getByRole("button", { name: "Comment" }).click();
    await commentModeIn(frame);
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

  test(`${mode}: a page that moves the frame itself to another of its pages is heard there`, async ({ page }) => {
    const go = `<button id="go" onclick="location.href='about.html'">Go</button>`;
    const { artifact } = await publish(d.base, d.token, `Self move ${mode}`, { "index.html": go, "about.html": ABOUT });
    const id = artifact.id;
    const index = await openArtifact(page, d.base, id, 1, mode);
    await index.locator("#go").click();
    const about = await aboutFrame(page, id);
    await expect(about.locator("h2")).toHaveText("Our team");
    // It greeted: the address bar follows it, and comment mode reaches it.
    await expect(page).toHaveURL(`${d.base}/a/${id}/about.html`);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect.poll(() => about.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  });

  test(`${mode}: another site's page the frame moves to hears nothing from the shell, even before its load`, async ({ page }) => {
    // Another site on this machine: its page records every message it gets,
    // and holds its own load open with an image that never arrives.
    const hung: ServerResponse[] = [];
    const spy = createServer((req, res) => {
      if (req.url === "/hang") { hung.push(res); return; }
      res.writeHead(200, { "content-type": "text/html" });
      res.end(`<script>window.got = []; addEventListener("message", e => window.got.push(e.data));</script><p id="spy">spy</p><img src="/hang">`);
    });
    await new Promise<void>(r => spy.listen(0, "127.0.0.1", r));
    const spyUrl = `http://127.0.0.1:${(spy.address() as AddressInfo).port}/spy`;
    try {
      const { artifact } = await publish(d.base, d.token, `Leaves ${mode}`, { "index.html": `<a id="out" href="${spyUrl}">Elsewhere</a>` });
      const index = await openArtifact(page, d.base, artifact.id, 1, mode);
      // The page greeted: comment mode reaches it.
      const toggle = page.getByRole("button", { name: "Comment", exact: true });
      await toggle.click();
      await expect.poll(() => index.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
      await toggle.click();
      await expect.poll(() => index.evaluate(() => document.documentElement.style.cursor)).not.toBe("crosshair");
      await index.locator("#out").click();
      await expect.poll(() => page.frame({ url: spyUrl }) !== null).toBe(true);
      const other = page.frame({ url: spyUrl })!;
      await expect(other.locator("#spy")).toHaveText("spy");
      expect(await other.evaluate(() => document.readyState)).not.toBe("complete");
      // The shell would send these to a page it still heard.
      await toggle.click();
      await toggle.click();
      await settle(page);
      expect(await other.evaluate(() => (window as unknown as { got: unknown[] }).got)).toEqual([]);
    } finally {
      for (const r of hung) r.destroy();
      spy.closeAllConnections();
      spy.close();
    }
  });
}

test("a thread on another page is read, answered, resolved and reopened in place, the frame staying on its page", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "In place", { "index.html": INDEX, "about.html": ABOUT });
  const id = artifact.id;
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Our team", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "about.html" }));
  form.set("body", "Name the team.");
  form.set("version", "1");
  const made = await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form });
  expect(made.status).toBe(201);
  const tid = (await made.json()).thread.id as string;
  await openArtifact(page, d.base, id, 1, "subdomain");
  const card = page.locator(`[data-thread="${tid}"]`);
  const head = card.locator("button.card-head");
  await expect(head).toHaveAttribute("aria-expanded", "false");
  await expect(card.locator(".fold-body")).toHaveText("Name the team.");
  await head.press("Enter");
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await expect(head).toBeFocused();
  await card.getByLabel("Reply").fill("Done: the Makers.");
  await card.getByRole("button", { name: "Reply", exact: true }).click();
  await expect(card.locator(".msg .body")).toHaveText(["Name the team.", "Done: the Makers."]);
  // A reply from elsewhere shows in the open card.
  await api(d.base, d.token, `/api/artifacts/${id}/threads/${tid}/comments`, { method: "POST", body: JSON.stringify({ body: "Seen it." }) });
  await expect(card.locator(".msg .body")).toHaveText(["Name the team.", "Done: the Makers.", "Seen it."]);
  await card.getByRole("button", { name: "Resolve" }).click();
  await expect.poll(async () => (await api(d.base, d.token, `/api/artifacts/${id}/threads/${tid}`)).thread.status).toBe("resolved");
  // It stays open where it went (Resolved), and reopens there.
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await card.getByRole("button", { name: "Reopen" }).click();
  await expect.poll(async () => (await api(d.base, d.token, `/api/artifacts/${id}/threads/${tid}`)).thread.status).toBe("open");
  // The card moved back to Open, a new card there, still open, with focus on its head.
  await expect(page.locator(`.section-open [data-thread="${tid}"]`).getByRole("button", { name: "Resolve" })).toBeVisible();
  await expect(head).toBeFocused();
  await expect(page).toHaveURL(`${d.base}/a/${id}`);
  await expect(page.locator("iframe.frame")).toHaveAttribute("src", new RegExp(`/v/1/$`));
  // Escape folds it, focus on its head.
  await card.getByLabel("Reply").focus();
  await page.keyboard.press("Escape");
  await expect(head).toHaveAttribute("aria-expanded", "false");
  await expect(head).toBeFocused();
});

test("a page the version does not hold gets a message instead of a frame", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Missing page", { "index.html": INDEX });
  await page.goto(`${d.base}/a/${artifact.id}/nope.html`);
  await expect(page.locator(".stage .empty")).toContainText("v1 has no page nope.html");
  await expect(page.locator("iframe.frame")).toHaveCount(0);
});

test("a viewer with no name is asked for one before a reopen, and the thread reopens once it has one", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Reopen by name", { "index.html": INDEX, "about.html": ABOUT });
  const id = artifact.id;
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Our team", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "about.html" }));
  form.set("body", "Name the team.");
  form.set("version", "1");
  const tid = (await (await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form })).json()).thread.id as string;
  await api(d.base, d.token, `/api/artifacts/${id}/threads/${tid}/resolve`, { method: "POST", body: "{}" });
  await openArtifact(page, d.base, id, 1, "subdomain", { lan: true });
  await page.locator(".section-resolved summary").click();
  const card = page.locator(`[data-thread="${tid}"]`);
  await card.locator("button.card-head").click();
  await card.getByRole("button", { name: "Reopen" }).click();
  // A request in the people menu, not an error banner; focus is in the name field.
  await expect(page.locator(".people .ask")).toHaveText("Add your name to reopen the thread: people see it beside what you do.");
  await expect(page.locator(".banner.notice")).toHaveCount(0);
  const name = page.getByLabel("Your name");
  await expect(name).toBeFocused();
  await page.keyboard.type("Mia");
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await api(d.base, d.token, `/api/artifacts/${id}/threads/${tid}`)).thread.status).toBe("open");
  // Its work done, the menu closes and focus goes to the thread's card, now in Open.
  await expect(page.locator(".people")).toHaveCount(0);
  await expect(page.locator(`.section-open [data-thread="${tid}"] .card-head`)).toBeFocused();
});
