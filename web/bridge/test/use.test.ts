import { describe, expect, it, vi } from "vitest";
import { type UseContext, resolveUse } from "../src/caps/resolve";
import { commentsContext } from "../src/comments-context";
import { makeUse } from "../src/use";

function fakeRpc(granted: Record<string, unknown>) {
  return {
    use: vi.fn(async (name: string) => (name in granted ? { config: granted[name] } : null)),
    call: vi.fn(async (ns: string, method: string, args: unknown[]) => ({ ns, method, args })),
    on: vi.fn(() => () => {}),
  };
}

/** A framed page's context whose shell grants `granted`. */
function context(granted: Record<string, unknown>) {
  const rpc = fakeRpc(granted);
  const o: UseContext = {
    rpc: rpc as never,
    sample: async () => ({ sampleNamespace: () => Object.freeze({ part: "sample" }) }) as never,
    env: { ctx: commentsContext, clip: () => import("../src/parts/clip") },
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

  it("aliases self to artifact, takes a plain null, and never rejects", async () => {
    const asked: string[] = [];
    const use = makeUse(async n => { asked.push(n); throw new Error("no part"); });
    expect(use("self")).toBe(use("artifact"));
    await expect(use("self")).resolves.toBeNull();
    await expect(use(42 as unknown as string)).resolves.toBeNull();
    expect(asked).toEqual(["artifact", ""]);
    await expect(makeUse(() => null)("db")).resolves.toBeNull();
  });
});

describe("resolveUse (a framed page's shell capabilities)", () => {
  it("the device capabilities resolve namespaces whose members call the shell", async () => {
    const { resolve } = context({ notify: {}, share: {}, file: {}, camera: {}, location: {} });
    const members = async (n: string) => Object.keys((await resolve(n)) as object).sort();
    expect(await members("notify")).toEqual(["cancel", "list", "schedule"]);
    expect(await members("share")).toEqual(["file", "text"]);
    expect(await members("file")).toEqual(["pick", "save"]);
    expect(await members("camera")).toEqual(["pick", "take"]);
    expect(await members("location")).toEqual(["current", "watch"]);
    const notify = (await resolve("notify")) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    await expect(notify.cancel("water")).resolves.toEqual({ ns: "notify", method: "cancel", args: ["water"] });
  });

  it("resolves what the shell grants as a frozen namespace calling it, and null for the rest", async () => {
    const { rpc, resolve } = context({ permissions: {}, sample: {} });
    const ns = (await resolve("permissions")) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    expect(Object.isFrozen(ns)).toBe(true);
    expect(Object.keys(ns).sort()).toEqual(["request", "state"]);
    await expect(ns.state("db")).resolves.toEqual({ ns: "permissions", method: "state", args: ["db"] });
    expect(await resolve("sample")).toEqual({ part: "sample" });
    for (const n of ["artifact", "comments", "files", "mcp", "toString", "__proto__", "nonsense"]) await expect(resolve(n)).resolves.toBeNull();
    // Unknown names reach no shell.
    expect(rpc.use.mock.calls.map(c => c[0])).toEqual(["permissions", "sample", "artifact", "comments"]);
  });

  it("resolves a granted artifact with its page-side members", async () => {
    const { resolve } = context({ artifact: {} });
    const ns = (await resolve("artifact")) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    expect(Object.keys(ns).sort()).toEqual(["edit", "publish", "sync"]);
    await expect(ns.edit()).rejects.toMatchObject({ code: "invalid_content" });
  });
});
