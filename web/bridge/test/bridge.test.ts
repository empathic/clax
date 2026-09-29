import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";

declare global { interface Window { claude?: { use(name: string): Promise<unknown> }; __artifax?: { artifact: string; version: number; contract: string } } }

// window.claude is defined non-configurable, so the bridge loads once and
// every test shares that single load.
beforeAll(async () => {
  document.body.innerHTML = "";
  const script = document.createElement("script");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "3"; script.dataset.contract = "0.2.61";
  document.body.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
});

describe("bridge", () => {
  afterEach(() => { vi.restoreAllMocks(); });

  it("exposes only use() and resolves null for every capability when unframed", async () => {
    expect(Object.keys(window.claude!)).toEqual(["use"]);
    for (const name of ["db", "artifact", "self", "permissions", "room", "sample", "nonsense"]) {
      await expect(window.claude!.use(name)).resolves.toBeNull();
    }
    expect(window.claude!.use("self")).toBe(window.claude!.use("artifact"));
  });

  it("is frozen and memoised", async () => {
    expect(Object.isFrozen(window.claude)).toBe(true);
    expect(window.claude!.use("db")).toBe(window.claude!.use("db"));
    expect(() => { (window as any).claude = {}; }).toThrow();
  });

  it("reads its metadata from the script tag", () => {
    expect(window.__artifax).toEqual({ artifact: "7q3k9mzx2b4t", version: 3, contract: "0.2.61" });
  });

  it("logs once and does not throw when window.claude cannot be redefined", async () => {
    const installed = window.claude;
    vi.resetModules();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(import("../src/bridge")).resolves.toBeDefined();
    expect(warn).toHaveBeenCalledTimes(1);
    expect(window.claude).toBe(installed);
  });
});
