import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { ask, enabled, forget, injectOverlay, originOf, overlayPresent, remember, restoreLoaders, sameOrigin, scriptId, type OriginsEnv } from "./origins";

let c: FakeChrome;
const env = () => ({ permissions: c.permissions, scripting: c.scripting, local: c.storage.local }) as unknown as OriginsEnv;
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

  it("does not inject the overlay twice into one document, and marks it only once injected", async () => {
    let fail = true;
    // The fake runs each probe in this test's global, as Chrome runs it in the tab's isolated world.
    c.scripting.executeScript = (async (inj: { files?: string[]; func?: () => unknown }) => {
      c.calls.push({ api: "scripting.executeScript", args: [inj] });
      if (inj.func) return [{ result: inj.func() }];
      if (fail) throw new Error("Frame with ID 0 was removed.");
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
    } finally {
      delete (globalThis as { claxOverlayLoaded?: boolean }).claxOverlayLoaded;
    }
  });

  it("reads a tab it cannot reach as having no overlay", async () => {
    c.scripting.executeScript = (async () => { throw new Error("Cannot access contents of the page."); }) as unknown as typeof c.scripting.executeScript;
    expect(await overlayPresent(env(), 7)).toBe(false);
  });
});
