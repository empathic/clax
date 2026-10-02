import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_JOINED, PRESENCE_INTERVAL_MS, makeRoom, roomNamespace, type PeersChange, type WirePeer } from "../src/caps/room";
import { CapabilityError } from "../src/rpc";

type Listener = (d: unknown) => void;

function fakeRpc(answer: (method: string, args: unknown[]) => unknown = () => null) {
  const listeners = new Map<string, Set<Listener>>();
  const calls: { method: string; args: unknown[] }[] = [];
  return {
    calls,
    push(topic: string, data: unknown) { for (const f of [...(listeners.get(topic) ?? [])]) f(data); },
    rpc: {
      call: vi.fn(async (_ns: string, method: string, args: unknown[]) => { calls.push({ method, args }); return answer(method, args); }),
      on: (_ns: string, topic: string, f: Listener) => {
        if (!listeners.has(topic)) listeners.set(topic, new Set());
        listeners.get(topic)!.add(f);
        return () => { listeners.get(topic)!.delete(f); };
      },
    },
  };
}

const peer = (label: string, over: Partial<WirePeer> = {}): WirePeer =>
  ({ peer: label, by: null, isMe: false, sameTab: false, kind: "viewer", guest: false, presence: {}, ...over });
const ME = "mmmmmmmmmmmmmmmm";
const OTHER = "oooooooooooooooo";

type Room = {
  emit(topic: unknown, data?: unknown): Promise<void>;
  on(topic: unknown, fn: unknown, onError?: (e: { code: string }) => void): () => void;
  presence(patch: unknown): Promise<void>;
  peers(): readonly { peer: string; presence: Record<string, unknown>; updatedAt: number }[];
  onPeers(fn: (c: PeersChange) => void, onError?: (e: { code: string }) => void): () => void;
  join(name: unknown): Promise<Record<string, (...a: unknown[]) => unknown> & { name: string }>;
  connected(): boolean;
  onConnection(fn: (c: boolean) => void, onError?: (e: { code: string }) => void): () => void;
  sendToClaudeSession(data: unknown): Promise<unknown>;
  canSendToClaudeSession(): Promise<string>;
};

function setup(answer?: (method: string, args: unknown[]) => unknown) {
  const f = fakeRpc(answer);
  const room = makeRoom(f.rpc as never) as unknown as Room;
  const up = () => {
    f.push("connection", { connected: true });
    f.push("welcome", { peer: ME });
    f.push("peers", { room: null, peers: [peer(ME, { isMe: true, sameTab: true })] });
  };
  return { ...f, room, up };
}

describe("room", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("asks the shell to connect once, as soon as the namespace exists", () => {
    const { calls } = setup();
    expect(calls).toEqual([{ method: "connect", args: [] }]);
  });

  it("peers() is a frozen empty array until the first answer, then the same frozen snapshot until something changes", () => {
    const { room, up, push } = setup();
    const empty = room.peers();
    expect(empty).toEqual([]);
    expect(Object.isFrozen(empty)).toBe(true);
    expect(room.peers()).toBe(empty);
    up();
    const one = room.peers();
    expect(one.map(p => p.peer)).toEqual([ME]);
    expect(Object.isFrozen(one) && Object.isFrozen(one[0]) && Object.isFrozen(one[0].presence)).toBe(true);
    expect(room.peers()).toBe(one);
    push("peer", { room: null, peer: peer(OTHER) });
    const two = room.peers();
    expect(two).not.toBe(one);
    expect(two.find(p => p.peer === ME)).toBe(one[0]);
  });

  it("onPeers first presents the room as joined, later only net changes, at most once per frame", async () => {
    const { room, up, push } = setup();
    const seen: PeersChange[] = [];
    room.onPeers(c => seen.push(c));
    await vi.advanceTimersByTimeAsync(100);
    expect(seen).toEqual([]);
    up();
    push("peer", { room: null, peer: peer(OTHER) });
    expect(seen).toEqual([]);
    await vi.advanceTimersByTimeAsync(60);
    expect(seen).toHaveLength(1);
    expect(seen[0].joined.map(p => p.peer).sort()).toEqual([ME, OTHER].sort());
    expect(seen[0].peers).toBe(room.peers());
    push("peer", { room: null, peer: peer(OTHER, { presence: { n: 1 } }) });
    push("peer", { room: null, peer: peer(OTHER, { presence: { n: 2 } }) });
    push("peer", { room: null, peer: peer("jjjjjjjjjjjjjjjj") });
    push("left", { room: null, peer: "jjjjjjjjjjjjjjjj" });
    await vi.advanceTimersByTimeAsync(60);
    expect(seen).toHaveLength(2);
    expect(seen[1].updated.map(p => p.presence)).toEqual([{ n: 2 }]);
    expect(seen[1].joined).toEqual([]);
    expect(seen[1].left).toEqual([]);
    push("left", { room: null, peer: OTHER });
    await vi.advanceTimersByTimeAsync(60);
    expect(seen[2].left.map(p => p.peer)).toEqual([OTHER]);
  });

  it("a peers frame replaces the room: peers missing from it are reported left", async () => {
    const { room, up, push } = setup();
    up();
    push("peer", { room: null, peer: peer(OTHER) });
    const seen: PeersChange[] = [];
    room.onPeers(c => seen.push(c));
    await vi.advanceTimersByTimeAsync(60);
    push("peers", { room: null, peers: [peer(ME, { isMe: true, sameTab: true })] });
    await vi.advanceTimersByTimeAsync(60);
    expect(seen.at(-1)!.left.map(p => p.peer)).toEqual([OTHER]);
  });

  it("presence merges locally at once, removes null fields, and is sent whole, coalesced", async () => {
    const { room, up, calls } = setup();
    up();
    await room.presence({ x: 1, y: 2 });
    await room.presence({ y: null, z: "a" });
    expect(room.peers().find(p => p.peer === ME)!.presence).toEqual({ x: 1, z: "a" });
    expect(calls.filter(c => c.method === "presence")).toEqual([]);
    await vi.advanceTimersByTimeAsync(PRESENCE_INTERVAL_MS);
    expect(calls.filter(c => c.method === "presence")).toEqual([{ method: "presence", args: [null, { x: 1, z: "a" }] }]);
  });

  it("presence refuses bad keys and oversized objects without applying them", async () => {
    const { room, up } = setup();
    up();
    await room.presence({ keep: 1 });
    for (const bad of [{ "1a": 1 }, { "a b": 1 }, { constructor: 1 }, { prototype: 1 }, { ["__proto__"]: 1 }, { big: "x".repeat(5000) }, [1], "x"]) {
      await expect(room.presence(bad)).rejects.toMatchObject({ code: "invalid_argument" });
    }
    expect(room.peers().find(p => p.peer === ME)!.presence).toEqual({ keep: 1 });
  });

  it("emit checks its arguments, drops silently while disconnected, and passes the shell's refusal through", async () => {
    const { room, up, calls } = setup((method, args) => {
      if (method === "emit" && args[1] === "clear") throw new CapabilityError("not_permitted", "admin only");
      return { dropped: false };
    });
    await expect(room.emit("Bad:topic")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(room.emit("ok", { big: "x".repeat(5000) })).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(room.emit("ok", { f: () => 1 })).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(room.emit("reaction")).resolves.toBeUndefined();
    expect(calls.some(c => c.method === "emit")).toBe(false);
    up();
    await room.emit("reaction", { kind: "wave" });
    expect(calls.at(-1)).toEqual({ method: "emit", args: [null, "reaction", { kind: "wave" }] });
    await room.emit("bare");
    expect(calls.at(-1)).toEqual({ method: "emit", args: [null, "bare"] });
    await expect(room.emit("clear")).rejects.toMatchObject({ code: "not_permitted" });
  });

  it("reports emits the daemon dropped once per page load", async () => {
    const report = vi.fn();
    vi.stubGlobal("reportError", report);
    const { room, up } = setup(() => ({ dropped: true }));
    up();
    await room.emit("a");
    await room.emit("a");
    expect(report).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("on: TypeError for a non-function, a malformed topic errors once on a microtask, messages reach their topic", async () => {
    const { room, up, push } = setup();
    expect(() => room.on("a", 42)).toThrow(TypeError);
    const onError = vi.fn();
    room.on("Bad:topic", () => {}, onError);
    expect(onError).not.toHaveBeenCalled();
    await Promise.resolve();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0]).toMatchObject({ code: "invalid_argument" });
    up();
    const got: unknown[] = [];
    const off = room.on("reaction", (m: unknown) => got.push(m));
    const msg = { ...peer(OTHER), topic: "reaction", data: { k: 1 } };
    push("msg", { room: null, msg: { ...msg, presence: undefined } });
    push("msg", { room: null, msg: { ...msg, topic: "other" } });
    off();
    off();
    push("msg", { room: null, msg });
    expect(got).toHaveLength(1);
    expect(got[0]).toMatchObject({ peer: OTHER, topic: "reaction", data: { k: 1 } });
  });

  it("join returns one object per name, leaves cleanly, and stops at 16", async () => {
    const { room, up, push, calls } = setup();
    up();
    await expect(room.join("Bad")).rejects.toMatchObject({ code: "invalid_argument" });
    const t = await room.join("table-1");
    expect(await room.join("table-1")).toBe(t);
    expect(t.name).toBe("table-1");
    push("peers", { room: "table-1", peers: [peer(ME, { isMe: true, sameTab: true })] });
    expect((t.peers as () => unknown[])()).toHaveLength(1);
    const onError = vi.fn();
    (t.onPeers as (f: () => void, e: () => void) => void)(() => {}, onError);
    await (t.leave as () => Promise<void>)();
    await (t.leave as () => Promise<void>)();
    expect(onError).not.toHaveBeenCalled();
    expect(calls.filter(c => c.method === "leave")).toEqual([{ method: "leave", args: ["table-1"] }]);
    await expect((t.emit as (x: string) => Promise<void>)("a")).rejects.toMatchObject({ code: "invalid_argument" });
    for (let i = 0; i < MAX_JOINED; i++) await room.join(`r${i}`);
    await expect(room.join("one-more")).rejects.toMatchObject({ code: "limit_reached" });
  });

  it("a terminal error reaches every listener once and every later call", async () => {
    const { room, up, push } = setup();
    up();
    const errors: string[] = [];
    room.onPeers(() => {}, e => errors.push("peers:" + e.code));
    room.on("reaction", () => {}, e => errors.push("on:" + e.code));
    room.onConnection(() => {}, e => errors.push("conn:" + e.code));
    push("peer", { room: null, peer: peer(OTHER) });
    push("error", { room: null, code: "revoked", message: "access changed" });
    push("error", { room: null, code: "revoked", message: "again" });
    expect(errors.sort()).toEqual(["conn:revoked", "on:revoked", "peers:revoked"]);
    await expect(room.emit("a")).rejects.toMatchObject({ code: "revoked" });
    await expect(room.presence({ a: 1 })).rejects.toMatchObject({ code: "revoked" });
    await expect(room.join("x")).rejects.toMatchObject({ code: "revoked" });
    expect(room.connected()).toBe(false);
    expect(room.peers().map(p => p.peer)).toEqual([ME]);
  });

  it("a named room's own error ends that room only", async () => {
    const { room, up, push } = setup();
    up();
    const t = await room.join("table-1");
    const onError = vi.fn();
    (t.on as (x: string, f: () => void, e: () => void) => void)("a", () => {}, onError);
    push("error", { room: "table-1", code: "upstream_error", message: "rejoin failed" });
    expect(onError).toHaveBeenCalledTimes(1);
    await expect((t.emit as (x: string) => Promise<void>)("a")).rejects.toMatchObject({ code: "upstream_error" });
    await expect(room.emit("a")).resolves.toBeUndefined();
    expect(await room.join("table-1")).not.toBe(t);
  });

  it("onConnection fires once with the current state after a microtask, then on each change", async () => {
    const { room, push } = setup();
    const seen: boolean[] = [];
    room.onConnection(c => seen.push(c));
    expect(seen).toEqual([]);
    await Promise.resolve();
    expect(seen).toEqual([false]);
    push("connection", { connected: true });
    push("connection", { connected: true });
    push("connection", { connected: false });
    expect(seen).toEqual([false, true, false]);
    expect(room.connected()).toBe(false);
  });

  it("the part's namespace is frozen and carries exactly the contract's members", () => {
    const f = fakeRpc();
    const ns = roomNamespace(f.rpc as never);
    expect(Object.isFrozen(ns)).toBe(true);
    expect(Object.keys(ns).sort()).toEqual(["canSendToClaudeSession", "connected", "emit", "join", "on", "onConnection", "onPeers", "peers", "presence", "sendToClaudeSession"]);
  });

  it("there is no Claude conversation beside the page", async () => {
    const { room } = setup();
    await expect(room.canSendToClaudeSession()).resolves.toBe("off");
    await expect(room.sendToClaudeSession({ label: "x" })).rejects.toMatchObject({ code: "claude_unavailable" });
  });
});
