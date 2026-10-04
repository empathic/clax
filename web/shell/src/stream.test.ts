import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { STREAM_DOWN, connNoticeText } from "./conn-notice";
import { DEAD_MS, EventStream, HIDDEN_MS, type Link, type LinkMaker, NOTICE_MS, type StreamEvent, TOKEN_WAIT_MS, leaderLink } from "./stream";
import type { HubMsg, TabMsg } from "./stream-hub";
import { Net } from "./test/fake-net";

const A = "7q3k9mzx2b4t";
const B = "8r4m0nzy3c5v";

/** A link the test drives: what the page sent, and hub messages handed back. */
class FakeLink implements Link {
  msgs: TabMsg[] = [];
  closed = false;
  constructor(public on: (m: HubMsg) => void, public lost: () => void, public pinged = true) {}
  send(m: TabMsg) { this.msgs.push(m); }
  close() { this.closed = true; }
  get topics(): string[] { const t = this.msgs.filter(m => m.t === "topics").at(-1); return t && t.t === "topics" ? t.topics : []; }
}

let links: FakeLink[];
const maker: LinkMaker = async (on, lost) => { const l = new FakeLink(on, lost); links.push(l); return l; };
const page = (type: string, persisted: boolean) => { const e = new Event(type); Object.defineProperty(e, "persisted", { value: persisted }); dispatchEvent(e); };
let visibility: DocumentVisibilityState = "visible";
const setVisibility = (v: DocumentVisibilityState) => { visibility = v; document.dispatchEvent(new Event("visibilitychange")); };
const flush = () => vi.advanceTimersByTimeAsync(0);

let s: EventStream;
beforeEach(() => {
  vi.useFakeTimers();
  links = [];
  visibility = "visible";
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
  vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 403 })));
  s = new EventStream(window, maker);
});
afterEach(() => {
  s.close();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("EventStream", () => {
  it("holds one link for every watcher, sends the union of their topics once, and routes by topic", async () => {
    const gallery: StreamEvent[] = [];
    const artifact: StreamEvent[] = [];
    s.watch(["gallery"], e => gallery.push(e));
    s.watch([`artifact:${A}`, `presence:${A}`], e => artifact.push(e));
    await flush();
    expect(links).toHaveLength(1);
    expect(links[0].msgs.filter(m => m.t === "topics")).toEqual([{ t: "topics", topics: [`artifact:${A}`, "gallery", `presence:${A}`] }]);
    links[0].on({ t: "live", topics: [`artifact:${A}`, `presence:${A}`] });
    links[0].on({ t: "event", topic: `artifact:${A}`, name: "version", data: { topic: `artifact:${A}`, artifact_id: A, n: 2 } });
    links[0].on({ t: "event", topic: "gallery", name: "version", data: { topic: "gallery", artifact_id: B, n: 3 } });
    links[0].on({ t: "resync", topic: "gallery" });
    expect(artifact).toEqual([{ type: "ready" }, { type: "version", topic: `artifact:${A}`, artifact_id: A, n: 2 }]);
    expect(gallery).toEqual([{ type: "version", topic: "gallery", artifact_id: B, n: 3 }, { type: "resync", topic: "gallery" }]);
  });

  it("changes topics on the same link as views mount and unmount", async () => {
    const offA = s.watch([`artifact:${A}`], () => {});
    await flush();
    const offG = s.watch(["gallery"], () => {});
    offA();
    await flush();
    expect(links).toHaveLength(1);
    expect(links[0].topics).toEqual(["gallery"]);
    offG();
    await flush();
    expect(links[0].topics).toEqual([]);
    expect(links).toHaveLength(1);
  });

  it("leaves the hub after the page has been hidden a while, and joins again (with ready) when it shows", async () => {
    const seen: StreamEvent[] = [];
    s.watch(["gallery"], e => seen.push(e));
    await flush();
    setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(HIDDEN_MS - 100);
    expect(links[0].closed).toBe(false);
    // Leaving, not only dropping its topics: a leader tab hands the
    // connection to a tab that is shown, as a hidden tab may be frozen.
    await vi.advanceTimersByTimeAsync(200);
    expect(links[0].closed).toBe(true);
    setVisibility("visible");
    await flush();
    expect(links).toHaveLength(2);
    expect(links[1].topics).toEqual(["gallery"]);
    links[1].on({ t: "live", topics: ["gallery"] });
    expect(seen).toEqual([{ type: "ready" }]);
  });

  it("keeps its topics through a short hide", async () => {
    s.watch(["gallery"], () => {});
    await flush();
    setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(HIDDEN_MS / 2);
    setVisibility("visible");
    await vi.advanceTimersByTimeAsync(HIDDEN_MS);
    expect(links[0].msgs.filter(m => m.t === "topics")).toHaveLength(1);
  });

  it("shows the notice while the hub says the stream is down, and says when it is back", async () => {
    const seen: StreamEvent[] = [];
    s.watch(["gallery"], e => seen.push(e));
    await flush();
    links[0].on({ t: "status", up: false });
    expect(seen).toEqual([{ type: "stream_down" }]);
    expect(connNoticeText()).toBeNull();
    await vi.advanceTimersByTimeAsync(NOTICE_MS);
    expect(connNoticeText()).toBe(STREAM_DOWN);
    links[0].on({ t: "status", up: true });
    expect(seen).toEqual([{ type: "stream_down" }, { type: "stream_up" }]);
    expect(connNoticeText()).toBeNull();
  });

  it("leaves the hub as the page is hidden for good, and joins again from the back/forward cache", async () => {
    s.watch(["gallery"], () => {});
    await flush();
    page("pagehide", true);
    expect(links[0].closed).toBe(true);
    page("pageshow", true);
    await flush();
    expect(links).toHaveLength(2);
    expect(links[1].topics).toEqual(["gallery"]);
  });

  it("counts a silent hub dead, shows the notice, and joins a new one", async () => {
    const seen: StreamEvent[] = [];
    s.watch(["gallery"], e => seen.push(e));
    await flush();
    await vi.advanceTimersByTimeAsync(DEAD_MS + DEAD_MS / 3);
    expect(links[0].closed).toBe(true);
    expect(links).toHaveLength(2);
    expect(links[1].topics).toEqual(["gallery"]);
    expect(seen).toEqual([{ type: "stream_down" }]);
    await vi.advanceTimersByTimeAsync(NOTICE_MS);
    expect(connNoticeText()).toBe(STREAM_DOWN);
    links[1].on({ t: "live", topics: ["gallery"] });
    expect(connNoticeText()).toBeNull();
    expect(seen.at(-1)).toEqual({ type: "ready" });
  });

  it("joins a new hub at once when the link says the hub is gone", async () => {
    s.watch(["gallery"], () => {});
    await flush();
    links[0].lost();
    await flush();
    expect(links).toHaveLength(2);
    expect(links[1].topics).toEqual(["gallery"]);
  });
  it("asks for a new stream once it has a link, when the viewer changed before it had one", async () => {
    s.watch(["gallery"], () => {});
    // The viewer changes while the link is still being made: the hub it
    // joins may hold a stream opened for the old viewer.
    s.reconnect();
    await flush();
    expect(links[0].msgs.filter(m => m.t === "reconnect")).toHaveLength(1);
    s.reconnect();
    expect(links[0].msgs.filter(m => m.t === "reconnect")).toHaveLength(2);
  });

  it("answers the hub's pings, so a hub without Web Locks keeps it", async () => {
    s.watch(["gallery"], () => {});
    await flush();
    links[0].on({ t: "ping" });
    expect(links[0].msgs.filter(m => m.t === "ping")).toHaveLength(1);
  });

  it("backs off while hubs keep failing before saying anything", async () => {
    const failing: LinkMaker = async (on, lost) => {
      const l = new FakeLink(on, lost);
      links.push(l);
      setTimeout(() => l.lost(), 0);
      return l;
    };
    const f = new EventStream(window, failing);
    f.watch(["gallery"], () => {});
    await vi.advanceTimersByTimeAsync(2000);
    expect(links.length).toBeGreaterThan(1);
    expect(links.length).toBeLessThanOrEqual(5);
    f.close();
  });

  it("stops asking for the shared worker once it failed before answering", async () => {
    let made = 0;
    class BrokenWorker {
      port = { onmessage: null, postMessage() {}, start() {}, close() {}, addEventListener() {} };
      private onError: (() => void) | null = null;
      constructor() { made++; setTimeout(() => this.onError?.(), 0); }
      addEventListener(type: string, cb: () => void) { if (type === "error") this.onError = cb; }
    }
    vi.stubGlobal("SharedWorker", BrokenWorker);
    const w = new EventStream(window);
    w.watch(["gallery"], () => {});
    await vi.advanceTimersByTimeAsync(TOKEN_WAIT_MS + 5000);
    expect(made).toBe(1);
    w.close();
  });
});

/** Web Locks in one realm: a request waits until the name is free. */
class Locks {
  held = new Set<string>();
  queue = new Map<string, (() => void)[]>();
  request(name: string, cb: () => unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const run = () => {
        this.held.add(name);
        Promise.resolve().then(cb).then(resolve, reject).finally(() => {
          this.held.delete(name);
          this.queue.get(name)?.shift()?.();
        });
      };
      if (this.held.has(name)) this.queue.set(name, [...(this.queue.get(name) ?? []), run]);
      else run();
    });
  }
}

/** BroadcastChannel in one realm: delivered to every other channel of the name. */
class Channel {
  static all = new Set<Channel>();
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(public name: string) { Channel.all.add(this); }
  postMessage(m: unknown) {
    const data = structuredClone(m);
    for (const c of Channel.all) if (c !== this && c.name === this.name) queueMicrotask(() => c.onmessage?.({ data }));
  }
  close() { Channel.all.delete(this); }
}

describe("the leader fallback", () => {
  let net: Net;
  beforeEach(() => {
    net = new Net();
    Channel.all.clear();
    vi.stubGlobal("BroadcastChannel", Channel);
    vi.stubGlobal("fetch", net.fetch);
    Object.defineProperty(navigator, "locks", { configurable: true, value: new Locks() });
  });
  afterEach(() => { Object.defineProperty(navigator, "locks", { configurable: true, value: undefined }); });

  it("elects one tab to hold the connection for all, routes to the others, and hands over when it leaves", async () => {
    const heard: HubMsg[][] = [[], [], []];
    const tabs = await Promise.all(heard.map(h => leaderLink(m => h.push(m), () => {})));
    tabs[0].send({ t: "topics", topics: ["gallery"] });
    tabs[1].send({ t: "topics", topics: [`artifact:${A}`] });
    tabs[2].send({ t: "topics", topics: [`artifact:${A}`, "gallery"] });
    await flush();
    expect(net.conns).toHaveLength(1);
    net.ready(net.conns[0], "0123456789abcdef0123456789abcdef");
    await flush();
    net.event(net.conns[0], "0123456789abcdef0123456789abcdef", 1, `artifact:${A}`, "version", { artifact_id: A, n: 2 });
    await flush();
    const events = heard.map(h => h.filter(m => m.t === "event").length);
    expect(events).toEqual([0, 1, 1]);
    expect(heard.map(h => h.some(m => m.t === "live"))).toEqual([true, true, true]);
    // The leader leaves: the next in line opens the connection, and every
    // remaining tab's topics come with it.
    tabs[0].close();
    await flush();
    expect(net.open).toHaveLength(1);
    expect(net.conns).toHaveLength(2);
    net.ready(net.open[0], "fedcba9876543210fedcba9876543210");
    await flush();
    expect(new Set(net.posts.at(-1)!.body.subscribe)).toEqual(new Set([`artifact:${A}`, "gallery"]));
    tabs[1].close();
    tabs[2].close();
  });
});
