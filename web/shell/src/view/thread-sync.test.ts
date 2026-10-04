import { describe, expect, it } from "vitest";
import type { Thread } from "../threads";
import { ThreadSync } from "./thread-sync";

const t = (id: string) => ({ id }) as Thread;

describe("ThreadSync", () => {
  it("replays changes made while a load was in flight on top of its answer", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const done = sync.begin();
    sync.change(ts => [...ts, t("event")]);
    done([t("loaded")]);
    expect(threads.map(x => x.id)).toEqual(["loaded", "event"]);
  });
  it("applies only the latest load's answer, and keeps nothing once it answered or failed", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const older = sync.begin();
    const newer = sync.begin();
    older([t("old")]);
    expect(threads).toEqual([]);
    newer(undefined);
    sync.change(ts => [...ts, t("later")]);
    expect(threads.map(x => x.id)).toEqual(["later"]);
    const again = sync.begin();
    again([t("fresh")]);
    expect(threads.map(x => x.id)).toEqual(["fresh"]);
  });
});

describe("ThreadSync ordering", () => {
  const th = (id: string, status: "open" | "resolved" = "open") => ({ id, status }) as Thread;
  const upsertT = (x: Thread) => (ts: Thread[]) => { const i = ts.findIndex(y => y.id === x.id); return i < 0 ? [...ts, x] : ts.map((y, k) => (k === i ? x : y)); };

  it("holds a thread once when a load's pages repeat it", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    sync.begin()([th("a"), th("b"), th("a", "resolved")]);
    expect(threads).toEqual([th("a", "resolved"), th("b")]);
  });

  it("a thread's answer that predates a later delta does not undo it", () => {
    let threads: Thread[] = [th("a")];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const fetched = sync.beginThread("a");
    sync.change(upsertT(th("a", "resolved")));
    fetched(th("a", "open"));
    expect(threads).toEqual([th("a", "resolved")]);
  });

  it("a delta, a list load and a thread load in any order leave one card per thread, newest state", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    // The view goes live: the list load starts; a delta adds thread a.
    const list = sync.begin();
    sync.change(upsertT(th("a")));
    // A delta that does not add up: thread a is fetched whole.
    const one = sync.beginThread("a");
    sync.change(upsertT(th("a", "resolved")));
    // The thread answers before the list, from before the resolve.
    one(th("a", "open"));
    expect(threads).toEqual([th("a", "resolved")]);
    // The list answers last, from before everything.
    list([th("b")]);
    expect(threads.map(x => x.id).sort()).toEqual(["a", "b"]);
    expect(threads.find(x => x.id === "a")!.status).toBe("resolved");
  });

  it("drops a thread's answer that a newer list answer already covers", () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const one = sync.beginThread("a");
    const list = sync.begin();
    list([th("a", "resolved")]);
    one(th("a", "open"));
    expect(threads).toEqual([th("a", "resolved")]);
  });

  it("a removal after a thread's load began stays removed", () => {
    let threads: Thread[] = [th("a")];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const one = sync.beginThread("a");
    sync.change(ts => ts.filter(x => x.id !== "a"));
    one(th("a"));
    expect(threads).toEqual([]);
  });
});
