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
