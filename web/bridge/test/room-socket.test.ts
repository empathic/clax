import { afterEach, describe, expect, it, vi } from "vitest";
import { ACK_TIMEOUT_MS, RECONNECT_MS, originRoom } from "../src/caps/room-socket";
import { CapabilityError } from "../src/rpc";

/** A WebSocket stand-in: what was sent, and the daemon's side. */
class FakeSocket {
  sent: Record<string, unknown>[] = [];
  closed: [number?, string?] | null = null;
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(readonly path: string) {}
  send(d: string) { this.sent.push(JSON.parse(d)); }
  close(code?: number, reason?: string) { this.closed = [code, reason]; }
  open() { this.onopen?.(); }
  frame(f: unknown) { this.onmessage?.({ data: JSON.stringify(f) }); }
  end(code: number, reason = "") { this.onclose?.({ code, reason }); }
}

/** A framing shell serving room: `viewer()` reports a viewer change. */
function framingShell(refuse?: string) {
  const fns: ((d: unknown) => void)[] = [];
  const asked: string[] = [];
  return {
    use: async (n: string) => (n === "room" ? { config: {} } : null),
    call: async (ns: string, m: string) => {
      asked.push(`${ns}.${m}`);
      if (refuse) throw new CapabilityError(refuse, "no rooms here");
      return null;
    },
    on: (_ns: string, topic: string, fn: (d: unknown) => void) => { if (topic === "viewer") fns.push(fn); return () => {}; },
    viewer: () => { for (const fn of fns) fn(null); },
    asked,
  };
}

function room(shell: ReturnType<typeof framingShell> | false = false, whoami: () => unknown = () => ({ kind: "visitor", id: "v_1", level: "public" })) {
  const sockets: FakeSocket[] = [];
  const t = originRoom({
    socket: path => { const s = new FakeSocket(path); sockets.push(s); return s as unknown as WebSocket; },
    call: async () => whoami(),
  }, shell as never);
  const pushed: [string, unknown][] = [];
  for (const topic of ["welcome", "peers", "peer", "left", "msg", "connection", "error"]) t.on("room", topic, d => pushed.push([topic, d]));
  return { t, sockets, pushed };
}

/** Sets the page's visibility and announces it. */
function show(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("the room's socket on the page's own origin", () => {
  afterEach(() => { vi.useRealTimers(); });

  it("opens no socket and refuses every call capability_disabled when the framing shell refuses watch so (the Clax app's)", async () => {
    const shell = framingShell("capability_disabled");
    const { t, sockets } = room(shell);
    await expect(t.call("room", "connect", [])).rejects.toMatchObject({ code: "capability_disabled" });
    await expect(t.call("room", "join", ["table"])).rejects.toMatchObject({ code: "capability_disabled" });
    await expect(t.call("room", "emit", [null, "hi"])).rejects.toMatchObject({ code: "capability_disabled" });
    expect(sockets).toEqual([]);
    expect(shell.asked).toEqual(["room.watch"]);
  });

  it("opens its socket when a framing shell's watch fails otherwise", async () => {
    const { t, sockets } = room(framingShell("upstream_error"));
    await t.call("room", "connect", []);
    expect(sockets).toHaveLength(1);
  });

  it("opens the site's room under one label, relays its frames, and answers emits from their acks", async () => {
    const { t, sockets, pushed } = room();
    await t.call("room", "connect", []);
    expect(sockets).toHaveLength(1);
    expect(sockets[0].path).toMatch(/^room\?peer=[0-9a-v]{16}$/);
    const s = sockets[0];
    s.open();
    s.frame({ t: "welcome", peer: "abc" });
    s.frame({ t: "peers", room: null, peers: [] });
    s.frame({ t: "bogus" });
    expect(pushed).toEqual([["connection", { connected: true }], ["welcome", { peer: "abc" }], ["peers", { room: null, peers: [] }]]);
    const emitted = t.call("room", "emit", [null, "move", { x: 1 }]);
    expect(s.sent.at(-1)).toEqual({ t: "emit", room: null, topic: "move", data: { x: 1 }, id: 1 });
    s.frame({ t: "ack", id: 1, dropped: true });
    expect(await emitted).toEqual({ dropped: true });
    const refused = t.call("room", "emit", [null, "move"]);
    s.frame({ t: "nack", id: 2, code: "spectator", message: "viewing" });
    await expect(refused).rejects.toMatchObject({ code: "upstream_error" });
    const joined = t.call("room", "join", ["r1"]);
    s.frame({ t: "nack", id: 3, code: "not_granted", message: "no" });
    await expect(joined).rejects.toMatchObject({ code: "not_granted" });
  });

  it("reconnects under the same label, sending its presence and joining its rooms again", async () => {
    vi.useFakeTimers();
    const { t, sockets } = room();
    await t.call("room", "connect", []);
    sockets[0].open();
    await t.call("room", "presence", [null, { x: 1 }]);
    const joined = t.call("room", "join", ["r1"]);
    sockets[0].frame({ t: "ack", id: 1 });
    await joined;
    sockets[0].end(1006);
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0]);
    expect(sockets).toHaveLength(2);
    expect(sockets[1].path).toBe(sockets[0].path);
    sockets[1].open();
    expect(sockets[1].sent.map(m => m.t)).toEqual(["presence", "join"]);
    // An emit nobody answers rejects after the ack timeout.
    const late = expect(t.call("room", "emit", [null, "move"])).rejects.toMatchObject({ code: "upstream_error" });
    await vi.advanceTimersByTimeAsync(ACK_TIMEOUT_MS);
    await late;
  });

  it("ends for good when the site refuses or replaces the socket", async () => {
    vi.useFakeTimers();
    const { t, sockets, pushed } = room();
    await t.call("room", "connect", []);
    sockets[0].open();
    sockets[0].end(4403, "revoked");
    expect(pushed.at(-1)).toEqual(["error", { room: null, code: "revoked", message: "this view's access to the room was withdrawn" }]);
    await vi.advanceTimersByTimeAsync(RECONNECT_MS.at(-1)! * 3);
    expect(sockets).toHaveLength(1);
    // A label another viewer holds is replaced, and the socket opens again.
    const other = room();
    await other.t.call("room", "connect", []);
    other.sockets[0].end(4403, "peer_taken");
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0]);
    expect(other.sockets).toHaveLength(2);
    expect(other.sockets[1].path).not.toBe(other.sockets[0].path);
    await expect(other.t.call("room", "frobnicate", [])).rejects.toMatchObject({ code: "capability_removed" });
  });

  it("answers an emit while down as not dropped, and ends a room the site refuses to join again", async () => {
    vi.useFakeTimers();
    const { t, sockets, pushed } = room();
    await t.call("room", "connect", []);
    expect(await t.call("room", "emit", [null, "move"])).toEqual({ dropped: false });
    sockets[0].open();
    const joined = t.call("room", "join", ["r1"]);
    sockets[0].frame({ t: "ack", id: 1 });
    await joined;
    sockets[0].end(1006);
    expect(await t.call("room", "emit", [null, "move"])).toEqual({ dropped: false });
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0]);
    sockets[1].open();
    const rejoin = sockets[1].sent.find(m => m.t === "join")!;
    sockets[1].frame({ t: "nack", id: rejoin.id, code: "not_granted", message: "no longer" });
    await vi.advanceTimersByTimeAsync(0);
    expect(pushed.at(-1)).toEqual(["error", { room: "r1", code: "not_granted", message: "no longer" }]);
    // Ended: the next reconnect does not join it again.
    sockets[1].end(1006);
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[1]);
    sockets[2].open();
    expect(sockets[2].sent.filter(m => m.t === "join")).toEqual([]);
  });

  it("reopens under the same label when a framing shell reports a viewer change, and the page sees no leave", async () => {
    const sh = framingShell();
    const { t, sockets, pushed } = room(sh);
    await t.call("room", "connect", []);
    await vi.waitFor(() => expect(sh.asked).toEqual(["room.watch"]));
    sockets[0].open();
    await t.call("room", "presence", [null, { x: 1 }]);
    const before = pushed.length;
    sh.viewer();
    expect(sockets).toHaveLength(2);
    expect(sockets[1].path).toBe(sockets[0].path);
    // The old socket's close is not news for the page; it closes once the new one opens.
    expect(sockets[0].closed).toBeNull();
    sockets[1].open();
    expect(sockets[0].closed).toEqual([1000, "renamed"]);
    expect(sockets[1].sent.map(m => m.t)).toEqual(["presence"]);
    expect(pushed.slice(before)).toEqual([["connection", { connected: true }]]);
  });

  it("top-level, reopens at the new level when the site's whoami changed while the page was away", async () => {
    let who = { kind: "visitor", id: "v_1", level: "public" };
    let asked = 0;
    const { t, sockets } = room(false, () => { asked++; return who; });
    try {
      await t.call("room", "connect", []);
      await vi.waitFor(() => expect(asked).toBe(1));
      sockets[0].open();
      // Back in view as the same viewer (once the first answer is in): the socket stays.
      await vi.waitFor(() => { show("hidden"); show("visible"); expect(asked).toBeGreaterThan(2); });
      expect(sockets).toHaveLength(1);
      who = { kind: "person", id: "u_2", level: "interact" };
      await vi.waitFor(() => { show("hidden"); show("visible"); expect(sockets).toHaveLength(2); });
      expect(sockets[1].path).toBe(sockets[0].path);
    } finally {
      delete (document as unknown as { visibilityState?: string }).visibilityState;
    }
  });
});
