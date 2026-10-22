import { describe, expect, it } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { dbHandler } from "./db";
import type { CapEnv } from "./host";

const AID = "7q3k9mzx2b4t";

function setup() {
  const posted: ShellToBridge[] = [];
  const env = { aid: AID, token: "tok", post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv;
  return { h: dbHandler(env, null as never), posted };
}
const events = (posted: ShellToBridge[]) => posted.map(m => [(m as { topic: string }).topic, (m as { data: unknown }).data]);

describe("db's change reports in the shell", () => {
  it("reports nothing until the frame's bridge watches, then each change the stream announces", async () => {
    const { h, posted } = setup();
    h.onEvent!({ type: "doc", artifact_id: AID, path: "tasks/a", version: 1 });
    expect(posted).toEqual([]);
    expect(await h.call("watch", [])).toBeNull();
    h.onEvent!({ type: "doc", artifact_id: AID, path: "tasks/a", version: 2 });
    h.onEvent!({ type: "resync" });
    h.onEvent!({ type: "version", artifact_id: AID, n: 2 } as never);
    h.onEvent!({ type: "stream_down" });
    h.onEvent!({ type: "stream_up" });
    h.onEvent!({ type: "stream_down" });
    h.onEvent!({ type: "ready" });
    expect(posted.every(m => m.type === "clax:event" && (m as { ns: string }).ns === "db")).toBe(true);
    expect(events(posted)).toEqual([
      ["changed", { path: "tasks/a" }],
      ["changed", {}],
      ["changed", {}],
      ["stream", { up: false }],
      ["stream", { up: true }],
      ["stream", { up: false }],
      ["stream", { up: true }],
      ["changed", {}],
    ]);
  });

  it("stops at a new frame document until it watches again, and for good once disposed", async () => {
    const { h, posted } = setup();
    await h.call("watch", []);
    h.reset!();
    h.onEvent!({ type: "resync" });
    expect(posted).toEqual([]);
    await h.call("watch", []);
    h.dispose!();
    h.onEvent!({ type: "resync" });
    expect(posted).toEqual([]);
  });

  it("reads and writes nothing itself: the page's own origin does", async () => {
    const { h } = setup();
    for (const m of ["get", "set", "subscribe", "query"]) await expect(h.call(m, ["tasks/a"])).rejects.toMatchObject({ code: "capability_removed" });
  });
});
