import { describe, expect, it, vi } from "vitest";
import type { CapEnv, Handler } from "./host";
import { lazyHandler } from "./lazy";

const env = {} as CapEnv;
const grants = {} as never;

describe("lazyHandler", () => {
  it("loads the handler on the first call only, once, and forwards the lifecycle to it", async () => {
    const inner: Handler = { call: vi.fn(async () => 7), reset: vi.fn(), leave: vi.fn(), dispose: vi.fn() };
    const load = vi.fn(async () => () => inner);
    const h = lazyHandler(load)(env, grants);
    h.leave!();
    expect(load).not.toHaveBeenCalled();
    await expect(h.call("x", [])).resolves.toBe(7);
    await h.call("y", []);
    expect(load).toHaveBeenCalledTimes(1);
    h.reset!();
    h.leave!();
    h.dispose!();
    expect([inner.reset, inner.leave, inner.dispose].map(f => (f as ReturnType<typeof vi.fn>).mock.calls.length)).toEqual([1, 1, 1]);
  });

  it("a chunk that cannot load rejects capability_disabled, and the next call tries again", async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(() => ({ call: async () => "ok" }));
    const h = lazyHandler(load)(env, grants);
    await expect(h.call("x", [])).rejects.toMatchObject({ code: "capability_disabled" });
    await expect(h.call("x", [])).resolves.toBe("ok");
  });

  it("after dispose, a chunk that arrives late makes no handler", async () => {
    let arrive!: (f: () => Handler) => void;
    const make = vi.fn(() => ({ call: async () => 1 }));
    const h = lazyHandler(() => new Promise(r => { arrive = r; }))(env, grants);
    const p = h.call("x", []);
    h.dispose!();
    arrive(make);
    await expect(p).rejects.toMatchObject({ code: "capability_disabled" });
    expect(make).not.toHaveBeenCalled();
  });
});
