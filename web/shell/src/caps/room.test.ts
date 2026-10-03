import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import type { CapEnv } from "./host";
import { ACK_TIMEOUT_MS, RECONNECT_MS, makeRoomHandler, type SocketLike } from "./room";

class FakeSocket implements SocketLike {
  static all: FakeSocket[] = [];
  readyState = 0;
  sent: unknown[] = [];
  onopen: ((e: unknown) => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(readonly url: string) { FakeSocket.all.push(this); }
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close(code = 1000, reason = "") { this.readyState = 3; this.onclose?.({ code, reason }); }
  open() { this.readyState = 1; this.onopen?.({}); }
  frame(v: unknown) { this.onmessage?.({ data: JSON.stringify(v) }); }
  drop(code = 1006, reason = "") { this.readyState = 3; this.onclose?.({ code, reason }); }
}

const WHO = { peer: "k3v6q2rt7wacd4fn", by: null, isMe: false, sameTab: false, kind: "viewer", guest: false };

type Viewers = (fn: (v: unknown) => void) => () => void;

function setup(token: string | null = null, viewers: Viewers = () => () => {}) {
  FakeSocket.all = [];
  const posted: ShellToBridge[] = [];
  const env = { aid: "7q3k9mzx2b4t", token, post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv;
  const handler = makeRoomHandler(url => new FakeSocket(url), undefined, viewers)(env, {} as never);
  const events = () => posted.filter(m => m.type === "clax:event").map(m => m as { topic: string; data: unknown });
  return { handler, posted, events, sock: () => FakeSocket.all.at(-1)! };
}

describe("room handler", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("opens one socket per document with a 16-character label and relays frames", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    await handler.call("connect", []);
    expect(FakeSocket.all).toHaveLength(1);
    expect(sock().url).toMatch(/^ws:\/\/[^/]+\/api\/artifacts\/7q3k9mzx2b4t\/room\?peer=[0-9a-v]{16}$/);
    sock().open();
    sock().frame({ t: "welcome", peer: "p" });
    sock().frame({ t: "peers", room: null, peers: [] });
    sock().frame({ t: "msg", room: null, msg: { ...WHO, topic: "a" } });
    expect(events().map(e => e.topic)).toEqual(["connection", "welcome", "peers", "msg"]);
    expect(events()[0].data).toEqual({ connected: true });
  });

  it("drops a daemon frame of another shape before it reaches the page", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    const bad: unknown[] = [
      { t: "welcome", peer: { x: 1 } },
      { t: "peers", room: null, peers: {} },
      { t: "peers", room: null, peers: [{ ...WHO }] },
      { t: "peers", room: "Bad Room", peers: [] },
      { t: "peers", room: null, peers: Array.from({ length: 257 }, () => ({ ...WHO, presence: {} })) },
      { t: "peer", room: null, peer: { ...WHO, isMe: "yes", presence: {} } },
      { t: "left", room: null, peer: 7 },
      { t: "msg", room: null, msg: { ...WHO, topic: "Not A Topic" } },
      { t: "error", room: null, code: "revoked", message: "forged" },
      { t: "connection", data: { connected: false } },
      [1, 2],
      "text",
    ];
    for (const f of bad) sock().frame(f);
    sock().onmessage?.({ data: new ArrayBuffer(4) });
    sock().onmessage?.({ data: "{not json" });
    expect(events().map(e => e.topic)).toEqual(["connection"]);
    sock().frame({ t: "peer", room: "table-1", peer: { ...WHO, presence: { pick: "A" } }, extra: "dropped" });
    expect(events().at(-1)).toEqual({ type: "clax:event", ns: "room", topic: "peer", data: { room: "table-1", peer: { ...WHO, presence: { pick: "A" } } } });
  });

  it("a nack with a malformed id or code settles nothing", async () => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    const p = handler.call("emit", [null, "x"]);
    const id = (sock().sent.at(-1) as { id: number }).id;
    sock().frame({ t: "nack", id: String(id), code: "not_permitted", message: "m" });
    sock().frame({ t: "nack", id, code: { evil: true }, message: "m" });
    sock().frame({ t: "ack", id, dropped: "yes" });
    sock().frame({ t: "ack", id });
    await expect(p).resolves.toEqual({ dropped: false });
  });

  it("the owner shell appends its token; a shell without one does not", async () => {
    const owner = setup("tok/en+1");
    await owner.handler.call("connect", []);
    expect(owner.sock().url).toMatch(/\?peer=[0-9a-v]{16}&token=tok%2Fen%2B1$/);
    const lan = setup();
    await lan.handler.call("connect", []);
    expect(lan.sock().url).not.toContain("token=");
  });

  it("emit waits for the ack, passes a nack's code through, and times out as upstream_error", async () => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    const ok = handler.call("emit", [null, "reaction", { k: 1 }]);
    const sent = sock().sent.at(-1) as { id: number };
    expect(sent).toEqual({ t: "emit", id: sent.id, room: null, topic: "reaction", data: { k: 1 } });
    sock().frame({ t: "ack", id: sent.id, dropped: true });
    await expect(ok).resolves.toEqual({ dropped: true });
    const bare = handler.call("emit", [null, "bare"]);
    expect(sock().sent.at(-1)).not.toHaveProperty("data");
    sock().frame({ t: "nack", id: (sock().sent.at(-1) as { id: number }).id, code: "not_permitted", message: "admin only" });
    await expect(bare).rejects.toMatchObject({ code: "not_permitted" });
    const slow = handler.call("emit", [null, "x"]);
    const settled = expect(slow).rejects.toMatchObject({ code: "upstream_error" });
    await vi.advanceTimersByTimeAsync(ACK_TIMEOUT_MS);
    await settled;
  });

  it("an emit while the socket is down resolves without sending", async () => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    await expect(handler.call("emit", [null, "x"])).resolves.toEqual({ dropped: false });
    expect(sock().sent).toEqual([]);
  });

  it("reconnects with backoff under the same label, re-sending presence and re-joining rooms", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    const first = sock();
    first.open();
    await handler.call("presence", [null, { pick: "B" }]);
    const joining = handler.call("join", ["table-1"]);
    first.frame({ t: "ack", id: (first.sent.at(-1) as { id: number }).id });
    await joining;
    first.drop();
    expect(events().at(-1)).toEqual({ type: "clax:event", ns: "room", topic: "connection", data: { connected: false } });
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0] - 1);
    expect(FakeSocket.all).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    const second = sock();
    expect(second).not.toBe(first);
    expect(second.url).toBe(first.url);
    second.open();
    expect(second.sent).toEqual([
      { t: "presence", room: null, state: { pick: "B" } },
      { t: "join", id: expect.any(Number), room: "table-1" },
    ]);
  });

  it("a failed re-join ends that room for the page", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    const joining = handler.call("join", ["table-1"]);
    sock().frame({ t: "ack", id: (sock().sent.at(-1) as { id: number }).id });
    await joining;
    sock().drop();
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0]);
    sock().open();
    sock().frame({ t: "nack", id: (sock().sent.at(-1) as { id: number }).id, code: "limit_reached", message: "full" });
    await vi.advanceTimersByTimeAsync(0);
    expect(events().at(-1)!.data).toEqual({ room: "table-1", code: "limit_reached", message: "full" });
  });

  it("4403 is terminal: an error for the page and no reconnect; 4409 stops quietly", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    sock().drop(4403, "revoked");
    expect(events().at(-1)!.data).toEqual({ room: null, code: "revoked", message: expect.any(String) });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);

    const again = setup();
    await again.handler.call("connect", []);
    again.sock().drop(4403, "not_granted");
    expect(again.events().at(-1)!.data).toMatchObject({ room: null, code: "not_granted" });

    const replaced = setup();
    await replaced.handler.call("connect", []);
    replaced.sock().open();
    replaced.sock().drop(4409, "replaced");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);
  });

  it.each(["reset", "leave"] as const)("%s closes the socket and the next document gets a new label", async which => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    const first = sock();
    first.open();
    handler[which]!();
    expect(first.readyState).toBe(3);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);
    await handler.call("connect", []);
    expect(sock()).not.toBe(first);
    expect(sock().url).not.toBe(first.url);
  });

  it("a rename reconnects under the same label without telling the page it went offline", async () => {
    let rename = () => {};
    const off = vi.fn();
    const { handler, events, sock } = setup(null, fn => { rename = () => fn({}); return off; });
    await handler.call("connect", []);
    const first = sock();
    first.open();
    await handler.call("presence", [null, { pick: "B" }]);
    rename();
    const second = sock();
    expect(second).not.toBe(first);
    expect(second.url).toBe(first.url);
    second.open();
    expect(first.readyState).toBe(3);
    expect(second.sent).toEqual([{ t: "presence", room: null, state: { pick: "B" } }]);
    expect(events().filter(e => e.topic === "connection").map(e => e.data)).toEqual([{ connected: true }, { connected: true }]);
    handler.dispose!();
    expect(off).toHaveBeenCalledTimes(1);
  });
});
