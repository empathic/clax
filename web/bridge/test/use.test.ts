import { describe, expect, it, vi } from "vitest";
import { makeUse } from "../src/use";

function fakeRpc(granted: Record<string, unknown>) {
  return {
    use: vi.fn(async (name: string) => (name in granted ? { config: granted[name] } : null)),
    call: vi.fn(async (ns: string, method: string, args: unknown[]) => ({ ns, method, args })),
    on: vi.fn(() => () => {}),
  };
}

describe("use()", () => {
  it("use resolves null unframed, for every name", async () => {
    const rpc = fakeRpc({ db: {} });
    const use = makeUse({ framed: false, rpc: rpc as never });
    for (const n of ["db", "permissions", "artifact", "nonsense"]) await expect(use(n)).resolves.toBeNull();
    expect(rpc.use).not.toHaveBeenCalled();
  });

  it("never resolves during the page's synchronous run and memoises one promise per name", async () => {
    const use = makeUse({ framed: true, rpc: fakeRpc({ permissions: {} }) as never });
    let resolved = false;
    const p = use("permissions");
    void p.then(() => { resolved = true; });
    expect(resolved).toBe(false);
    expect(use("permissions")).toBe(p);
    await p;
    expect(resolved).toBe(true);
  });

  it("aliases self to artifact and resolves unknown, undeclared, and v1-excluded names to null", async () => {
    const rpc = fakeRpc({ artifact: {} });
    const use = makeUse({ framed: true, rpc: rpc as never });
    expect(use("self")).toBe(use("artifact"));
    expect(await use("self")).not.toBeNull();
    for (const n of ["files", "mcp", "room", "sample", "db", "toString", "__proto__"]) await expect(use(n)).resolves.toBeNull();
    await expect(use(42 as unknown as string)).resolves.toBeNull();
    expect(rpc.use.mock.calls.map(c => c[0])).toEqual(["artifact", "db"]);
  });

  it("resolves a frozen namespace whose members call the shell", async () => {
    const rpc = fakeRpc({ permissions: {} });
    const use = makeUse({ framed: true, rpc: rpc as never });
    const ns = (await use("permissions")) as Record<string, (...a: unknown[]) => Promise<unknown>>;
    expect(Object.isFrozen(ns)).toBe(true);
    expect(Object.keys(ns).sort()).toEqual(["request", "state"]);
    await expect(ns.state("db")).resolves.toEqual({ ns: "permissions", method: "state", args: ["db"] });
    expect(() => { (ns as Record<string, unknown>).state = null; }).toThrow();
  });

  it("never rejects, even when the shell connection throws", async () => {
    const rpc = { use: vi.fn(async () => { throw new Error("boom"); }), call: vi.fn(), on: vi.fn() };
    const use = makeUse({ framed: true, rpc: rpc as never });
    await expect(use("db")).resolves.toBeNull();
  });
});
