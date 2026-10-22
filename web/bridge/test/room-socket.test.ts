import { afterEach, describe, expect, it, vi } from "vitest";
import { ACK_TIMEOUT_MS, RECONNECT_MS, originRoom } from "../src/caps/room-socket";

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

function room() {
  const sockets: FakeSocket[] = [];
  const t = originRoom({ socket: path => { const s = new FakeSocket(path); sockets.push(s); return s as unknown as WebSocket; } });
  const pushed: [string, unknown][] = [];
  for (const topic of ["welcome", "peers", "peer", "left", "msg", "connection", "error"]) t.on("room", topic, d => pushed.push([topic, d]));
  return { t, sockets, pushed };
}

describe("the room's socket on the page's own origin", () => {
  afterEach(() => { vi.useRealTimers(); });

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
});
