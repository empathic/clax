import { describe, expect, it, vi } from "vitest";
import { type UseContext, resolveUse } from "../src/caps/resolve";
import { siteOf } from "../src/caps/origin";
import type { OriginClient } from "../src/caps/origin-client";
import { commentsContext } from "../src/comments-context";
import { makeUse } from "../src/use";

function fakeRpc(granted: Record<string, unknown>) {
  return {
    use: vi.fn(async (name: string) => (name in granted ? { config: granted[name] } : null)),
    call: vi.fn(async (ns: string, method: string, args: unknown[]) => ({ ns, method, args })),
    on: vi.fn(() => () => {}),
  };
}

/** A context whose shell grants `granted`; the page's own site declares `declared`. */
function context(framed: boolean, granted: Record<string, unknown>, declared: Record<string, unknown> = {}) {
  const rpc = fakeRpc(granted);
  const client: OriginClient = {
    call: async (cap, method) => (cap === "artifact" ? { capabilities: declared } : method === "whoami" ? { kind: "person", level: "view", can_edit: false } : null),
    socket: () => { throw new Error("no socket"); },
  };
  const o: UseContext = {
    framed,
    rpc: rpc as never,
    room: async () => ({ roomNamespace: () => Object.freeze({ part: "room" }), originRoom: () => ({}) }) as never,
    sample: async () => ({ sampleNamespace: () => Object.freeze({ part: "sample" }) }) as never,
    env: { ctx: commentsContext, clip: () => import("../src/parts/clip") },
    site: siteOf(client),
  };
  return { rpc, resolve: (n: string) => resolveUse(n, o) };
}

describe("use()", () => {
  it("never resolves during the page's synchronous run and memoises one promise per name", async () => {
    const use = makeUse(async () => "ns");
    let resolved = false;
    const p = use("permissions");
    void p.then(() => { resolved = true; });
    expect(resolved).toBe(false);
    expect(use("permissions")).toBe(p);
    expect(await p).toBe("ns");
  });

  it("aliases self to artifact, and never rejects", async () => {
    const asked: string[] = [];
    const use = makeUse(async n => { asked.push(n); throw new Error("no part"); });
    expect(use("self")).toBe(use("artifact"));
    await expect(use("self")).resolves.toBeNull();
    await expect(use(42 as unknown as string)).resolves.toBeNull();
    expect(asked).toEqual(["artifact", ""]);
  });
});

describe("resolveUse", () => {
  it("resolves the origin's capabilities from the site, framed or top-level, never asking the shell", async () => {
    for (const framed of [false, true]) {
      const { rpc, resolve } = context(framed, { db: {}, room: {}, assets: {} }, { db: {}, room: {} });
      expect(Object.keys((await resolve("db")) as object).sort()).toEqual(["collection", "doc"]);
      expect(await resolve("room")).toEqual({ part: "room" });
      expect(await resolve("user")).not.toBeNull();
      // Declared but not the owner's: refused, whatever a shell would grant.
      expect(await resolve("assets")).toBeNull();
      expect(rpc.use.mock.calls.map(c => c[0]).filter(n => n !== "db" && n !== "user")).toEqual([]);
    }
  });

  it("resolves the shell's capabilities null top-level, without asking any shell", async () => {
    const { rpc, resolve } = context(false, { permissions: {}, artifact: {}, sample: {} });
    for (const n of ["permissions", "artifact", "comments", "sample", "downloads"]) await expect(resolve(n)).resolves.toBeNull();
    expect(rpc.use).not.toHaveBeenCalled();
  });

  it("framed, resolves what the shell grants as a frozen namespace calling it, and null for the rest", async () => {
    const { rpc, resolve } = context(true, { permissions: {}, sample: {} });
    const ns = (await resolve("permissions")) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    expect(Object.isFrozen(ns)).toBe(true);
    expect(Object.keys(ns).sort()).toEqual(["request", "state"]);
    await expect(ns.state("db")).resolves.toEqual({ ns: "permissions", method: "state", args: ["db"] });
    expect(await resolve("sample")).toEqual({ part: "sample" });
    for (const n of ["artifact", "comments", "files", "mcp", "toString", "__proto__", "nonsense"]) await expect(resolve(n)).resolves.toBeNull();
    // Unknown names reach no shell.
    expect(rpc.use.mock.calls.map(c => c[0])).toEqual(["permissions", "sample", "artifact", "comments"]);
  });
});
