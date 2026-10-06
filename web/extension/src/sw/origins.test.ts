import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { ask, bootNonce, enabled, forget, injectOverlay, originOf, overlayPresent, remember, restoreLoaders, sameOrigin, scriptId, type OriginsEnv } from "./origins";

let c: FakeChrome;
const BOOT = "b".repeat(32);
const env = () => ({ permissions: c.permissions, scripting: c.scripting, local: c.storage.local, boot: async () => BOOT }) as unknown as OriginsEnv;
beforeEach(() => { c = fakeChrome(); });

describe("origins", () => {
  it("reads only http and https origins", () => {
    expect(originOf("http://localhost:5173/a?b#c")).toBe("http://localhost:5173");
    expect(originOf("chrome://extensions")).toBeNull();
    expect(originOf("file:///x")).toBeNull();
    expect(originOf("nonsense")).toBeNull();
  });

  it("asks for the origin, registers the loader once, and forgets both", async () => {
    expect(await ask(env(), "http://localhost:5173")).toBe(true);
    await remember(env(), "http://localhost:5173");
    const reg = c.calls.find(x => x.api === "scripting.registerContentScripts")!.args[0] as chrome.scripting.RegisteredContentScript[];
    expect(reg[0]).toMatchObject({ id: scriptId("http://localhost:5173"), matches: ["http://localhost:5173/*"], js: ["loader.js"], runAt: "document_idle", allFrames: false, persistAcrossSessions: true });
    expect(c.storage.local.data.origins).toEqual(["http://localhost:5173"]);
    expect(await enabled(env(), "http://localhost:5173")).toBe(true);
    await forget(env(), "http://localhost:5173");
    expect(c.calls.some(x => x.api === "scripting.unregisterContentScripts")).toBe(true);
    expect(c.granted.has("http://localhost:5173/*")).toBe(false);
    expect(c.storage.local.data.origins).toEqual([]);
    expect(await enabled(env(), "http://localhost:5173")).toBe(false);
  });

  it("registers again the loader of each origin it is on whose permission is held, when Chromium dropped it", async () => {
    const a = "http://localhost:5173", b = "http://localhost:5174", d = "http://localhost:5175";
    c.storage.local.data.origins = [a, b, d];
    c.granted.add(`${a}/*`);
    c.granted.add(`${d}/*`);
    c.scripting.getRegisteredContentScripts = (async () => [{ id: scriptId(d) }]) as unknown as typeof c.scripting.getRegisteredContentScripts;
    await restoreLoaders(env());
    const reg = c.calls.filter(x => x.api === "scripting.registerContentScripts").flatMap(x => x.args[0] as chrome.scripting.RegisteredContentScript[]);
    expect(reg.map(r => r.id)).toEqual([scriptId(a)]);
    expect(reg[0]).toMatchObject({ matches: [`${a}/*`], js: ["loader.js"], persistAcrossSessions: true });
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

  it("gives each origin its own script ID", () => {
    expect(scriptId("http://localhost:5173")).not.toBe(scriptId("http://localhost:5174"));
    expect(scriptId("http://localhost:5173")).toMatch(/^clax-loader-[0-9a-f]+$/);
  });

  it("injects the overlay into the top frame only", async () => {
    await injectOverlay(env(), 7);
    const injects = c.calls.filter(x => x.api === "scripting.executeScript").map(x => x.args[0] as { files?: string[] });
    expect(injects.filter(a => a.files)).toEqual([{ target: { tabId: 7, allFrames: false }, files: ["overlay.js"] }]);
  });

  it("does not inject the overlay twice into one document, and counts only a started overlay of this load of the extension", async () => {
    let fail = true;
    const g = globalThis as { claxOverlayStarted?: unknown; claxBoot?: string };
    // The fake runs each function in this test's global, as Chrome runs it in
    // the tab's isolated world; `overlay.js` marks the world as the overlay
    // does (content/presence.ts), alive for the boot nonce it started under.
    c.scripting.executeScript = (async (inj: { files?: string[]; func?: (...a: unknown[]) => unknown; args?: unknown[] }) => {
      c.calls.push({ api: "scripting.executeScript", args: [inj] });
      if (inj.func) return [{ result: inj.func(...(inj.args ?? [])) }];
      if (fail) throw new Error("Frame with ID 0 was removed.");
      const boot = g.claxBoot;
      g.claxOverlayStarted = { alive: (b: unknown) => b === boot, stop: () => {} };
      return [{ result: undefined }];
    }) as unknown as typeof c.scripting.executeScript;
    const files = () => c.calls.filter(x => x.api === "scripting.executeScript" && (x.args[0] as { files?: string[] }).files).length;
    try {
      await expect(injectOverlay(env(), 7)).rejects.toThrow("removed");
      expect(await overlayPresent(env(), 7)).toBe(false);
      fail = false;
      expect(await injectOverlay(env(), 7)).toBe(true);
      expect(await overlayPresent(env(), 7)).toBe(true);
      expect(await injectOverlay(env(), 7)).toBe(false);
      expect(files()).toBe(2);
      // The extension loaded again (a new boot nonce): the old overlay's mark
      // stays in the world, but it is not this load's, so it is injected again.
      const again = { ...env(), boot: async () => "c".repeat(32) } as OriginsEnv;
      expect(await overlayPresent(again, 7)).toBe(false);
      expect(await injectOverlay(again, 7)).toBe(true);
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

  it("reads a tab it cannot reach as having no overlay", async () => {
    c.scripting.executeScript = (async () => { throw new Error("Cannot access contents of the page."); }) as unknown as typeof c.scripting.executeScript;
    expect(await overlayPresent(env(), 7)).toBe(false);
  });
});
