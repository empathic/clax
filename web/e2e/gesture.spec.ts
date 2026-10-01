import { test, expect, type Frame, type Page } from "@playwright/test";
import { api, openArtifact, record, registerSession, startDaemon } from "./fixtures";

// The viewer's gesture check (`web/shell/src/caps/gesture.ts`) against a page
// that moves focus into itself with window.focus(): input the viewer gives
// the shell never counts, while a click, a Tab into the page, and an area
// drag still do. Nothing reads the frame before the viewer's click: a
// Playwright read or evaluate grants the shell user activation.

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

/** Publishes `html` declaring `capabilities`, owned by a live session (so
 * sendToClaude has an agent that can receive it). */
async function publishLive(title: string, html: string, capabilities: Record<string, unknown>) {
  const s = await registerSession(d.base, d.token);
  const res = await fetch(`${d.base}/api/artifacts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${d.token}`, "x-clax-session": s.id },
    body: JSON.stringify({ title, capabilities, files: { "index.html": { content: html, encoding: "utf8" } } }),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return ((await res.json()) as { artifact: { id: string } }).artifact.id;
}

/** A page that, every 150 ms while armed, pulls focus into itself and makes
 * `verb`'s call ("open": openComposer; "send": sendToClaude), writing the
 * count and the latest result into #status; its button, and the key O, make
 * the same call from the viewer's input (disarming the timer first) into
 * #clicked. */
const PULLER = (verb: "open" | "send", armed = true) => `<!doctype html><html><head><title>Puller</title>
<style>body{margin:0;font:16px sans-serif}main{padding:16px}#b{padding:8px 16px}</style></head>
<body><main><h2 id="t">Title</h2><button id="b">Do it</button><p id="status">0</p><p id="clicked">-</p><p id="focused">no</p></main>
<script>(async () => {
  window.armed = ${armed};
  const c = await claude.use("comments");
  const el = document.getElementById("t");
  const anchor = await c.anchorFor(el);
  const call = () => ${verb === "open" ? `c.openComposer({ element: el })` : `c.sendToClaude({ anchor, text: "From the page." })`};
  const show = r => r && typeof r === "object" && "threadId" in r ? "sent" : JSON.stringify(r);
  let n = 0;
  const s = document.getElementById("status");
  setInterval(async () => {
    if (!window.armed) return;
    window.focus();
    const r = await call().then(show, e => e.code);
    n++;
    s.textContent = n + " " + r;
  }, 150);
  const b = document.getElementById("b");
  b.addEventListener("pointerdown", () => { window.armed = false; });
  b.addEventListener("keydown", () => { window.armed = false; });
  b.addEventListener("focus", () => { document.getElementById("focused").textContent = "yes"; });
  document.addEventListener("keydown", async e => { if (e.key === "o") { window.armed = false; document.getElementById("clicked").textContent = await call().then(show, e => e.code); } });
  b.addEventListener("click", async () => { window.armed = false; document.getElementById("clicked").textContent = await call().then(show, e => e.code); });
})().catch(e => { document.getElementById("status").textContent = "setup " + (e.code || e.message); });</script></body></html>`;

/** The poll count in #status. */
const polls = async (f: Frame) => Number((await f.locator("#status").textContent())!.split(" ")[0]);
/** Waits until the page has polled `n` more times. */
async function pollsMore(f: Frame, n: number) {
  const from = await polls(f);
  await expect.poll(() => polls(f)).toBeGreaterThanOrEqual(from + n);
}

const frameBox = async (page: Page) => (await page.locator("iframe.frame").boundingBox())!;

// A page that, 150 ms after comment mode comes on, pulls focus into itself and
// posts a pick of its own (start and pick, with a magenta screenshot).
const FORGER = `<!doctype html><html><head><title>Forger</title>
<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px}#empty{height:260px;background:#f8fafc}</style></head>
<body><main><h1>Forgery</h1><p id="para">The viewer's own paragraph, worth a comment.</p><div id="empty"></div><p id="forged">0</p></main>
<script>
let n = 0;
async function forge() {
  const c = new OffscreenCanvas(1, 1);
  const ctx = c.getContext("2d");
  ctx.fillStyle = "#ff00ff";
  ctx.fillRect(0, 0, 1, 1);
  const png = await (await c.convertToBlob({ type: "image/png" })).arrayBuffer();
  window.focus();
  const anchor = { kind: "element", selector: "#para", quote: "FORGED BY PAGE", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
  const pickId = "forged" + n;
  parent.postMessage({ type: "clax:pick-start", pickId, version: window.__clax.version, anchor }, "*");
  parent.postMessage({ type: "clax:pick", pickId, version: window.__clax.version, anchor, clipPng: png }, "*");
  document.getElementById("forged").textContent = String(++n);
}
addEventListener("message", e => { if (e.data && e.data.type === "clax:comment-mode" && e.data.on) setTimeout(forge, 150); });
</script></body></html>`;

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a page that pulls focus cannot open the composer after the viewer clicks the shell's name field; the viewer's click in the page can`, async ({ page }) => {
    const id = await publishLive(`Pull open ${mode}`, PULLER("open"), { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, id, 1, mode);
    await page.waitForTimeout(1_000);
    await page.getByRole("textbox", { name: "Your name" }).click();
    await page.waitForTimeout(1_500);
    await expect(page.locator(".composer")).toHaveCount(0);
    await pollsMore(f, 8);
    await expect(f.locator("#status")).not.toContainText("true");
    await expect(page.locator(".composer")).toHaveCount(0);
    // The viewer's own click on the page's button opens it.
    await f.locator("#b").click();
    await expect(f.locator("#clicked")).toHaveText(JSON.stringify({ opened: true }));
    await expect(page.locator(".composer")).toHaveCount(1);
  });

  test(`${mode}: a page that pulls focus cannot ride keys typed in the shell with the pointer resting on the page`, async ({ page }) => {
    const id = await publishLive(`Pull typing ${mode}`, PULLER("open", false), { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, id, 1, mode);
    const name = page.getByRole("textbox", { name: "Your name" });
    await name.click();
    // The pointer comes to rest on the page, and the click's activation lapses.
    const fb = await frameBox(page);
    await page.mouse.move(fb.x + fb.width / 2, fb.y + fb.height - 20, { steps: 4 });
    await page.waitForTimeout(5_500);
    await name.press("S");
    // Armed only now (evaluate grants activation, as the typing does anyway).
    await f.evaluate(() => { (window as unknown as { armed: boolean }).armed = true; });
    await name.pressSequentially("am", { delay: 120 });
    await pollsMore(f, 8);
    await expect(f.locator("#status")).not.toContainText("true");
    await expect(page.locator(".composer")).toHaveCount(0);
  });

  test(`${mode}: a page that pulls focus cannot send to the agent after the viewer clicks the shell's name field; the viewer's click in the page can`, async ({ page }) => {
    const id = await publishLive(`Pull send ${mode}`, PULLER("send"), { comments: {} });
    const f = await openArtifact(page, d.base, id, 1, mode);
    await page.waitForTimeout(1_000);
    await page.getByRole("textbox", { name: "Your name" }).click();
    await page.waitForTimeout(1_500);
    // Refused before consent is asked or anything is written.
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await pollsMore(f, 8);
    await expect(f.locator("#status")).not.toContainText("sent");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const threads = async () => ((await api(d.base, d.token, `/api/artifacts/${id}/threads?include_resolved=true`)) as { threads: { sent_to_agent: boolean }[] }).threads;
    expect(await threads()).toHaveLength(0);
    // The viewer's own click in the page within 5.5 s of their click on the
    // name field is refused with a code the page can show ("click again").
    const named = Date.now();
    await f.locator("#b").click();
    await expect(f.locator("#clicked")).toHaveText("shell_input_recent");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // Past that, their click asks consent and sends.
    await page.waitForTimeout(Math.max(0, 5_700 - (Date.now() - named)));
    await f.locator("#b").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect(f.locator("#clicked")).toHaveText("sent");
    await expect.poll(async () => (await threads()).map(t => t.sent_to_agent)).toEqual([true]);
  });

  test(`${mode}: a Tab from the shell into the page and a key there opens the composer`, async ({ page }) => {
    const id = await publishLive(`Tab ${mode}`, PULLER("open", false), { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, id, 1, mode);
    await page.getByRole("textbox", { name: "Your name" }).click();
    for (let i = 0; i < 30 && (await f.locator("#focused").textContent()) !== "yes"; i++) await page.keyboard.press("Tab");
    await expect(f.locator("#focused")).toHaveText("yes");
    await page.keyboard.press("Enter");
    await expect(f.locator("#clicked")).toHaveText(JSON.stringify({ opened: true }));
    await expect(page.locator(".composer")).toHaveCount(1);
  });

  test(`${mode}: a key in the page after a click there opens the composer with the pointer moved back over the shell`, async ({ page }) => {
    const id = await publishLive(`Keys ${mode}`, PULLER("open", false), { comments: { composer_only: true } });
    const f = await openArtifact(page, d.base, id, 1, mode);
    await f.locator("#t").click();
    await page.getByRole("button", { name: "Comment", exact: true }).hover();
    await page.keyboard.press("o");
    await expect(f.locator("#clicked")).toHaveText(JSON.stringify({ opened: true }));
    await expect(page.locator(".composer")).toHaveCount(1);
  });

  test(`${mode}: a pick the page forges after the viewer clicks Comment opens no composer; the viewer's click and area drag do`, async ({ page }) => {
    const id = await publishLive(`Forger ${mode}`, FORGER, {});
    await record(page);
    const f = await openArtifact(page, d.base, id, 1, mode);
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    const forgedSeen = (n: number) => expect.poll(() => page.evaluate(pid => (window as any).claxMsgs.filter((m: any) => m.pickId === pid).length, `forged${n}`)).toBe(2);
    await comment.click();
    await expect(f.locator("#forged")).toHaveText("1");
    await forgedSeen(0);
    await page.waitForTimeout(500);
    await expect(page.locator(".composer")).toHaveCount(0);
    // The viewer's click on the paragraph.
    await f.locator("#para").click();
    const composer = page.locator(".composer");
    await expect(composer).toHaveCount(1);
    await expect(composer.locator(".composer-quote")).toContainText("worth a comment");
    await composer.getByRole("button", { name: "Cancel" }).click();
    await expect(composer).toHaveCount(0);
    // Cancel brings comment mode back on, and the page forges again on the
    // back of the viewer's Cancel click in the shell: refused.
    await expect(comment).toHaveAttribute("aria-pressed", "true");
    await expect(f.locator("#forged")).toHaveText("2");
    await forgedSeen(1);
    await page.waitForTimeout(500);
    await expect(composer).toHaveCount(0);
    // The viewer turns comment mode off and on (a fresh Comment click for the
    // page to ride): refused again. Then the viewer drags an area over the
    // empty panel.
    await comment.click();
    await expect(comment).toHaveAttribute("aria-pressed", "false");
    await comment.click();
    await expect(f.locator("#forged")).toHaveText("3");
    await forgedSeen(2);
    await page.waitForTimeout(500);
    await expect(composer).toHaveCount(0);
    // boundingBox is in the shell's viewport.
    const box = (await f.locator("#empty").boundingBox())!;
    const x = box.x + 20;
    const y = box.y + 20;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 90, y + 60, { steps: 4 });
    await page.mouse.move(x + 180, y + 120, { steps: 4 });
    await page.mouse.up();
    await expect(composer).toHaveCount(1);
    await expect(composer.locator(".composer-quote")).toHaveText(/^Area in #empty/);
  });
}

// A page that acts only on a signal from the test (`window.go(kind, delayMs)`),
// so each attack runs at a known moment: "open" pulls focus and calls
// openComposer, "send" pulls focus and calls sendToClaude, "forge" pulls focus
// and posts a pick of its own (start and pick, with a magenta screenshot).
// The result goes into #result; `permissions.request` at load (`askAtLoad`) raises the
// consent dialog without any gesture. The page is tall, with #target 600 px
// down, so a scroll-to moves it.
const ATTACKER = (askAtLoad: boolean) => `<!doctype html><html><head><title>Attacker</title>
<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px}#empty{height:200px;background:#f8fafc}#target{position:absolute;top:600px;left:40px;margin:0}#spacer{height:3000px}</style></head>
<body><main><h1 id="t">Attack</h1><p id="para">The viewer's own paragraph, worth a comment.</p><div id="empty"></div><p id="result">-</p><button id="b">Do it</button> <button id="send">Send</button><h2 id="target">Pinned target</h2><div id="spacer"></div></main>
<script>(async () => {
  const c = await claude.use("comments");
  const el = document.getElementById("t");
  const anchor = await c.anchorFor(el);
  const out = document.getElementById("result");
  const show = r => r && typeof r === "object" && "threadId" in r ? "sent" : JSON.stringify(r);
  async function forge() {
    const cv = new OffscreenCanvas(1, 1);
    const ctx = cv.getContext("2d");
    ctx.fillStyle = "#ff00ff";
    ctx.fillRect(0, 0, 1, 1);
    const png = await (await cv.convertToBlob({ type: "image/png" })).arrayBuffer();
    window.focus();
    const a = { kind: "element", selector: "#para", quote: "FORGED BY PAGE", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
    parent.postMessage({ type: "clax:pick-start", pickId: "forged", version: window.__clax.version, anchor: a }, "*");
    parent.postMessage({ type: "clax:pick", pickId: "forged", version: window.__clax.version, anchor: a, clipPng: png }, "*");
    return "forged";
  }
  const act = kind => kind === "forge" ? forge()
    : (window.focus(), kind === "open" ? c.openComposer({ element: el }) : c.sendToClaude({ anchor, text: "From the page, not the viewer." }));
  window.go = (kind, delay = 0) => setTimeout(async () => { out.textContent = await act(kind).then(show, e => e.code); }, delay);
  document.getElementById("b").addEventListener("click", async () => { out.textContent = await act("open").then(show, e => e.code); });
  document.getElementById("send").addEventListener("click", async () => { out.textContent = await act("send").then(show, e => e.code); });
  // The page raises its own consent dialog (permissions.request needs no
  // gesture), and scrolls itself.
  const perm = await claude.use("permissions");
  const asked = r => { document.getElementById("t").dataset.asked = r.comments === "granted" ? "yes" : r.comments; };
  window.ask = () => perm.request(["comments"]).then(asked, e => { document.getElementById("t").dataset.asked = e.code; });
  window.scrollPage = dy => scrollBy(0, dy);
  ${askAtLoad ? `window.ask();` : ""}
  document.getElementById("t").dataset.ready = "yes";
})().catch(e => { document.getElementById("result").textContent = "setup " + (e.code || e.message); });</script></body></html>`;

/** Signals the page to act; returns once it has answered. */
async function go(f: Frame, kind: "open" | "send" | "forge", delayMs = 0) {
  await f.evaluate(([k, ms]) => { document.getElementById("result")!.textContent = "-"; (window as unknown as { go(k: string, ms: number): void }).go(k, ms); }, [kind, delayMs] as const);
  await expect(f.locator("#result")).not.toHaveText("-", { timeout: 10_000 + delayMs });
}

const threadsOf = async (id: string) => ((await api(d.base, d.token, `/api/artifacts/${id}/threads?include_resolved=true`)) as { threads: { id: string; sent_to_agent: boolean; comments: { body: string }[] }[] }).threads;

/** Opens `id` and waits for its page to set up. */
async function openReady(page: Page, id: string, mode: "subdomain" | "sandbox") {
  const f = await openArtifact(page, d.base, id, 1, mode);
  await expect(f.locator("#t")).toHaveAttribute("data-ready", "yes");
  return f;
}

/** The viewer picks the paragraph (moving there from the shell) and gets the composer. */
async function pickPara(page: Page, f: Frame) {
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await f.locator("#para").click();
  const composer = page.locator(".composer");
  await expect(composer.locator(".composer-quote")).toContainText("worth a comment");
  return composer;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const delay of [0, 500]) {
    test(`${mode}: a page forging a pick${delay ? " 500 ms" : ""} after the viewer's Cancel in the composer opens nothing; comment mode resumes and the viewer's next click picks`, async ({ page }) => {
      const id = await publishLive(`Cancel ${mode} ${delay}`, ATTACKER(false), { comments: { composer_only: true } });
      await record(page);
      const f = await openReady(page, id, mode);
      const composer = await pickPara(page, f);
      await composer.getByRole("button", { name: "Cancel" }).click();
      await expect(composer).toHaveCount(0);
      await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "true");
      // The composer has gone from under the resting pointer.
      await page.waitForTimeout(300);
      await go(f, "forge", delay);
      await expect.poll(() => page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.pickId === "forged").length)).toBe(2);
      await page.waitForTimeout(500);
      await expect(composer).toHaveCount(0);
      // The viewer moves on and clicks another element: that pick counts.
      const tb = (await f.locator("#target").boundingBox())!;
      await page.mouse.move(tb.x + 20, tb.y + tb.height / 2, { steps: 8 });
      await page.mouse.down();
      await page.mouse.up();
      await expect(composer.locator(".composer-quote")).toContainText("Pinned target");
    });
  }

  test(`${mode}: a page forging a pick after the viewer's Post opens nothing`, async ({ page }) => {
    const id = await publishLive(`Post ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    await record(page);
    const f = await openReady(page, id, mode);
    const composer = await pickPara(page, f);
    await composer.locator("textarea").fill("A real comment.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    await expect(composer).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "true");
    await page.waitForTimeout(300);
    await go(f, "forge");
    await expect.poll(() => page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.pickId === "forged").length)).toBe(2);
    await page.waitForTimeout(500);
    await expect(composer).toHaveCount(0);
    expect((await threadsOf(id)).map(t => t.comments[0].body)).toEqual(["A real comment."]);
  });

  test(`${mode}: after the viewer clicks Allow for the page's own create, the page cannot send to the agent`, async ({ page }) => {
    const id = await publishLive(`Allow ${mode}`, ATTACKER(true), { comments: {} });
    const f = await openReady(page, id, mode);
    const allow = page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true });
    // The dialog sits over the frame, so its buttons do too.
    const fb = await frameBox(page);
    const ab = (await allow.boundingBox())!;
    expect(ab.x > fb.x && ab.x + ab.width < fb.x + fb.width && ab.y > fb.y && ab.y + ab.height < fb.y + fb.height).toBe(true);
    await allow.click();
    await expect(f.locator("#t")).toHaveAttribute("data-asked", "yes");
    await page.waitForTimeout(300);
    await go(f, "send");
    await expect(f.locator("#result")).toHaveText("claude_unavailable");
    expect((await threadsOf(id)).filter(t => t.sent_to_agent)).toHaveLength(0);
  });

  test(`${mode}: after the viewer answers Allow with Enter, the pointer resting over the page, the page cannot send to the agent`, async ({ page }) => {
    const id = await publishLive(`Allow key ${mode}`, ATTACKER(true), { comments: {} });
    const f = await openReady(page, id, mode);
    const dialog = page.getByRole("dialog");
    const allow = dialog.getByRole("button", { name: "Allow", exact: true });
    await expect(allow).toBeEnabled();
    // The pointer comes to rest on the dialog's backdrop, over the frame.
    const fb = await frameBox(page);
    await page.mouse.move(fb.x + 30, fb.y + fb.height - 30, { steps: 4 });
    await page.keyboard.press("Tab");
    await expect(allow).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(f.locator("#t")).toHaveAttribute("data-asked", "yes");
    await page.waitForTimeout(300);
    await go(f, "send");
    await expect(f.locator("#result")).toHaveText("claude_unavailable");
    expect((await threadsOf(id)).filter(t => t.sent_to_agent)).toHaveLength(0);
  });

  test(`${mode}: after the viewer clicks Don't allow, the page cannot reach sendToClaude or the composer`, async ({ page }) => {
    const id = await publishLive(`Deny ${mode}`, ATTACKER(true), { comments: {} });
    const f = await openReady(page, id, mode);
    await page.getByRole("dialog").getByRole("button", { name: "Don't allow", exact: true }).click();
    await expect(f.locator("#t")).not.toHaveAttribute("data-asked", "yes");
    await page.waitForTimeout(300);
    // Refused at the gesture check (claude_unavailable), not at consent.
    await go(f, "send");
    await expect(f.locator("#result")).toHaveText("claude_unavailable");
    await go(f, "open");
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: false }));
    await expect(page.locator(".composer")).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test(`${mode}: after the viewer clicks a pin that the page's scroll moves away, the page cannot open the composer`, async ({ page }) => {
    const id = await publishLive(`Pin ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "#target", quote: "Pinned target", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "A pinned note.");
    form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form })).status).toBe(201);
    const f = await openReady(page, id, mode);
    const pin = page.locator(".thread-pin");
    await expect(pin).toHaveCount(1);
    const before = (await pin.boundingBox())!;
    await pin.click();
    // The page scrolls the target to the middle: the pin leaves the pointer.
    await expect.poll(async () => (await pin.boundingBox())?.y ?? -1).not.toBe(before.y);
    await page.waitForTimeout(500);
    await go(f, "open");
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: false }));
    await expect(page.locator(".composer")).toHaveCount(0);
  });

  test(`${mode}: after the viewer dismisses a banner over the page, the page cannot open the composer`, async ({ page }) => {
    const id = await publishLive(`Banner ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    // The thread list fails, so the shell says so in a banner.
    await page.route(u => u.pathname === `/api/artifacts/${id}/threads`, route => route.request().method() !== "GET"
      ? route.continue()
      : route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: { code: "internal", message: "down" } }) }));
    const f = await openReady(page, id, mode);
    const banner = page.locator(".banner.notice");
    await expect(banner).toContainText("Could not load comments");
    const fb = await frameBox(page);
    const bb = (await banner.boundingBox())!;
    expect(bb.y > fb.y && bb.x > fb.x && bb.x + bb.width < fb.x + fb.width).toBe(true);
    await banner.getByRole("button", { name: "Dismiss" }).click();
    await expect(banner).toHaveCount(0);
    await page.waitForTimeout(300);
    await go(f, "open");
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: false }));
    await expect(page.locator(".composer")).toHaveCount(0);
  });

  test(`${mode}: a tap on the page's button after a tap on the shell's name field opens the composer`, async ({ browser }) => {
    const id = await publishLive(`Tap ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const ctx = await browser.newContext({ hasTouch: true });
    const page = await ctx.newPage();
    const f = await openReady(page, id, mode);
    const nb = (await page.getByRole("textbox", { name: "Your name" }).boundingBox())!;
    await page.touchscreen.tap(nb.x + nb.width / 2, nb.y + nb.height / 2);
    const bb = (await f.locator("#b").boundingBox())!;
    await page.touchscreen.tap(bb.x + bb.width / 2, bb.y + bb.height / 2);
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
    await expect(page.locator(".composer")).toHaveCount(1);
    await ctx.close();
  });

  test(`${mode}: a Shift+Tab from the shell back into the page and a key there opens the composer`, async ({ page }) => {
    const id = await publishLive(`Shift-Tab ${mode}`, PULLER("open", false), { comments: { composer_only: true } });
    // A thread, so the Threads panel after the page has controls to start from.
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "#t", quote: "Title", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "A note.");
    form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form })).status).toBe(201);
    const f = await openArtifact(page, d.base, id, 1, mode);
    const threads = page.getByRole("button", { name: /^Threads/ });
    await expect(threads).toHaveText("Threads (1)");
    if ((await threads.getAttribute("aria-pressed")) !== "true") await threads.click();
    await page.locator(".sidebar .card-head").first().click();
    await page.keyboard.press("Shift+Tab");
    for (let i = 0; i < 10 && (await f.locator("#focused").textContent()) !== "yes"; i++) await page.keyboard.press("Shift+Tab");
    await expect(f.locator("#focused")).toHaveText("yes");
    await page.keyboard.press("Enter");
    await expect(f.locator("#clicked")).toHaveText(JSON.stringify({ opened: true }));
    await expect(page.locator(".composer")).toHaveCount(1);
  });

  test(`${mode}: a page republishing itself on a timer after the viewer clicked the shell is refused; the viewer's click in the page publishes`, async ({ page }) => {
    const html = `<!doctype html><html><head><title>Autosave</title></head><body><p id="status">-</p><button id="save">Save</button><script>(async () => {
  const a = await claude.use("artifact");
  const next = "<!doctype html><html><body><p id=\\"done\\">republished</p></body></html>";
  const s = document.getElementById("status");
  setInterval(() => { window.focus(); a.publish(next).then(() => { s.textContent = "published"; }, e => { s.textContent = e.code + ": " + e.message; }); }, 300);
  document.getElementById("save").addEventListener("click", () => { a.publish(next).catch(() => {}); });
})();</script></body></html>`;
    const res = await fetch(`${d.base}/api/artifacts`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
      body: JSON.stringify({ title: `Autosave ${mode}`, capabilities: { artifact: {} }, files: { "index.html": { content: html, encoding: "utf8" } } }) });
    const id = ((await res.json()) as { artifact: { id: string } }).artifact.id;
    const f = await openArtifact(page, d.base, id, 1, mode);
    await page.waitForTimeout(1_000);
    await page.getByRole("textbox", { name: "Your name" }).click();
    await page.waitForTimeout(2_000);
    const current = async () => ((await (await fetch(`${d.base}/api/artifacts/${id}`)).json()) as { artifact: { current_version: number } }).artifact.current_version;
    expect(await current()).toBe(1);
    await expect(f.locator("#status")).toHaveText("rate_limited: publish from the viewer's own input in the page, never on load or a timer");
    // More than 5.5 s after the click on the name field, the viewer's click in the page publishes.
    await page.waitForTimeout(3_700);
    await f.locator("#save").click();
    await expect.poll(current).toBe(2);
  });
}

/** The pointer comes to rest on the page at (x, y) of the shell's viewport,
 * moving there in steps from where it is. */
async function restAt(page: Page, x: number, y: number) {
  await page.mouse.move(x, y, { steps: 6 });
  await page.waitForTimeout(100);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const key of ["Enter", "Space"] as const) {
    test(`${mode}: with the pointer resting on the page, the page's own consent dialog answered by ${key} gives it no gesture (A1)`, async ({ page }) => {
      const id = await publishLive(`A1 ${key} ${mode}`, ATTACKER(false), { comments: {} });
      const f = await openReady(page, id, mode);
      await page.getByRole("textbox", { name: "Your name" }).click();
      const fb = await frameBox(page);
      await restAt(page, fb.x + fb.width / 2, fb.y + fb.height / 2);
      // The dialog appears under the resting pointer.
      await f.evaluate(() => { void (window as unknown as { ask(): Promise<void> }).ask(); });
      const allow = page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true });
      await expect(allow).toBeEnabled();
      await page.keyboard.press("Tab");
      await expect(allow).toBeFocused();
      await page.keyboard.press(key);
      await expect(f.locator("#t")).toHaveAttribute("data-asked", "yes");
      await page.waitForTimeout(300);
      await go(f, "send");
      await expect(f.locator("#result")).not.toHaveText("sent");
      expect((await threadsOf(id)).filter(t => t.sent_to_agent)).toHaveLength(0);
      await go(f, "open");
      await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: false }));
    });
  }

  for (const order of ["pin, then typing", "typing, then pin"] as const) {
    test(`${mode}: a pin the page scrolls under the resting pointer and away gives it no gesture on keys typed in the shell (A2, ${order})`, async ({ page }) => {
      const id = await publishLive(`A2 ${order} ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
      const form = new FormData();
      form.set("anchor", JSON.stringify({ kind: "element", selector: "#target", quote: "Pinned target", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
      form.set("body", "A pinned note.");
      form.set("version", "1");
      expect((await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form })).status).toBe(201);
      const f = await openReady(page, id, mode);
      const pin = page.locator(".thread-pin");
      await expect(pin).toHaveCount(1);
      const name = page.getByRole("textbox", { name: "Your name" });
      await name.click();
      const pb = (await pin.boundingBox())!;
      const x = pb.x + pb.width / 2;
      const y = pb.y + pb.height / 2 - 200;
      await restAt(page, x, y);
      const scroll = async (dy: number) => {
        const before = (await pin.boundingBox())!.y;
        await f.evaluate(v => { (window as unknown as { scrollPage(dy: number): void }).scrollPage(v); }, dy);
        await expect.poll(async () => (await pin.boundingBox())?.y ?? before).not.toBe(before);
        await page.waitForTimeout(150);
      };
      if (order === "pin, then typing") {
        await scroll(200);
        await name.pressSequentially("Sam", { delay: 60 });
        await scroll(-200);
      } else {
        await name.pressSequentially("Sam", { delay: 60 });
        await scroll(200);
        await scroll(-200);
      }
      await go(f, "open");
      await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: false }));
      await expect(page.locator(".composer")).toHaveCount(0);
    });
  }
}

// A page that, once armed (`arm(verb)`, or at load with `armAtLoad`), starts
// on its own timer at its first `mousemove`: every 250 ms for 6 s it pulls
// focus into itself and calls sendToClaude ("send") or artifact.publish
// ("publish"), recording each outcome in `window.res`. `ask()` raises its
// consent dialog.
const timerPage = (armAtLoad?: "send" | "publish") => `<!doctype html><html><head><title>Timer</title>
<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px}</style></head>
<body><main><h1 id="t">A page with a timer</h1><p id="p">Some text to pick.</p></main><div style="height:1500px"></div>
<script>(async () => {
  const c = await claude.use("comments");
  const a = await claude.use("artifact");
  const perm = await claude.use("permissions");
  const anchor = await c.anchorFor(document.getElementById("t"));
  window.res = [];
  window.ask = () => perm.request(["comments"]);
  window.arm = verb => addEventListener("mousemove", () => {
    const end = Date.now() + 6000;
    const tick = () => {
      if (Date.now() > end) return;
      window.focus();
      const call = verb === "send" ? c.sendToClaude({ anchor, text: "From the page's timer." }) : a.publish("<!doctype html><html><body><p>republished by the page</p></body></html>");
      call.then(() => window.res.push("ok"), e => window.res.push(e.code));
      setTimeout(tick, 250);
    };
    tick();
  }, { once: true });
  ${armAtLoad ? `window.arm(${JSON.stringify(armAtLoad)});` : ""}
  document.body.dataset.ready = "yes";
})();</script></body></html>`;
const TIMER_PAGE = timerPage();

/** Drops `text` at (x, y) of the shell's viewport, as a drag from another
 * application does (no pointer or key event reaches the shell). */
async function dropText(page: Page, x: number, y: number, text: string) {
  const cdp = await page.context().newCDPSession(page);
  const data = { items: [{ mimeType: "text/plain", data: text }], dragOperationsMask: 1 };
  for (const type of ["dragEnter", "dragOver", "drop"] as const) await cdp.send("Input.dispatchDragEvent", { type, x, y, data });
  await cdp.detach();
}

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const verb of ["send", "publish"] as const) {
    for (const target of ["composer", "name field"] as const) {
      test(`${mode}: text dropped into the ${target}, then a move onto the page, gives the page no strict gesture for ${verb === "send" ? "sendToClaude" : "artifact.publish"} (N4)`, async ({ page }) => {
        const id = await publishLive(`Drop ${verb} ${target} ${mode}`, TIMER_PAGE, { comments: {}, artifact: {} });
        const f = await openArtifact(page, d.base, id, 1, mode);
        await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
        if (verb === "send") {
          // A grant stored once.
          await f.evaluate(() => { void (window as unknown as { ask(): Promise<unknown> }).ask(); });
          const allow = page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true });
          await expect(allow).toBeEnabled();
          await allow.click();
          await expect(page.getByRole("dialog")).toHaveCount(0);
        }
        let at: { x: number; y: number };
        if (target === "composer") {
          await page.getByRole("button", { name: "Comment", exact: true }).click();
          const pb = (await f.locator("#p").boundingBox())!;
          await page.mouse.move(pb.x + 20, pb.y + pb.height / 2, { steps: 8 });
          await page.mouse.down();
          await page.mouse.up();
          const tb = (await page.locator(".composer textarea").boundingBox())!;
          at = { x: tb.x + tb.width / 2, y: tb.y + tb.height / 2 };
        } else {
          const nb = (await page.getByRole("textbox", { name: "Your name" }).boundingBox())!;
          at = { x: nb.x + nb.width / 2, y: nb.y + nb.height / 2 };
        }
        const fb = await frameBox(page);
        await page.mouse.move(at.x, at.y, { steps: 8 });
        // Quiet: more than 5.5 s with no input to the shell before the drop.
        await page.waitForTimeout(6_000);
        await f.evaluate(v => { (window as unknown as { arm(v: string): void }).arm(v); }, verb);
        await dropText(page, at.x, at.y, "quoted text");
        // The viewer moves over the page; the page's timer starts.
        await page.mouse.move(fb.x + 150, fb.y + 300, { steps: 10 });
        await page.waitForTimeout(6_500);
        if (verb === "send") {
          const threads = await threadsOf(id);
          expect(threads.filter(t => t.sent_to_agent)).toHaveLength(0);
          const res = await f.evaluate(() => (window as unknown as { res: string[] }).res);
          expect(res).not.toContain("ok");
          expect(res).toContain("shell_input_recent");
        } else {
          const cur = ((await (await fetch(`${d.base}/api/artifacts/${id}`)).json()) as { artifact: { current_version: number } }).artifact.current_version;
          expect(cur).toBe(1);
        }
      });
    }
  }

  test(`${mode}: after a click on Cancel over the page, at most one pointer move is lost to the page (N8)`, async ({ page }) => {
    const id = await publishLive(`Moves ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const f = await openReady(page, id, mode);
    await f.evaluate(() => { const w = window as unknown as { moves: number[] }; w.moves = []; addEventListener("mousemove", e => w.moves.push(e.clientX), true); });
    const bb = (await f.locator("#b").boundingBox())!;
    await page.mouse.move(bb.x + bb.width / 2, bb.y + bb.height / 2, { steps: 6 });
    await page.mouse.down();
    await page.mouse.up();
    const cancel = page.locator(".composer").getByRole("button", { name: "Cancel" });
    const cb = (await cancel.boundingBox())!;
    const at = { x: Math.round(cb.x + cb.width / 2), y: Math.round(cb.y + cb.height / 2) };
    await page.mouse.move(at.x, at.y, { steps: 8 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(page.locator(".composer")).toHaveCount(0);
    // After the double-click interval, 1 px moves to the left.
    await page.waitForTimeout(700);
    await f.evaluate(() => { (window as unknown as { moves: number[] }).moves.length = 0; });
    for (let dx = 1; dx <= 12; dx++) {
      await page.mouse.move(at.x - dx, at.y);
      await page.waitForTimeout(30);
    }
    const fb = await frameBox(page);
    const got = new Set(await f.evaluate(() => (window as unknown as { moves: number[] }).moves));
    const lost = [];
    for (let dx = 1; dx <= 12; dx++) if (!got.has(Math.floor(at.x - dx - fb.x))) lost.push(dx);
    expect(lost).toHaveLength(1);
  });

  test(`${mode}: the second click of a double-click on Cancel over the page does not reach the page (N9)`, async ({ page }) => {
    const id = await publishLive(`Double ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const f = await openReady(page, id, mode);
    await f.evaluate(() => { const w = window as unknown as { presses: number }; w.presses = 0; addEventListener("pointerdown", () => { w.presses++; }, true); });
    const bb = (await f.locator("#b").boundingBox())!;
    await page.mouse.move(bb.x + bb.width / 2, bb.y + bb.height / 2, { steps: 6 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(page.locator(".composer")).toHaveCount(1);
    const presses = () => f.evaluate(() => (window as unknown as { presses: number }).presses);
    const before = await presses();
    const cb = (await page.locator(".composer").getByRole("button", { name: "Cancel" }).boundingBox())!;
    await page.mouse.move(cb.x + cb.width / 2, cb.y + cb.height / 2, { steps: 8 });
    await page.mouse.dblclick(cb.x + cb.width / 2, cb.y + cb.height / 2);
    await expect(page.locator(".composer")).toHaveCount(0);
    await page.waitForTimeout(300);
    expect(await presses()).toBe(before);
  });

  test(`${mode}: a page posting pick starts cannot show the hint while the viewer is idle on a shell control, moves over the shell, or types in it (N10)`, async ({ page }) => {
    const id = await publishLive(`Hint ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    await page.addInitScript(() => {
      if (window.top !== window) return;
      const w = window as unknown as { hints: number };
      w.hints = 0;
      new MutationObserver(ms => { for (const m of ms) for (const n of m.addedNodes) if (n instanceof Element && n.matches(".gesture-hint")) w.hints++; })
        .observe(document, { childList: true, subtree: true });
    });
    const f = await openReady(page, id, mode);
    // On comment mode, the page posts a pick start of its own every 100 ms.
    await f.evaluate(() => {
      let n = 0;
      addEventListener("message", e => {
        if (e.data?.type === "clax:comment-mode" && e.data.on) setInterval(() => parent.postMessage({ type: "clax:pick-start", pickId: `spam${n++}` }, "*"), 100);
      });
    });
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    const cb = (await comment.boundingBox())!;
    await page.mouse.move(cb.x + cb.width / 2, cb.y + cb.height / 2, { steps: 4 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(comment).toHaveAttribute("aria-pressed", "true");
    // Idle on the Comment button.
    await page.waitForTimeout(1_500);
    // Moving over the shell's sidebar and header.
    const name = page.getByRole("textbox", { name: "Your name" });
    const nb = (await name.boundingBox())!;
    for (let i = 0; i < 20; i++) { await page.mouse.move(nb.x + (i % 10) * 4, nb.y + nb.height / 2 + (i % 5) * 3); await page.waitForTimeout(100); }
    // Typing in the name field, with the pointer resting on the page.
    await page.mouse.down();
    await page.mouse.up();
    const fb = await frameBox(page);
    await page.mouse.move(fb.x + fb.width / 2, fb.y + fb.height - 40, { steps: 6 });
    await name.pressSequentially("Sam Smith", { delay: 120 });
    await page.waitForTimeout(500);
    expect(await page.evaluate(() => (window as unknown as { hints: number }).hints)).toBe(0);
  });

  test(`${mode}: the viewer's own second refused press in the page, after the hint has gone, shows the hint again (N10)`, async ({ page }) => {
    const id = await publishLive(`Hint again ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const f = await openReady(page, id, mode);
    const composer = await pickPara(page, f);
    const cancel = composer.getByRole("button", { name: "Cancel" });
    const cb = (await cancel.boundingBox())!;
    await page.mouse.move(Math.round(cb.x + cb.width / 2), Math.round(cb.y + cb.height / 2), { steps: 6 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(composer).toHaveCount(0);
    // Past the double-click interval the hole is open: the press reaches the
    // page, whose pick is refused.
    await page.waitForTimeout(700);
    await page.mouse.down();
    await page.mouse.up();
    const hint = page.locator(".gesture-hint");
    await expect(hint).toHaveText("Move the pointer to pick");
    await expect(hint).toHaveCount(0, { timeout: 4_000 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(hint).toHaveText("Move the pointer to pick");
    await expect(composer).toHaveCount(0);
  });

  test(`${mode}: a touch tap that lands on a band counts as the finger's arrival: it opens the composer (N12)`, async ({ browser }) => {
    const id = await publishLive(`Touch band ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const ctx = await browser.newContext({ hasTouch: true });
    const page = await ctx.newPage();
    const f = await openReady(page, id, mode);
    const name = page.getByRole("textbox", { name: "Your name" });
    const nb = (await name.boundingBox())!;
    await page.mouse.move(nb.x + nb.width / 2, nb.y + nb.height / 2, { steps: 4 });
    await page.mouse.down();
    await page.mouse.up();
    // The mouse comes to rest on the page, and a key raises the bands.
    const bb = (await f.locator("#b").boundingBox())!;
    await page.mouse.move(bb.x + bb.width / 2 + 40, bb.y + 60, { steps: 6 });
    await page.keyboard.press("S");
    await expect(page.locator(".frame-shield")).toHaveCSS("display", "block");
    // The tap lowers the bands, and its click, hit-tested after, reaches the page.
    await page.touchscreen.tap(bb.x + bb.width / 2, bb.y + bb.height / 2);
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
    await ctx.close();
  });
}

/** Stores the page's comments grant: the page raises its consent dialog and
 * the viewer allows it. */
async function grant(page: Page, f: Frame) {
  await f.evaluate(() => { void (window as unknown as { ask(): Promise<unknown> }).ask(); });
  const allow = page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true });
  await expect(allow).toBeEnabled();
  await allow.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
}

/** After the page's timer has run: nothing was sent to the agent (and the
 * page was refused at the gesture check, before any consent dialog), or the
 * artifact is still at v1. */
async function expectRefused(page: Page, f: Frame | null, id: string, verb: "send" | "publish") {
  if (verb === "send") {
    expect((await threadsOf(id)).filter(t => t.sent_to_agent)).toHaveLength(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    if (f) expect(await f.evaluate(() => (window as unknown as { res: string[] }).res)).not.toContain("ok");
  } else {
    const cur = ((await (await fetch(`${d.base}/api/artifacts/${id}`)).json()) as { artifact: { current_version: number } }).artifact.current_version;
    expect(cur).toBe(1);
  }
}

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const verb of ["send", "publish"] as const) {
    const call = verb === "send" ? "sendToClaude" : "artifact.publish";
    for (const how of ["a composition, then its commit", "a commit alone (the emoji picker, dictation)"] as const) {
      test(`${mode}: ${how} from an input method into the name field, then a move onto the page, gives the page no strict gesture for ${call} (N11)`, async ({ page }) => {
        const id = await publishLive(`IME ${verb} ${how.slice(0, 8)} ${mode}`, TIMER_PAGE, { comments: {}, artifact: {} });
        const f = await openArtifact(page, d.base, id, 1, mode);
        await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
        if (verb === "send") await grant(page, f);
        // Focus in the name field, by the viewer's click.
        const nb = (await page.getByRole("textbox", { name: "Your name" }).boundingBox())!;
        await page.mouse.move(nb.x + nb.width / 2, nb.y + nb.height / 2, { steps: 8 });
        await page.mouse.down();
        await page.mouse.up();
        await f.evaluate(v => { (window as unknown as { arm(v: string): void }).arm(v); }, verb);
        // Quiet: more than 5.5 s with no input to the shell before the commit.
        await page.waitForTimeout(6_000);
        const cdp = await page.context().newCDPSession(page);
        if (how.startsWith("a composition")) {
          await cdp.send("Input.imeSetComposition", { text: "かな", selectionStart: 2, selectionEnd: 2 });
          await page.waitForTimeout(150);
          await cdp.send("Input.insertText", { text: "仮名" });
        } else {
          await cdp.send("Input.insertText", { text: "👍" });
        }
        await cdp.detach();
        // The viewer moves over the page; the page's timer starts.
        const fb = await frameBox(page);
        await page.mouse.move(fb.x + 150, fb.y + 300, { steps: 10 });
        await page.waitForTimeout(6_500);
        await expectRefused(page, f, id, verb);
      });
    }

    for (const how of ["a key", "a click"] as const) {
      test(`${mode}: ${how} on the shell before its script runs, then a move onto the page as it loads, gives the page no strict gesture for ${call} (N13)`, async ({ page }) => {
        const id = await publishLive(`Load ${verb} ${how} ${mode}`, timerPage(verb), { comments: {}, artifact: {} });
        // Learn the frame's box on a first load; the layout is the same next time.
        const f0 = await openArtifact(page, d.base, id, 1, mode);
        await expect(f0.locator("body")).toHaveAttribute("data-ready", "yes");
        const fb = await frameBox(page);
        // Reload with the shell's script held back until the viewer's input.
        let release!: () => void;
        const held = new Promise<void>(r => { release = r; });
        // The module script the artifact page's HTML itself names.
        const html = await (await page.request.get(`${d.base}/a/${id}`)).text();
        const entry = /<script type="module"[^>]*\ssrc="([^"]+)"/.exec(html)?.[1];
        expect(entry, "the artifact entry's module script").toMatch(/^\/_clax\/shell\/.+\.js$/);
        let heldHits = 0;
        await page.route(u => u.pathname === entry, async route => { heldHits++; await held; await route.continue(); });
        await page.goto(`${d.base}/a/${id}`, { waitUntil: "commit" });
        await page.waitForTimeout(300);
        if (how === "a key") await page.keyboard.press("a");
        else { await page.mouse.move(40, 40); await page.mouse.down(); await page.mouse.up(); }
        release();
        // The viewer moves over the page as it shows; its timer starts.
        for (let i = 0; i < 12; i++) {
          await page.waitForTimeout(250);
          await page.mouse.move(fb.x + fb.width / 2 + (i % 2) * 30, fb.y + fb.height / 2 + i * 5, { steps: 3 });
        }
        await page.waitForTimeout(4_000);
        expect(heldHits, "the shell's script was held").toBe(1);
        await expectRefused(page, null, id, verb);
      });
    }

    for (const host of ["plain", "open", "closed"] as const) for (const from of ["the shell", "the page"] as const) test(`${mode}: with focus in ${from}, a click in another frame in the shell document (a password manager's menu${host === "plain" ? "" : `, in a ${host} shadow root`}), then a move onto the page, gives the page no strict gesture for ${call} (N14)`, async ({ page }) => {
      const id = await publishLive(`Other frame ${verb} ${host} ${mode}`, TIMER_PAGE, { comments: {}, artifact: {} });
      const f = await openArtifact(page, d.base, id, 1, mode);
      await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
      if (verb === "send") await grant(page, f);
      // A frame of another origin beside the name field, as an extension injects.
      const nb = (await page.getByRole("textbox", { name: "Your name" }).boundingBox())!;
      const xf = { x: nb.x, y: nb.y + nb.height + 40, w: 180, h: 50 };
      const src = d.base.replace("localhost", "127.0.0.1") + "/healthz";
      // Plain, or inside a shadow root on a custom element, as password
      // managers inject their inline menus. Clicked only once its document
      // has loaded: input sent at a frame of another site while its process
      // is still starting can wait for that frame indefinitely, and nothing
      // can find a frame in a closed shadow root to wait on it later.
      await page.evaluate(({ url, r, how }) => new Promise<void>((resolve, reject) => {
        const i = document.createElement("iframe");
        i.addEventListener("load", () => resolve(), { once: true });
        setTimeout(() => reject(new Error("the injected frame did not load in 20 s")), 20_000);
        i.src = url;
        Object.assign(i.style, { display: "block", width: `${r.w}px`, height: `${r.h}px`, background: "white" });
        const box = document.createElement(how === "plain" ? "div" : "x-menu");
        Object.assign(box.style, { position: "fixed", left: `${r.x}px`, top: `${r.y}px`, zIndex: "9999", display: "block" });
        if (how === "plain") box.appendChild(i); else box.attachShadow({ mode: how }).appendChild(i);
        document.documentElement.appendChild(box);
      }), { url: src, r: xf, how: host });
      // Focus where the viewer left it: the name field, or the page's text.
      const at = from === "the shell" ? { x: nb.x + nb.width / 2, y: nb.y + nb.height / 2 } : await (async () => { const b = (await f.locator("#p").boundingBox())!; return { x: b.x + 20, y: b.y + b.height / 2 }; })();
      await page.mouse.move(at.x, at.y, { steps: 8 });
      await page.mouse.down();
      await page.mouse.up();
      await f.evaluate(v => { (window as unknown as { arm(v: string): void }).arm(v); }, verb);
      await page.waitForTimeout(6_000);
      await page.mouse.move(xf.x + 40, xf.y + 20, { steps: 5 });
      await page.mouse.down();
      await page.mouse.up();
      const fb = await frameBox(page);
      await page.mouse.move(fb.x + fb.width / 2, fb.y + fb.height / 2, { steps: 10 });
      await page.waitForTimeout(6_500);
      await expectRefused(page, f, id, verb);
    });
  }

  test(`${mode}: after a click in the page, a wheel over the shell and a click back in the page still count`, async ({ page }) => {
    const id = await publishLive(`Wheel shell ${mode}`, ATTACKER(false), { comments: { composer_only: true } });
    const f = await openReady(page, id, mode);
    const pb = (await f.locator("#para").boundingBox())!;
    await page.mouse.move(pb.x + 20, pb.y + pb.height / 2, { steps: 8 });
    await page.mouse.down();
    await page.mouse.up();
    // Focus stays in the page while the viewer scrolls the shell's header.
    const nb = (await page.getByRole("textbox", { name: "Your name" }).boundingBox())!;
    await page.mouse.move(nb.x + nb.width / 2, nb.y + nb.height / 2, { steps: 8 });
    await page.mouse.wheel(0, 40);
    await page.waitForTimeout(200);
    const bb = (await f.locator("#b").boundingBox())!;
    await page.mouse.move(bb.x + bb.width / 2, bb.y + bb.height / 2, { steps: 8 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
  });
}
