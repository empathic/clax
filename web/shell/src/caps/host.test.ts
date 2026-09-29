import { describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { CapError } from "./errors";
import { CapabilityHost, type CapEnv, type HandlerFactory } from "./host";
import { REGISTRY } from "./registry";

function env(over: Partial<CapEnv> = {}) {
  const posted: ShellToBridge[] = [];
  const e: CapEnv = {
    aid: "7q3k9mzx2b4t", version: 1, pinned: false, token: "t",
    viewer: async () => ({ publicId: "u_00000000000000000000aa", name: "Alex" }),
    declared: { db: {}, comments: {} },
    prompt: vi.fn(async () => "allow" as const),
    post: m => posted.push(m),
    reload: vi.fn(),
    ...over,
  };
  return { e, posted };
}

describe("CapabilityHost", () => {
  it("grants declared capabilities and permissions, refuses the rest", async () => {
    const { e, posted } = env();
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const [i, name] of ["db", "permissions", "user", "assets", "files", "comments"].entries()) {
      await host.handle({ type: "artifax:use", id: `u${i}`, name });
    }
    expect(posted.map(m => (m as { granted: boolean }).granted)).toEqual([true, true, true, false, false, true]);
    expect(posted[0]).toMatchObject({ config: {} });
  });

  it("answers artifact declared under its legacy name self", async () => {
    const { e, posted } = env({ declared: { self: { note: 1 } } });
    await new CapabilityHost(Promise.resolve(e), REGISTRY, null).handle({ type: "artifax:use", id: "u", name: "artifact" });
    expect(posted[0]).toMatchObject({ granted: true, config: { note: 1 } });
  });

  it("returns handler values and maps errors, never leaving a call unanswered", async () => {
    const { e, posted } = env();
    const factories: Record<string, HandlerFactory> = {
      db: () => ({ async call(method) { if (method === "get") return { ok: 1 }; if (method === "bad") throw new CapError("invalid_argument", "no", { path: "x" }); throw new Error("boom"); } }),
    };
    const host = new CapabilityHost(Promise.resolve(e), factories, null);
    await host.handle({ type: "artifax:call", id: "1", ns: "db", method: "get", args: [] });
    await host.handle({ type: "artifax:call", id: "2", ns: "db", method: "bad", args: [] });
    await host.handle({ type: "artifax:call", id: "3", ns: "db", method: "other", args: [] });
    await host.handle({ type: "artifax:call", id: "4", ns: "files", method: "list", args: [] });
    await host.handle({ type: "artifax:call", id: "5", ns: "comments", method: "create", args: [] });
    expect(posted).toEqual([
      { type: "artifax:call-result", id: "1", ok: true, value: { ok: 1 } },
      { type: "artifax:call-result", id: "2", ok: false, error: { code: "invalid_argument", message: "no", path: "x" } },
      { type: "artifax:call-result", id: "3", ok: false, error: { code: "upstream_error", message: "boom" } },
      { type: "artifax:call-result", id: "4", ok: false, error: { code: "not_granted", message: "files is not available to this view" } },
      { type: "artifax:call-result", id: "5", ok: false, error: { code: "capability_removed", message: "comments is not part of this runtime" } },
    ]);
  });

  it("permissions: state and request, with one dialog", async () => {
    const { e, posted } = env();
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const call = async (method: string, args: unknown[]) => { await host.handle({ type: "artifax:call", id: method + posted.length, ns: "permissions", method, args }); return (posted.at(-1) as { value: unknown }).value; };
    expect(await call("state", [])).toEqual({ db: "granted", user: "granted", comments: "prompt" });
    expect(await call("state", ["room"])).toBe("unavailable");
    expect(await call("request", [["comments"]])).toEqual({ comments: "granted" });
    expect(await call("request", [])).toEqual({ db: "granted", user: "granted", comments: "granted" });
    expect(e.prompt).toHaveBeenCalledTimes(1);
  });

  it("answers requests when the viewer lookup failed, keeping grants for the page load only", async () => {
    const { e, posted } = env({ viewer: async () => { throw new Error("403 nope"); } });
    const storage = { getItem: vi.fn(() => null), setItem: vi.fn() };
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, storage as unknown as Storage);
    await host.handle({ type: "artifax:use", id: "u", name: "permissions" });
    await host.handle({ type: "artifax:call", id: "c", ns: "permissions", method: "request", args: [["comments"]] });
    expect(posted).toEqual([
      { type: "artifax:use-result", id: "u", granted: true, config: {} },
      { type: "artifax:call-result", id: "c", ok: true, value: { comments: "granted" } },
    ]);
    expect(storage.setItem).not.toHaveBeenCalled();
  });

  it("dispose ends every handler: no further fetches, and late results post nothing", async () => {
    vi.useFakeTimers();
    try {
      let release: (r: Response) => void = () => {};
      let fetches = 0;
      vi.stubGlobal("fetch", vi.fn(() => {
        fetches++;
        return fetches === 1 ? Promise.resolve(new Response(JSON.stringify({ doc: { path: "t/1", id: "1", data: {}, version: 1 } })))
          : new Promise<Response>(r => { release = r; });
      }));
      const { e, posted } = env({ declared: { db: {} } });
      const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
      await host.handle({ type: "artifax:call", id: "c1", ns: "db", method: "subscribe", args: ["s1", { kind: "doc", path: "t/1" }] });
      host.onEvent({ type: "stream_down" });
      const late = host.handle({ type: "artifax:call", id: "c2", ns: "db", method: "get", args: ["t/2"] });
      await vi.advanceTimersByTimeAsync(0);
      const before = posted.length;
      host.dispose();
      release(new Response(JSON.stringify({ doc: { path: "t/2", id: "2", data: {}, version: 2 } })));
      await late;
      await vi.advanceTimersByTimeAsync(120_000);
      await host.handle({ type: "artifax:call", id: "c3", ns: "db", method: "get", args: ["t/3"] });
      await host.handle({ type: "artifax:use", id: "u", name: "db" });
      expect([fetches, posted.length]).toEqual([2, before]);
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });
});
