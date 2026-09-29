import { beforeEach, describe, expect, it, vi } from "vitest";
import { commentsContext, commentsLocals } from "../src/caps/comments";

type Listener = (d: unknown) => void;
function fakeRpc(answer: (method: string, args: unknown[]) => unknown = () => ({ opened: true })) {
  const listeners = new Map<string, Listener>();
  return {
    emit: (topic: string, data: unknown) => listeners.get(topic)?.(data),
    rpc: {
      call: vi.fn(async (_ns: string, method: string, args: unknown[]) => answer(method, args)),
      on: (_ns: string, topic: string, f: Listener) => { listeners.set(topic, f); return () => listeners.delete(topic); },
    },
  };
}

describe("comments (page side)", () => {
  beforeEach(() => {
    document.body.innerHTML = `<main><section class="card"><h2>Q3</h2><p>Revenue grew.</p></section><div data-uncommentable><button id="u">x</button></div></main>`;
    commentsContext.version = 3;
    commentsContext.file = "notes.html";
    commentsContext.live = false;
  });

  it("openComposer builds the anchor in the page and never sends a detached target", async () => {
    const f = fakeRpc();
    const c = commentsLocals(f.rpc as never, {}) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    await expect(c.openComposer({ element: document.querySelector("section")! })).resolves.toEqual({ opened: true });
    const sent = (f.rpc.call.mock.calls[0][2] as [{ anchor: { kind: string; selector: string; quote: string }; version: number }])[0];
    expect(sent.anchor).toMatchObject({ kind: "element", selector: "body > main > section" });
    expect(sent.anchor.quote).toContain("Revenue grew.");
    expect(sent.version).toBe(3);
    expect((sent.anchor as unknown as { file: string }).file).toBe("notes.html");
    await expect(c.openComposer({ element: document.createElement("div") })).rejects.toMatchObject({ code: "invalid" });
    await expect(c.openComposer({})).rejects.toMatchObject({ code: "invalid" });
    await expect(c.openComposer({ element: document.getElementById("u")! })).resolves.toEqual({ opened: false });
  });

  it("anchorFor and create validate before the shell", async () => {
    const f = fakeRpc(() => ({ threadId: "t", commentId: "c" }));
    const c = commentsLocals(f.rpc as never, {}) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    const anchor = (await c.anchorFor(document.querySelector("h2")!)) as { path: string; x: number; y: number };
    expect(anchor.path).toBe("body > main > section > h2");
    await expect(c.create({ anchor, text: "  " })).rejects.toMatchObject({ code: "invalid" });
    await expect(c.create({ anchor, text: "a\u0007b" })).rejects.toMatchObject({ code: "invalid" });
    await expect(c.create({ anchor, text: "x".repeat(4097) })).rejects.toMatchObject({ code: "invalid" });
    await expect(c.create({ anchor: { path: 1 }, text: "ok" })).rejects.toMatchObject({ code: "invalid" });
    await expect(c.create({ anchor, text: "Line one\nline two" })).resolves.toEqual({ threadId: "t", commentId: "c" });
    const sent = (f.rpc.call.mock.calls.at(-1)![2] as [{ anchor: { selector: string; quote: string } }])[0];
    expect(sent.anchor.selector).toBe("body > main > section > h2");
    await expect(c.reply("t", "")).rejects.toMatchObject({ code: "invalid" });
  });

  it("customAnchors needs the declaration, all three callbacks, and one registration at a time", async () => {
    const f = fakeRpc(() => null);
    const cb = { mode: vi.fn(), threads: vi.fn(), reveal: vi.fn() };
    const off = commentsLocals(f.rpc as never, {}) as { customAnchors(x: unknown): Promise<unknown> };
    await expect(off.customAnchors(cb)).rejects.toMatchObject({ code: "not_granted" });
    const on = commentsLocals(f.rpc as never, { customAnchors: true }) as { customAnchors(x: unknown): Promise<Record<string, unknown>> };
    await expect(on.customAnchors({ mode: vi.fn() })).rejects.toMatchObject({ code: "invalid" });
    const ctl = await on.customAnchors(cb);
    expect(commentsContext.live).toBe(true);
    await expect(on.customAnchors(cb)).rejects.toMatchObject({ code: "invalid" });
    f.emit("mode", { on: true });
    f.emit("threads", { list: [{ id: "h1", anchor: "shape-1", resolved: false, active: false }] });
    expect(cb.mode).toHaveBeenCalledWith(true);
    expect(cb.threads).toHaveBeenCalledWith([{ id: "h1", anchor: "shape-1", resolved: false, active: false }]);
    expect(ctl.areas).toBe(true);
    f.emit("reveal", { id: "h1" });
    expect(cb.reveal).not.toHaveBeenCalled();
    (ctl.placed as (m: unknown) => void)({ h1: { x: 10, y: 20 } });
    f.emit("reveal", { id: "h1" });
    expect(cb.reveal).toHaveBeenCalledWith("h1");
    const [path, at] = (ctl.domAnchor as (el: Element) => [string, { x: number; y: number }])(document.querySelector("h2")!);
    expect(path).toBe("body > main > section > h2");
    expect(typeof at.x).toBe("number");
    expect(() => (ctl.domAnchor as (el: Element) => unknown)(document.createElement("p"))).toThrow(TypeError);
    await expect((ctl.compose as (a: string, at: unknown) => Promise<unknown>)("", { x: 1, y: 1 })).rejects.toMatchObject({ code: "invalid" });
    await (ctl.compose as (a: string, at: unknown, o: unknown) => Promise<unknown>)("shape-1", { x: 1, y: 1 }, { label: "Red square" });
    expect(f.rpc.call.mock.calls.find(c => c[1] === "compose")![2]).toEqual([{ anchor: "shape-1", dom: false, label: "Red square", detail: undefined, version: 3 }]);
    (ctl.release as () => void)();
    (ctl.release as () => void)();
    expect(commentsContext.live).toBe(false);
    expect(ctl.areas).toBe(false);
    await expect((ctl.compose as (a: string, at: unknown) => Promise<unknown>)("shape-1", { x: 1, y: 1 })).rejects.toMatchObject({ code: "invalid" });
    expect(f.rpc.call.mock.calls.filter(c => c[1] === "release")).toHaveLength(1);
  });

  it("compose sends the area flag only while areas are possible, asks the shell before rendering, and sends the clip after under the nonce", async () => {
    const order: string[] = [];
    const f = fakeRpc(method => { order.push(method); return method === "compose" ? { opened: true, clipNonce: "n1" } : null; });
    const on = commentsLocals(f.rpc as never, { customAnchors: true }) as { customAnchors(x: unknown): Promise<Record<string, (...a: unknown[]) => unknown>> };
    const ctl = await on.customAnchors({ mode() {}, threads() {}, reveal() {} });
    const sent = () => (f.rpc.call.mock.calls.filter(c => c[1] === "compose").at(-1)![2] as Record<string, unknown>[])[0];
    await ctl.compose("shape-1", { x: 1, y: 1 }, { area: true });
    expect(sent()).not.toHaveProperty("area");
    // A post or send in flight: the shell says areas are off.
    f.emit("mode", { on: true, canArea: false });
    expect(ctl.areas).toBe(false);
    await ctl.compose("shape-1", { x: 1, y: 1 }, { area: true });
    expect(sent()).not.toHaveProperty("area");
    f.emit("mode", { on: true, canArea: true });
    expect(ctl.areas).toBe(true);
    await ctl.compose("shape-1", { x: 1, y: 1 }, { area: true });
    expect(sent()).toMatchObject({ anchor: "shape-1", dom: false, area: true, clipPending: false });
    const [path, at] = ctl.domAnchor(document.querySelector("h2")!) as [string, unknown];
    order.length = 0;
    // The page sees only `opened`, never the nonce.
    await expect(ctl.compose(path, at, { area: true })).resolves.toEqual({ opened: true });
    expect(sent()).toMatchObject({ anchor: path, dom: true, area: true, clipPending: true });
    expect(sent()).not.toHaveProperty("clipPng");
    // The clip is rendered after the shell answered, and sent under its nonce (jsdom cannot render: its error goes).
    await vi.waitFor(() => expect(order).toEqual(["compose", "composeClip"]));
    const clip = (f.rpc.call.mock.calls.find(c => c[1] === "composeClip")![2] as Record<string, unknown>[])[0];
    expect(clip.nonce).toBe("n1");
    expect(typeof clip.clipError).toBe("string");
    f.emit("mode", { on: false });
    expect(ctl.areas).toBe(false);
    ctl.release();
  });

  it("calls the page's mode callback only when comment mode starts or ends, not when areas turn on or off", async () => {
    const f = fakeRpc(() => ({ opened: true }));
    const on = commentsLocals(f.rpc as never, { customAnchors: true }) as { customAnchors(x: unknown): Promise<Record<string, unknown>> };
    const mode = vi.fn();
    const ctl = await on.customAnchors({ mode, threads() {}, reveal() {} });
    f.emit("mode", { on: true, canArea: true });
    f.emit("mode", { on: true, canArea: false });
    expect(ctl.areas).toBe(false);
    f.emit("mode", { on: true, canArea: true });
    expect(ctl.areas).toBe(true);
    expect(mode.mock.calls).toEqual([[true]]);
    f.emit("mode", { on: false });
    expect(mode.mock.calls).toEqual([[true], [false]]);
    (ctl.release as () => void)();
  });

  it("composer_only keeps only DOM anchor paths", async () => {
    const f = fakeRpc(() => ({ opened: true }));
    const on = commentsLocals(f.rpc as never, { composer_only: true, customAnchors: true }) as { customAnchors(x: unknown): Promise<Record<string, (...a: unknown[]) => unknown>> };
    const ctl = await on.customAnchors({ mode() {}, threads() {}, reveal() {} });
    await expect(ctl.compose("shape-1", { x: 0, y: 0 }) as Promise<unknown>).rejects.toMatchObject({ code: "invalid" });
    const [path, at] = ctl.domAnchor(document.querySelector("h2")!) as [string, unknown];
    await expect(ctl.compose(path, at) as Promise<unknown>).resolves.toEqual({ opened: true });
    ctl.release();
  });
});
