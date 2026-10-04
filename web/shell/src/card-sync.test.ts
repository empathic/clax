import { afterEach, describe, expect, it, vi } from "vitest";
import type { Artifact, AttentionSummary } from "./api";
import { CardSync } from "./ui/card-sync";
import { WorkingFeed } from "./ui/working-feed.svelte";

const art = (id: string, v = 1): Artifact => ({ id, title: id, description: null, icon: null, updated_at: "x", current_version: v, pinned: false });
const mark = (open: string[]): AttentionSummary => ({ addressed: [], addressed_v: null, new_replies: [], open_in: open, seen: null });

/** A stream the test drives, as `EventSource` would deliver it. */
function stubStream() {
  const s = { emit: (_t: string, _d: unknown) => {} };
  vi.stubGlobal("EventSource", class {
    listeners = new Map<string, (e: MessageEvent) => void>();
    constructor() { s.emit = (t, d) => this.listeners.get(t)?.(new MessageEvent(t, { data: JSON.stringify(d) })); }
    addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, fn); }
    close() {}
  });
  return s;
}

/** The daemon as the gallery sees it: `answer` maps a URL to its body; each
 * answer waits for `gate` when one is set for that URL. */
function stubApi(answer: (url: string) => unknown, gate: Record<string, Promise<void>> = {}) {
  const urls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    urls.push(url);
    await gate[url];
    return new Response(JSON.stringify(answer(url)));
  }));
  return urls;
}

function gallery(list: Artifact[], att: Record<string, AttentionSummary>) {
  const g = { list, att };
  const feed = new WorkingFeed();
  const sync = new CardSync({ list: () => g.list, att: () => g.att, setList: l => { g.list = l; }, setAtt: a => { g.att = a; } }, feed);
  return { g, feed, sync };
}
const settle = () => new Promise(r => setTimeout(r, 0));

describe("CardSync", () => {
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("an event naming an artifact refetches that card and its attention alone, and updates it in place", async () => {
    const stream = stubStream();
    const urls = stubApi(url => url.startsWith("/api/artifacts?artifact=b") ? { artifacts: [art("b", 2)] }
      : url === "/api/viewers/me/attention?artifact=b" ? { artifacts: { b: mark(["t2"]) } } : { artifacts: [] });
    const { g, sync } = gallery([art("a"), art("b"), art("c")], { a: mark(["t1"]) });
    sync.start();
    stream.emit("version", { type: "version", artifact_id: "b", n: 2 });
    stream.emit("thread", { type: "thread", artifact_id: "b", thread: {} });
    await vi.waitFor(() => expect(g.list[1].current_version).toBe(2));
    await settle();
    expect(urls).toEqual(["/api/artifacts?artifact=b", "/api/viewers/me/attention?artifact=b"]);
    expect(g.list.map(a => [a.id, a.current_version])).toEqual([["a", 1], ["b", 2], ["c", 1]]);
    expect(g.att).toEqual({ a: mark(["t1"]), b: mark(["t2"]) });
    sync.stop();
  });

  it("a card whose artifact is gone leaves, with its attention", async () => {
    stubStream();
    stubApi(url => url.startsWith("/api/artifacts") ? { artifacts: [] } : { artifacts: {} });
    const { g, sync } = gallery([art("a"), art("b")], { a: mark(["t1"]), b: mark(["t2"]) });
    await sync.one("a");
    expect(g.list.map(a => a.id)).toEqual(["b"]);
    expect(g.att).toEqual({ b: mark(["t2"]) });
  });

  it("an answer started before a newer one never overwrites it", async () => {
    stubStream();
    let open!: () => void;
    const gate = { "/api/artifacts": new Promise<void>(r => { open = r; }) };
    stubApi(url => url === "/api/artifacts" ? { artifacts: [art("a", 1), art("b", 1)] }
      : url === "/api/artifacts?artifact=a" ? { artifacts: [art("a", 3)] }
      : url.includes("?artifact=a") ? { artifacts: { a: mark(["new"]) } } : { artifacts: { a: mark(["old"]) } }, gate);
    const { g, sync } = gallery([art("a"), art("b")], {});
    const full = sync.full();
    await sync.one("a");
    expect(g.list[0].current_version).toBe(3);
    open();
    await full;
    expect(g.list.map(a => [a.id, a.current_version])).toEqual([["a", 3], ["b", 1]]);
    expect(g.att).toEqual({ a: mark(["new"]) });
  });

  it("the stream's ready and resync refetch everything; a failed refetch keeps the cards", async () => {
    const stream = stubStream();
    let fail = false;
    const urls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      urls.push(url);
      if (fail) throw new Error("network");
      return new Response(JSON.stringify(url === "/api/artifacts" ? { artifacts: [art("a", 2)] } : { artifacts: { a: mark([]) } }));
    }));
    const { g, sync } = gallery([art("a")], { a: mark(["t"]) });
    sync.start();
    stream.emit("ready", {});
    await vi.waitFor(() => expect(g.list[0].current_version).toBe(2));
    expect(urls).toEqual(["/api/artifacts", "/api/viewers/me/attention"]);
    fail = true;
    stream.emit("resync", { dropped: 1 });
    await vi.waitFor(() => expect(urls.length).toBe(4));
    await settle();
    expect(g.list[0].current_version).toBe(2);
    expect(g.att).toEqual({ a: mark([]) });
    sync.stop();
  });

  it("refetches everything every minute while the page is visible, not while it is hidden", async () => {
    stubStream();
    vi.useFakeTimers();
    const urls = stubApi(url => url === "/api/artifacts" ? { artifacts: [] } : { artifacts: {} });
    const { sync } = gallery([], {});
    sync.start();
    const hidden = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(urls).toEqual([]);
    hidden.mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    expect(urls).toEqual(["/api/artifacts", "/api/viewers/me/attention"]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(urls.length).toBe(4);
    sync.stop();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(urls.length).toBe(4);
  });
});
