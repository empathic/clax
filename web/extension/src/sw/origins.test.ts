import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { ask, bootNonce, dropLoaders, injectOverlay, originOf, overlayPresent, probeDocument, sameOrigin, type OriginsEnv } from "./origins";

let c: FakeChrome;
const BOOT = "b".repeat(32);
const env = () => ({ permissions: c.permissions, scripting: c.scripting, boot: async () => BOOT }) as unknown as OriginsEnv;
beforeEach(() => { c = fakeChrome(); });

describe("origins", () => {
  it("reads only http and https origins", () => {
    expect(originOf("http://localhost:5173/a?b#c")).toBe("http://localhost:5173");
    expect(originOf("chrome://extensions")).toBeNull();
    expect(originOf("file:///x")).toBeNull();
    expect(originOf("nonsense")).toBeNull();
  });

  it("asks for the origin's permission and registers no content script", async () => {
    expect(await ask(env(), "http://localhost:5173")).toBe(true);
    expect(c.granted.has("http://localhost:5173/*")).toBe(true);
    expect(c.calls.some(x => x.api === "scripting.registerContentScripts")).toBe(false);
  });

  it("unregisters the loaders an earlier build registered per origin, and forgets its origins", async () => {
    c.storage.local.data.origins = ["http://localhost:5173"];
    c.scripting.getRegisteredContentScripts = (async () => [{ id: "clax-loader-6874" }, { id: "someone-else" }]) as unknown as typeof c.scripting.getRegisteredContentScripts;
    await dropLoaders(c.scripting as never, c.storage.local);
    expect(c.calls.filter(x => x.api === "scripting.unregisterContentScripts").map(x => x.args[0])).toEqual([{ ids: ["clax-loader-6874"] }]);
    expect("origins" in c.storage.local.data).toBe(false);
  });

  it("answers false when Chrome refuses or fails the request", async () => {
    c.permissions.request = (async () => { throw new Error("no gesture"); }) as typeof c.permissions.request;
    expect(await ask(env(), "http://localhost:5173")).toBe(false);
  });

  it("tells whether a page's URL is of its sender's origin", () => {
    expect(sameOrigin("http://localhost:5173/a#b", "http://localhost:5173/")).toBe(true);
    expect(sameOrigin("http://evil.example/", "http://localhost:5173/")).toBe(false);
    expect(sameOrigin("http://localhost:5173/", undefined)).toBe(false);
    expect(sameOrigin("chrome://x", "chrome://x")).toBe(false);
  });

  /** The fake's top document of each tab: its origin and its ID, as Chrome's probe answers them. */
  const top = (origin: string, documentId = "doc-1") => {
    c.scripting.executeScript = (async (inj: { files?: string[]; func?: (...a: unknown[]) => unknown; args?: unknown[] }) => {
      c.calls.push({ api: "scripting.executeScript", args: [inj] });
      return [{ documentId, result: inj.func?.name === "probeDocument" ? { present: false, origin } : undefined }];
    }) as unknown as typeof c.scripting.executeScript;
  };

  it("probes the top frame, then injects the overlay into that one document only", async () => {
    top("http://localhost:5173", "doc-7");
    expect(await injectOverlay(env(), 7, "http://localhost:5173")).toBe(true);
    const injects = c.calls.filter(x => x.api === "scripting.executeScript").map(x => x.args[0] as { target: unknown; files?: string[] });
    expect(injects[0].target).toEqual({ tabId: 7, frameIds: [0] });
    expect(injects.slice(1).map(a => a.target)).toEqual([{ tabId: 7, documentIds: ["doc-7"] }, { tabId: 7, documentIds: ["doc-7"] }]);
    expect(injects.filter(a => a.files)).toEqual([{ target: { tabId: 7, documentIds: ["doc-7"] }, files: ["overlay.js"] }]);
  });

  it("injects nothing into a document of another origin than the one Clax is on for", async () => {
    top("https://elsewhere.example");
    expect(await injectOverlay(env(), 7, "http://localhost:5173")).toBeNull();
    expect(c.calls.filter(x => x.api === "scripting.executeScript")).toHaveLength(1);
  });

  it("does not inject the overlay twice into one document, and counts only a started overlay of this load of the extension", async () => {
    let fail = true;
    const g = globalThis as { claxOverlayStarted?: unknown; claxBoot?: string };
    // The fake runs each function in this test's global, as Chrome runs it in
    // the tab's isolated world; `overlay.js` marks the world as the overlay
    // does (content/presence.ts), alive for the boot nonce it started under.
    c.scripting.executeScript = (async (inj: { files?: string[]; func?: (...a: unknown[]) => unknown; args?: unknown[] }) => {
      c.calls.push({ api: "scripting.executeScript", args: [inj] });
      if (inj.func) return [{ documentId: "doc-1", result: inj.func(...(inj.args ?? [])) }];
      if (fail) throw new Error("Script failed to load.");
      const boot = g.claxBoot;
      g.claxOverlayStarted = { alive: (b: unknown) => b === boot, stop: () => {} };
      return [{ result: undefined }];
    }) as unknown as typeof c.scripting.executeScript;
    const files = () => c.calls.filter(x => x.api === "scripting.executeScript" && (x.args[0] as { files?: string[] }).files).length;
    try {
      await expect(injectOverlay(env(), 7, location.origin)).rejects.toThrow("failed to load");
      expect(await overlayPresent(env(), 7)).toBe(false);
      fail = false;
      expect(await injectOverlay(env(), 7, location.origin)).toBe(true);
      expect(await overlayPresent(env(), 7)).toBe(true);
      expect(await injectOverlay(env(), 7, location.origin)).toBe(false);
      expect(files()).toBe(2);
      // The extension loaded again (a new boot nonce): the old overlay's mark
      // stays in the world, but it is not this load's, so it is injected again.
      const again = { ...env(), boot: async () => "c".repeat(32) } as OriginsEnv;
      expect(await overlayPresent(again, 7)).toBe(false);
      expect(await injectOverlay(again, 7, location.origin)).toBe(true);
      expect(await overlayPresent(again, 7)).toBe(true);
      // A mark that is not the overlay's (a stale flag of another shape) is no overlay.
      g.claxOverlayStarted = true;
      expect(await overlayPresent(again, 7)).toBe(false);
    } finally {
      delete g.claxOverlayStarted;
      delete g.claxBoot;
    }
  });

  it("keeps one boot nonce per load of the extension, across the worker's restarts", async () => {
    let n = 0;
    const random = () => String(++n).padStart(32, "0");
    const first = bootNonce(c.storage.session as never, random);
    const a = await first();
    expect(await first()).toBe(a);
    // A restarted worker reads it back.
    expect(await bootNonce(c.storage.session as never, random)()).toBe(a);
    // A reload of the extension clears session storage: a new nonce.
    await c.storage.session.remove("boot");
    const b = await bootNonce(c.storage.session as never, random)();
    expect(b).not.toBe(a);
    expect(b).toMatch(/^[0-9a-f]{32}$/);
  });

  it("probes again when the tab moved to another document before the injection, and gives up quietly the second time", async () => {
    const g = globalThis as { claxOverlayStarted?: unknown; claxBoot?: string };
    let docs = 0;
    let gone = 1;
    c.scripting.executeScript = (async (inj: { files?: string[]; func?: (...a: unknown[]) => unknown; args?: unknown[]; target: { documentIds?: string[] } }) => {
      if (inj.func === probeDocument) return [{ documentId: `doc-${++docs}`, result: inj.func(...(inj.args ?? [])) }];
      // The document probed is gone by the time the injection reaches it.
      if (gone-- > 0) throw new Error(`No document with id ${inj.target.documentIds![0]} in tab with id 7`);
      if (inj.func) return [{ result: inj.func(...(inj.args ?? [])) }];
      g.claxOverlayStarted = { alive: () => true, stop: () => {} };
      return [{ result: undefined }];
    }) as unknown as typeof c.scripting.executeScript;
    try {
      expect(await injectOverlay(env(), 7, location.origin)).toBe(true);
      expect(docs).toBe(2);
      delete g.claxOverlayStarted;
      gone = 2;
      expect(await injectOverlay(env(), 7, location.origin)).toBeNull();
    } finally {
      delete g.claxOverlayStarted;
      delete g.claxBoot;
    }
  });

  it("reads a tab it cannot reach as having no overlay", async () => {
    c.scripting.executeScript = (async () => { throw new Error("Cannot access contents of the page."); }) as unknown as typeof c.scripting.executeScript;
    expect(await overlayPresent(env(), 7)).toBe(false);
  });
});
