// The Clax app's view (`/a/<id>?app=1`): no chrome, `window.claxHost` for the
// app that hosts it, messages to the app's channel (`window.ClaxHost`), Send,
// and a newer version followed in place with the scroll kept. The host's
// calls go in through the DevTools protocol with no user gesture
// (`quietEval`), as the app's `runJavaScript` does.
import { type CDPSession, type Locator, type Page } from "@playwright/test";
import { test, expect, type Daemon, api, contentFrame, publishAs, publishNext, record, registerSession } from "./fixtures";
import { quietEval } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const PAGE = `<!doctype html><html><head><title>App</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:24px}h2{margin:0 0 24px}.tall{height:3000px}</style>
</head>
<body><main><h2 id="t">Quarterly goals</h2><p>Grow revenue in every region this year.</p><div class="tall"></div><p id="end">The end</p></main></body></html>`;

type HostMessage = { type: string; [k: string]: unknown };

/** The app's channel: every message the view posts to its host, parsed. */
async function hostChannel(page: Page) {
  await page.addInitScript(() => {
    if (window.top !== window) return;
    (window as unknown as { claxToHost: unknown[] }).claxToHost = [];
    (window as unknown as { ClaxHost: unknown }).ClaxHost = { postMessage: (m: string) => (window as unknown as { claxToHost: unknown[] }).claxToHost.push(JSON.parse(m)) };
  });
}
const toHost = (page: Page) => quietEval<HostMessage[]>(page, "window.claxToHost");
const ofType = async (page: Page, type: string) => (await toHost(page)).filter(m => m.type === type);
const host = (page: Page, call: string) => quietEval<unknown>(page, `window.claxHost.${call}`);

async function openApp(page: Page, title: string, mode: "subdomain" | "sandbox", app = true) {
  const s = await registerSession(d.base, d.token, "app");
  const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE });
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  await hostChannel(page);
  await record(page);
  await page.goto(`${d.base}/a/${artifact.id}${app ? "?app=1" : ""}`);
  const frame = await contentFrame(page, artifact.id, 1);
  return { id: artifact.id, sid: s.id, frame };
}

/** Holds one finger on `target` past the bridge's hold, then lifts it. */
async function holdOn(cdp: CDPSession, target: Locator) {
  const b = (await target.boundingBox())!;
  const at = { x: b.x + Math.min(40, b.width / 2), y: b.y + b.height / 2 };
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
  await new Promise(r => setTimeout(r, 800));
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
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
  });

  test(`${mode}: a browser's view has no claxHost and keeps its chrome`, async ({ page }) => {
    await openApp(page, `App browser ${mode}`, mode, false);
    await expect(page.locator(".topbar")).toBeVisible();
    expect(await quietEval<string>(page, "typeof window.claxHost")).toBe("undefined");
    expect(await quietEval<boolean>(page, "document.documentElement.classList.contains('app')")).toBe(false);
    expect(await toHost(page)).toEqual([]);
  });
}

test("a long-press picks and Send sends to the app session", async ({ page }) => {
  const { id, sid, frame } = await openApp(page, "App send", "subdomain");
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await host(page, "enterCommentMode()");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  const cdp = await page.context().newCDPSession(page);
  await holdOn(cdp, frame.locator("#t"));
  await expect.poll(async () => (await ofType(page, "state")).at(-1)?.draft ?? null).toMatchObject({ label: "«Quarterly goals»" });
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
  await host(page, `setDraftText("make it blue")`);
  await expect(page.locator(".composer textarea")).toHaveValue("make it blue");
  await host(page, "submitDraft()");
  await expect.poll(async () => (await ofType(page, "sent")).length).toBe(1);
  const thread = (await ofType(page, "sent"))[0].thread as string;
  const t = await api(d.base, d.token, `/api/artifacts/${id}/threads/${thread}`) as { thread: { sent_to_agent: boolean; comments: { body: string }[] } };
  expect(t.thread.sent_to_agent).toBe(true);
  expect(t.thread.comments[0].body).toBe("make it blue");
  // The app session's feedback poll hands it the comment.
  const fb = await api(d.base, d.token, `/api/sessions/${sid}/feedback?wait=0`) as { threads?: { id: string }[] };
  expect(JSON.stringify(fb)).toContain(thread);
});

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a published version lands in place with the scroll kept`, async ({ page }) => {
    const { id, sid, frame } = await openApp(page, `App follow ${mode}`, mode);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    // The page reports its scroll once the welcome's part has loaded, which
    // may be after this first scroll: it moves by a pixel until a report of
    // where it is arrives.
    const reported = () => page.evaluate(() => (window as unknown as { claxMsgs: { type: string; y?: number }[] }).claxMsgs.filter(m => m.type === "clax:scroll").at(-1)?.y ?? null);
    await expect.poll(async () => {
      const y = await frame.evaluate(() => { scrollTo(0, scrollY === 1200 ? 1201 : 1200); return scrollY; });
      const r = await reported();
      return r !== null && Math.abs(r - y) <= 1;
    }).toBe(true);
    const y = (await reported())!;
    await publishNext(d.base, d.token, sid, id, 1, { files: { "index.html": PAGE.replace("Quarterly goals", "Quarterly goals, in blue") } });
    await expect.poll(async () => (await ofType(page, "landed")).length, { timeout: 10_000 }).toBe(1);
    expect((await ofType(page, "landed"))[0]).toEqual({ type: "landed", version: 2 });
    const next = await contentFrame(page, id, 2);
    await expect(next.locator("#t")).toHaveText("Quarterly goals, in blue");
    await expect.poll(() => next.evaluate(() => scrollY)).toBeGreaterThanOrEqual(y - 2);
    expect(await next.evaluate(() => scrollY)).toBeLessThanOrEqual(y + 2);
    expect(y).toBeGreaterThanOrEqual(1199);
    expect(new URL(page.url()).search).toBe("?app=1");
  });
}
