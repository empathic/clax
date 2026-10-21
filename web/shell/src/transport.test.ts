import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { CHUNK, CHUNKED_OVER, type Channel, type HostReceivers, hostEmbed, installTransport, onHost, readEmbed, transport } from "./transport";

const ORIGIN = "https://app.clax.invalid";
/** For the cases that encode or decode megabytes: they wait on no condition, so only the machine's load bounds them. */
const HEAVY = { timeout: 120_000 };
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
    await f("/_clax/bridge.js"); await f("https://example.com/api/x"); await f(new URL("https://example.com/api/y"));
    expect(fb).toHaveBeenCalledTimes(3);
  });
  it("takes a URL or a Request on its origin as a string", async () => {
    const { sent, f } = open();
    void f(new URL("/api/u?x=1", ORIGIN));
    void f(new Request(`${ORIGIN}/api/r`, { method: "PUT", body: "r" }));
    await vi.waitFor(() => expect(sent.length).toBe(2));
    expect(sent.map(m => [m.method, m.path])).toEqual([["GET", "/api/u?x=1"], ["PUT", "/api/r"]]);
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
  it("chunks a body over the limit, each chunk its slice of the body, in order", async () => {
    const { sent, f } = open();
    const body = new Uint8Array(CHUNKED_OVER + 3).map((_, i) => (i * 7 + (i >> 20)) % 251);
    void f("/api/assets", { method: "POST", body });
    await vi.waitFor(() => expect(sent.filter(m => m.type === "callChunk").length).toBe(9));
    const [call, ...chunks] = sent;
    expect(call).toMatchObject({ type: "call", chunked: true, body: null, body64: null });
    expect(chunks.map(c => [c.id, c.n, c.last])).toEqual([...Array(9).keys()].map(n => [call.id, n, n === 8]));
    // Decoded with Node's base64, not the transport's.
    const same = chunks.map((c, n) => Buffer.from(c.body64 as string, "base64").equals(body.subarray(n * CHUNK, (n + 1) * CHUNK)));
    expect(same).toEqual(chunks.map(() => true));
  }, HEAVY);
  it("answers a result sent in parts: each part's base64 joins the next, and the last carries the status and headers", async () => {
    const { sent, t, f } = open();
    // Parts of a multiple of 3 bytes, so their base64 texts join into the body's.
    const PART = 3 << 18;
    const body = new Uint8Array(PART * 2 + 5).map((_, i) => i % 253);
    const p = f("/api/big");
    await vi.waitFor(() => expect(sent.length).toBe(1));
    const id = sent[0].id as number;
    // Encoded with Node's base64, not the transport's.
    const part = (n: number) => Buffer.from(body.subarray(n * PART, (n + 1) * PART)).toString("base64");
    let settled = false;
    void p.then(() => { settled = true; });
    // Another load's part, which names this call's ID: dropped.
    t.callResult("O".repeat(16), id, { body64: part(1), more: true });
    t.callResult(NONCE, id, { body64: part(0), more: true });
    t.callResult(NONCE, id, { body64: part(1), more: true });
    await new Promise(r => setTimeout(r, 0));
    expect(settled).toBe(false);
    t.callResult(NONCE, id, { status: 200, headers: [["content-type", "application/octet-stream"]], body64: part(2) });
    const r = await p;
    expect(r.status).toBe(200);
    expect(r.headers.get("content-type")).toBe("application/octet-stream");
    expect(Buffer.from(await r.arrayBuffer()).equals(Buffer.from(body))).toBe(true);
  }, HEAVY);
  it("assembles the host relay's own scripts for a 2.5 MiB answer into its body", async () => {
    // Written by apps/clax_app/test/core/channel_relay_test.dart from what ChannelRelay runs.
    const fx = JSON.parse(readFileSync(join(__dirname, "../../../apps/clax_app/test/fixtures/relay_answer_parts.json"), "utf8")) as { nonce: string; id: number; bytes: number; scripts: string[] };
    const sent: Record<string, unknown>[] = [];
    const t = transport(() => ({ postMessage: (m: string) => sent.push(JSON.parse(m) as Record<string, unknown>) }), vi.fn(), ORIGIN, fx.nonce);
    const p = t.fetch("/api/blob");
    await vi.waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0].id).toBe(fx.id);
    expect(fx.scripts.filter(js => js.includes('"more":true'))).toHaveLength(fx.scripts.length - 1);
    for (const js of fx.scripts) new Function("window", js)({ claxTransport: t });
    const r = await p;
    expect(r.status).toBe(200);
    const want = Buffer.from(Uint8Array.from({ length: fx.bytes }, (_, i) => (i * 31 + (i >> 9)) & 255));
    expect(Buffer.from(await r.arrayBuffer()).equals(want)).toBe(true);
  }, HEAVY);
  it("an abort between parts rejects the call, posts callAbort, and drops the parts after it; a part of no waiting call does nothing", async () => {
    const { sent, t, f } = open();
    const ac = new AbortController();
    const ps = [f("/api/a", { signal: ac.signal }), f("/api/b")];
    await vi.waitFor(() => expect(sent.length).toBe(2));
    const [a, b] = ["/api/a", "/api/b"].map(path => sent.find(m => m.path === path)!.id as number);
    t.callResult(NONCE, a, { body64: "AAAA", more: true });
    ac.abort();
    await expect(ps[0]).rejects.toThrow(/abort/i);
    expect(sent.at(-1)).toEqual({ type: "callAbort", id: a, nonce: NONCE });
    t.callResult(NONCE, a, { body64: "AAAA", more: true });
    t.callResult(NONCE, a, { status: 200, headers: [], body64: "AAAA" });
    t.callResult(NONCE, 999, { body64: "AAAA", more: true });
    t.callResult(NONCE, b, { status: 200, headers: [], body: "b" });
    expect(await (await ps[1]).text()).toBe("b");
  });
  it("answers a null-body status with no body, whatever the app sent", async () => {
    const { sent, t, f } = open();
    const p = f("/api/x", { method: "DELETE" });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    t.callResult(NONCE, sent[0].id as number, { status: 204, headers: [], body: "ignored" });
    const r = await p;
    expect(r.status).toBe(204);
    expect(r.body).toBeNull();
  });
  it("rejects with a TypeError on a malformed result", async () => {
    const { sent, t, f } = open();
    const bad = [{ status: 0, headers: [] }, { status: 200, headers: [], body64: "not base64!" }, null];
    const ps = bad.map(() => f("/api/x"));
    await vi.waitFor(() => expect(sent.length).toBe(bad.length));
    bad.forEach((r, i) => t.callResult(NONCE, sent[i].id as number, r as never));
    for (const p of ps) await expect(p).rejects.toBeInstanceOf(TypeError);
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
  it("subscribes from the stream's resume point, its Last-Event-ID or last_event_id, when it has one", async () => {
    const { sent, f } = open();
    await f("/api/stream", { headers: { "Last-Event-ID": "abc:4" } });
    await f("/api/stream?last_event_id=abc%3A9");
    await f("/api/stream");
    expect(sent.map(m => m.lastEventId)).toEqual(["abc:4", "abc:9", undefined]);
    expect(Object.keys(sent[2]).sort()).toEqual(["id", "nonce", "type"]);
  });
  it("ends an open stream its signal aborts, and unsubscribes", async () => {
    const { sent, t, f } = open();
    const ac = new AbortController();
    const r = await f("/api/stream", { signal: ac.signal });
    const id = sent[0].id as number;
    const reader = r.body!.getReader();
    ac.abort();
    await expect(reader.read()).rejects.toThrow(/abort/i);
    expect(sent.at(-1)).toEqual({ type: "unsubscribe", id, nonce: NONCE });
    t.streamBlock(NONCE, id, "late");
    expect(sent.filter(m => m.type === "unsubscribe").length).toBe(1);
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
  it("rejects a call and a stream with a TypeError when the document has no channel, and leaves nothing waiting", async () => {
    /** Settles with what `p` rejects with, so a call that never settles fails the test by its timeout. */
    const noChannel = async (p: Promise<unknown>) => {
      const e = await p.then(() => null, (x: unknown) => x);
      expect(e).toBeInstanceOf(TypeError);
      expect((e as Error).message).toBe("no app channel");
    };
    let ch: Partial<Channel> | undefined;
    const sent: Record<string, unknown>[] = [];
    const t = transport(() => ch as Channel | undefined, vi.fn(), ORIGIN, NONCE);
    await noChannel(t.fetch("/api/x", { method: "POST", body: "x" }));
    await noChannel(t.fetch("/api/stream"));
    ch = {};
    await noChannel(t.fetch("/api/y"));
    await noChannel(t.fetch("/api/stream"));
    ch = { postMessage: m => sent.push(JSON.parse(m) as Record<string, unknown>) };
    // The refused calls' IDs answer nothing.
    for (const id of [1, 2, 3, 4]) t.callResult(NONCE, id, { status: 500, headers: [], body: "late" });
    const p = t.fetch("/api/z");
    await vi.waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0]).toMatchObject({ type: "call", path: "/api/z" });
    t.callResult(NONCE, sent[0].id as number, { status: 204, headers: [] });
    expect((await p).status).toBe(204);
  });
  it("rejects with a TypeError a part, or the last part after parts, that carries no body64, never gluing it on as text", async () => {
    const { sent, t, f } = open();
    const ps = [f("/api/a"), f("/api/b"), f("/api/c")];
    await vi.waitFor(() => expect(sent.length).toBe(3));
    const [a, b, c] = sent.map(m => m.id as number);
    t.callResult(NONCE, a, { body64: "AAAA", more: true });
    t.callResult(NONCE, a, { status: 200, headers: [], body64: null, body: "x" });
    t.callResult(NONCE, b, { body64: "AAAA", more: true });
    t.callResult(NONCE, b, { status: 200, headers: [] });
    t.callResult(NONCE, c, { body64: null, more: true } as never);
    t.callResult(NONCE, c, { status: 200, headers: [], body64: "AAAA" });
    for (const p of ps) await expect(p).rejects.toBeInstanceOf(TypeError);
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

describe("the transport on Apple's origins (clax:)", () => {
  const APPLE = "clax://app";
  it("relays /api on clax://app, relative or absolute, and passes anything else to the fallback", async () => {
    const sent: Record<string, unknown>[] = [];
    const fb = vi.fn<typeof fetch>(async () => new Response("x"));
    const t = transport(() => ({ postMessage: (m: string) => sent.push(JSON.parse(m) as Record<string, unknown>) }), fb, APPLE, NONCE);
    void t.fetch("/api/a?x=1");
    void t.fetch("clax://app/api/b", { method: "PUT", body: "b" });
    void t.fetch(new URL("/api/c", APPLE));
    await vi.waitFor(() => expect(sent.length).toBe(3));
    // A body is read before its call goes, so the PUT may go last.
    expect(sent.map(m => [m.method, m.path]).sort((x, y) => String(x[1]).localeCompare(String(y[1])))).toEqual([["GET", "/api/a?x=1"], ["PUT", "/api/b"], ["GET", "/api/c"]]);
    const elsewhere = ["clax://abcdefghijkl.c/api/x", "clax://abcdefghijkl.c/v/1/", "/_clax/bridge.js", "clax://app/a/abcdefghijkl", "https://app/api/x", "clax://app.c/api/x"];
    for (const u of elsewhere) await t.fetch(u);
    expect(fb.mock.calls.map(c => String(c[0]))).toEqual(elsewhere);
    expect(sent).toHaveLength(3);
  });
  it("subscribes to clax://app/api/stream and answers its blocks", async () => {
    const sent: Record<string, unknown>[] = [];
    const t = transport(() => ({ postMessage: (m: string) => sent.push(JSON.parse(m) as Record<string, unknown>) }), vi.fn(), APPLE, NONCE);
    const r = await t.fetch("clax://app/api/stream?last_event_id=s%3A3");
    expect(sent).toEqual([{ type: "subscribe", id: 1, lastEventId: "s:3", nonce: NONCE }]);
    t.streamBlock(NONCE, 1, "event: ready\ndata: {}\n\n");
    expect(new TextDecoder().decode((await r.body!.getReader().read()).value)).toBe("event: ready\ndata: {}\n\n");
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
  const win = { document: doc, fetch: native, ClaxHost: { postMessage: (m: string) => posted.push(m) } } as unknown as Window & { claxTransport?: HostReceivers };
  return { win, posted, native, doc };
}

describe("installTransport", () => {
  it("reads the embed meta, and nothing else", () => {
    expect(readEmbed(fakeWin(META).doc)).toEqual({ transport: "host", shell: "clax://app", content: "clax://{id}.c" });
    expect(readEmbed(fakeWin(null).doc)).toBeNull();
    expect(readEmbed(fakeWin("{").doc)).toBeNull();
    expect(readEmbed(fakeWin(`{"transport":"http","shell":"x","content":"y"}`).doc)).toBeNull();
    expect(readEmbed(fakeWin(`{"transport":"host","shell":"https://app.clax.invalid","content":"https://{id}.c.clax.invalid"}`).doc)).toEqual({ transport: "host", shell: "https://app.clax.invalid", content: "https://{id}.c.clax.invalid" });
  });

  it("refuses an embed whose origins are not https: or clax:, or whose content names {id} other than once", () => {
    for (const [shell, content] of [["http://app.clax.invalid", "https://{id}.c.clax.invalid"], ["https://app.clax.invalid", "http://{id}.c.clax.invalid"], ["clax://app", "ws://{id}.c"],
      ["clax://app", "clax://c"], ["clax://app", "clax://{id}.{id}.c"], ["app", "clax://{id}.c"], ["clax:app", "clax://{id}.c"]]) {
      expect(readEmbed(fakeWin(JSON.stringify({ transport: "host", shell, content })).doc)).toBeNull();
    }
  });

  it("leaves a browser's document alone", () => {
    const { win, native, doc } = fakeWin(null);
    expect(installTransport(win)).toBe(false);
    expect(win.fetch).toBe(native);
    expect(win.claxTransport).toBeUndefined();
    expect(hostEmbed(win)).toBeNull();
    expect(onHost(win)).toBe(false);
    expect(doc.querySelector("style")).toBeNull();
  });

  it("in app mode gives any shell document its own receivers, posts its API calls once init gives the nonce, and hides the notices", async () => {
    // No claxHost here: a gallery, or an artifact view the app opened without app=1.
    const { win, posted, native, doc } = fakeWin(META);
    expect(installTransport(win)).toBe(true);
    expect(Object.keys(win.claxTransport!).sort()).toEqual(["callResult", "embed", "init", "streamBlock", "streamEnd"]);
    expect(hostEmbed(win)).toEqual({ transport: "host", shell: "clax://app", content: "clax://{id}.c" });
    expect(Object.isFrozen(win.claxTransport)).toBe(true);
    expect(Object.isFrozen(hostEmbed(win))).toBe(true);
    expect(onHost(win)).toBe(true);
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
