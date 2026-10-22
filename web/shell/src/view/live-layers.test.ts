import { describe, expect, it } from "vitest";
import { BRIDGE_TYPES, SHELL_TYPES } from "../../../bridge/src/protocol";
import { acceptFromFrame } from "../bridge-link";
import {
  DRAFT_EVERY_MS, DRAFT_MAX, DraftSender, FrameKeys, LIVE_PALETTE, type LiveBatch, type LiveOut, LiveSocket, RECONNECT_MS, type SocketLike, TYPING_FRESH_MS,
  applyBatch, checkLiveFrame, emptyLayer, framePeers, liveUrl, peerTags, typingLine, writersIn,
} from "./live-layers";

const T = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";

/** Timers a test runs by hand. */
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

const batch = (b: Partial<LiveBatch>): LiveBatch => ({ t: "live", senders: [], drop: [], others: 0, present: 0, ...b });

describe("the protocol between the shell and the bridge", () => {
  it("names the live layers' messages each way", () => {
    expect(SHELL_TYPES.has("clax:peers")).toBe(true);
    expect(BRIDGE_TYPES.has("clax:cursor")).toBe(true);
    expect(BRIDGE_TYPES.has("clax:peer-at")).toBe(true);
    expect(SHELL_TYPES.has("clax:cursor")).toBe(false);
  });

  it("takes a pointer from the frame at the artifact's site origin only", () => {
    const frame = {} as Window;
    const site = "http://7q3k9mzx2b4t.localhost:7480";
    const data = { type: "clax:cursor", anchor: { path: [0], offset: -1, x: 0.5, y: 0.5 }, selection: null };
    const e = (origin: string, source: unknown) => ({ isTrusted: true, source, origin, data }) as unknown as MessageEvent;
    expect(acceptFromFrame(e(site, frame), frame, site)).toEqual(data);
    expect(acceptFromFrame(e("http://localhost:7480", frame), frame, site)).toBeNull();
    expect(acceptFromFrame(e("null", frame), frame, site)).toBeNull();
    expect(acceptFromFrame(e(site, {}), frame, site)).toBeNull();
  });
});

describe("batches", () => {
  it("fold over what is known: parts left out stay, null clears, drop removes", () => {
    let s = applyBatch(emptyLayer(), batch({ senders: [{ person_id: "u_a", color: 2, cursor: { version: 1, file: "index.html", anchor: null, selection: { kind: "element", path: [0] } }, typing: { thread_id: null }, draft: { thread_id: null, text: "hi" } }] }), 100);
    expect(s.peers.u_a.sender.draft?.text).toBe("hi");
    s = applyBatch(s, batch({ senders: [{ person_id: "u_a", color: 2, cursor: null }], others: 3 }), 200);
    expect(s.peers.u_a.sender.cursor).toBeNull();
    expect(s.peers.u_a.sender.draft?.text).toBe("hi");
    expect(s.peers.u_a.typedAt).toBe(100);
    expect(s.others).toBe(3);
    s = applyBatch(s, batch({ drop: ["u_a"] }), 300);
    expect(s.peers).toEqual({});
  });

  it("are checked before they are taken", () => {
    expect(checkLiveFrame(JSON.stringify({ t: "welcome", person_id: "u_a", color: 1 }))).toEqual({ t: "welcome", person_id: "u_a", color: 1 });
    expect(checkLiveFrame(JSON.stringify(batch({ senders: [{ person_id: "u_a", color: 0, draft: { thread_id: T, text: "x" } }] })))?.t).toBe("live");
    expect(checkLiveFrame("nope")).toBeNull();
    expect(checkLiveFrame(JSON.stringify(batch({ senders: [{ person_id: "u_a", color: 0, draft: { thread_id: "x", text: "x" } }] })))).toBeNull();
    expect(checkLiveFrame(JSON.stringify({ t: "live", senders: [], drop: [1], others: 0 }))).toBeNull();
  });
});

describe("writers", () => {
  const s = applyBatch(emptyLayer(), batch({ senders: [
    { person_id: "u_a", color: 1, typing: { thread_id: T }, draft: { thread_id: T, text: "Looks" } },
    { person_id: "u_b", color: 2, typing: { thread_id: null }, draft: null },
  ] }), 1000);

  it("are the people typing or drafting in that thread, typing while fresh", () => {
    expect(writersIn(s, T, 1000)).toEqual([{ person_id: "u_a", color: 1, text: "Looks", typing: true }]);
    expect(writersIn(s, null, 1000)).toEqual([{ person_id: "u_b", color: 2, text: "", typing: true }]);
    // Stale typing with no draft is no one; a draft stays without "is typing".
    expect(writersIn(s, null, 1000 + TYPING_FRESH_MS)).toEqual([]);
    expect(writersIn(s, T, 1000 + TYPING_FRESH_MS)).toEqual([{ person_id: "u_a", color: 1, text: "Looks", typing: false }]);
  });

  it("are said in one line", () => {
    expect(typingLine(["Mia"])).toBe("Mia is typing…");
    expect(typingLine(["Mia", "Ben"])).toBe("Mia and Ben are typing…");
    expect(typingLine(["Mia", "Ben", "Ann"])).toBe("Mia and 2 others are typing…");
  });
});

describe("the frame's peers", () => {
  const s = applyBatch(emptyLayer(), batch({ senders: [
    { person_id: "u_a", color: 9, cursor: { version: 2, file: "index.html", anchor: { path: [1], offset: 3, x: 0, y: 0 }, selection: null } },
    { person_id: "u_b", color: 0, cursor: { version: 1, file: "index.html", anchor: { path: [1], offset: 3, x: 0, y: 0 }, selection: null } },
  ] }), 0);

  it("are those on the file and version shown, by opaque key and colour, never a name or ID", () => {
    const keys = new FrameKeys();
    const peers = framePeers(s, 2, "index.html", keys);
    expect(peers).toEqual([{ key: "k1", color: LIVE_PALETTE[1], anchor: { path: [1], offset: 3, x: 0, y: 0 }, selection: null }]);
    expect(JSON.stringify(peers)).not.toContain("u_a");
    expect(framePeers(s, 2, null, keys)).toEqual([]);
    // The tags come back by key, named by the shell.
    expect(peerTags(s, [{ key: "k1", x: 10, y: 20 }, { key: "k9", x: 0, y: 0 }], keys, () => "Mia")).toEqual([{ key: "k1", name: "Mia", color: 9, x: 10, y: 20 }]);
    keys.reset();
    expect(keys.person("k1")).toBeUndefined();
  });
});

describe("DraftSender", () => {
  it("says typing at once, then the latest draft at most every 150 ms, and clears", () => {
    const t = manual();
    const sent: LiveOut[] = [];
    const d = new DraftSender(m => sent.push(m), t);
    d.input(T, "a");
    d.input(T, "ab");
    expect(sent).toEqual([{ t: "typing", thread_id: T }]);
    t.advance(DRAFT_EVERY_MS);
    expect(sent.at(-1)).toEqual({ t: "draft", thread_id: T, text: "ab" });
    d.input(null, "x".repeat(DRAFT_MAX + 10));
    t.advance(DRAFT_EVERY_MS);
    expect((sent.at(-1) as { text: string }).text.length).toBe(DRAFT_MAX);
    d.input(T, "  ");
    expect(sent.at(-1)).toEqual({ t: "draft", thread_id: T, text: "" });
    d.resend();
    expect(sent.at(-1)).toEqual({ t: "draft", thread_id: null, text: "x".repeat(DRAFT_MAX) });
    d.stop();
  });
});

describe("LiveSocket", () => {
  function fakeSockets() {
    const made: (SocketLike & { sent: string[]; closed?: number })[] = [];
    const make = (_url: string) => {
      const s = { readyState: 0, onopen: null, onmessage: null, onclose: null, sent: [] as string[], send(d: string) { this.sent.push(d); }, close(code?: number) { this.closed = code; } } as SocketLike & { sent: string[]; closed?: number };
      made.push(s);
      return s;
    };
    return { made, make };
  }

  it("sends only while open, hands frames over, and reopens after a close", () => {
    const t = manual();
    const { made, make } = fakeSockets();
    const got: unknown[] = [];
    let opened = 0;
    const ls = new LiveSocket("ws://x/api/artifacts/a/live", { welcome: w => got.push(w), batch: b => got.push(b), opened: () => opened++, closed: () => got.push("closed") }, make, t);
    ls.start();
    ls.send({ t: "open", thread_id: null });
    expect(made[0].sent).toEqual([]);
    made[0].onopen!({});
    ls.send({ t: "open", thread_id: null });
    expect(made[0].sent).toEqual([JSON.stringify({ t: "open", thread_id: null })]);
    made[0].onmessage!({ data: JSON.stringify({ t: "welcome", person_id: "u_a", color: 3 }) });
    made[0].onmessage!({ data: "garbage" });
    expect(got).toEqual([{ t: "welcome", person_id: "u_a", color: 3 }]);
    made[0].onclose!({ code: 1006 });
    t.advance(RECONNECT_MS[0]);
    expect(made.length).toBe(2);
    made[1].onopen!({});
    expect(opened).toBe(2);
    ls.stop();
    expect(made[1].closed).toBe(1000);
  });

  it("does not reopen after a close the daemon means for good", () => {
    for (const code of [4410, 1008]) {
      const t = manual();
      const { made, make } = fakeSockets();
      const ls = new LiveSocket("ws://x", { welcome() {}, batch() {}, opened() {}, closed() {} }, make, t);
      ls.start();
      made[0].onclose!({ code });
      t.advance(60_000);
      expect(made.length, String(code)).toBe(1);
    }
  });

  it("reopens after a 4429, as after any close, until paused: the stream decides a spectator", () => {
    const t = manual();
    const { made, make } = fakeSockets();
    const finals: boolean[] = [];
    const ls = new LiveSocket("ws://x", { welcome() {}, batch() {}, opened() {}, closed: f => finals.push(f) }, make, t);
    ls.start();
    made[0].onclose!({ code: 4429, reason: "no_live_seat" });
    expect(finals).toEqual([false]);
    t.advance(RECONNECT_MS[0]);
    expect(made.length).toBe(2);
    made[1].onclose!({ code: 4429, reason: "no_live_seat" });
    ls.pause();
    t.advance(60_000);
    expect(made.length).toBe(2);
    ls.resume();
    expect(made.length).toBe(3);
  });

  it("stays closed while paused, a pending reopen included, until resumed", () => {
    const t = manual();
    const { made, make } = fakeSockets();
    let closes = 0;
    const ls = new LiveSocket("ws://x", { welcome() {}, batch() {}, opened() {}, closed: () => closes++ }, make, t);
    ls.start();
    made[0].onclose!({ code: 1006 });
    ls.pause();
    t.advance(60_000);
    expect(made.length).toBe(1);
    ls.resume();
    expect(made.length).toBe(2);
    ls.pause();
    expect(made[1].closed).toBe(1000);
    // The paused socket's own close is not heard.
    const before = closes;
    made[1].onclose!({ code: 4429, reason: "seat_idle" });
    expect(closes).toBe(before);
    t.advance(60_000);
    expect(made.length).toBe(2);
  });

  it("is opened on the shell's own origin", () => {
    expect(liveUrl("7q3k9mzx2b4t", { protocol: "http:", host: "localhost:7480" })).toBe("ws://localhost:7480/api/artifacts/7q3k9mzx2b4t/live");
    expect(liveUrl("7q3k9mzx2b4t", { protocol: "https:", host: "clax.example" })).toBe("wss://clax.example/api/artifacts/7q3k9mzx2b4t/live");
  });

  it("has no URL in the Clax app, nor on a scheme it would have to guess a WebSocket scheme for", () => {
    expect(liveUrl("7q3k9mzx2b4t", { protocol: "https:", host: "app.clax.invalid" }, true)).toBeNull();
    expect(liveUrl("7q3k9mzx2b4t", { protocol: "clax:", host: "app" }, true)).toBeNull();
    expect(liveUrl("7q3k9mzx2b4t", { protocol: "clax:", host: "app" }, false)).toBeNull();
  });

  it("opens no socket without a URL, on start or on resume", () => {
    const t = manual();
    const { made, make } = fakeSockets();
    const ls = new LiveSocket(null, { welcome() {}, batch() {}, opened() {}, closed() {} }, make, t);
    ls.start();
    ls.pause();
    ls.resume();
    t.advance(60_000);
    ls.send({ t: "open", thread_id: null });
    ls.stop();
    expect(made).toEqual([]);
  });
});
