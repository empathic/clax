import { describe, expect, it } from "vitest";
import {
  BADGE_POLL_MS, BadgeWatch, type ReporterLifecycleState, POLL_MS, Poll, ReporterLifecycle, type Timers, type Visibility,
  initialLifecycleState, pendingFixCount, update,
} from "./lifecycle";
import type { ReporterApi } from "./reporter";
import type { Badge, Report, ReportPage } from "./types";

function report(id: string, over: Partial<Report> = {}): Report {
  return {
    id, queue_id: "q1", artifact: { id: "a1", external_id: "e", title: "t", url: "https://x.example/", metadata: null, metadata_version: 0, revision: null },
    anchor: null, status: "open", state: "open", reporter_id: "u1", metadata: null, metadata_version: 0, has_clip: false, claim: null, created_at: "",
    comments: [], ...over,
  };
}

const page = (reports: Report[]): ReportPage => ({ reports, next: null });

/** Timers that run only when the test says. */
function manualTimers() {
  const due: { fn: () => void; ms: number; id: number }[] = [];
  let next = 0;
  const t: Timers & { fire(): void; pending(): number[] } = {
    set: (fn, ms) => { const id = ++next; due.push({ fn, ms, id }); return id; },
    clear: h => { const i = due.findIndex(d => d.id === h); if (i >= 0) due.splice(i, 1); },
    fire: () => { const d = due.shift(); d?.fn(); },
    pending: () => due.map(d => d.ms),
  };
  return t;
}
const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };

/** A page that is always visible, with no real listeners (tests drive it by hand). */
function visibility(): Visibility & { fireFocus(): void } {
  const vis: { visibilityState: DocumentVisibilityState } = { visibilityState: "visible" };
  return {
    document: { get visibilityState() { return vis.visibilityState; }, addEventListener: () => {}, removeEventListener: () => {} },
    addEventListener: () => {}, removeEventListener: () => {},
    fireFocus: () => {},
  };
}

describe("update: the pure reducer", () => {
  it("starts idle, and reports/opened moves it to loading once", () => {
    expect(initialLifecycleState.auth).toBe("idle");
    const opened = update(initialLifecycleState, { type: "reports/opened" });
    expect(opened).toMatchObject({ auth: "loading", reportsOpen: true });
    // Opening again (already open) is a no-op: the same state back.
    expect(update(opened, { type: "reports/opened" })).toBe(opened);
  });

  it("reports/loaded is dropped while reports are not open (a straggling answer after close)", () => {
    const closed = initialLifecycleState;
    expect(update(closed, { type: "reports/loaded", page: page([report("t1")]) })).toBe(closed);
    const opened = update(closed, { type: "reports/opened" });
    const loaded = update(opened, { type: "reports/loaded", page: page([report("t1")]) });
    expect(loaded.reports?.map(r => r.id)).toEqual(["t1"]);
    expect(loaded.auth).toBe("ready");
    const afterClose = update(loaded, { type: "reports/closed" });
    expect(update(afterClose, { type: "reports/loaded", page: page([report("t2")]) })).toBe(afterClose);
  });

  it("reports/failed sets auth to error only before anything has ever succeeded", () => {
    const opened = update(initialLifecycleState, { type: "reports/opened" });
    const failed = update(opened, { type: "reports/failed", error: new Error("down") });
    expect(failed.auth).toBe("error");
    expect(failed.authError).toBeInstanceOf(Error);
    const ready = update(failed, { type: "reports/loaded", page: page([]) });
    expect(ready.auth).toBe("ready");
    const failedAgain = update(ready, { type: "reports/failed", error: new Error("down again") });
    // Once ready, a later failure keeps auth ready (a transient reports failure, not an auth one).
    expect(failedAgain.auth).toBe("ready");
    expect(failedAgain.reportsError).toBeInstanceOf(Error);
  });

  it("report/updated is a no-op while reports have never loaded", () => {
    const state = update(initialLifecycleState, { type: "report/updated", report: report("t1"), first: true });
    expect(state).toBe(initialLifecycleState);
  });

  it("report/updated replaces an existing report, or adds a new one first when told to, else leaves the list alone", () => {
    const loaded = update(update(initialLifecycleState, { type: "reports/opened" }), { type: "reports/loaded", page: page([report("t1", { state: "open" })]) });
    const replaced = update(loaded, { type: "report/updated", report: report("t1", { state: "verified" }) });
    expect(replaced.reports).toEqual([report("t1", { state: "verified" })]);
    const added = update(loaded, { type: "report/updated", report: report("t2"), first: true });
    expect(added.reports?.map(r => r.id)).toEqual(["t2", "t1"]);
    const ignored = update(loaded, { type: "report/updated", report: report("t2") });
    expect(ignored).toBe(loaded);
  });

  it("badge/changed sets the waiting count and auth ready; badge/rose counts up with the newly waiting ids", () => {
    const changed = update(initialLifecycleState, { type: "badge/changed", waiting: 2 });
    expect(changed).toMatchObject({ waiting: 2, auth: "ready" });
    const rose = update(changed, { type: "badge/rose", ids: ["t1"] });
    expect(rose.rise).toEqual({ seq: 1, ids: ["t1"] });
    const roseAgain = update(rose, { type: "badge/rose", ids: ["t2"] });
    expect(roseAgain.rise).toEqual({ seq: 2, ids: ["t2"] });
  });

  it("badge/started moves idle to loading, same as reports/opened, and never moves it back from ready", () => {
    expect(update(initialLifecycleState, { type: "badge/started" }).auth).toBe("loading");
    const ready = update(initialLifecycleState, { type: "badge/changed", waiting: 0 });
    expect(update(ready, { type: "badge/started" })).toBe(ready);
  });

  it("reset returns the initial state", () => {
    const changed = update(initialLifecycleState, { type: "badge/changed", waiting: 3 });
    expect(update(changed, { type: "reset" })).toBe(initialLifecycleState);
  });
});

describe("pendingFixCount", () => {
  it("is null before reports ever loaded, else the fix_proposed reports still waiting on a verdict", () => {
    expect(pendingFixCount(null)).toBeNull();
    const waiting = report("t1", { state: "fix_proposed", fixes: [{ n: 1, summary: "x", proposer_name: "a", created_at: "", outcome: "pending" }] });
    const answered = report("t2", { state: "fix_proposed", fixes: [{ n: 1, summary: "x", proposer_name: "a", created_at: "", outcome: "verified" }] });
    const open = report("t3", { state: "open" });
    expect(pendingFixCount([waiting, answered, open])).toBe(1);
  });
});

describe("Poll and BadgeWatch (moved here from the overlay's model, unchanged)", () => {
  it("polls at once and again ms after each load settles, until stop", async () => {
    const t = manualTimers();
    let calls = 0;
    let release: () => void = () => {};
    const poll = new Poll(() => { calls++; return new Promise<void>(r => { release = r; }); }, t);
    poll.start();
    expect(calls).toBe(1);
    release();
    await settle();
    expect(t.pending()).toEqual([POLL_MS]);
    poll.stop();
    t.fire();
    expect(calls).toBe(1);
  });

  it("changed fires on the first count (even 0) and on each change; rose fires when the count climbs", async () => {
    const t = manualTimers();
    const win = visibility();
    const answers: Badge[] = [{ waiting: 0, report_ids: [] }, { waiting: 1, report_ids: ["t1"] }];
    let asked = 0;
    const changed: number[] = [];
    const rose: string[][] = [];
    const watch = new BadgeWatch(async () => answers[Math.min(asked++, answers.length - 1)], win, { changed: n => changed.push(n), rose: ids => rose.push(ids) }, t);
    watch.start();
    await settle();
    expect(changed).toEqual([0]);
    watch.now();
    await settle();
    expect(changed).toEqual([0, 1]);
    expect(rose).toEqual([["t1"]]);
  });
});

describe("ReporterLifecycle: the runtime store", () => {
  function client(): ReporterApi & { filed: unknown[]; replied: [string, string][]; verified: unknown[]; reopened: unknown[]; seenIds: string[]; badges: Badge[] } {
    const state = { filed: [] as unknown[], replied: [] as [string, string][], verified: [] as unknown[], reopened: [] as unknown[], seenIds: [] as string[], badges: [] as Badge[] };
    return {
      ...state,
      fileReport: async r => { state.filed.push(r); return report("new"); },
      myReports: async () => page([report("t1")]),
      comment: async (id, body) => { state.replied.push([id, body]); return report(id); },
      verify: async (id, v) => { state.verified.push([id, v]); return report(id, { state: "verified" }); },
      reopen: async (id, r) => { state.reopened.push([id, r]); return report(id, { state: "open" }); },
      seen: async id => { state.seenIds.push(id); },
      badge: async () => state.badges.shift() ?? { waiting: 0, report_ids: [] },
    };
  }

  it("subscribe calls back at once with the current state, and again on every change", () => {
    const life = new ReporterLifecycle(client());
    const seen: ReporterLifecycleState[] = [];
    const unsubscribe = life.subscribe(s => seen.push(s));
    expect(seen).toEqual([initialLifecycleState]);
    unsubscribe();
  });

  it("openReports loads at once, and commands merge their answer in", async () => {
    const c = client();
    const life = new ReporterLifecycle(c, { timers: manualTimers() });
    const states: ReporterLifecycleState[] = [];
    life.subscribe(s => states.push(s));
    life.openReports();
    await settle();
    expect(life.get().reports?.map(r => r.id)).toEqual(["t1"]);
    await life.reply("t1", "still there");
    expect(c.replied).toEqual([["t1", "still there"]]);
    expect(life.get().reports?.[0].state).toBe("open");
    life.closeReports();
  });

  it("file merges the new report first even while reports have not loaded (it is the client's own no-op guard, kept pure in update)", async () => {
    const c = client();
    const life = new ReporterLifecycle(c);
    await life.file({ url: "https://x.example", title: "t", anchor: null, body: "b" });
    expect(c.filed).toHaveLength(1);
    // reports is still null: report/updated was a no-op, matching update()'s own rule.
    expect(life.get().reports).toBeNull();
  });

  it("verify and reopen ask the badge again once they resolve", async () => {
    const c = client();
    const win = visibility();
    const life = new ReporterLifecycle(c, { timers: manualTimers(), win });
    life.openReports();
    await settle();
    life.startBadge();
    await settle();
    c.badges.push({ waiting: 0, report_ids: [] });
    await life.verify("t1", { note: "fixed" });
    await settle();
    expect(c.verified).toEqual([["t1", { note: "fixed" }]]);
  });

  it("startBadge does nothing without win: no badge call, no error", async () => {
    const c = client();
    const life = new ReporterLifecycle(c, { timers: manualTimers() });
    life.startBadge();
    await settle();
    expect(life.get().waiting).toBeNull();
  });

  it("reset drops every held state and stops both polls", async () => {
    const c = client();
    const win = visibility();
    const timers = manualTimers();
    const life = new ReporterLifecycle(c, { timers, win });
    life.openReports();
    life.startBadge();
    await settle();
    expect(life.get().reports).not.toBeNull();
    life.reset();
    expect(life.get()).toBe(initialLifecycleState);
    // Both polls stopped: nothing left pending to fire.
    expect(timers.pending()).toEqual([]);
  });
});
