import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";

import { BRIDGE_TYPES } from "../src/protocol";
import { hashFor, helloFor, isBridgeSrc, isFirstBridge, readMeta } from "../src/meta";

declare global { interface Window { claude?: { use(name: string): Promise<unknown> }; __clax?: { artifact: string; version: number; contract: string; file: string } } }

// window.claude is defined non-configurable, so the bridge loads once and
// every test shares that single load.
let first: HTMLScriptElement;
const bridgeScript = (src = "/_clax/bridge.js?v=0123456789ab") => {
  const s = document.createElement("script");
  s.setAttribute("src", src);
  return s;
};
const runAs = (script: HTMLScriptElement) => Object.defineProperty(document, "currentScript", { value: script, configurable: true });
beforeAll(async () => {
  document.body.innerHTML = "";
  // A page's own global of the bridge's name does not keep the bridge out.
  (window as { __clax?: unknown }).__clax = { preset: true };
  const script = first = bridgeScript();
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
    expect(window.__clax).toEqual({ artifact: "7q3k9mzx2b4t", version: 3, contract: "0.2.61", file: "docs/about.html" });
  });

  it("reports the page's fragment to the shell", () => {
    expect(hashFor("#docs%2Fcontract.md")).toEqual({ type: "clax:hash", hash: "#docs%2Fcontract.md" });
    expect(hashFor("")).toEqual({ type: "clax:hash", hash: "" });
    expect(BRIDGE_TYPES.has("clax:hash")).toBe(true);
  });

  it("names its file in the hello, the index when the tag names none", () => {
    const tag = document.createElement("script");
    tag.dataset.artifact = "7q3k9mzx2b4t"; tag.dataset.version = "2";
    expect(readMeta(tag)).toEqual({ artifact: "7q3k9mzx2b4t", version: 2, contract: "", file: "index.html" });
    tag.dataset.file = "about.html";
    expect(helloFor(readMeta(tag))).toEqual({ type: "clax:hello", artifact: "7q3k9mzx2b4t", version: 2, file: "about.html" });
    expect(readMeta(null).file).toBe("index.html");
  });

  it("installs although the page preset a window.__clax", () => {
    expect(window.__clax).not.toEqual({ preset: true });
    expect(window.claude).toBeDefined();
  });

  it("a duplicate bridge tag in the same document stands down", async () => {
    const installed = window.claude;
    const meta = window.__clax;
    const dup = bridgeScript();
    dup.dataset.artifact = "7q3k9mzx2b4t"; dup.dataset.version = "9";
    document.body.appendChild(dup);
    runAs(dup);
    try {
      vi.resetModules();
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      await import("../src/bridge");
      expect(warn).not.toHaveBeenCalled();
      expect(window.claude).toBe(installed);
      expect(window.__clax).toBe(meta);
    } finally {
      dup.remove();
      runAs(first);
    }
  });

  it("counts only the daemon's exact tag forms when finding the first bridge", () => {
    expect(isBridgeSrc("/_clax/bridge.js")).toBe(true);
    expect(isBridgeSrc("/_clax/bridge.js?v=0a1b")).toBe(true);
    for (const src of ["/_clax/bridge.jsx", "/_clax/bridge.js?v=XY", "https://x/_clax/bridge.js", "/_clax/bridge.js?v=1&x", null]) expect(isBridgeSrc(src)).toBe(false);
    const other = bridgeScript("/_clax/bridge.js?v=NOTHEX");
    other.dataset.artifact = "x";
    const noData = bridgeScript();
    document.body.prepend(other, noData);
    try {
      expect(isFirstBridge(first)).toBe(true);
      expect(isFirstBridge(null)).toBe(true);
    } finally {
      other.remove(); noData.remove();
    }
  });

  it("logs once and does not throw when window.claude cannot be redefined", async () => {
    const installed = window.claude;
    // The document's first bridge loading again runs in full.
    runAs(first);
    vi.resetModules();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(import("../src/bridge")).resolves.toBeDefined();
    expect(warn).toHaveBeenCalledTimes(1);
    expect(window.claude).toBe(installed);
  });
});
