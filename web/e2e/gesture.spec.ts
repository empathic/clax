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
    headers: { "content-type": "application/json", authorization: `Bearer ${d.token}`, "x-artifax-session": s.id },
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
  parent.postMessage({ type: "artifax:pick-start", pickId }, "*");
  parent.postMessage({ type: "artifax:pick", pickId, version: window.__artifax.version, anchor, clipPng: png }, "*");
  document.getElementById("forged").textContent = String(++n);
}
addEventListener("message", e => { if (e.data && e.data.type === "artifax:comment-mode" && e.data.on) setTimeout(forge, 150); });
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
    // The viewer's own click asks consent and sends.
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
    const forgedSeen = (n: number) => expect.poll(() => page.evaluate(pid => (window as any).artifaxMsgs.filter((m: any) => m.pickId === pid).length, `forged${n}`)).toBe(2);
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
    // Again, then an area drag over the empty panel.
    await comment.click();
    await expect(f.locator("#forged")).toHaveText("2");
    await forgedSeen(1);
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
