import { describe, expect, it, vi } from "vitest";
import { CHUNKED_OVER, type Transport, installTransport, readEmbed, transport } from "./transport";

const ORIGIN = "https://app.clax.invalid";
const NONCE = "N".repeat(16);

/** A transport over a channel that records what it posts, given `NONCE`. */
function open() {
  const sent: Record<string, unknown>[] = [];
  const t = transport(() => ({ postMessage: (m: string) => sent.push(JSON.parse(m) as Record<string, unknown>) }), vi.fn(), ORIGIN);
  t.init(NONCE);
  return { sent, t, f: t.fetch };
}

describe("the transport", () => {
  it("relays an /api call with the nonce and resolves on its result", async () => {
    const { sent, t, f } = open();
    const p = f("/api/artifacts", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0]).toMatchObject({ type: "call", method: "POST", path: "/api/artifacts", body: "{}", nonce: NONCE });
    t.callResult(NONCE, sent[0].id as number, { status: 201, headers: [["content-type", "application/json"]], body: '{"ok":true}' });
    const r = await p;
    expect(r.status).toBe(201);
    expect(await r.json()).toEqual({ ok: true });
  });
  it("passes anything but /api on its origin to the fallback", async () => {
    const fb = vi.fn(async () => new Response("x"));
    const f = transport(() => undefined, fb, ORIGIN).fetch;
    await f("/_clax/bridge.js"); await f("https://example.com/api/x");
    expect(fb).toHaveBeenCalledTimes(2);
  });
  it("sends a binary body as base64", async () => {
    const { sent, f } = open();
    void f("/api/assets", { method: "POST", body: new Uint8Array([0, 255]) });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0]).toMatchObject({ body: null, body64: "AP8=" });
  });
  it("answers a binary result from body64", async () => {
    const { sent, t, f } = open();
    const p = f("/api/assets/x");
    await vi.waitFor(() => expect(sent.length).toBe(1));
    t.callResult(NONCE, sent[0].id as number, { status: 200, headers: [["content-type", "image/png"]], body64: "AP8=" });
    expect([...new Uint8Array(await (await p).arrayBuffer())]).toEqual([0, 255]);
  });
  it("chunks a body over the limit", async () => {
    const { sent, f } = open();
    void f("/api/assets", { method: "POST", body: new Uint8Array(CHUNKED_OVER + 1) });
    await vi.waitFor(() => expect(sent.filter(m => m.type === "callChunk").length).toBe(9));
    expect(sent.at(-1)).toMatchObject({ type: "callChunk", last: true });
  });
  it("aborts", async () => {
    const { sent, f } = open();
    const ac = new AbortController();
    const p = f("/api/x", { signal: ac.signal });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    ac.abort();
    await expect(p).rejects.toThrow(/abort/i);
    expect(sent.at(-1)).toMatchObject({ type: "callAbort" });
  });
  it("streams /api/stream from pushed blocks and ends", async () => {
    const { sent, t, f } = open();
    const r = await f("/api/stream");
    const id = sent[0].id as number;
    expect(sent[0]).toMatchObject({ type: "subscribe" });
    const reader = r.body!.getReader();
    t.streamBlock(NONCE, id, "event: ready\ndata: {}\n\n");
    expect(new TextDecoder().decode((await reader.read()).value)).toContain("event: ready");
    t.streamEnd(NONCE, id);
    expect((await reader.read()).done).toBe(true);
  });
  it("unsubscribes when the stream is cancelled", async () => {
    const { sent, f } = open();
    const r = await f("/api/stream");
    const id = sent[0].id as number;
    await r.body!.cancel();
    expect(sent.at(-1)).toEqual({ type: "unsubscribe", id, nonce: NONCE });
  });
  it("drops a late answer from an old load that names an ID the new load uses too", async () => {
    const OLD = "O".repeat(16);
    const old = transport(() => ({ postMessage: () => {} }), vi.fn(), ORIGIN);
    old.init(OLD);
    void old.fetch("/api/old");
    const { sent, t, f } = open();
    const call = f("/api/x");
    await vi.waitFor(() => expect(sent.length).toBe(1));
    const stream = await f("/api/stream");
    await vi.waitFor(() => expect(sent.length).toBe(2));
    const c = sent.find(m => m.type === "call")!;
    const sub = sent.find(m => m.type === "subscribe")!;
    // The new load's first call has the ID the old load's first call had.
    expect(c.id).toBe(1);
    // The app's answers to the old load's call and stream, run in the new document.
    let settled = false;
    void call.then(() => { settled = true; });
    t.callResult(OLD, c.id as number, { status: 500, headers: [], body: "old" });
    const reader = stream.body!.getReader();
    t.streamBlock(OLD, sub.id as number, "event: old\n\n");
    t.streamEnd(OLD, sub.id as number);
    t.streamBlock(NONCE, sub.id as number, "event: new\n\n");
    expect(new TextDecoder().decode((await reader.read()).value)).toBe("event: new\n\n");
    await new Promise(r => setTimeout(r, 0));
    expect(settled).toBe(false);
    t.callResult(NONCE, c.id as number, { status: 200, headers: [], body: "new" });
    expect(await (await call).text()).toBe("new");
  });
  it("rejects a call and a stream aborted before init, and posts neither", async () => {
    const sent: string[] = [];
    const t = transport(() => ({ postMessage: m => sent.push(m) }), vi.fn(), ORIGIN);
    const ac = new AbortController();
    const call = t.fetch("/api/x", { method: "POST", body: "x", signal: ac.signal });
    const stream = t.fetch("/api/stream", { signal: ac.signal });
    await new Promise(r => setTimeout(r, 0));
    ac.abort();
    await expect(call).rejects.toThrow(/abort/i);
    await expect(stream).rejects.toThrow(/abort/i);
    t.init(NONCE);
    await new Promise(r => setTimeout(r, 0));
    expect(sent).toEqual([]);
  });
  it("posts nothing before init, and every call made before it once it comes", async () => {
    const sent: string[] = [];
    const t = transport(() => ({ postMessage: m => sent.push(m) }), vi.fn(), ORIGIN);
    void t.fetch("/api/a");
    t.init("short");
    await new Promise(r => setTimeout(r, 0));
    expect(sent).toEqual([]);
    t.init(NONCE);
    await vi.waitFor(() => expect(sent.length).toBe(1));
    t.init("M".repeat(16));
    void t.fetch("/api/b");
    await vi.waitFor(() => expect(sent.length).toBe(2));
    expect(sent.map(m => JSON.parse(m).nonce)).toEqual([NONCE, NONCE]);
  });
});

const META = `{"transport":"host","shell":"clax://app","content":"clax://{id}.c"}`;

function fakeWin(meta: string | null, nonce?: string) {
  const doc = document.implementation.createHTMLDocument("shell");
  if (nonce !== undefined) {
    const m = doc.createElement("meta");
    m.name = "clax-nonce";
    m.content = nonce;
    doc.head.append(m);
  }
  if (meta !== null) {
    const m = doc.createElement("meta");
    m.name = "clax-embed";
    m.content = meta;
    doc.head.append(m);
  }
  const posted: string[] = [];
  const native = vi.fn(async () => new Response("native"));
  const win = { document: doc, fetch: native, ClaxHost: { postMessage: (m: string) => posted.push(m) } } as unknown as Window & { claxTransport?: Omit<Transport, "fetch"> };
  return { win, posted, native, doc };
}

describe("installTransport", () => {
  it("reads the embed meta, and nothing else", () => {
    expect(readEmbed(fakeWin(META).doc)).toEqual({ transport: "host", shell: "clax://app", content: "clax://{id}.c" });
    expect(readEmbed(fakeWin(null).doc)).toBeNull();
    expect(readEmbed(fakeWin("{").doc)).toBeNull();
    expect(readEmbed(fakeWin(`{"transport":"http","shell":"x","content":"y"}`).doc)).toBeNull();
  });

  it("leaves a browser's document alone", () => {
    const { win, native, doc } = fakeWin(null);
    expect(installTransport(win)).toBe(false);
    expect(win.fetch).toBe(native);
    expect(win.claxTransport).toBeUndefined();
    expect(doc.querySelector("style")).toBeNull();
  });

  it("in app mode gives any shell document its own receivers, posts its API calls once init gives the nonce, and hides the notices", async () => {
    // No claxHost here: a gallery, or an artifact view the app opened without app=1.
    const { win, posted, native, doc } = fakeWin(META);
    expect(installTransport(win)).toBe(true);
    expect(Object.keys(win.claxTransport!).sort()).toEqual(["callResult", "init", "streamBlock", "streamEnd"]);
    expect([...doc.querySelectorAll("style")].map(s => s.textContent)).toEqual([".conn-notice{display:none!important}"]);
    const p = win.fetch("/api/artifacts/abc");
    await win.fetch("clax://abc.c/v/1/index.html");
    expect(native).toHaveBeenCalledOnce();
    expect(posted).toEqual([]);
    win.claxTransport!.init(NONCE);
    await vi.waitFor(() => expect(posted.length).toBe(1));
    const m = JSON.parse(posted[0]) as Record<string, unknown>;
    expect(m).toMatchObject({ type: "call", method: "GET", path: "/api/artifacts/abc", nonce: NONCE });
    win.claxTransport!.callResult(NONCE, m.id as number, { status: 204, headers: [] });
    expect((await p).status).toBe(204);
  });

  it("posts from boot under the nonce the app wrote into the document, and ignores init", async () => {
    const { win, posted } = fakeWin(META, NONCE);
    installTransport(win);
    void win.fetch("/api/a");
    await vi.waitFor(() => expect(posted.length).toBe(1));
    win.claxTransport!.init("M".repeat(16));
    void win.fetch("/api/b");
    await vi.waitFor(() => expect(posted.length).toBe(2));
    expect(posted.map(m => JSON.parse(m).nonce)).toEqual([NONCE, NONCE]);
  });

  it("waits for init when the document's nonce is not well formed", async () => {
    const { win, posted } = fakeWin(META, "short");
    installTransport(win);
    void win.fetch("/api/a");
    await new Promise(r => setTimeout(r, 0));
    expect(posted).toEqual([]);
    win.claxTransport!.init(NONCE);
    await vi.waitFor(() => expect(posted.length).toBe(1));
    expect(JSON.parse(posted[0]).nonce).toBe(NONCE);
  });
});
