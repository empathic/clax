import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { ask, enabled, forget, injectOverlay, originOf, remember, sameOrigin, scriptId, type OriginsEnv } from "./origins";

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

  it("does not inject the overlay twice into one document", async () => {
    let loaded = false;
    c.scripting.executeScript = (async (inj: { files?: string[]; func?: () => boolean }) => {
      c.calls.push({ api: "scripting.executeScript", args: [inj] });
      if (inj.func) { const had = loaded; loaded = true; return [{ result: had }]; }
      return [{ result: undefined }];
    }) as unknown as typeof c.scripting.executeScript;
    await injectOverlay(env(), 7);
    await injectOverlay(env(), 7);
    expect(c.calls.filter(x => x.api === "scripting.executeScript" && (x.args[0] as { files?: string[] }).files)).toHaveLength(1);
  });
});
