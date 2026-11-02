// The Clax DevTools panel's Queues view (its BRIEF's Addenda 1 and 2), with
// the extension paired to a real daemon: DevTools opens on the site's page,
// the Clax panel shows the queue that takes the site's reports, a report
// filed meanwhile appears with no reload (the worker's queues ping through
// the gateway's live-only stream), and "Show on the page" outlines the
// report's element in the inspected page. The panel is driven over the
// browser endpoint, as Playwright reaches no DevTools frontend.
import { ReporterClient } from "../sdk/src/reporter";
import { ServiceClient } from "../sdk/src/service";
import { expect, test } from "./extension-fixtures";

test.use({ devtools: true });

type Target = { type: string; url: string; webSocketDebuggerUrl: string };
/** A CDP page session: `ev` evaluates an expression (awaiting a promise) and answers its value. */
async function session(url: string) {
  const ws = new WebSocket(url);
  await new Promise(r => { ws.onopen = r; });
  let id = 0;
  const wait = new Map<number, (m: { result?: { result?: { value?: unknown }; exceptionDetails?: unknown } }) => void>();
  ws.onmessage = e => { const m = JSON.parse(String(e.data)); wait.get(m.id)?.(m); };
  const ev = (expression: string) => new Promise<unknown>(ok => {
    const i = ++id;
    wait.set(i, m => ok(m.result?.exceptionDetails ? { error: m.result.exceptionDetails } : m.result?.result?.value));
    ws.send(JSON.stringify({ id: i, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
  });
  return { ev, close: () => ws.close() };
}
/** Polls `f` until it answers truthy, or fails after `ms`. */
async function until<T>(what: string, f: () => Promise<T>, ms = 15_000): Promise<NonNullable<T>> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v as NonNullable<T>;
    if (Date.now() > end) throw new Error(`${what}: not in ${ms} ms`);
    await new Promise(r => setTimeout(r, 200));
  }
}

test("the DevTools panel's Queues view shows the site's queue live, and shows a report's element on the page", async ({ live }) => {
  test.setTimeout(120_000);
  const site = new URL(live.siteUrl).origin;
  // The owner's queue for the site, and a service grant, from the owner's browser (a loopback page of the daemon).
  const owner = await live.ctx.newPage();
  await owner.goto(`${live.daemon.base}/queues`);
  const made = await owner.evaluate(async origin => {
    const post = (p: string, b: unknown) => fetch(p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }).then(r => r.json());
    const q = await post("/api/queues", { name: "Live site feedback", origins: [origin], reporters_see: "own" });
    const g = await post(`/api/queues/${q.queue.id}/service-grants`, { name: "backend" });
    return { qid: q.queue.id as string, token: g.token as string, key: g.private_key as string };
  }, site);
  const service = new ServiceClient({ instance: live.daemon.base, queue: made.qid, token: made.token, privateKey: made.key });
  const reporter = new ReporterClient({
    instance: live.daemon.base, queue: made.qid,
    auth: publicKey => service.mintReporterToken({ reporter_id: "u1", origin: site, public_key: publicKey }),
    fetch: (u, init) => fetch(u, { ...init, headers: { ...(init?.headers as Record<string, string>), origin: site } }),
  });
  const anchor = { kind: "element", selector: "#app", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: "The app", file: "" };
  const file = (body: string) => reporter.fileReport({ url: live.siteUrl, title: "Live site", anchor, body });
  await file("The app is blank");

  const page = await live.ctx.newPage();
  await page.goto(live.siteUrl);
  const list = async () => (await (await fetch(`${live.cdp()}/json/list`)).json()) as Target[];
  // Show the Clax panel in each DevTools window, then take the one inspecting the site's page.
  const panel = await until("the Clax panel inspecting the site", async () => {
    for (const f of (await list()).filter(t => t.url.startsWith("devtools://"))) {
      const s = await session(f.webSocketDebuggerUrl);
      await s.ev(`(async () => { const L = await import("./ui/legacy/legacy.js"); const v = L.InspectorView.InspectorView.instance(); const ids = v.tabbedPane.tabIds ? v.tabbedPane.tabIds() : []; const id = ids.find(i => i.includes("Clax")); if (id) await v.showPanel(id); return id; })()`);
      s.close();
    }
    for (const p of (await list()).filter(t => t.url.endsWith("/devtools-panel.html"))) {
      const s = await session(p.webSocketDebuggerUrl);
      const href = await s.ev(`new Promise(r => chrome.devtools.inspectedWindow.eval("location.href", v => r(v)))`);
      if (href === live.siteUrl) return s;
      s.close();
    }
    return null;
  }, 30_000);
  // Its text, laid out or not (a DevTools frame not in front is not laid out).
  const text = () => panel.ev(`document.getElementById("dt-queues-panel").textContent`) as Promise<string>;
  await panel.ev(`document.getElementById("dt-queues").click()`);
  await until("the site's queue and its report", async () => /Live site feedback[\s\S]*On this page[\s\S]*The app is blank/.test(await text()))
    .catch(async e => { throw new Error(`${e.message}; the panel says: ${await text()}`); });
  expect(await panel.ev(`document.querySelector('.qv a[target]')?.getAttribute("href")`)).toBe(`${live.daemon.base}/queues/${made.qid}`);

  // Live: a report filed now appears with no reload, and is said.
  await file("The button is grey");
  await until("the new report, live", async () => (await text()).includes("The button is grey"));
  expect(await panel.ev(`document.querySelector(".qv .heard").textContent`)).toBe("1 new report.");

  // A report opens in place; Show on the page outlines its element in the page.
  await panel.ev(`[...document.querySelectorAll(".qv a.card")].find(a => a.textContent.includes("The app is blank")).click()`);
  await until("the report's page", async () => (await text()).includes("Show on the page"));
  await panel.ev(`[...document.querySelectorAll("button")].find(b => b.textContent === "Show on the page").click()`);
  // The outline lasts two seconds.
  await until("the element outlined in the page", () => page.evaluate(() => (document.getElementById("app") as HTMLElement).style.outlineWidth === "3px"), 2000);
  await until("Show on the page said", async () => (await panel.ev(`document.querySelector(".notices [role=status]").textContent`)) === "Shown on the page, and selected in Elements.");
  panel.close();
});
