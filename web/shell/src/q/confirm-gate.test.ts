// The confirmations' feed (`confirm-live`) loads only in a local owner's
// browser: a hosted daemon answers every confirmation route 404
// (crates/clax-server/src/routes/confirmations.rs), and its owner's shell
// token is `cxo_`.
import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => { vi.doUnmock("./confirm-live"); vi.doUnmock("./shared"); vi.unstubAllGlobals(); vi.resetModules(); });

describe("the confirmations' feed", () => {
  for (const [who, token, loads] of [["a local owner's browser", "tok_local", true], ["a hosted owner's browser", "cxo_shell", false], ["anyone else's", null, false]] as const) {
    it(`${loads ? "loads" : "does not load"} in ${who} once the inbox goes live`, async () => {
      vi.resetModules();
      let follows = 0;
      let hear: ((e: { type: string }) => void) | undefined;
      vi.doMock("./confirm-live", () => ({ follow: () => { follows++; return () => {}; } }));
      const feed = { start() {}, stop() {}, listen: () => () => {}, onCount: () => () => {}, hear: (f: (e: { type: string }) => void) => { hear = f; return () => {}; }, latest: 0 };
      vi.doMock("./shared", () => ({ shared: () => ({ inbox: feed, questions: { start() {}, stop() {} } }) }));
      const tokens: string[] = [];
      vi.stubGlobal("fetch", vi.fn(async (u: string) => {
        if (u !== "/api/token") return new Response("{}");
        tokens.push(u);
        return token ? new Response(JSON.stringify({ token })) : new Response("{}", { status: 403 });
      }));
      const q = await import("./index");
      q.start({ onNotify: () => () => {} } as never);
      hear!({ type: "ready" });
      await vi.waitFor(() => expect(tokens).toHaveLength(1));
      await new Promise(r => setTimeout(r, 0));
      await vi.dynamicImportSettled();
      expect(follows).toBe(loads ? 1 : 0);
      q.stop();
    });
  }
});
