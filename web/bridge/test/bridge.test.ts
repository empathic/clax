import { describe, it, expect, vi, afterEach } from "vitest";

afterEach(() => { vi.restoreAllMocks(); });

// window.claude is non-configurable, so it persists for the rest of this file:
// the second test relies on the first having installed it.
describe("bridge", () => {
  it("installs window.claude.use resolving null", async () => {
    await import("../src/bridge");
    expect(await (window as any).claude.use("anything")).toBeNull();
  });

  it("logs once and does not throw when window.claude is already locked", async () => {
    const installed = (window as any).claude;
    vi.resetModules();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(import("../src/bridge")).resolves.toBeDefined();
    expect(warn).toHaveBeenCalledTimes(1);
    expect((window as any).claude).toBe(installed);
  });
});
