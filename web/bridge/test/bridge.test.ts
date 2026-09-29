import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";

import { BRIDGE_TYPES } from "../src/protocol";
import { hashFor, helloFor, readMeta } from "../src/meta";

declare global { interface Window { claude?: { use(name: string): Promise<unknown> }; __artifax?: { artifact: string; version: number; contract: string; file: string } } }

// window.claude is defined non-configurable, so the bridge loads once and
// every test shares that single load.
beforeAll(async () => {
  document.body.innerHTML = "";
  const script = document.createElement("script");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "3"; script.dataset.contract = "0.2.61"; script.dataset.file = "docs/about.html";
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
    expect(window.__artifax).toEqual({ artifact: "7q3k9mzx2b4t", version: 3, contract: "0.2.61", file: "docs/about.html" });
  });

  it("reports the page's fragment to the shell", () => {
    expect(hashFor("#docs%2Fcontract.md")).toEqual({ type: "artifax:hash", hash: "#docs%2Fcontract.md" });
    expect(hashFor("")).toEqual({ type: "artifax:hash", hash: "" });
    expect(BRIDGE_TYPES.has("artifax:hash")).toBe(true);
  });

  it("names its file in the hello, the index when the tag names none", () => {
    const tag = document.createElement("script");
    tag.dataset.artifact = "7q3k9mzx2b4t"; tag.dataset.version = "2";
    expect(readMeta(tag)).toEqual({ artifact: "7q3k9mzx2b4t", version: 2, contract: "", file: "index.html" });
    tag.dataset.file = "about.html";
    expect(helloFor(readMeta(tag))).toEqual({ type: "artifax:hello", artifact: "7q3k9mzx2b4t", version: 2, file: "about.html" });
    expect(readMeta(null).file).toBe("index.html");
  });

  it("a second bridge in the same document does nothing", async () => {
    const installed = window.claude;
    const meta = window.__artifax;
    vi.resetModules();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await import("../src/bridge");
    expect(warn).not.toHaveBeenCalled();
    expect(window.claude).toBe(installed);
    expect(window.__artifax).toBe(meta);
  });

  it("logs once and does not throw when window.claude cannot be redefined", async () => {
    const installed = window.claude;
    // Without the first bridge's marker the second load runs in full.
    delete (window as { __artifax?: unknown }).__artifax;
    vi.resetModules();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(import("../src/bridge")).resolves.toBeDefined();
    expect(warn).toHaveBeenCalledTimes(1);
    expect(window.claude).toBe(installed);
  });
});
