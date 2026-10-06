import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { ask, enabled, forget, injectOverlay, originOf, remember, scriptId, type OriginsEnv } from "./origins";

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

  it("gives each origin its own script ID", () => {
    expect(scriptId("http://localhost:5173")).not.toBe(scriptId("http://localhost:5174"));
    expect(scriptId("http://localhost:5173")).toMatch(/^clax-loader-[0-9a-f]+$/);
  });

  it("injects the overlay into the top frame only", async () => {
    await injectOverlay(env(), 7);
    expect(c.calls.find(x => x.api === "scripting.executeScript")!.args[0]).toEqual({ target: { tabId: 7, allFrames: false }, files: ["overlay.js"] });
  });
});
