// The Clax app's view (`/a/<id>?app=1`): no chrome, `window.claxHost` for the
// app that hosts it, messages to the app's channel (`window.ClaxHost`), Send,
// a newer version, published or restored, offered to the app and swapped in
// place on its reloadLatest with the page's documents and scroll kept, and
// the app's own history of pages and fragments. The app's side is ./app-host.ts.
import { type CDPSession, type Frame, type Locator, type Page } from "@playwright/test";
import { hostChannel, host, initHost, NONCE, noFrames, ofType, toHost } from "./app-host";
import { test, expect, type Daemon, api, contentFrame, publishAs, publishNext, record, registerSession, streamLive } from "./fixtures";
import { advance, quietEval } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const PAGE = `<!doctype html><html><head><title>App</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:24px}h2{margin:0 0 24px}.tall{height:3000px}</style>
</head>
<body><main><h2 id="t">Quarterly goals</h2><p>Grow revenue in every region this year.</p><div class="tall"></div><p id="end">The end</p></main></body></html>`;

async function openApp(page: Page, title: string, mode: "subdomain" | "sandbox", app = true, opts: { beforeGoto?: () => Promise<void>; html?: string; capabilities?: Record<string, unknown>; init?: boolean } = {}) {
  const s = await registerSession(d.base, d.token, "app");
  const html = opts.html ?? PAGE;
  const { artifact } = opts.capabilities
    ? await api(d.base, d.token, "/api/artifacts", { method: "POST", session: s.id, body: JSON.stringify({ title, capabilities: opts.capabilities, files: { "index.html": { content: html, encoding: "utf8" } } }) }) as { artifact: { id: string } }
    : await publishAs(d.base, d.token, s.id, title, { "index.html": html });
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  await hostChannel(page);
  await record(page);
  await opts.beforeGoto?.();
  await page.goto(`${d.base}/a/${artifact.id}${app ? "?app=1" : ""}`);
  const frame = await contentFrame(page, artifact.id, 1);
  if (app && opts.init !== false) await initHost(page);
  return { id: artifact.id, sid: s.id, frame };
}

/** The selectors of the outlines the page has shown the shell so far. */
const hovers = (page: Page) => quietEval<string[]>(page, "window.claxMsgs.filter(m => m.type === 'clax:hover').map(m => m.selector)");

/** Holds one finger on `target` until the bridge's hold arms (it outlines
 * what is under the finger, `selector`), then lifts it. */
async function holdOn(page: Page, cdp: CDPSession, target: Locator, selector: string) {
  const b = (await target.boundingBox())!;
  const at = { x: b.x + Math.min(40, b.width / 2), y: b.y + b.height / 2 };
  const seen = (await hovers(page)).length;
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
  await expect.poll(async () => (await hovers(page)).slice(seen)).toContain(selector);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

/** The last scroll position the page reported to the shell. */
const reported = (page: Page) => page.evaluate(() => (window as unknown as { claxMsgs: { type: string; y?: number }[] }).claxMsgs.filter(m => m.type === "clax:scroll").at(-1)?.y ?? null);

/** Scrolls the page to about `y` until the shell has heard where it is, and
 * returns that position. The page reports its scroll once the welcome's part
 * has loaded, which may be after a first scroll: it moves by a pixel until a
 * report of where it is arrives. */
async function scrollAndReport(page: Page, frame: Frame, y: number): Promise<number> {
  await expect.poll(async () => {
    const at = await frame.evaluate(t => { scrollTo(0, scrollY === t ? t + 1 : t); return scrollY; }, y);
    const r = await reported(page);
    return r !== null && Math.abs(r - at) <= 1;
  }).toBe(true);
  return (await reported(page))!;
}

/** Expects the frame at `y` within 2 px, once the restore has run. */
async function expectScrollNear(frame: Frame, y: number) {
  await expect.poll(() => frame.evaluate(() => scrollY)).toBeGreaterThanOrEqual(y - 2);
  expect(await frame.evaluate(() => scrollY)).toBeLessThanOrEqual(y + 2);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: app=1 hides the chrome and posts ready`, async ({ page }) => {
    const { id } = await openApp(page, `App chrome ${mode}`, mode);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    expect((await ofType(page, "ready"))[0]).toEqual({ type: "ready", artifact: id, version: 1, latest: 1, file: "index.html" });
    await expect(page.locator(".topbar")).toBeHidden();
    await expect(page.locator(".phone-tabs")).toBeHidden();
    const box = (await page.locator("iframe.frame").boundingBox())!;
    expect(box.y).toBe(0);
    expect(box.height).toBe(800);
  });

  test(`${mode}: the page cannot reach claxHost`, async ({ page }) => {
    const { frame } = await openApp(page, `App forge ${mode}`, mode);
    expect(await quietEval<string>(page, "typeof window.claxHost.attestPointer")).toBe("function");
    // The page's realm is another origin (or an opaque one): the shell's
    // window properties throw there.
    const tried = await frame.evaluate(() => {
      const out: string[] = [];
      for (const w of [window.parent, window.top]) {
        try { (w as unknown as { claxHost: { attestPointer(p: unknown): void } }).claxHost.attestPointer({ x: 1, y: 1 }); out.push("reached"); } catch (e) { out.push((e as Error).name); }
      }
      return out;
    });
    expect(tried).toEqual(["SecurityError", "SecurityError"]);
    // Nor can it learn the nonce: it hears none of the shell's channel
    // messages, and nothing the shell posts it carries the nonce.
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    const heard = await frame.evaluate(() => JSON.stringify((window as unknown as { claxMsgs?: unknown[] }).claxMsgs ?? []));
    expect(heard).not.toContain(NONCE);
    expect(await frame.evaluate(() => JSON.stringify((window as unknown as { claxToHost?: unknown }).claxToHost ?? null))).toBe("null");
  });

  test(`${mode}: a page that greets before the host module loads still gets ready and app mode`, async ({ page }) => {
    // The host module is held until the page has greeted the shell and been
    // welcomed without it, as on a slow phone.
    let held = 0;
    const { frame } = await openApp(page, `App late host ${mode}`, mode, true, {
      beforeGoto: async () => { await page.route(/\/_clax\/shell\/host-[^/]*\.js/, async route => {
        held++;
        await expect.poll(() => quietEval<boolean>(page, "(window.claxMsgs ?? []).some(m => m.type === 'clax:hello')")).toBe(true);
        await route.continue();
      }); },
    });
    expect(held).toBe(1);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    // Welcomed again with app: true, the page reports its scroll.
    await expect.poll(async () => {
      await frame.evaluate(() => scrollTo(0, scrollY === 300 ? 301 : 300));
      return quietEval<boolean>(page, "window.claxMsgs.some(m => m.type === 'clax:scroll')");
    }).toBe(true);
  });

  test(`${mode}: a browser's view has no claxHost and keeps its chrome`, async ({ page }) => {
    await openApp(page, `App browser ${mode}`, mode, false);
    await expect(page.locator(".topbar")).toBeVisible();
    expect(await quietEval<string>(page, "typeof window.claxHost")).toBe("undefined");
    expect(await quietEval<boolean>(page, "document.documentElement.classList.contains('app')")).toBe(false);
    expect(await toHost(page)).toEqual([]);
  });
}

for (const frames of [true, false]) {
test(`a long-press picks and Send sends to the app session${frames ? "" : ", with no animation frames"}`, async ({ page }) => {
  if (!frames) await noFrames(page);
  const { id, sid, frame } = await openApp(page, `App send${frames ? "" : " no frames"}`, "subdomain");
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await host(page, "enterCommentMode()");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  const cdp = await page.context().newCDPSession(page);
  await holdOn(page, cdp, frame.locator("#t"), "#t");
  await expect.poll(async () => (await ofType(page, "state")).at(-1)?.draft ?? null).toMatchObject({ kind: "element", label: "“Quarterly goals”", quote: "Quarterly goals" });
  // The app's composer is the send box: the shell's is not shown.
  await expect(page.locator(".composer")).toBeAttached();
  await expect(page.locator(".composer")).toBeHidden();
  await expect(page.getByRole("button", { name: "Send", exact: true })).toHaveCount(0);
  // The pick's screenshot goes to the app for its preview.
  const key = ((await ofType(page, "state")).at(-1)!.draft as { key: string }).key;
  await expect.poll(async () => (await ofType(page, "draftClip")).map(m => m.key)).toEqual([key]);
  const png = Buffer.from((await ofType(page, "draftClip"))[0].png as string, "base64");
  expect(png.subarray(1, 4).toString("latin1")).toBe("PNG");
  await host(page, `setDraftText("make it", { composer: true })`);
  await host(page, `submitDraft({ text: "make it blue" })`);
  await expect.poll(async () => (await ofType(page, "sent")).length).toBe(1);
  const thread = (await ofType(page, "sent"))[0].thread as string;
  const t = await api(d.base, d.token, `/api/artifacts/${id}/threads/${thread}`) as { thread: { sent_to_agent: boolean; has_clip: boolean; anchor: { kind: string; quote: string }; comments: { body: string }[] } };
  expect(t.thread.sent_to_agent).toBe(true);
  expect(t.thread.comments[0].body).toBe("make it blue");
  expect(t.thread.anchor).toMatchObject({ kind: "element", quote: "Quarterly goals" });
  expect(t.thread.has_clip).toBe(true);
  await expect.poll(async () => ((await ofType(page, "threads")).at(-1)?.threads as { id: string; clip: boolean }[] | undefined)?.find(x => x.id === thread)?.clip).toBe(true);
  // The app's thread list names what the thread is on, in words.
  await expect.poll(async () => ((await ofType(page, "threads")).at(-1)?.threads as { id: string; label: string }[] | undefined)?.find(x => x.id === thread)?.label).toBe("“Quarterly goals”");
  // The app session's feedback poll hands it the comment.
  const fb = await api(d.base, d.token, `/api/sessions/${sid}/feedback?wait=0`) as { threads?: { id: string }[] };
  expect(JSON.stringify(fb)).toContain(thread);
});
}

test("a picked row of elements is labelled with a space between their texts, as its quote is not", async ({ page }) => {
  const html = PAGE.replace('<p>Grow', '<div id="row" style="padding-left:80px"><b>Mon</b><span>5 km</span></div><p>Grow');
  const { frame } = await openApp(page, "App spaced label", "subdomain", true, { html });
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await host(page, "enterCommentMode()");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  const cdp = await page.context().newCDPSession(page);
  await holdOn(page, cdp, frame.locator("#row"), "#row");
  await expect.poll(async () => (await ofType(page, "state")).at(-1)?.draft ?? null).toMatchObject({ label: "“Mon 5 km”", quote: "Mon 5 km" });
});

/** A page that keeps one note in its documents: Add writes it, and the page
 * shows it while it exists. */
const NOTES = `<!doctype html><html><head><title>Notes</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:24px}h2{margin:0 0 24px}.tall{height:3000px}</style>
</head>
<body><main><h2 id="t">Notes</h2><p id="note">none</p><button id="add">Add</button><div class="tall"></div><p id="end">The end</p></main>
<script>
(async () => {
  const db = await claude.use("db");
  db.doc("notes/n1").onSnapshot(s => { document.getElementById("note").textContent = s.exists ? s.data().text : "none"; });
  document.getElementById("add").onclick = () => db.doc("notes/n1").set({ text: "kept" });
})();
</script></body></html>`;

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a published version is offered, not loaded, and lands in place on reloadLatest with the documents and scroll kept; a restore likewise`, async ({ page }) => {
    const { id, sid, frame } = await openApp(page, `App notes ${mode}`, mode, true, { html: NOTES, capabilities: { db: {} } });
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    await frame.locator("#add").tap();
    await expect(frame.locator("#note")).toHaveText("kept");
    await streamLive(page);
    const y = await scrollAndReport(page, frame, 1200);
    expect(y).toBeGreaterThanOrEqual(1199);

    await publishNext(d.base, d.token, sid, id, 1, { files: { "index.html": NOTES.replace(">Notes</h2>", ">Notes, in blue</h2>") } });
    await expect.poll(async () => (await ofType(page, "newer")).length, { timeout: 10_000 }).toBe(1);
    expect((await ofType(page, "newer"))[0]).toEqual({ type: "newer", version: 2 });
    // Never swapped without the app's word: version 1 still shows.
    expect((await ofType(page, "landed"))).toEqual([]);
    await expect(frame.locator("#t")).toHaveText("Notes");
    await host(page, "reloadLatest()");
    await expect.poll(async () => (await ofType(page, "landed")).length, { timeout: 10_000 }).toBe(1);
    expect((await ofType(page, "landed"))[0]).toEqual({ type: "landed", version: 2 });
    const v2 = await contentFrame(page, id, 2);
    await expect(v2.locator("#t")).toHaveText("Notes, in blue");
    await expect(v2.locator("#note")).toHaveText("kept");
    await expectScrollNear(v2, y);

    // Undo: version 1's files come back as version 3, over the same
    // documents, and the view follows it as it followed version 2.
    await api(d.base, d.token, `/api/artifacts/${id}/versions:restore`, { method: "POST", session: sid, body: JSON.stringify({ version: 1, if_version: 2 }) });
    await expect.poll(async () => (await ofType(page, "newer")).length, { timeout: 10_000 }).toBe(2);
    await host(page, "reloadLatest()");
    await expect.poll(async () => (await ofType(page, "landed")).length, { timeout: 10_000 }).toBe(2);
    expect((await ofType(page, "landed"))[1]).toEqual({ type: "landed", version: 3 });
    const v3 = await contentFrame(page, id, 3);
    await expect(v3.locator("#t")).toHaveText("Notes");
    await expect(v3.locator("#note")).toHaveText("kept");
    await expectScrollNear(v3, y);
    expect(new URL(page.url()).search).toBe("?app=1");
  });
}

test("a version published after the hidden-page threshold is still offered and lands while the WebView reports the document hidden", async ({ page }) => {
  // As the app's WKWebView does from load on.
  await page.addInitScript(() => {
    if (window.top !== window) return;
    Object.defineProperty(document, "visibilityState", { get: () => "hidden", configurable: true });
    Object.defineProperty(document, "hidden", { get: () => true, configurable: true });
  });
  const { id, sid } = await openApp(page, "App hidden", "subdomain");
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  expect(await quietEval<string>(page, "document.visibilityState")).toBe("hidden");
  // Past the stream's 30 s hidden-page threshold (web/shell/src/stream.ts HIDDEN_MS), on the shell's test clock.
  await advance(page, 60_000);
  await publishNext(d.base, d.token, sid, id, 1, { files: { "index.html": PAGE.replace("Quarterly goals", "Quarterly goals, later") } });
  await expect.poll(async () => (await ofType(page, "newer")).length, { timeout: 10_000 }).toBe(1);
  await host(page, "reloadLatest()");
  await expect.poll(async () => (await ofType(page, "landed")).length, { timeout: 10_000 }).toBe(1);
  const next = await contentFrame(page, id, 2);
  await expect(next.locator("#t")).toHaveText("Quarterly goals, later");
});

test("with no animation frames, a permission dialog the page raised before the app's init arms once the app inits", async ({ page }) => {
  await noFrames(page);
  const html = `<!doctype html><html><head><title>Asks</title></head><body><pre id="out">waiting</pre><script>
claude.use("permissions").then(async perm => { document.getElementById("out").textContent = JSON.stringify(await perm.request(["comments"])); });
</script></body></html>`;
  const { frame } = await openApp(page, "App asks early", "subdomain", true, { html, capabilities: { comments: {} }, init: false });
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("post comments on this artifact under your name");
  const allow = dialog.getByRole("button", { name: "Allow", exact: true });
  // No frame ever comes and the app has not said the page is shown: Allow waits.
  await expect.poll(() => quietEval<string>(page, "typeof window.claxHost")).toBe("object");
  await advance(page, 5_000);
  await expect(allow).toBeDisabled();
  await initHost(page);
  await expect(allow).toBeEnabled();
  await allow.tap();
  await expect(frame.locator("#out")).toHaveText(JSON.stringify({ comments: "granted" }));
});

// A phone's WebView with the app's composer and keyboard over the page's
// lower part: the page keeps its size, and the shell shows no composer.
test.describe("a short view", () => {
  test.use({ viewport: { width: 390, height: 380 } });

  test("comment mode frames the page; a tap picks, the shell shows no Send, and the app's Send sends", async ({ page }) => {
    const { frame } = await openApp(page, "App short view", "subdomain");
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    const framed = () => page.locator(".stage").evaluate(el => getComputedStyle(el, "::after").boxShadow !== "none");
    expect(await framed()).toBe(false);
    await host(page, "enterCommentMode()");
    await expect.poll(framed).toBe(true);
    await frame.locator("#t").tap();
    await expect.poll(async () => (await ofType(page, "state")).at(-1)?.draft ?? null).toMatchObject({ label: "“Quarterly goals”" });
    await expect(page.locator(".composer")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Cancel", exact: true })).toHaveCount(0);
    await host(page, `submitDraft({ text: "make it blue" })`);
    await expect.poll(async () => (await ofType(page, "sent")).length).toBe(1);
  });
});

test("back() moves through the app's own fragments and pages in place, and across a version swap", async ({ page }) => {
  const ROUTES = `<!doctype html><title>R</title><h1 id=t>Routes</h1><a id=item href="#/item/3">item</a><a id=other href="other.html">other</a>`;
  const s = await registerSession(d.base, d.token, "app");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Routes", { "index.html": ROUTES, "other.html": "<!doctype html><title>O</title><h1 id=o>Other</h1>" });
  const id = artifact.id;
  await hostChannel(page);
  await page.goto(`${d.base}/a/${id}?app=1`);
  const f1 = await contentFrame(page, id, 1);
  await initHost(page);
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  const back = async () => (await ofType(page, "history")).at(-1)?.back;
  expect(await back()).toBe(false);
  // A route: one entry.
  await f1.locator("#item").click();
  await expect.poll(back).toBe(true);
  await expect.poll(() => f1.evaluate(() => location.hash)).toBe("#/item/3");
  // A page of the app: another.
  await f1.evaluate(() => { location.hash = ""; });
  await f1.evaluate(() => (document.getElementById("other") as HTMLAnchorElement).click());
  await expect.poll(() => page.frames().some(f => f.url().endsWith("/other.html"))).toBe(true);
  // A version swap keeps the trail.
  await publishNext(d.base, d.token, s.id, id, 1, { files: { "other.html": "<!doctype html><title>O</title><h1 id=o>Other, later</h1>" } });
  await expect.poll(async () => (await ofType(page, "newer")).length, { timeout: 10_000 }).toBe(1);
  await host(page, "reloadLatest()");
  await expect.poll(async () => (await ofType(page, "landed")).length, { timeout: 10_000 }).toBe(1);
  const other = page.frames().find(f => /\/v\/2\/other\.html$/.test(f.url()))!;
  await expect(other.locator("#o")).toHaveText("Other, later");
  // Back: the index page of version 2, then its route, then its start.
  await host(page, "back()");
  await expect.poll(() => page.frames().some(f => /\/v\/2\/(#.*)?$/.test(f.url()))).toBe(true);
  expect(new URL(page.url()).pathname).toBe(`/a/${id}`);
  await host(page, "back()");
  const index = page.frames().find(f => /\/v\/2\/(#.*)?$/.test(f.url()))!;
  await expect.poll(() => index.evaluate(() => location.hash)).toBe("#/item/3");
  await host(page, "back()");
  await expect.poll(() => index.evaluate(() => location.hash)).toBe("");
  await expect.poll(back).toBe(false);
});
