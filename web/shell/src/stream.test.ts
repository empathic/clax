import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { STREAM_DOWN, connNoticeText } from "./conn-notice";
import type { ArtifactEvent } from "./events";
import { CONNECT_MS, EventStream, NOTICE_MS } from "./stream";

class FakeES {
  static all: FakeES[] = [];
  static get open() { return FakeES.all.filter(e => !e.closed); }
  listeners = new Map<string, ((e: MessageEvent) => void)[]>();
  closed = false;
  constructor(public url: string) { FakeES.all.push(this); }
  addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, [...(this.listeners.get(t) ?? []), fn]); }
  close() { this.closed = true; }
  emit(t: string, data: unknown, id = "") {
    for (const fn of this.listeners.get(t) ?? []) fn(new MessageEvent(t, { data: JSON.stringify(data), lastEventId: id }));
  }
}

const A = "7q3k9mzx2b4t";
const B = "8r4m0nzy3c5v";
const page = (type: string, persisted: boolean) => { const e = new Event(type); Object.defineProperty(e, "persisted", { value: persisted }); dispatchEvent(e); };

let s: EventStream;
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("EventSource", FakeES);
  FakeES.all = [];
  s = new EventStream();
});
afterEach(() => {
  s.close();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("EventStream", () => {
  it("holds one stream for every watcher, carrying the union of their topics", () => {
    const gallery: ArtifactEvent[] = [];
    const artifact: ArtifactEvent[] = [];
    s.watch({ types: ["working", "version"] }, e => gallery.push(e));
    expect(FakeES.open.map(e => e.url)).toEqual(["/api/events?types=working,version"]);
    s.watch({ artifact: A }, e => artifact.push(e));
    // Another watcher needs more: the stream reopens, and there is still one.
    expect(FakeES.open.map(e => e.url)).toEqual(["/api/events"]);
    const es = FakeES.open[0];
    es.emit("ready", { resumed: false }, "e-1");
    es.emit("thread", { type: "thread", artifact_id: A, thread: {} }, "e-2");
    es.emit("version", { type: "version", artifact_id: B, n: 2 }, "e-3");
    // Each watcher hears its own topics, and `ready`.
    expect(gallery.map(e => e.type)).toEqual(["ready", "version"]);
    expect(artifact.map(e => e.type)).toEqual(["ready", "thread"]);
  });

  it("merges artifact watchers into one comma list", () => {
    const offA = s.watch({ artifact: A }, () => {});
    s.watch({ artifact: B }, () => {});
    expect(FakeES.open.map(e => e.url)).toEqual([`/api/events?artifact=${A},${B}`]);
    // A narrower set keeps the open stream.
    offA();
    expect(FakeES.all).toHaveLength(2);
    expect(FakeES.open).toHaveLength(1);
  });

  it("closes the stream when its last watcher goes", () => {
    const off1 = s.watch({ artifact: A }, () => {});
    const off2 = s.watch({ artifact: A }, () => {});
    expect(FakeES.all).toHaveLength(1);
    off1();
    expect(FakeES.open).toHaveLength(1);
    off2();
    expect(FakeES.open).toHaveLength(0);
    vi.advanceTimersByTime(60_000);
    expect(FakeES.all).toHaveLength(1);
  });

  it("gives a watcher that joins an open stream its own first ready", async () => {
    s.watch({ artifact: A }, () => {});
    FakeES.open[0].emit("ready", { resumed: false }, "e-5");
    const late: ArtifactEvent[] = [];
    s.watch({ artifact: A }, e => late.push(e));
    expect(FakeES.all).toHaveLength(1);
    await Promise.resolve();
    expect(late).toEqual([{ type: "ready" }]);
  });

  it("reopens a stream that failed with backoff, resuming after the last event, and refetches nothing when it resumed", () => {
    const seen: ArtifactEvent[] = [];
    s.watch({ artifact: A }, e => seen.push(e));
    FakeES.open[0].emit("ready", { resumed: false }, "ep-1");
    FakeES.open[0].emit("version", { type: "version", artifact_id: A, n: 2 }, "ep-2");
    FakeES.open[0].emit("error", {});
    expect(FakeES.open).toHaveLength(0);
    expect(seen.map(e => e.type)).toEqual(["ready", "version", "stream_down"]);
    vi.advanceTimersByTime(600);
    expect(FakeES.open.map(e => e.url)).toEqual([`/api/events?artifact=${A}&last_event_id=ep-2`]);
    FakeES.open[0].emit("ready", { resumed: true }, "ep-2");
    FakeES.open[0].emit("version", { type: "version", artifact_id: A, n: 3 }, "ep-3");
    expect(seen.map(e => e.type)).toEqual(["ready", "version", "stream_down", "version"]);
    // A reconnect the daemon could not resume says `ready`: refetch.
    FakeES.open[0].emit("error", {});
    vi.advanceTimersByTime(600);
    expect(FakeES.open[0].url).toContain("last_event_id=ep-3");
    FakeES.open[0].emit("ready", { resumed: false }, "other-9");
    expect(seen.map(e => e.type).slice(-2)).toEqual(["stream_down", "ready"]);
  });

  it("backs off further after each failure in a row, and starts over once up", () => {
    s.watch({ artifact: A }, () => {});
    const opens = () => FakeES.all.length;
    FakeES.all.at(-1)!.emit("error", {});
    vi.advanceTimersByTime(399);
    expect(opens()).toBe(1);
    vi.advanceTimersByTime(201);
    expect(opens()).toBe(2);
    FakeES.all.at(-1)!.emit("error", {});
    vi.advanceTimersByTime(799);
    expect(opens()).toBe(2);
    vi.advanceTimersByTime(401);
    expect(opens()).toBe(3);
    FakeES.all.at(-1)!.emit("ready", {});
    FakeES.all.at(-1)!.emit("error", {});
    vi.advanceTimersByTime(600);
    expect(opens()).toBe(4);
  });

  it("counts a stream that never says ready as stuck, and retries it", () => {
    const seen: ArtifactEvent[] = [];
    s.watch({ artifact: A }, e => seen.push(e));
    vi.advanceTimersByTime(CONNECT_MS - 1);
    expect(FakeES.all[0].closed).toBe(false);
    vi.advanceTimersByTime(1);
    expect(FakeES.all[0].closed).toBe(true);
    expect(seen).toEqual([{ type: "stream_down" }]);
    vi.advanceTimersByTime(600);
    expect(FakeES.open).toHaveLength(1);
  });

  it("shows the notice only once the stream has been down a while, and hides it when it is back", () => {
    s.watch({ artifact: A }, () => {});
    FakeES.open[0].emit("ready", {});
    FakeES.open[0].emit("error", {});
    expect(connNoticeText()).toBeNull();
    vi.advanceTimersByTime(NOTICE_MS);
    expect(connNoticeText()).toBe(STREAM_DOWN);
    expect(document.querySelector(".conn-notice")?.getAttribute("role")).toBe("status");
    FakeES.open.at(-1)!.emit("ready", {});
    expect(connNoticeText()).toBeNull();
  });

  it("tells watchers the stream is down once per outage", () => {
    const seen: ArtifactEvent[] = [];
    s.watch({ artifact: A }, e => seen.push(e));
    for (let i = 0; i < 3; i++) {
      FakeES.all.at(-1)!.emit("error", {});
      vi.advanceTimersByTime(30_000);
    }
    expect(seen.filter(e => e.type === "stream_down")).toHaveLength(1);
  });

  it("closes as the page is hidden and resumes when the back/forward cache restores it", () => {
    const seen: ArtifactEvent[] = [];
    s.watch({ types: ["version"] }, e => seen.push(e));
    FakeES.open[0].emit("ready", {}, "ep-7");
    page("pagehide", true);
    expect(FakeES.open).toHaveLength(0);
    // Hidden: no retry and no notice.
    vi.advanceTimersByTime(60_000);
    expect(FakeES.all).toHaveLength(1);
    expect(connNoticeText()).toBeNull();
    page("pageshow", true);
    expect(FakeES.open.map(e => e.url)).toEqual(["/api/events?types=version&last_event_id=ep-7"]);
    FakeES.open[0].emit("ready", { resumed: true }, "ep-7");
    expect(seen).toEqual([{ type: "ready" }]);
  });

  it("stays closed after a page hide that is a real unload", () => {
    s.watch({ artifact: A }, () => {});
    page("pagehide", false);
    expect(FakeES.open).toHaveLength(0);
    page("pageshow", false);
    vi.advanceTimersByTime(60_000);
    expect(FakeES.open).toHaveLength(0);
  });

  it("reconnects on request, resuming", () => {
    s.watch({ artifact: A }, () => {});
    FakeES.open[0].emit("ready", {}, "ep-4");
    s.reconnect();
    expect(FakeES.all[0].closed).toBe(true);
    expect(FakeES.open.map(e => e.url)).toEqual([`/api/events?artifact=${A}&last_event_id=ep-4`]);
  });

  it("never puts a token in its URL", () => {
    s.watch({ artifact: A }, () => {});
    expect(FakeES.all[0].url).not.toContain("token");
  });
});
