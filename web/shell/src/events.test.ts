import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type StreamEvent, pageStream } from "./stream";
import { FakeWorker, workerWith } from "./test/fake-worker";
import { MOUNT_TIMEOUT_MS } from "./test/timeouts";
import { subscribeGallery } from "./working-events";

const A = "7q3k9mzx2b4t";

describe("the page stream", { timeout: MOUNT_TIMEOUT_MS }, () => {
  beforeEach(() => {
    FakeWorker.all = [];
    vi.stubGlobal("SharedWorker", FakeWorker);
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ token: "t" }))));
  });
  afterEach(() => { pageStream().close(); vi.unstubAllGlobals(); });

  it("joins the shared worker by name, says hello, and sends its topics", async () => {
    pageStream().watch([`artifact:${A}`, `presence:${A}`], () => {});
    const w = await workerWith(`artifact:${A}`);
    expect(FakeWorker.all).toHaveLength(1);
    expect(w.opts?.name).toBe("clax-stream");
    expect(String(w.url)).toMatch(/stream-worker/);
    expect(w.msgs[0]).toMatchObject({ t: "hello" });
    expect(w.topics).toEqual([`artifact:${A}`, `presence:${A}`]);
  });

  it("forwards each topic's events with their fields, ready when live, and resync", async () => {
    const seen: StreamEvent[] = [];
    pageStream().watch([`artifact:${A}`], e => seen.push(e));
    const w = await workerWith(`artifact:${A}`);
    w.live();
    w.emit(`artifact:${A}`, "feedback_state", { artifact_id: A, thread_id: "01J9", state: "sent" });
    w.emit(`artifact:${A}`, "doc", { artifact_id: A, path: "tasks/a", version: null });
    w.send({ t: "resync", topic: `artifact:${A}` });
    expect(seen).toEqual([
      { type: "ready" },
      { type: "feedback_state", topic: `artifact:${A}`, artifact_id: A, thread_id: "01J9", state: "sent" },
      { type: "doc", topic: `artifact:${A}`, artifact_id: A, path: "tasks/a", version: null },
      { type: "resync", topic: `artifact:${A}` },
    ]);
  });

  it("subscribeGallery watches the gallery topic, and its unwatch leaves the hub's topics", async () => {
    const seen: StreamEvent[] = [];
    const off = subscribeGallery(e => seen.push(e));
    const w = await workerWith("gallery");
    w.emit("gallery", "working", { artifact_id: A, working: [{ agent: "a_1111aaaa", harness: "claude", threads: 2, started_at: "s" }] });
    expect(seen).toEqual([{ type: "working", topic: "gallery", artifact_id: A, working: [{ agent: "a_1111aaaa", harness: "claude", threads: 2, started_at: "s" }] }]);
    off();
    await vi.waitFor(() => expect(w.topics).toEqual([]));
  });

  it("falls back to a hub of its own without shared workers, Web Locks or BroadcastChannel", async () => {
    vi.stubGlobal("SharedWorker", undefined);
    vi.stubGlobal("BroadcastChannel", undefined);
    const urls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (u: string) => { urls.push(String(u)); return new Promise<Response>(() => {}); }));
    pageStream().watch(["gallery"], () => {});
    await vi.waitFor(() => expect(urls).toContain("/api/stream"));
  });
});
