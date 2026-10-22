// The shell's app transport end to end: the shell documents and content of
// a daemon that serves as the core an app embeds, loaded from the app's
// origins, with the shell's API calls and event stream over the app's
// channel (./app-scheme.ts plays the app's side): calls bound to the load's nonce, a previous
// load's late answers dropped, bodies in chunks, aborts before and after the nonce, the stream
// paced by the core's end blocks, a suspended WebView, origin isolation,
// comment mode, clips, the app's more menu, and the browser's shell as it
// was.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { hostChannel, host } from "./app-host";
import { type AppHost, type AppHostOptions, BINDING, PART, SHELL, SseBlocks, answerScripts, content, startEmbedDaemon } from "./app-scheme";
import { test as base, expect, type Daemon, api, publish, publishWith } from "./fixtures";
import { quietEval } from "./time";
import { REQUEST_STUCK } from "../shell/src/conn-notice";
import { STUCK_MS } from "../shell/src/lifecycle";

const test = base.extend<object, { embed: Daemon }>({
  // oxlint-disable-next-line no-empty-pattern
  embed: [async ({}, use) => {
    const d = await startEmbedDaemon();
    await use(d);
    await d.stop();
  }, { scope: "worker", timeout: 30_000 }],
});

let d: Daemon;
test.beforeEach(({ embed }) => { d = embed; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const PAGE = `<!doctype html><html><head><title>App</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:24px}h2{margin:0 0 24px}</style>
</head><body><main><h2 id="t">Quarterly goals</h2><p>Grow revenue in every region this year.</p></main></body></html>`;

/** A 1×1 PNG, as a thread's clip. */
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The content frame showing version `n` of `id`, from its app origin, once parsed. */
async function appFrame(page: Page, id: string, n: number): Promise<Frame> {
  const url = new RegExp(`^${escape(content(id))}/v/${n}/`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 10_000 }).toBe(true);
  const frame = page.frame({ url })!;
  await frame.waitForLoadState("domcontentloaded");
  return frame;
}

/** Publishes `html` and opens it in the app through the app's host: the
 * app's view (`?app=1`), or with `app: false` the shell document as a
 * browser shows it. */
async function openInApp(page: Page, title: string, opts: { html?: string; app?: boolean; host?: AppHostOptions } = {}) {
  const { artifact } = await publish(d.base, d.token, title, { "index.html": opts.html ?? PAGE });
  const h = await hostChannel(page, { relay: d, ...opts.host });
  await page.goto(`${SHELL}/a/${artifact.id}${opts.app === false ? "" : "?app=1"}`);
  const frame = await appFrame(page, artifact.id, 1);
  return { id: artifact.id, host: h, frame };
}

/** Waits until the view's stream carries its topics (`claxStreamLive`). */
async function live(h: AppHost) {
  await expect.poll(() => h.evalNow<number>("window.claxStreamLive ?? 0")).toBeGreaterThan(0);
}

/** Posts a viewer's thread on `id` v1, with a clip, straight to the daemon. */
async function postThread(id: string, body: string): Promise<{ id: string }> {
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "#t", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
  form.set("body", body);
  form.set("version", "1");
  form.set("clip", new Blob([PNG], { type: "image/png" }), "clip.png");
  const res = await fetch(`${d.base}/api/artifacts/${id}/threads`, { method: "POST", body: form, headers: { origin: d.base } });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()).thread;
}

test("the artifact view loads and a comment posts through the channel", async ({ page }) => {
  const { id, host: h, frame } = await openInApp(page, "Transport comment");
  await expect.poll(() => h.ofType("ready").length).toBeGreaterThan(0);
  await host(page, "enterCommentMode()");
  // The page's bridge takes the shell's orders from the app's shell origin,
  // which the core names in its tag (data-origin).
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  expect(await frame.evaluate(() => document.querySelector<HTMLScriptElement>("script[data-origin]")?.dataset.origin)).toBe(SHELL);
  await frame.locator("#t").tap();
  await expect.poll(() => (h.ofType("state").at(-1)?.draft as { label?: string } | undefined)?.label ?? null).toBe("“Quarterly goals”");
  await host(page, `submitDraft({ text: "make it blue" })`);
  await expect.poll(() => h.ofType("sent").length).toBe(1);
  const thread = h.ofType("sent")[0].thread as string;
  const t = await api(d.base, d.token, `/api/artifacts/${id}/threads/${thread}`) as { thread: { comments: { body: string }[]; has_clip: boolean } };
  expect(t.thread.comments[0].body).toBe("make it blue");
  expect(t.thread.has_clip).toBe(true);
  expect(h.callsTo("POST", `/api/artifacts/${id}/threads`)).toHaveLength(1);
});

test("the relay sends one stream message per SSE block, however the core's body is cut", () => {
  const sse = new SseBlocks();
  expect(sse.push("id: s:1\nevent: ready\ndata: {}\n\nid: s:2\nev")).toEqual(["id: s:1\nevent: ready\ndata: {}\n\n"]);
  expect(sse.push("ent: x\ndata: {}\n")).toEqual([]);
  expect(sse.push("\n: keep-alive\n\n: tail")).toEqual(["id: s:2\nevent: x\ndata: {}\n\n", ": keep-alive\n\n"]);
  expect(sse.rest()).toBe(": tail");
  expect(sse.rest()).toBe("");
});

test("calls go out from boot under the document's nonce, each answer names it, and a message under another nonce is dropped", async ({ page }) => {
  const { host: h } = await openInApp(page, "Transport nonce", { host: { autoInit: false } });
  const meta = await h.evalNow<string>(`document.querySelector('meta[name="clax-nonce"]').content`);
  expect(meta).toBe(await h.pinned());
  expect(h.pins).toEqual(["message"]);
  // Before the app's init, the view has made its calls and opened its stream.
  await expect.poll(() => h.streamsOf().length).toBeGreaterThan(0);
  await expect.poll(() => h.calls.filter(c => c.answered).length).toBeGreaterThan(0);
  expect(h.ran.filter(e => e.includes("claxHost.init"))).toEqual([]);
  expect(h.calls.every(c => c.nonce === meta)).toBe(true);
  // Every answer the host ran names the load's nonce first (the stream's
  // first block included, which the daemon sends after the subscription opens).
  await expect.poll(() => h.ran.some(e => e.startsWith("window.claxTransport.streamBlock("))).toBe(true);
  await h.idle();
  const answers = h.ran.filter(e => /claxTransport\.(callResult|streamBlock|streamEnd)\(/.test(e));
  expect(answers.length).toBeGreaterThan(0);
  expect(answers.every(e => /^window\.claxTransport\.\w+\((".*?"), /.exec(e)?.[1] === JSON.stringify(meta))).toBe(true);
  expect(h.dropped).toEqual([]);
  // What the core answers: no header the daemon's HTTP server adds on the
  // wire, and one stream block per message, its blank line included.
  const results = answers.filter(e => e.startsWith("window.claxTransport.callResult(")).map(e => JSON.parse(/^window\.claxTransport\.callResult\(".*?", \d+, (.*)\)$/s.exec(e)![1]) as { headers: [string, string][] });
  expect(results.flatMap(r => r.headers.map(([k]) => k)).filter(k => ["connection", "keep-alive", "transfer-encoding", "date"].includes(k))).toEqual([]);
  const blocks = answers.filter(e => e.startsWith("window.claxTransport.streamBlock(")).map(e => JSON.parse(/^window\.claxTransport\.streamBlock\(".*?", \d+, (".*")\)$/s.exec(e)![1]) as string);
  expect(blocks.length).toBeGreaterThan(0);
  expect(blocks.filter(b => !b.endsWith("\n\n") || b.indexOf("\n\n") !== b.length - 2)).toEqual([]);
  // A held-open route is refused whatever follows its path.
  await h.evalNow(`window.ClaxHost.postMessage(JSON.stringify({ type: "call", nonce: ${JSON.stringify(meta)}, id: 9998, method: "GET", path: "/api/stream#x", headers: [], body: null, body64: null }))`);
  await expect.poll(() => h.calls.find(c => c.id === 9998)?.answered?.status).toBe(404);
  expect(JSON.parse(h.calls.find(c => c.id === 9998)!.answered!.text!).error.code).toBe("not_in_app");

  await h.evalNow(`window.ClaxHost.postMessage(JSON.stringify({ type: "call", nonce: "another-load-0123456789", id: 9999, method: "GET", path: "/api/artifacts", headers: [], body: null, body64: null }))`);
  await h.barrier();
  expect(h.dropped).toEqual([{ why: "nonce", message: expect.objectContaining({ id: 9999 }) }]);
  expect(h.calls.filter(c => c.id === 9999)).toEqual([]);

  await h.init();
  await expect.poll(() => h.ofType("ready").length).toBeGreaterThan(0);
});

test("a reload's calls and stream take only their own load's answers: the previous load's late ones are dropped", async ({ page }) => {
  // The text of each load's event stream, as the shell's own fetch reads it.
  await page.addInitScript(shell => {
    if (location.origin !== shell || window.top !== window) return;
    const stream: string[] = [];
    Object.assign(window, { claxStreamLog: stream });
    let inner = window.fetch;
    let wrapped: typeof fetch | null = null;
    Object.defineProperty(window, "fetch", {
      configurable: true,
      get: () => wrapped ?? inner,
      set: (f: typeof fetch) => {
        inner = f;
        wrapped = async (input, init) => {
          const res = await inner(input, init);
          const path = new URL(input instanceof Request ? input.url : String(input), location.href).pathname;
          if (path === "/api/stream" && res.body) {
            const r = res.clone().body!.getReader();
            const dec = new TextDecoder();
            void (async () => { for (;;) { const { value, done } = await r.read(); if (done) return; stream.push(dec.decode(value)); } })().catch(() => {});
          }
          return res;
        };
      },
    });
  }, SHELL);
  const { artifact } = await publish(d.base, d.token, "Transport stale", { "index.html": PAGE });
  const id = artifact.id;
  const h = await hostChannel(page, { relay: d, autoInit: false });
  const PROBES = 40;
  const probes = (nonce: string) => h.calls.filter(c => c.nonce === nonce && c.path.includes("probe="));
  /** Asks for the threads `PROBES` times from the shell, and waits until the
   * core answered every call of the load (its own and those) into the
   * held queue. */
  const probe = async (nonce: string, tag: string) => {
    await expect.poll(() => h.calls.some(c => c.nonce === nonce)).toBe(true);
    await h.evalNow(`window.probes = Promise.all(Array.from({ length: ${PROBES} }, (_, k) => fetch("/api/artifacts/${id}/threads?probe=${tag}-" + k).then(r => r.text()))); 0`);
    await expect.poll(() => probes(nonce).length === PROBES && h.calls.filter(c => c.nonce === nonce).every(c => c.answered)).toBe(true);
  };
  // The app's answers to the first load are sent and not yet run.
  h.hold();
  await page.goto(`${SHELL}/a/${id}?app=1`);
  const first = await h.pinned();
  await probe(first, "one");
  // What the core answers the next load differs: it has a thread.
  const thread = await postThread(id, "a thread after the first load");
  await page.reload();
  const second = await h.pinned();
  expect(second).not.toBe(first);
  await probe(second, "two");
  // The two loads' call IDs overlap: had a first load's answer landed on
  // the second load's call of its ID, that call would hold it.
  const ids = new Set(h.calls.filter(c => c.nonce === second).map(c => c.id));
  expect(probes(first).filter(c => ids.has(c.id)).length).toBeGreaterThan(PROBES / 2);
  // The first load's answers run first, into the second load's document.
  await h.release();
  const ran = h.ran.filter(e => e.startsWith("window.claxTransport.callResult("));
  const own = ran.findIndex(e => e.startsWith(`window.claxTransport.callResult(${JSON.stringify(second)}`));
  expect(ran.slice(0, own).length).toBeGreaterThan(PROBES);
  expect(ran.slice(own).some(e => e.startsWith(`window.claxTransport.callResult(${JSON.stringify(first)}`))).toBe(false);
  const answered = await h.evalNow<string[]>("window.probes");
  expect(answered.filter(t => !t.includes(thread.id))).toEqual([]);
  await h.init();
  await expect.poll(() => h.ofType("ready").length).toBeGreaterThan(0);
  await expect.poll(() => (h.ofType("threads").at(-1)?.threads as { id: string }[] | undefined)?.map(t => t.id) ?? []).toContain(thread.id);
  // A late push of the first load's subscription of the same ID as the
  // second load's stream: neither its block nor its end reaches that stream.
  await live(h);
  const s = h.streamsOf(second).at(-1)!;
  const opened = h.subscriptions.length;
  await h.evalNow(`window.claxTransport.streamBlock(${JSON.stringify(first)}, ${s.id}, "event: late-push\\ndata: {}\\n\\n"); window.claxTransport.streamEnd(${JSON.stringify(first)}, ${s.id})`);
  await h.barrier();
  await publish(d.base, d.token, "Transport stale", { "index.html": PAGE.replace("Quarterly", "Yearly") }, 1, id);
  await expect.poll(() => h.ofType("newer").map(m => m.version)).toContain(2);
  expect(h.subscriptions.length).toBe(opened);
  expect((await quietEval<string[]>(page, "window.claxStreamLog")).join("")).not.toContain("late-push");
});

test("the load is pinned as the app's surface pins it: by the first candidate of its navigation the main frame presents, never by another navigation's nonce", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Transport pin", { "index.html": PAGE });
  const h = await hostChannel(page, { relay: d, holdPosts: true, readMeta: false });
  /** Posts straight to the host's binding, past the held channel. */
  const raw = (nonce: string) => h.evalNow(`window.${BINDING}(JSON.stringify({ type: "barrier", mark: "raw", nonce: ${JSON.stringify(nonce)} })); 0`);
  const meta = () => h.evalNow<string>(`document.querySelector('meta[name="clax-nonce"]').content`);
  await page.goto(`${SHELL}/a/${artifact.id}?app=1`);
  const first = await meta();
  await raw("not-a-candidate-0123456789");
  await expect.poll(() => h.dropped.length).toBe(1);
  expect(h.nonce).toBeNull();
  await raw(first);
  expect(await h.pinned()).toBe(first);
  expect(h.pins).toEqual(["message"]);
  await h.evalNow("window.claxReleasePosts(); 0");
  await expect.poll(() => h.ofType("ready", first).length).toBeGreaterThan(0);
  // The next navigation's set holds only its own nonce: the first one's, presented first, pins nothing.
  await page.reload();
  const second = await meta();
  expect(second).not.toBe(first);
  const dropped = h.dropped.length;
  await raw(first);
  await expect.poll(() => h.dropped.length).toBe(dropped + 1);
  expect(h.nonce).toBeNull();
  await raw(second);
  expect(await h.pinned()).toBe(second);
  expect(h.loads).toEqual([first, second]);
});

test("an init the app ran for the previous load leaves the next load's channel its own", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Transport stale init", { "index.html": PAGE });
  const h = await hostChannel(page, { relay: d });
  h.hold();
  await page.goto(`${SHELL}/a/${artifact.id}?app=1`);
  const first = await h.pinned();
  const inits = (nonce: string) => h.waiting.filter(e => e.startsWith("window.claxHost.init(") && e.includes(JSON.stringify(nonce)));
  await expect.poll(() => inits(first).length).toBe(1);
  await page.reload();
  const second = await h.pinned();
  await expect.poll(() => inits(second).length).toBe(1);
  // The first load's init runs in the second load's document, before its own.
  await h.release();
  await expect.poll(() => h.ofType("ready", second).length).toBeGreaterThan(0);
  // Both inits ran, the first load's before the second's, so the second
  // document did take a stale init first.
  const ran = h.ran.filter(e => e.startsWith("window.claxHost.init("));
  const at = (nonce: string) => ran.findIndex(e => e.includes(JSON.stringify(nonce)));
  expect(at(first)).toBeGreaterThanOrEqual(0);
  expect(at(second)).toBeGreaterThan(at(first));
  expect(h.dropped.filter(m => m.why === "nonce" && (m.message as { nonce?: string }).nonce === first)).toEqual([]);
});

test("a same-document navigation keeps the load's channel; a reload the core answers 404 commits with no nonce and leaves no channel", async ({ page }) => {
  const { id, host: h } = await openInApp(page, "Transport no nonce");
  await live(h);
  await expect.poll(() => h.ofType("ready").length).toBeGreaterThan(0);
  const first = await h.pinned();
  // A navigation within the document is no new load.
  await h.evalNow(`history.pushState(null, "", location.pathname + "?app=1&same=1"); 0`);
  await expect.poll(() => h.sameDocument.length).toBe(1);
  await h.barrier();
  expect(h.nonce).toBe(first);
  const inits = () => h.ran.filter(e => /^window\.(claxHost|claxTransport)\.init\(/.test(e)).length;
  const ran = inits();
  const streams = h.streamsOf(first);
  // The reload's main-frame request is numbered, but the core answers it 404, so no nonce is minted for it.
  h.failNextScheme(u => u.pathname === `/a/${id}`, 404, "not_found");
  await page.reload();
  await expect.poll(() => h.scheme.at(-1)).toEqual({ url: `${SHELL}/a/${id}?app=1&same=1`, status: 404, mainFrame: true });
  await expect.poll(() => h.nonce).toBeNull();
  expect(h.loads).toEqual([first]);
  // The previous load's streams are closed, and its nonce takes nothing more.
  expect(streams.every(s => s.ended !== undefined)).toBe(true);
  await h.evalNow(`window.ClaxHost.postMessage(JSON.stringify({ type: "call", nonce: ${JSON.stringify(first)}, id: 7777, method: "GET", path: "/api/artifacts", headers: [], body: null, body64: null })); 0`);
  await expect.poll(() => h.dropped).toContainEqual({ why: "nonce", message: expect.objectContaining({ id: 7777 }) });
  expect(h.calls.filter(c => c.id === 7777)).toEqual([]);
  // No init ran into the document the core refused.
  await page.waitForLoadState("load");
  await h.idle();
  expect(inits()).toBe(ran);
  // A main-frame navigation to the shell's host outside /a/ is refused as
  // Android's host refuses it, unnumbered, and has no channel either.
  const inbox = await page.goto(`${SHELL}/inbox`);
  expect(inbox!.status()).toBe(403);
  expect(inbox!.headers()["content-type"]).toBe("text/plain");
  expect(await inbox!.text()).toBe("refused");
  expect(h.nonce).toBeNull();
  expect(h.loads).toEqual([first]);
});

test("a body over 8 MiB goes to the core in 1 MiB chunks and arrives whole", async ({ page }) => {
  const { id, host: h } = await openInApp(page, "Transport chunks");
  await live(h);
  const size = 9 << 20;
  const status = await h.evalNow<number>(`fetch("/api/artifacts/${id}/versions", { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ if_version: 1, files: { "index.html": { content: ${JSON.stringify(PAGE)}, encoding: "utf8" }, "big.txt": { content: "x".repeat(${size}), encoding: "utf8" } } }) }).then(r => r.status)`);
  expect(status).toBeLessThan(300);
  const [call] = h.callsTo("POST", `/api/artifacts/${id}/versions`);
  expect(call.chunked).toBe(true);
  const chunks = h.ofType("callChunk").filter(m => m.id === call.id);
  const bytes = chunks.map(m => Buffer.from(m.body64 as string, "base64").length);
  expect(chunks.map(m => m.n)).toEqual(chunks.map((_, i) => i));
  expect(chunks.map(m => m.last)).toEqual(chunks.map((_, i) => i === chunks.length - 1));
  expect(bytes.slice(0, -1).every(b => b === 1 << 20)).toBe(true);
  expect(bytes.reduce((a, b) => a + b, 0)).toBeGreaterThan(size);
  const big = await (await fetch(`${d.base}/api/artifacts/${id}/versions/2/files/big.txt`)).text();
  expect(big.length).toBe(size);
});

/** The body of the relay's shared fixture: byte i is (i * 31 + (i >> 9)) & 255. */
const fixtureBody = (n: number) => Buffer.from(Uint8Array.from({ length: n }, (_, i) => (i * 31 + (i >> 9)) & 255));

test("the stand-in answers a long body in exactly the scripts the app's relay runs", () => {
  // Written by apps/clax_app/test/core/channel_relay_test.dart from what ChannelRelay runs.
  const fx = JSON.parse(readFileSync(new URL("../../apps/clax_app/test/fixtures/relay_answer_parts.json", import.meta.url), "utf8")) as { nonce: string; id: number; bytes: number; scripts: string[] };
  const scripts = answerScripts(fx.nonce, fx.id, { status: 200, headers: [["content-type", "application/octet-stream"]], body: fixtureBody(fx.bytes) });
  expect(scripts.length).toBe(fx.scripts.length);
  expect(scripts.map((js, i) => js === fx.scripts[i])).toEqual(fx.scripts.map(() => true));
});

test("an answer over the part size reaches the shell's fetch whole, in parts; one aborted after its first part sends no more", async ({ page }) => {
  const { id, host: h } = await openInApp(page, "Transport long answer");
  await live(h);
  const body = fixtureBody((5 << 19) + 7);
  const res = await fetch(`${d.base}/api/artifacts/${id}/versions`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
    body: JSON.stringify({ if_version: 1, files: { "index.html": { content: PAGE, encoding: "utf8" }, "big.bin": { content: body.toString("base64"), encoding: "base64" } } }) });
  expect(res.status).toBeLessThan(300);
  const path = `/api/artifacts/${id}/versions/2/files/big.bin`;
  const nonce = await h.pinned();
  const partsOf = (callId: number) => h.ran.filter(e => e.startsWith(`window.claxTransport.callResult(${JSON.stringify(nonce)}, ${callId}, {"body64":`) && e.endsWith(`,"more":true})`));
  const hash = await h.evalNow<string>(`fetch(${JSON.stringify(path)}).then(r => r.arrayBuffer()).then(b => crypto.subtle.digest("SHA-256", b)).then(d => [...new Uint8Array(d)].map(x => x.toString(16).padStart(2, "0")).join(""))`);
  expect(hash).toBe(createHash("sha256").update(body).digest("hex"));
  const [whole] = h.callsTo("GET", path);
  expect(partsOf(whole.id)).toHaveLength(Math.floor(body.length / PART));

  // Aborted once its first part has run in the shell.
  const unstall = h.stallParts(c => c.path === `${path}?abort=1`);
  await h.evalNow(`window.bigAbort = new AbortController(); window.big = fetch(${JSON.stringify(`${path}?abort=1`)}, { signal: bigAbort.signal }).then(r => r.arrayBuffer()).then(() => "read", e => e.name); 0`);
  await expect.poll(() => h.callsTo("GET", `${path}?abort=1`).length).toBe(1);
  const [aborted] = h.callsTo("GET", `${path}?abort=1`);
  await expect.poll(() => partsOf(aborted.id).length).toBe(1);
  await h.evalNow("bigAbort.abort(); 0");
  expect(await h.evalNow<string>("window.big")).toBe("AbortError");
  await h.barrier();
  expect(aborted.aborted).toBe(true);
  unstall();
  await h.idle();
  expect(partsOf(aborted.id)).toHaveLength(1);
  expect(h.ran.filter(e => e.startsWith(`window.claxTransport.callResult(${JSON.stringify(nonce)}, ${aborted.id}, {"status"`))).toEqual([]);
  // The shell's next call is answered as any other.
  expect(await h.evalNow<number>(`fetch("/api/artifacts/${id}").then(r => r.status)`)).toBe(200);
});

test("a shell that posts nothing before its load is pinned by its clax-nonce meta and gets claxHost.init", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Transport meta pin", { "index.html": PAGE });
  const h = await hostChannel(page, { relay: d, holdPosts: true });
  await page.goto(`${SHELL}/a/${artifact.id}?app=1`);
  const nonce = await h.pinned();
  expect(nonce).toBe(await h.evalNow<string>(`document.querySelector('meta[name="clax-nonce"]').content`));
  expect(h.pins).toEqual(["meta"]);
  await expect.poll(() => h.ran.filter(e => e.startsWith("window.claxHost.init(") && e.includes(JSON.stringify(nonce))).length).toBe(1);
  expect(h.accepted).toEqual([]);
  await h.releasePosts();
  await expect.poll(() => h.ofType("ready").length).toBeGreaterThan(0);
});

test("before the nonce an aborted call or stream rejects and posts nothing; after it, an abort posts callAbort or unsubscribe and its answer is dropped", async ({ page }) => {
  // A document without the nonce meta, as the app over loopback serves it.
  const { id, host: h } = await openInApp(page, "Transport aborts", { host: { nonceMeta: false, autoInit: false } });
  expect(await h.evalNow<boolean>(`document.querySelector('meta[name="clax-nonce"]') === null`)).toBe(true);
  const before = await h.evalNow<string[]>(`(async () => {
    const ac = new AbortController();
    const call = fetch("/api/artifacts/${id}?probe=before", { signal: ac.signal }).then(() => "answered", e => e.name);
    const stream = fetch("/api/stream", { signal: ac.signal }).then(() => "answered", e => e.name);
    ac.abort();
    return [await call, await stream];
  })()`);
  expect(before).toEqual(["AbortError", "AbortError"]);
  await h.barrier();
  // Nothing went out before the nonce, the view's own calls included.
  expect(h.calls).toEqual([]);
  expect(h.subscriptions).toEqual([]);
  await h.init();
  await live(h);
  expect(h.calls.filter(c => c.path.includes("probe=before"))).toEqual([]);

  // A call the core has not answered, aborted.
  const unstall = h.stall(c => c.path.includes("probe=after"));
  await h.evalNow(`window.probeAbort = new AbortController(); window.probe = fetch("/api/artifacts/${id}?probe=after", { signal: probeAbort.signal }).then(r => r.status, e => e.name); 0`);
  await expect.poll(() => h.calls.filter(c => c.path.includes("probe=after")).length).toBe(1);
  await h.evalNow("probeAbort.abort()");
  expect(await h.evalNow<string>("window.probe")).toBe("AbortError");
  await h.barrier();
  const [aborted] = h.calls.filter(c => c.path.includes("probe=after"));
  expect(h.ofType("callAbort")).toEqual([{ type: "callAbort", id: aborted.id }]);
  unstall();
  // A call after it is answered; the aborted one never is.
  expect(await h.evalNow<number>(`fetch("/api/artifacts/${id}?probe=later").then(r => r.status)`)).toBe(200);
  expect(aborted.answered).toBeUndefined();
  expect(h.ran.filter(e => e.startsWith(`window.claxTransport.callResult(${JSON.stringify(h.nonce)}, ${aborted.id},`))).toEqual([]);

  // A stream of the shell's own, aborted: the host closes its subscription.
  const streams = h.subscriptions.length;
  await h.evalNow(`window.streamAbort = new AbortController(); window.stream = fetch("/api/stream", { signal: streamAbort.signal }).then(r => r.body.getReader().read()).then(() => "read", e => e.name); 0`);
  await expect.poll(() => h.subscriptions.length).toBe(streams + 1);
  const s = h.subscriptions.at(-1)!;
  await h.evalNow("streamAbort.abort()");
  await h.barrier();
  expect(h.ofType("unsubscribe")).toEqual([{ type: "unsubscribe", id: s.id }]);
  expect(s.ended).toBe("unsubscribed");
});

test("the stream reopens by the core's end block: other ends back off, ready resets the backoff, and a final refusal stops", async ({ page }) => {
  await page.clock.install();
  const { host: h } = await openInApp(page, "Transport end blocks");
  await live(h);
  await page.clock.pauseAt(await h.evalNow<number>("Date.now()") + 1000);
  const opened = () => h.streamsOf().length;
  /** Runs `ms` of the page's clock, and waits until what the shell posted meanwhile has reached the host. */
  const run = async (ms: number) => { await page.clock.runFor(ms); await h.idle(); await h.barrier(); };
  /** Expects no reopen within `low` ms (a backoff's least) and one within
   * `high` ms (its most), from once what the host sent has run. */
  const reopensBetween = async (low: number, high: number) => {
    await h.idle();
    await h.barrier();
    const n = opened();
    await run(low - 10);
    expect(opened()).toBe(n);
    await run(high - low + 20);
    await expect.poll(opened).toBe(n + 1);
    return h.streamsOf().at(-1)!;
  };
  /** Waits until stream `s`'s ready has run in the shell. */
  const ready = (s: { id: number }) => expect.poll(() => h.ran.some(e => e.startsWith(`window.claxTransport.streamBlock(${JSON.stringify(h.nonce)}, ${s.id},`) && e.includes("event: ready"))).toBe(true);

  // A stream that ended reopens after the backoff's first step (0.5 s ± 20 %),
  // from where it stopped; one that ends again before its ready, after the second.
  h.endNextStream({ reason: "stream_ended" });
  h.endStreams({ reason: "stream_ended" });
  const first = await reopensBetween(400, 600);
  expect(first.lastEventId).toMatch(/^.+:\d+$/);
  const second = await reopensBetween(800, 1200);
  expect(second.lastEventId).toMatch(/^.+:\d+$/);
  // Its ready resets the backoff: the next end backs off from the first step again.
  await ready(second);
  h.endStreams({ reason: "stream_ended" });
  await ready(await reopensBetween(400, 600));

  // A refusal asking again can change (503) backs off; so does a core that closed.
  h.endNextStream({ reason: "stream_refused", status: 503 });
  h.endStreams({ reason: "core_closed" });
  await reopensBetween(400, 600);
  await ready(await reopensBetween(800, 1200));

  // A refusal asking again cannot change (403) stops it.
  h.endNextStream({ reason: "stream_refused", status: 403 });
  h.endStreams({ reason: "stream_ended" });
  await reopensBetween(400, 600);
  await h.idle();
  await h.barrier();
  const n = opened();
  await run(60_000);
  expect(opened()).toBe(n);
});

test("a WebView suspended longer than a tab's expiry, and a relay held for a minute, keep the view following on one stream", async ({ page }) => {
  await page.clock.install();
  const { id, host: h } = await openInApp(page, "Transport suspended");
  await live(h);
  await expect.poll(() => h.ofType("ready").length).toBeGreaterThan(0);
  const streams = h.subscriptions.length;
  const topics = h.callsTo("POST", /^\/api\/stream\//).length;
  // Its timers stood still past a tab's expiry (3 min), then run at once.
  await page.clock.fastForward(4 * 60_000);
  await publish(d.base, d.token, "Transport suspended", { "index.html": PAGE.replace("Quarterly", "Yearly") }, 1, id);
  await expect.poll(() => h.ofType("newer").map(m => m.version)).toContain(2);
  // The page's tab kept its topics: none was dropped from the stream.
  await h.barrier();
  expect(h.callsTo("POST", /^\/api\/stream\//).slice(topics).map(c => JSON.parse(c.body ?? "{}").unsubscribe ?? []).flat()).toEqual([]);
  // The relay answers nothing for a minute.
  h.hold();
  await publish(d.base, d.token, "Transport suspended", { "index.html": PAGE.replace("Quarterly", "Monthly") }, 2, id);
  await expect.poll(() => h.waiting.some(e => e.startsWith("window.claxTransport.streamBlock("))).toBe(true);
  await page.clock.fastForward(60_000);
  await h.release();
  await expect.poll(() => h.ofType("newer").map(m => m.version)).toContain(3);
  // The same stream all along.
  expect(h.subscriptions.length).toBe(streams);
});

test("a call the core holds past the stuck limit raises the shell's notice, which the app's transport hides", async ({ page }) => {
  await page.clock.install();
  // An artifact the core has none of: its shell document has no bootstrap,
  // so the view asks for the artifact, retrying while the call is stuck.
  const id = "zzzzzzzzzzzz";
  const h = await hostChannel(page, { relay: d });
  const asked = (c: { method: string; path: string }) => c.method === "GET" && c.path.split("?")[0] === `/api/artifacts/${id}`;
  const unstall = h.stall(asked);
  await page.goto(`${SHELL}/a/${id}?app=1`);
  await expect.poll(() => h.calls.filter(asked).length).toBe(1);
  await page.clock.runFor(STUCK_MS + 100);
  // The notice is raised (not hidden by the shell), and the transport's style hides it.
  const notice = page.locator(".conn-notice");
  await expect(notice).toBeAttached();
  await expect(notice).toHaveText(REQUEST_STUCK);
  expect(await notice.evaluate(e => (e as HTMLElement).hidden)).toBe(false);
  await expect(notice).toBeHidden();
  // The stuck call was abandoned and asked again.
  await page.clock.runFor(2_000);
  await expect.poll(() => h.calls.filter(asked).length).toBeGreaterThan(1);
  expect(h.ofType("callAbort").map(m => m.id)).toContain(h.calls.filter(asked)[0].id);
  unstall();
});

const PROBE = `<!doctype html><html><head><title>Probe</title></head><body><h2 id="t">Probe</h2><script>
(async () => {
  const read = async u => { try { const r = await fetch(u); return { type: r.type, status: r.status, body: await r.text() }; } catch (e) { return { error: e.name }; } };
  const shellDoc = await read(${JSON.stringify(SHELL)} + "/a/" + location.hostname.split(".")[0] + "?app=1");
  const shellApi = await read(${JSON.stringify(SHELL)} + "/api/artifacts");
  let parentDoc;
  try { parentDoc = typeof parent.document.title; } catch (e) { parentDoc = e.name; }
  const channel = typeof window.ClaxHost;
  try { window.${BINDING}(JSON.stringify({ type: "call", id: 4242, method: "GET", path: "/api/artifacts", headers: [], body: null, body64: null })); } catch { /* no binding */ }
  parent.postMessage({ probe: { shellDoc, shellApi, parentDoc, channel } }, "*");
})();
</script></body></html>`;

test("a page reads nothing of the shell and reaches nothing on its channel", async ({ page }) => {
  await page.addInitScript(shell => {
    if (location.origin !== shell || window.top !== window) return;
    addEventListener("message", e => { if (e.data?.probe) (window as unknown as { claxProbe: unknown }).claxProbe = { origin: e.origin, ...e.data.probe }; });
  }, SHELL);
  const { id, host: h, frame } = await openInApp(page, "Transport probe", { html: PROBE });
  await expect.poll(() => quietEval<unknown>(page, "window.claxProbe ?? null"), { timeout: 10_000 }).not.toBeNull();
  const probe = await quietEval<{ origin: string; shellDoc: { error?: string; type?: string; body?: string }; shellApi: { error?: string; type?: string; body?: string }; parentDoc: string; channel: string }>(page, "window.claxProbe");
  expect(probe.origin).toBe(content(id));
  for (const r of [probe.shellDoc, probe.shellApi]) {
    expect(r.error === "TypeError" || r.type === "opaque").toBe(true);
    expect(r.body ?? "").toBe("");
  }
  expect(probe.parentDoc).toBe("SecurityError");
  // The channel is in the shell's frames only, and the host takes no message
  // from another frame.
  expect(probe.channel).toBe("undefined");
  expect(h.dropped).toContainEqual({ why: "frame", message: expect.objectContaining({ id: 4242 }) });
  expect(h.calls.filter(c => c.id === 4242 && c.path === "/api/artifacts")).toEqual([]);
  // The scheme answers a shell document only to the main frame's
  // navigation. The content policy keeps the probe's fetch of it from
  // leaving the page; the frame's navigation of itself to it reaches the
  // scheme.
  await frame.evaluate(u => { location.replace(u); }, `${SHELL}/a/${id}?app=1`);
  const shellDocs = () => h.scheme.filter(s => s.url.startsWith(`${SHELL}/a/`) && !s.mainFrame);
  await expect.poll(() => shellDocs().length).toBeGreaterThanOrEqual(1);
  const refused = shellDocs();
  expect(refused.map(s => s.status)).toEqual(refused.map(() => 403));
});

test("the app opens no WebSocket: the live layers start none, and a page's room is refused capability_disabled", async ({ page }) => {
  // A socket from the app would leave its scheme handlers for the network,
  // the room's with the owner's token in its URL.
  const sockets: string[] = [];
  await page.routeWebSocket(/.*/, ws => { sockets.push(ws.url()); ws.close(); });
  const html = `<!doctype html><html><head><title>Room</title></head><body><p id="out">waiting</p>
<script>(async () => { const out = document.getElementById("out"); const room = await claude.use("room"); if (!room) { out.textContent = "null"; return; }
try { await room.join("table"); out.textContent = "joined"; } catch (e) { out.textContent = e.code; } })();</script></body></html>`;
  const { artifact } = await publishWith(d.base, d.token, "Transport no sockets", html, { room: {} });
  const h = await hostChannel(page, { relay: d });
  await page.goto(`${SHELL}/a/${artifact.id}?app=1`);
  const frame = await appFrame(page, artifact.id, 1);
  await live(h);
  await expect(frame.locator("#out")).toHaveText("capability_disabled");
  // The live layers start with the presence reporter, whose first read follows the first paint.
  await expect.poll(() => h.callsTo("GET", `/api/artifacts/${artifact.id}/presence`).length).toBeGreaterThan(0);
  await h.idle();
  await h.barrier();
  expect(sockets).toEqual([]);
});

test("a page's conditional request is answered whole: the scheme forwards Accept, never If-None-Match or If-Modified-Since", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Transport conditional", { "index.html": PAGE, "notes.html": "<!doctype html><p>notes</p>" });
  const h = await hostChannel(page, { relay: d });
  await page.goto(`${SHELL}/a/${artifact.id}?app=1`);
  const frame = await appFrame(page, artifact.id, 1);
  // The core's validator, as a page reads it from its own page.
  const plain = await frame.evaluate(async () => {
    const r = await fetch("notes.html", { cache: "no-store" });
    return { status: r.status, etag: r.headers.get("etag"), body: await r.text() };
  });
  expect(plain.status).toBe(200);
  expect(plain.etag).toMatch(/^"[0-9a-f]+"$/);
  const asked = await frame.evaluate(async etag => {
    const one = async (headers: Record<string, string>) => { const r = await fetch("notes.html", { headers, cache: "no-store" }); return { status: r.status, body: await r.text() }; };
    return [await one({ "if-none-match": etag }), await one({ "if-modified-since": new Date().toUTCString() })];
  }, plain.etag!);
  // Each is answered 200 with the whole page, as no WebView here takes a 304 from its host.
  expect(asked).toEqual([{ status: 200, body: plain.body }, { status: 200, body: plain.body }]);
  expect(h.scheme.filter(s => s.status === 304)).toEqual([]);
});

test("the core's redirects never reach the WebView: one within the same base is followed in the core, /v/<n> is a meta refresh, any other is 404", async ({ page }) => {
  // The bridge leaves a link marked external to the browser, which follows it.
  const html = PAGE.replace("</main>", `<a id="go" href="index.html" rel="external">Again</a></main>`);
  const { id, host: h, frame } = await openInApp(page, "Transport redirects", { html });
  const at = (path: string) => expect.poll(() => page.frames().some(f => f.url() === `${content(id)}${path}`), { timeout: 10_000 }).toBe(true);
  // index.html, which the core answers 308 to ./, is answered the page itself under its own URL.
  await frame.locator("#go").click();
  await at("/v/1/index.html");
  const followed = page.frames().find(f => f.url() === `${content(id)}/v/1/index.html`)!;
  await expect(followed.locator("#t")).toHaveText("Quarterly goals");
  // /v/1, which the core answers 308 to 1/, is a page that refreshes to it.
  await followed.evaluate(() => { location.href = "/v/1"; });
  await at("/v/1/");
  const refreshed = page.frames().find(f => f.url() === `${content(id)}/v/1/`)!;
  await expect(refreshed.locator("#t")).toHaveText("Quarterly goals");
  // A request for /v/1 that is not a document's is answered 404.
  expect(await refreshed.evaluate(() => fetch("/v/1").then(r => r.status))).toBe(404);
  const answered = (path: string) => h.scheme.filter(s => s.url === `${content(id)}${path}`).map(s => s.status);
  expect(answered("/v/1/index.html")).toEqual([200]);
  expect(answered("/v/1")).toEqual([200, 404]);
  expect(h.scheme.filter(s => s.status >= 300 && s.status < 400)).toEqual([]);
});

test.describe("at desktop width", () => {
  test.use({ hasTouch: false, isMobile: false, viewport: { width: 1280, height: 800 } });

  test("a thread's clip is fetched over the channel and shown by object URL", async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, "Transport clip", { "index.html": PAGE });
    const thread = await postThread(artifact.id, "with a clip");
    const h = await hostChannel(page, { relay: d });
    await page.goto(`${SHELL}/a/${artifact.id}`);
    await appFrame(page, artifact.id, 1);
    const img = page.locator("img.thumb").first();
    await expect(img).toBeVisible();
    await expect.poll(() => img.getAttribute("src")).toMatch(/^blob:/);
    await expect.poll(() => img.evaluate(e => (e as HTMLImageElement).naturalWidth)).toBe(1);
    expect(h.callsTo("GET", new RegExp(`/api/artifacts/${artifact.id}/threads/${thread.id}/clip`)).length).toBe(1);
  });
});

test("in the app the more menu has no Open raw or Copy link; a browser's shell keeps them, with no transport", async ({ page, daemon }) => {
  await openInApp(page, "Transport menu", { app: false });
  await page.getByRole("button", { name: "More" }).click();
  await expect(page.getByRole("menuitem").first()).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Open raw" })).toHaveCount(0);
  await expect(page.getByRole("menuitem", { name: "Copy link" })).toHaveCount(0);

  // The browser's shell, on a daemon that embeds nothing.
  const browser = await page.context().newPage();
  const transport: string[] = [];
  const apiCalls: string[] = [];
  browser.on("request", r => {
    const u = new URL(r.url());
    if (u.pathname === "/_clax/transport.js") transport.push(r.url());
    if (u.pathname.startsWith("/api/")) apiCalls.push(u.pathname);
  });
  const { artifact } = await publish(daemon.base, daemon.token, "Browser menu", { "index.html": PAGE });
  await browser.goto(`${daemon.base}/a/${artifact.id}`);
  await expect(browser.getByRole("button", { name: "More" })).toBeVisible();
  // Its calls go over the network, with the platform's fetch.
  await expect.poll(() => apiCalls.length).toBeGreaterThan(0);
  expect(await quietEval<string>(browser, "typeof window.claxTransport")).toBe("undefined");
  expect(await quietEval<boolean>(browser, `document.querySelector('meta[name="clax-embed"]') === null`)).toBe(true);
  expect(transport).toEqual([]);
  await browser.getByRole("button", { name: "More" }).click();
  await expect(browser.getByRole("menuitem", { name: "Open raw" })).toBeVisible();
  await expect(browser.getByRole("menuitem", { name: "Copy link" })).toBeVisible();
  await browser.close();
});
