import { afterEach, describe, expect, it, vi } from "vitest";
import type { BridgeToShell } from "../src/protocol";
import { CapabilityError, Rpc, USE_TIMEOUT_MS } from "../src/rpc";

describe("Rpc", () => {
  afterEach(() => { vi.useRealTimers(); });

  it("queues requests until the shell welcomes the frame, then sends them in order", () => {
    const sent: BridgeToShell[] = [];
    const rpc = new Rpc(m => sent.push(m));
    void rpc.use("db");
    void rpc.call("db", "get", ["tasks/t1"]);
    expect(sent).toEqual([]);
    rpc.connect();
    expect(sent.map(m => m.type)).toEqual(["clax:use", "clax:call"]);
    expect(sent[1]).toMatchObject({ ns: "db", method: "get", args: ["tasks/t1"] });
  });

  it("resolves a granted use with its config and a refused one with null", async () => {
    const sent: BridgeToShell[] = [];
    const rpc = new Rpc(m => sent.push(m));
    rpc.connect();
    const a = rpc.use("db");
    const b = rpc.use("room");
    const [ua, ub] = sent as Extract<BridgeToShell, { type: "clax:use" }>[];
    rpc.accept({ type: "clax:use-result", id: ua.id, granted: true, config: { rules: [] } });
    rpc.accept({ type: "clax:use-result", id: ub.id, granted: false, config: null });
    await expect(a).resolves.toEqual({ config: { rules: [] } });
    await expect(b).resolves.toBeNull();
  });

  it("use resolves null after 10 s without an answer", async () => {
    vi.useFakeTimers();
    const rpc = new Rpc(() => {});
    rpc.connect();
    const p = rpc.use("db");
    vi.advanceTimersByTime(USE_TIMEOUT_MS - 1);
    let settled = false;
    void p.then(() => { settled = true; });
    await Promise.resolve();
    expect(settled).toBe(false);
    vi.advanceTimersByTime(1);
    await expect(p).resolves.toBeNull();
  });

  it("rejects a failed call with the shell's code, message, and extra fields", async () => {
    const sent: BridgeToShell[] = [];
    const rpc = new Rpc(m => sent.push(m));
    rpc.connect();
    const p = rpc.call("artifact", "publish", ["<!doctype html>"]);
    const id = (sent[0] as Extract<BridgeToShell, { type: "clax:call" }>).id;
    rpc.accept({ type: "clax:call-result", id, ok: false, error: { code: "conflict", message: "newer version", live: "4" } });
    const e = await p.catch(x => x);
    expect(e).toBeInstanceOf(CapabilityError);
    expect(e).toMatchObject({ code: "conflict", message: "newer version", live: "4" });
  });

  it("rejects arguments that cannot be posted with transform_error", async () => {
    const rpc = new Rpc(() => {});
    rpc.connect();
    await expect(rpc.call("db", "set", ["t/1", { f: () => 1 }])).rejects.toMatchObject({ code: "transform_error" });
  });

  it("dispatches events by capability and topic until unsubscribed", () => {
    const rpc = new Rpc(() => {});
    const seen: unknown[] = [];
    const off = rpc.on("db", "snapshot", d => seen.push(d));
    rpc.accept({ type: "clax:event", ns: "db", topic: "snapshot", data: 1 });
    rpc.accept({ type: "clax:event", ns: "db", topic: "other", data: 2 });
    off();
    rpc.accept({ type: "clax:event", ns: "db", topic: "snapshot", data: 3 });
    expect(seen).toEqual([1]);
  });
});
