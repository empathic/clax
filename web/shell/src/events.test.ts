import { describe, it, expect, vi } from "vitest";
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
});
