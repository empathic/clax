import { afterEach, describe, expect, it, vi } from "vitest";
import { locationLocals } from "../src/caps/location";
import { CapabilityError } from "../src/rpc";

/** A fake shell: each call kept with its answer, and the pushes it makes. */
function shell() {
  const calls: { method: string; args: unknown[]; resolve(v: unknown): void; reject(e: unknown): void }[] = [];
  const listeners = new Map<string, Set<(d: unknown) => void>>();
  const rpc = {
    call: (_ns: string, method: string, args: unknown[]) => new Promise((resolve, reject) => { calls.push({ method, args, resolve, reject }); }),
    on: (ns: string, topic: string, fn: (d: unknown) => void) => {
      const k = `${ns}/${topic}`;
      if (!listeners.has(k)) listeners.set(k, new Set());
      listeners.get(k)!.add(fn);
      return () => { listeners.get(k)!.delete(fn); };
    },
  };
  const push = (topic: string, data: unknown) => { for (const f of [...(listeners.get(`location/${topic}`) ?? [])]) f(data); };
  const watch = (locationLocals(rpc as never) as { watch(f: unknown, o?: unknown): () => void }).watch;
  return { calls, push, watch, listening: () => [...listeners.values()].reduce((n, s) => n + s.size, 0) };
}

afterEach(() => vi.restoreAllMocks());

describe("location.watch in the page", () => {
  it("starts a watch, delivers its updates only, and stop() unwatches it", () => {
    const s = shell();
    const got: unknown[] = [];
    const stop = s.watch((p: unknown) => got.push(p), { minDistance: 25 });
    expect(s.calls[0]).toMatchObject({ method: "watch", args: [{ watch: "w1", minDistance: 25 }] });
    s.push("update", { watch: "w1", value: { lat: 1, lon: 2, accuracy: 9, precise: true } });
    s.push("update", { watch: "w9", value: { lat: 5 } });
    expect(got).toEqual([{ lat: 1, lon: 2, accuracy: 9, precise: true }]);
    stop();
    expect(s.calls[1]).toMatchObject({ method: "unwatch", args: [{ watch: "w1" }] });
    s.push("update", { watch: "w1", value: { lat: 3 } });
    expect(got).toHaveLength(1);
    expect(s.listening()).toBe(0);
  });

  it("an end push, or a refused start, reaches onError once and stops the updates", async () => {
    const s = shell();
    const errors: CapabilityError[] = [];
    const got: unknown[] = [];
    s.watch((p: unknown) => got.push(p), { onError: (e: CapabilityError) => errors.push(e) });
    s.push("end", { watch: "w1", code: "stopped", message: "the app left the screen" });
    s.push("update", { watch: "w1", value: {} });
    expect(errors.map(e => e.code)).toEqual(["stopped"]);
    expect(got).toEqual([]);
    s.watch(() => {}, { onError: (e: CapabilityError) => errors.push(e) });
    s.calls[1].reject(new CapabilityError("system_denied", "Android will not ask again"));
    await Promise.resolve();
    await Promise.resolve();
    expect(errors.map(e => e.code)).toEqual(["stopped", "system_denied"]);
    expect(s.listening()).toBe(0);
  });

  it("refuses a missing callback", () => {
    expect(() => shell().watch(undefined)).toThrow(TypeError);
  });
});
