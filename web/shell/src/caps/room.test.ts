import { describe, expect, it } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import type { CapEnv } from "./host";
import { makeRoomHandler } from "./room";

function setup() {
  const posted: ShellToBridge[] = [];
  const fns = new Set<(v: unknown) => void>();
  const viewers = (fn: (v: unknown) => void) => { fns.add(fn); return () => { fns.delete(fn); }; };
  const env = { aid: "7q3k9mzx2b4t", post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv;
  return { h: makeRoomHandler(viewers)(env, null as never), posted, change: () => { for (const fn of [...fns]) fn({}); }, fns };
}

describe("room's viewer reports in the shell", () => {
  it("tells a watching frame each time its viewer changes, and nothing before", async () => {
    const { h, posted, change } = setup();
    change();
    expect(posted).toEqual([]);
    await h.call("watch", []);
    await h.call("watch", []);
    change();
    expect(posted).toEqual([{ type: "clax:event", ns: "room", topic: "viewer", data: null }]);
  });

  it("stops at a new document, when it leaves, and once disposed", async () => {
    for (const end of ["reset", "leave", "dispose"] as const) {
      const { h, posted, change, fns } = setup();
      await h.call("watch", []);
      h[end]!();
      change();
      expect([posted, fns.size]).toEqual([[], 0]);
    }
  });

  it("opens no socket and relays nothing else: the page's own origin holds the room", async () => {
    const { h } = setup();
    for (const m of ["connect", "emit", "join", "presence"]) await expect(h.call(m, [])).rejects.toMatchObject({ code: "capability_removed" });
  });
});
