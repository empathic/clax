import { afterEach, describe, expect, it, vi } from "vitest";

// parts-url.ts reads the names the build compiles in; here they are a global.
const loadParts = async (built: Record<string, string>) => {
  vi.stubGlobal("__CLAX_PARTS__", built);
  vi.resetModules();
  return (await import("../src/parts-url")).loadParts("file:///clax-daemon/_clax/bridge/bridge.js?v=1");
};
const message = (p: Promise<unknown>) => p.then(() => "loaded", (e: unknown) => (e instanceof Error ? e.message : String(e)));

describe("the parts by URL", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("gives every part a loader, and one the build left out rejects naming it", async () => {
    const parts = await loadParts({ comment: "comment-abc.js", clip: "clip-abc.js", caps: "caps-abc.js", room: "room-abc.js", sample: "sample-abc.js", live: "live-abc.js" });
    expect(Object.keys(parts).sort()).toEqual(["caps", "clip", "comment", "live", "origin", "room", "sample", "test"]);
    await expect(message(parts.test())).resolves.toBe("no URL for the test part");
  });

  it("loads a part from beside the bridge script, and a retry under a query naming the attempt", async () => {
    const parts = await loadParts({ comment: "comment-abc.js", clip: "clip-abc.js", caps: "caps-abc.js", room: "room-abc.js", sample: "sample-abc.js", live: "live-abc.js", test: "test-abc.js" });
    // Nothing is there: the failed import names the file it asked for.
    await expect(message(parts.test())).resolves.toContain("/_clax/bridge/test-abc.js");
    await expect(message(parts.test(2))).resolves.toContain("/_clax/bridge/test-abc.js?retry=2");
  });
});
