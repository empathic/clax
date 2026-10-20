import { describe, expect, it, vi } from "vitest";
import { REQUEST_STUCK, connNoticeText, connNotices, connTrouble } from "./conn-notice";
import { hostFetch, callResult, installTransport, readEmbed, streamBlock, streamEnd, CHUNKED_OVER } from "./transport";

function link() {
  const sent: Record<string, unknown>[] = [];
  return { sent, link: { post: (m: Record<string, unknown>) => sent.push(m), ready: Promise.resolve() } };
}
const ORIGIN = "https://app.clax.invalid";

describe("hostFetch", () => {
  it("relays an /api call and resolves on its result", async () => {
    const { sent, link: l } = link();
    const f = hostFetch(l, vi.fn(), ORIGIN);
    const p = f("/api/artifacts", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0]).toMatchObject({ type: "call", method: "POST", path: "/api/artifacts", body: "{}" });
    callResult(sent[0].id as number, { status: 201, headers: [["content-type", "application/json"]], body: '{"ok":true}' });
    const r = await p;
    expect(r.status).toBe(201);
    expect(await r.json()).toEqual({ ok: true });
  });
  it("passes anything but /api on its origin to the fallback", async () => {
    const fb = vi.fn(async () => new Response("x"));
    const f = hostFetch(link().link, fb, ORIGIN);
    await f("/_clax/bridge.js"); await f("https://example.com/api/x");
    expect(fb).toHaveBeenCalledTimes(2);
  });
  it("sends a binary body as base64", async () => {
    const { sent, link: l } = link();
    void hostFetch(l, vi.fn(), ORIGIN)("/api/assets", { method: "POST", body: new Uint8Array([0, 255]) });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0]).toMatchObject({ body: null, body64: "AP8=" });
  });
  it("answers a binary result from body64", async () => {
    const { sent, link: l } = link();
    const p = hostFetch(l, vi.fn(), ORIGIN)("/api/assets/x");
    await vi.waitFor(() => expect(sent.length).toBe(1));
    callResult(sent[0].id as number, { status: 200, headers: [["content-type", "image/png"]], body64: "AP8=" });
    expect([...new Uint8Array(await (await p).arrayBuffer())]).toEqual([0, 255]);
  });
  it("chunks a body over the limit", async () => {
    const { sent, link: l } = link();
    void hostFetch(l, vi.fn(), ORIGIN)("/api/assets", { method: "POST", body: new Uint8Array(CHUNKED_OVER + 1) });
    await vi.waitFor(() => expect(sent.filter(m => m.type === "callChunk").length).toBe(9));
    expect(sent.at(-1)).toMatchObject({ type: "callChunk", last: true });
  });
  it("aborts", async () => {
    const { sent, link: l } = link();
    const ac = new AbortController();
    const p = hostFetch(l, vi.fn(), ORIGIN)("/api/x", { signal: ac.signal });
    await vi.waitFor(() => expect(sent.length).toBe(1));
    ac.abort();
    await expect(p).rejects.toThrow(/abort/i);
    expect(sent.at(-1)).toMatchObject({ type: "callAbort" });
  });
  it("streams /api/stream from pushed blocks and ends", async () => {
    const { sent, link: l } = link();
    const r = await hostFetch(l, vi.fn(), ORIGIN)("/api/stream");
    const id = sent[0].id as number;
    expect(sent[0]).toMatchObject({ type: "subscribe" });
    const reader = r.body!.getReader();
    streamBlock(id, "event: ready\ndata: {}\n\n");
    expect(new TextDecoder().decode((await reader.read()).value)).toContain("event: ready");
    streamEnd(id);
    expect((await reader.read()).done).toBe(true);
  });
  it("unsubscribes when the stream is cancelled", async () => {
    const { sent, link: l } = link();
    const r = await hostFetch(l, vi.fn(), ORIGIN)("/api/stream");
    const id = sent[0].id as number;
    await r.body!.cancel();
    expect(sent.at(-1)).toEqual({ type: "unsubscribe", id });
  });
});

const META = `{"transport":"host","shell":"clax://app","content":"clax://{id}.c"}`;

function fakeWin(meta: string | null) {
  const doc = document.implementation.createHTMLDocument("shell");
  if (meta !== null) {
    const m = doc.createElement("meta");
    m.name = "clax-embed";
    m.content = meta;
    doc.head.append(m);
  }
  const posted: string[] = [];
  const native = vi.fn(async () => new Response("native"));
  const win = { document: doc, fetch: native, ClaxHost: { postMessage: (m: string) => posted.push(m) } } as unknown as Window;
  return { win, posted, native };
}

describe("installTransport", () => {
  it("reads the embed meta, and nothing else", () => {
    expect(readEmbed(fakeWin(META).win.document)).toEqual({ transport: "host", shell: "clax://app", content: "clax://{id}.c" });
    expect(readEmbed(fakeWin(null).win.document)).toBeNull();
    expect(readEmbed(fakeWin("{").win.document)).toBeNull();
    expect(readEmbed(fakeWin(`{"transport":"http","shell":"x","content":"y"}`).win.document)).toBeNull();
  });

  it("leaves a browser's fetch alone", () => {
    const { win, native } = fakeWin(null);
    expect(installTransport(win, Promise.resolve("n".repeat(16)))).toBe(false);
    expect(win.fetch).toBe(native);
  });

  it("in app mode posts the shell's API calls with the nonce, once given, and shows no notices", async () => {
    const { win, posted, native } = fakeWin(META);
    let give: (n: string) => void = () => {};
    expect(installTransport(win, new Promise<string>(r => { give = r; }))).toBe(true);
    try {
      connTrouble("artifact", true, REQUEST_STUCK, win.document);
      expect(connNoticeText()).toBeNull();
      const p = win.fetch("/api/artifacts/abc");
      await win.fetch("clax://abc.c/v/1/index.html");
      expect(native).toHaveBeenCalledOnce();
      expect(posted).toEqual([]);
      give("N".repeat(16));
      await vi.waitFor(() => expect(posted.length).toBe(1));
      const m = JSON.parse(posted[0]) as Record<string, unknown>;
      expect(m).toMatchObject({ type: "call", method: "GET", path: "/api/artifacts/abc", nonce: "N".repeat(16) });
      callResult(m.id as number, { status: 204, headers: [] });
      expect((await p).status).toBe(204);
    } finally {
      connTrouble("artifact", false, REQUEST_STUCK, win.document);
      connNotices(true);
    }
  });
});
