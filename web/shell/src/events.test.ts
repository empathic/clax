import { describe, it, expect, vi, afterEach } from "vitest";
import { subscribe } from "./events";

class FakeES {
  static last: FakeES;
  listeners = new Map<string, (e: MessageEvent) => void>();
  closed = false;
  constructor(public url: string) { FakeES.last = this; }
  addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, fn); }
  close() { this.closed = true; }
  emit(t: string, data: unknown) { this.listeners.get(t)?.(new MessageEvent(t, { data: JSON.stringify(data) })); }
}

describe("subscribe", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("opens a filtered stream, forwards parsed events, and closes on unsubscribe", () => {
    vi.stubGlobal("EventSource", FakeES);
    const seen: unknown[] = [];
    const off = subscribe("7q3k9mzx2b4t", e => seen.push(e));
    expect(FakeES.last.url).toBe("/api/events?artifact=7q3k9mzx2b4t");
    FakeES.last.emit("version", { type: "version", artifact_id: "7q3k9mzx2b4t", n: 4 });
    FakeES.last.emit("artifact_deleted", { type: "artifact_deleted", artifact_id: "7q3k9mzx2b4t" });
    expect(seen).toEqual([{ type: "version", artifact_id: "7q3k9mzx2b4t", n: 4 }, { type: "artifact_deleted", artifact_id: "7q3k9mzx2b4t" }]);
    off();
    expect(FakeES.last.closed).toBe(true);
  });

  it("turns a resync event into a typed event carrying the dropped count", () => {
    vi.stubGlobal("EventSource", FakeES);
    const seen: unknown[] = [];
    subscribe("7q3k9mzx2b4t", e => seen.push(e));
    FakeES.last.emit("resync", { dropped: 7 });
    expect(seen).toEqual([{ type: "resync", dropped: 7 }]);
  });

  it("forwards the comment events", () => {
    vi.stubGlobal("EventSource", FakeES);
    const seen: unknown[] = [];
    subscribe("7q3k9mzx2b4t", e => seen.push(e));
    const fs = { type: "feedback_state", artifact_id: "7q3k9mzx2b4t", thread_id: "01J9", state: "sent", tier: "stop_hook", since: "2026-09-29T10:00:00.000Z", resends: 0, exhausted: false };
    const thread = { type: "thread", artifact_id: "7q3k9mzx2b4t", thread: { id: "01J9" } };
    const comment = { type: "comment", artifact_id: "7q3k9mzx2b4t", thread_id: "01J9", comment: { id: "c" } };
    const resolved = { type: "thread_resolved", artifact_id: "7q3k9mzx2b4t", thread_id: "01J9", resolved_by: "viewer:x", resolved_at: "t" };
    FakeES.last.emit("feedback_state", fs);
    FakeES.last.emit("thread", thread);
    FakeES.last.emit("comment", comment);
    FakeES.last.emit("thread_resolved", resolved);
    expect(seen).toEqual([fs, thread, comment, resolved]);
  });
});
