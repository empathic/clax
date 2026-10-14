import { afterEach, describe, expect, it, vi } from "vitest";
import { LiveHook } from "../src/live-hook";
import { LIVE_TAG, LiveLayer, acceptPeers, atPath, elementSelection, locate, pathOf, rangeOf, rangeSelection, textOffset } from "../src/live";
import { type BridgeToShell, LIVE_CURSOR_MS, PEERS_MAX } from "../src/protocol";

function manual() {
  let now = 0;
  const due: { at: number; fn: () => void; id: number }[] = [];
  let seq = 0;
  return {
    setTimeout: (fn: () => void, ms: number) => { const id = ++seq; due.push({ at: now + ms, fn, id }); return id; },
    clearTimeout: (h: unknown) => { const i = due.findIndex(d => d.id === h); if (i >= 0) due.splice(i, 1); },
    advance(ms: number) {
      now += ms;
      for (;;) {
        due.sort((a, b) => a.at - b.at);
        const d = due[0];
        if (!d || d.at > now) return;
        due.shift();
        d.fn();
      }
    },
  };
}

afterEach(() => { document.body.innerHTML = ""; document.querySelector(LIVE_TAG)?.remove(); getSelection()?.removeAllRanges(); });

describe("element paths and text offsets", () => {
  it("name an element by child indexes from the body, and find it again", () => {
    document.body.innerHTML = "<main><h1>Title</h1><p>One <b>two</b> three</p></main>";
    const b = document.querySelector("b")!;
    expect(pathOf(document, b)).toEqual([0, 1, 0]);
    expect(atPath(document, [0, 1, 0])).toBe(b);
    expect(atPath(document, [0, 9])).toBeNull();
    expect(pathOf(document, document.body)).toEqual([]);
    expect(pathOf(document, document.documentElement)).toBeNull();
  });

  it("place a text offset across the element's text nodes", () => {
    document.body.innerHTML = "<p>One <b>two</b> three</p>";
    const p = document.querySelector("p")!;
    const two = p.querySelector("b")!.firstChild!;
    expect(textOffset(p, two, 1)).toBe(5);
    const [node, off] = locate(p, 5)!;
    expect(node).toBe(two);
    expect(off).toBe(1);
  });

  it("carry a text selection to another document with the same structure", () => {
    document.body.innerHTML = "<p>Alpha beta gamma</p>";
    const t = document.querySelector("p")!.firstChild!;
    const r = document.createRange();
    r.setStart(t, 6);
    r.setEnd(t, 10);
    const sel = rangeSelection(document, r)!;
    expect(sel).toEqual({ kind: "range", start: { path: [0], offset: 6, x: 0, y: 0 }, end: { path: [0], offset: 10, x: 0, y: 0 } });
    // The same page laid out anew (another width, another tab).
    document.body.innerHTML = "<p>Alpha beta gamma</p>";
    expect(rangeOf(document, sel as Extract<typeof sel, { kind: "range" }>)?.toString()).toBe("beta");
    expect(elementSelection(document, document.querySelector("p")!)).toEqual({ kind: "element", path: [0] });
  });
});

describe("peers from the shell", () => {
  it("keep only well-formed ones: an opaque key, a hex colour, and anchors of the right shapes", () => {
    const ok = { key: "k1", color: "#2f62d0", anchor: { path: [0], offset: -1, x: 0.5, y: 0.5 }, selection: { kind: "area", path: [0], x: 0, y: 0, w: 0.5, h: 0.5 } };
    expect(acceptPeers([ok])).toEqual([ok]);
    expect(acceptPeers([{ ...ok, key: "u_abc" }, { ...ok, color: "red" }, { ...ok, key: "k2", color: "#2f62d0\"><script>" }])).toEqual([]);
    expect(acceptPeers([{ ...ok, anchor: { path: [0], offset: -1, x: 2, y: 0 }, selection: { kind: "blob" } }])).toEqual([{ key: "k1", color: "#2f62d0", anchor: null, selection: null }]);
    expect(acceptPeers(Array.from({ length: PEERS_MAX + 5 }, (_, i) => ({ ...ok, key: `k${i}` }))).length).toBe(PEERS_MAX);
    expect(acceptPeers("nope")).toEqual([]);
  });
});

describe("LiveLayer", () => {
  it("reports the text selection at once, then at most every 100 ms with the latest", () => {
    document.body.innerHTML = "<p>Alpha beta gamma</p>";
    const t = manual();
    const sent: BridgeToShell[] = [];
    const layer = new LiveLayer(document, m => sent.push(m), t);
    const text = document.querySelector("p")!.firstChild!;
    const select = (a: number, b: number) => {
      const r = document.createRange();
      r.setStart(text, a);
      r.setEnd(text, b);
      getSelection()!.removeAllRanges();
      getSelection()!.addRange(r);
      document.dispatchEvent(new Event("selectionchange"));
    };
    select(0, 5);
    expect(sent).toEqual([{ type: "clax:cursor", anchor: null, selection: { kind: "range", start: { path: [0], offset: 0, x: 0, y: 0 }, end: { path: [0], offset: 5, x: 0, y: 0 } }, pick: null }]);
    select(0, 6);
    select(0, 10);
    expect(sent.length).toBe(1);
    t.advance(LIVE_CURSOR_MS);
    expect(sent.length).toBe(2);
    expect((sent[1] as { selection: { end: { offset: number } } }).selection.end.offset).toBe(10);
    // Nothing new: nothing sent.
    t.advance(LIVE_CURSOR_MS * 3);
    expect(sent.length).toBe(2);
    layer.destroy();
  });

  it("reports the area being drawn in place of the selection", () => {
    document.body.innerHTML = "<div style='width:100px;height:100px'></div>";
    const t = manual();
    const sent: BridgeToShell[] = [];
    const layer = new LiveLayer(document, m => sent.push(m), t);
    layer.setDrag({ left: 10, top: 10, width: 20, height: 20 });
    const m = sent.at(-1) as Extract<BridgeToShell, { type: "clax:cursor" }>;
    expect(m.selection?.kind).toBe("area");
    t.advance(LIVE_CURSOR_MS);
    layer.setDrag(null);
    t.advance(LIVE_CURSOR_MS);
    expect((sent.at(-1) as typeof m).selection).toBeNull();
    layer.destroy();
  });

  it("draws peers' selections and pointers outside the body, and says where the pointers are", async () => {
    document.body.innerHTML = "<main><p>Alpha beta gamma</p><section>Box</section></main>";
    const sent: BridgeToShell[] = [];
    const layer = new LiveLayer(document, m => sent.push(m), manual());
    const bodyBefore = document.body.innerHTML;
    layer.peers([
      { key: "k1", color: "#2f62d0", anchor: { path: [0, 1], offset: -1, x: 0.5, y: 0.5 }, selection: { kind: "element", path: [0, 1] } },
      { key: "k2", color: "#8a3fd1", anchor: null, selection: { kind: "area", path: [0, 1], x: 0.1, y: 0.1, w: 0.5, h: 0.5 } },
    ]);
    layer.draw();
    const host = document.querySelector(LIVE_TAG)!;
    expect(host.parentElement).toBe(document.documentElement);
    expect(document.body.innerHTML).toBe(bodyBefore);
    const root = host.shadowRoot!;
    expect(root.querySelectorAll(".e").length).toBe(1);
    expect(root.querySelectorAll(".a").length).toBe(1);
    expect(root.querySelectorAll(".c").length).toBe(1);
    expect(root.textContent).not.toMatch(/k1|k2/);
    expect(sent.at(-1)).toEqual({ type: "clax:peer-at", peers: [{ key: "k1", x: 0, y: 0 }] });
    // A peer who left takes its arrow.
    layer.peers([]);
    layer.draw();
    expect(root.querySelectorAll(".c, .e, .a").length).toBe(0);
    layer.destroy();
    expect(document.querySelector(LIVE_TAG)).toBeNull();
    // A draw that was due when it was destroyed does not bring it back.
    const again = new LiveLayer(document, () => {}, manual());
    again.peers([{ key: "k1", color: "#2f62d0", anchor: { path: [0, 1], offset: -1, x: 0.5, y: 0.5 }, selection: null }]);
    again.destroy();
    await new Promise(r => requestAnimationFrame(() => r(null)));
    expect(document.querySelector(LIVE_TAG)).toBeNull();
  });

  it("draws again when an element's content scrolls, and only re-says the pointers' places when the document does", () => {
    document.body.innerHTML = "<div id='pane' style='overflow:auto;height:50px'><p>Alpha</p></div>";
    const sent: BridgeToShell[] = [];
    const layer = new LiveLayer(document, m => sent.push(m), manual());
    const redraw = vi.spyOn(layer, "redraw");
    // Scroll events do not bubble: the layer hears an element's in the capture phase.
    document.getElementById("pane")!.dispatchEvent(new Event("scroll"));
    expect(redraw).toHaveBeenCalledTimes(1);
    document.dispatchEvent(new Event("scroll"));
    expect(redraw).toHaveBeenCalledTimes(1);
    layer.destroy();
  });
});

describe("LiveHook", () => {
  const setup = () => {
    const sent: BridgeToShell[] = [];
    const load = vi.fn(() => import("../src/parts/live"));
    const hook = new LiveHook(document, m => sent.push(m), manual(), load);
    const settle = () => load.mock.results.at(-1)?.value.then(() => new Promise(r => setTimeout(r, 0)));
    return { sent, load, hook, settle };
  };
  const peers = (list: unknown[], here?: boolean) => ({ type: "clax:peers" as const, peers: list as never, ...(here === undefined ? {} : { here }) });

  it("loads nothing and reports nothing while no one else is on the artifact", async () => {
    document.body.innerHTML = "<p>Alpha</p>";
    const { sent, load, hook } = setup();
    hook.peers(peers([], false));
    hook.peers(peers([]));
    hook.picked(document.querySelector("p")!);
    hook.quotes([], { resolve: () => null });
    expect(load).not.toHaveBeenCalled();
    getSelection()!.selectAllChildren(document.querySelector("p")!);
    document.dispatchEvent(new Event("selectionchange"));
    expect(sent).toEqual([]);
  });

  it("loads the live part once someone else is here, then reports the pointer, the selection and the pick made before", async () => {
    document.body.innerHTML = "<main><p>Alpha</p><section>Box</section></main>";
    const { sent, load, hook, settle } = setup();
    hook.picked(document.querySelector("section")!);
    hook.peers(peers([], true));
    hook.peers(peers([], true));
    await settle();
    expect(load).toHaveBeenCalledTimes(1);
    expect(sent.at(-1)).toEqual({ type: "clax:cursor", anchor: null, selection: null, pick: { kind: "element", path: [0, 1] } });
    // Everyone left: the layer stays, drawing no one.
    hook.peers(peers([], false));
    await settle();
    expect(load).toHaveBeenCalledTimes(1);
    expect(document.querySelector(LIVE_TAG)).toBeNull();
    hook.destroy();
  });

  it("loads the live part for the first open quote, without reporting the pointer", async () => {
    document.body.innerHTML = "<p>Alpha beta</p>";
    const { sent, load, hook, settle } = setup();
    const r = document.createRange();
    r.selectNodeContents(document.querySelector("p")!);
    const anchors = [{ id: "a", anchor: {} as never, highlight: true }, { id: "b", anchor: {} as never }];
    const resolve = vi.fn(() => ({ method: "quote" as const, element: document.querySelector("p")!, range: r }));
    hook.quotes(anchors, { resolve });
    await settle();
    expect(load).toHaveBeenCalledTimes(1);
    expect(resolve).toHaveBeenCalledTimes(1);
    expect(sent).toEqual([]);
  });
});

