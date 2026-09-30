import { test, expect, type Frame, type Page } from "@playwright/test";
import { api, openArtifact, registerSession, startDaemon } from "./fixtures";

// Commenting several times in a row, as a viewer does it: each flow is driven
// with page.mouse in steps (so the shell sees realistic moves) and ends either
// working on the first try or showing the shell's hint, never failing
// silently (the gesture rules are in web/shell/src/caps/gesture.ts).

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

// Two paragraphs at the top left, and a large empty area below and to the
// right, where the composer's buttons sit over the page.
const PAGE = `<!doctype html><html><head><title>Flows</title>
<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px;max-width:420px}#space{height:2000px}</style></head>
<body><main><p id="p1">First paragraph, worth a comment.</p><p id="p2">Second paragraph, worth another.</p>
<p><button id="open">Comment here</button> <button id="send">Send to the agent</button></p><p id="result">-</p></main><div id="space"></div>
<script>(async () => {
  const c = await claude.use("comments");
  const el = document.getElementById("p1");
  const anchor = await c.anchorFor(el);
  const out = document.getElementById("result");
  const show = r => r && typeof r === "object" && "threadId" in r ? "sent" : JSON.stringify(r);
  document.getElementById("open").addEventListener("click", async () => { out.textContent = await c.openComposer({ element: el }).then(show, e => e.code); });
  document.getElementById("send").addEventListener("click", async () => { out.textContent = await c.sendToClaude({ anchor, text: "From the page's button." }).then(show, e => e.code); });
  document.body.dataset.ready = "yes";
})();</script></body></html>`;

async function publishLive(title: string, withThread = false) {
  const s = await registerSession(d.base, d.token);
  const res = await fetch(`${d.base}/api/artifacts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${d.token}`, "x-artifax-session": s.id },
    body: JSON.stringify({ title, capabilities: { comments: {} }, files: { "index.html": { content: PAGE, encoding: "utf8" } } }),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  const id = ((await res.json()) as { artifact: { id: string } }).artifact.id;
  if (withThread) {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "#p1", quote: "First paragraph, worth a comment.", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "An earlier note.");
    form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form })).status).toBe(201);
  }
  return id;
}

async function open(page: Page, id: string, mode: "subdomain" | "sandbox") {
  const f = await openArtifact(page, d.base, id, 1, mode);
  await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
  return f;
}

/** The centre of `sel` in the frame, in the shell's viewport. */
async function centre(f: Frame, sel: string) {
  const b = (await f.locator(sel).boundingBox())!;
  return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
}

/** Moves to `sel` in steps and clicks it. */
async function clickIn(page: Page, f: Frame, sel: string) {
  const c = await centre(f, sel);
  await page.mouse.move(c.x, c.y, { steps: 10 });
  await page.mouse.down();
  await page.mouse.up();
}

/** Moves to a shell control in steps and clicks it. */
async function clickShell(page: Page, loc: import("@playwright/test").Locator) {
  const b = (await loc.boundingBox())!;
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 10 });
  await page.mouse.down();
  await page.mouse.up();
}

type P = { x: number; y: number };
/** A hand starting from rest: step lengths 1, 1, 2, 3, 5, … px, one event
 * each about 8 ms apart, from `a` toward `b`; ends on `b`. */
async function glide(page: Page, a: P, b: P) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = Math.hypot(dx, dy);
  let s = 0;
  for (const step of [1, 1, 2, 3, 5, 8, 13, 20, 30, 40, 40, 40, 40, 40, 40, 40]) {
    s += step;
    if (s >= len) break;
    await page.mouse.move(a.x + (dx * s) / len, a.y + (dy * s) / len);
    await page.waitForTimeout(8);
  }
  await page.mouse.move(b.x, b.y);
}

/** The centre of `loc`, rounded to whole pixels (a press point at which a
 * boundary event lands exactly 2 px away after a 1, 1 px start). */
async function whole(loc: import("@playwright/test").Locator) {
  const b = (await loc.boundingBox())!;
  return { x: Math.round(b.x + b.width / 2), y: Math.round(b.y + b.height / 2) };
}

const composer = (page: Page) => page.locator(".composer");
const quote = (page: Page) => page.locator(".composer .composer-quote");
const hint = (page: Page) => page.locator(".gesture-hint");

/** Comment mode on, then a first pick of #p1 and a comment typed at once
 * (the composer's textarea has focus). */
async function firstPick(page: Page, f: Frame) {
  await clickShell(page, page.getByRole("button", { name: "Comment", exact: true }));
  await clickIn(page, f, "#p1");
  await expect(quote(page)).toContainText("First paragraph");
  await expect(composer(page).locator("textarea")).toBeFocused();
  await page.keyboard.type("A first comment.");
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: flow 1, Post by click, then click a different element: works`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 1 ${mode}`), mode);
    await firstPick(page, f);
    await clickShell(page, composer(page).getByRole("button", { name: "Post comment" }));
    await expect(composer(page)).toHaveCount(0);
    await clickIn(page, f, "#p2");
    await expect(quote(page)).toContainText("Second paragraph");
  });

  test(`${mode}: flow 2, Post with the keyboard, pointer resting on the page, then click a different element: works`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 2 ${mode}`), mode);
    await firstPick(page, f);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(composer(page).getByRole("button", { name: "Post comment" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(composer(page)).toHaveCount(0);
    await clickIn(page, f, "#p2");
    await expect(quote(page)).toContainText("Second paragraph");
  });

  test(`${mode}: flow 3, after Post, drag an area without moving first: the hint shows, and after a move the drag works`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 3 ${mode}`), mode);
    await firstPick(page, f);
    const post = composer(page).getByRole("button", { name: "Post comment" });
    const pb = (await post.boundingBox())!;
    const at = { x: pb.x + pb.width / 2, y: pb.y + pb.height / 2 };
    await clickShell(page, post);
    await expect(composer(page)).toHaveCount(0);
    // The pointer rests where Post was, over the page's empty area: a drag
    // from there, without a move first (a moment later, as a hand would).
    await page.waitForTimeout(100);
    await page.mouse.down();
    await page.mouse.move(at.x - 120, at.y - 80, { steps: 6 });
    await page.mouse.up();
    await expect(hint(page)).toHaveText("Move the pointer to pick");
    await expect(composer(page)).toHaveCount(0);
    // After a move, a drag over the empty area draws an area.
    await page.mouse.move(at.x - 200, at.y - 150, { steps: 6 });
    await page.mouse.down();
    await page.mouse.move(at.x - 60, at.y - 60, { steps: 6 });
    await page.mouse.up();
    await expect(quote(page)).toHaveText(/^Area in/);
  });

  test(`${mode}: flow 4, Cancel, then click the same spot: the hint shows, and after a move a click works`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 4 ${mode}`), mode);
    await firstPick(page, f);
    await clickShell(page, composer(page).getByRole("button", { name: "Cancel" }));
    await expect(composer(page)).toHaveCount(0);
    // The same spot, a moment later, as a hand would click again.
    await page.waitForTimeout(100);
    await page.mouse.down();
    await page.mouse.up();
    await expect(hint(page)).toHaveText("Move the pointer to pick");
    await expect(composer(page)).toHaveCount(0);
    await clickIn(page, f, "#p2");
    await expect(quote(page)).toContainText("Second paragraph");
  });

  test(`${mode}: flow 5, a reply in the sidebar, then a click in the page: works`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 5 ${mode}`, true), mode);
    const threads = page.getByRole("button", { name: /^Threads/ });
    await expect(threads).toHaveText("Threads (1)");
    if ((await threads.getAttribute("aria-pressed")) !== "true") await clickShell(page, threads);
    await clickShell(page, page.getByRole("button", { name: "Comment", exact: true }));
    const reply = page.getByRole("textbox", { name: "Reply" }).first();
    await clickShell(page, reply);
    await page.keyboard.type("A reply.");
    await page.keyboard.press("Enter");
    await expect(page.locator(".thread-card").first()).toContainText("A reply.");
    await clickIn(page, f, "#p2");
    await expect(quote(page)).toContainText("Second paragraph");
  });

  test(`${mode}: flow 6, the page's buttons after the name field: openComposer works; sendToClaude asks to click again within 5.5 s, then sends`, async ({ page }) => {
    const id = await publishLive(`Flow 6 ${mode}`);
    const f = await open(page, id, mode);
    const name = page.getByRole("textbox", { name: "Your name" });
    await clickShell(page, name);
    await page.keyboard.type("Sam");
    await page.keyboard.press("Enter");
    const typed = Date.now();
    await clickIn(page, f, "#open");
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
    await clickShell(page, composer(page).getByRole("button", { name: "Cancel" }));
    await clickIn(page, f, "#send");
    await expect(f.locator("#result")).toHaveText("shell_input_recent");
    // The Cancel click was shell input too: 5.5 s after it, a click sends.
    await page.waitForTimeout(5_700);
    expect(Date.now() - typed).toBeGreaterThan(5_500);
    await clickIn(page, f, "#p2");
    await clickIn(page, f, "#send");
    const allow = page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true });
    await expect(allow).toBeEnabled();
    await clickShell(page, allow);
    await expect(f.locator("#result")).toHaveText("sent");
    expect(((await api(d.base, d.token, `/api/artifacts/${id}/threads`)) as { threads: { sent_to_agent: boolean }[] }).threads.map(t => t.sent_to_agent)).toEqual([true]);
  });

  test(`${mode}: flow 1 with a hand's ease-in move (1, 1, 2 px) from a whole-pixel Post: works (N12)`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 1 ease ${mode}`), mode);
    await firstPick(page, f);
    const post = await whole(composer(page).getByRole("button", { name: "Post comment" }));
    await page.mouse.move(post.x, post.y, { steps: 10 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(composer(page)).toHaveCount(0);
    await page.waitForTimeout(100);
    await glide(page, post, await centre(f, "#p2"));
    await page.mouse.down();
    await page.mouse.up();
    await expect(quote(page)).toContainText("Second paragraph");
  });

  test(`${mode}: flow 4 with a hand's ease-in move (1, 1, 2 px) after the hint: works (N12)`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 4 ease ${mode}`), mode);
    await firstPick(page, f);
    const cancel = await whole(composer(page).getByRole("button", { name: "Cancel" }));
    await page.mouse.move(cancel.x, cancel.y, { steps: 10 });
    await page.mouse.down();
    await page.mouse.up();
    await expect(composer(page)).toHaveCount(0);
    await page.waitForTimeout(100);
    await page.mouse.down();
    await page.mouse.up();
    await expect(hint(page)).toHaveText("Move the pointer to pick");
    await glide(page, cancel, await centre(f, "#p2"));
    await page.mouse.down();
    await page.mouse.up();
    await expect(quote(page)).toContainText("Second paragraph");
  });

  test(`${mode}: flow 6 variant, then a hand's ease-in move (1, 1, 2 px) after the hint: works (N12)`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 6b ease ${mode}`), mode);
    await clickShell(page, page.getByRole("textbox", { name: "Your name" }));
    const c = await whole(f.locator("#open"));
    await page.mouse.move(c.x, c.y, { steps: 10 });
    await page.keyboard.type("Sam", { delay: 120 });
    await page.waitForTimeout(300);
    await page.mouse.down();
    await page.mouse.up();
    await expect(hint(page)).toHaveText("Move the pointer, then click again");
    await glide(page, c, { x: c.x + 14, y: c.y });
    await page.mouse.down();
    await page.mouse.up();
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
  });

  test(`${mode}: flow 6 variant, then a wheel on a band: the page's button works after it (N12)`, async ({ page }) => {
    const f = await open(page, await publishLive(`Flow 6b wheel ${mode}`), mode);
    await clickShell(page, page.getByRole("textbox", { name: "Your name" }));
    const c = await whole(f.locator("#open"));
    await page.mouse.move(c.x, c.y, { steps: 10 });
    await page.keyboard.type("Sam", { delay: 120 });
    await page.waitForTimeout(300);
    await expect(page.locator(".frame-shield")).toHaveCSS("display", "block");
    await page.mouse.wheel(0, 40);
    await page.waitForTimeout(300);
    await page.mouse.wheel(0, -40);
    await page.waitForTimeout(300);
    const c2 = await whole(f.locator("#open"));
    await page.mouse.move(c2.x, c2.y);
    await page.mouse.down();
    await page.mouse.up();
    await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
  });

  for (const how of ["keys typed with the mouse resting on the page", "Post clicked with the mouse"] as const) {
    test(`${mode}: a finger's first tap on the page after ${how}: works (N12)`, async ({ browser }) => {
      const ctx = await browser.newContext({ hasTouch: true });
      const page = await ctx.newPage();
      const f = await open(page, await publishLive(`Hybrid ${how.slice(0, 4)} ${mode}`), mode);
      if (how === "Post clicked with the mouse") {
        await firstPick(page, f);
        await clickShell(page, composer(page).getByRole("button", { name: "Post comment" }));
        await expect(composer(page)).toHaveCount(0);
      } else {
        await clickShell(page, page.getByRole("textbox", { name: "Your name" }));
        const b = await centre(f, "#open");
        await page.mouse.move(b.x + 150, b.y + 200, { steps: 10 });
        await page.keyboard.type("Sam", { delay: 120 });
        await expect(page.locator(".frame-shield")).toHaveCSS("display", "block");
      }
      await page.waitForTimeout(700);
      const target = await centre(f, how === "Post clicked with the mouse" ? "#p2" : "#open");
      const done = how === "Post clicked with the mouse"
        ? async () => (await quote(page).count()) > 0 && ((await quote(page).textContent()) ?? "").includes("Second paragraph")
        : async () => (await f.locator("#result").textContent()) === JSON.stringify({ opened: true });
      // The tap lands on a band: it counts as the finger's arrival, and its
      // click, hit-tested after the bands come down, reaches the page.
      await expect(page.locator(".frame-shield")).toHaveCSS("display", "block");
      await page.touchscreen.tap(target.x, target.y);
      await expect.poll(done).toBe(true);
      await ctx.close();
    });
  }

  for (const delay of [30, 120]) {
    test(`${mode}: flow 6 variant, typing in the name field at ${delay} ms a key with the pointer resting on the page: a click without moving shows the hint; after a move it works`, async ({ page }) => {
      const f = await open(page, await publishLive(`Flow 6b ${delay} ${mode}`), mode);
      const name = page.getByRole("textbox", { name: "Your name" });
      await clickShell(page, name);
      const c = await centre(f, "#open");
      await page.mouse.move(c.x, c.y, { steps: 10 });
      await page.keyboard.type("Sam", { delay });
      // The pointer moved inside the page, where the shell cannot see it, so
      // the bands raised at the keys cover it: the press reaches nothing, and
      // the viewer is told.
      await page.mouse.down();
      await page.mouse.up();
      await expect(hint(page)).toHaveText("Move the pointer, then click again");
      await expect(f.locator("#result")).toHaveText("-");
      await expect(composer(page)).toHaveCount(0);
      await page.mouse.move(c.x + 12, c.y, { steps: 3 });
      await page.mouse.down();
      await page.mouse.up();
      await expect(f.locator("#result")).toHaveText(JSON.stringify({ opened: true }));
    });
  }
}
