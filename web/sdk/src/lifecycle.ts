// The reporter's headless lifecycle (Bobby, 2026-11-06: "the lifecycle
// could and should be separate from the UI — think Elm or Svelte"): loading,
// the reporter's reports and their states, the count waiting on a verdict or
// an unread reply, and the commands that change them (file, reply, verify,
// reopen, refresh), apart from any UI. No DOM and no Svelte import here: a
// host's own view (or the overlay's, `overlay/src/Overlay.svelte`, which
// consumes this rather than owning it) reads the state through `subscribe`,
// a Svelte store (`subscribe(fn) => unsubscribe`, called at once with the
// current state) — `writable`, `derived` and `get` from `svelte/store` all
// work on a `ReporterLifecycle` as they do on any store of Svelte's own.
//
// `update` is the pure half (an Elm-style model and messages): given a
// state and a message, the next state, with no network, clock or DOM of
// its own, so it is tested as plain data in and out (lifecycle.test.ts).
// `ReporterLifecycle` is the runtime: it owns a `Poll` (the reporter's own
// reports, run only while `openReports()` is on, same cadence as "Your
// reports" always had, `POLL_MS`) and a `BadgeWatch` (the waiting count,
// run only while `startBadge()` is on, while the page is visible,
// `BADGE_POLL_MS`), turns what they see into messages, and runs `update`
// on each; its own methods (`file`, `reply`, `verify`, `reopen`, `seen`,
// `refresh`) are the lifecycle's commands, each a call to the `ReporterApi`
// (`ReporterClient`, or a fake) followed by the message the call's answer
// makes.
import type { Badge, Report, ReportPage } from "./types";
import type { NewReport, Reopening, ReporterApi, Verification } from "./reporter";

/** Timers the polls run on; injected in tests. */
export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}
export const realTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: h => clearTimeout(h as ReturnType<typeof setTimeout>),
};

/** How often "Your reports" asks for new replies while it is open. */
export const POLL_MS = 15_000;
/** How often the badge is asked for while the page is visible. */
export const BADGE_POLL_MS = 60_000;

/** Calls `load` at once and then `ms` after each call settles, until
 * `stop`: never two at once, and none after `stop`. A failed load is
 * `onError`'s and the poll keeps going. */
export class Poll {
  private timer: unknown = null;
  private live = false;

  constructor(private readonly load: () => Promise<void>, private readonly timers: Timers = realTimers, private readonly ms = POLL_MS, private readonly onError: (e: unknown) => void = () => {}) {}

  start(): void {
    if (this.live) return;
    this.live = true;
    void this.tick();
  }

  /** Loads now (a reply was just sent), and restarts the wait. */
  now(): void {
    if (!this.live) return;
    this.timers.clear(this.timer);
    this.timer = null;
    void this.tick();
  }

  stop(): void {
    this.live = false;
    this.timers.clear(this.timer);
    this.timer = null;
  }

  private running = false;
  private again = false;

  private async tick(): Promise<void> {
    if (this.running) { this.again = true; return; }
    this.running = true;
    try {
      await this.load();
    } catch (e) {
      if (this.live) this.onError(e);
    } finally {
      this.running = false;
    }
    if (!this.live) return;
    if (this.again) { this.again = false; void this.tick(); return; }
    this.timer = this.timers.set(() => { this.timer = null; void this.tick(); }, this.ms);
  }
}

/** What the badge poll watches: whether the page is visible, and its
 * `focus` and `visibilitychange`. */
export interface Visibility {
  document: { visibilityState: DocumentVisibilityState; addEventListener(t: "visibilitychange", fn: () => void): void; removeEventListener(t: "visibilitychange", fn: () => void): void };
  addEventListener(t: "focus", fn: () => void): void;
  removeEventListener(t: "focus", fn: () => void): void;
}

/** What the badge poll tells: the count when it changed (the first count
 * too), and `rose` with the reports newly waiting when it rose since the
 * last count. */
export interface BadgeHooks {
  changed(n: number): void;
  rose(newly: string[]): void;
}

/** The badge, asked for every `BADGE_POLL_MS` while the page is visible,
 * at once when it becomes visible or gets focus, and on `now`; a failed ask
 * is dropped and the next one tried. */
export class BadgeWatch {
  private last: Badge | null = null;
  private readonly poll: Poll;
  private live = false;

  constructor(load: () => Promise<Badge>, private readonly win: Visibility, private readonly hooks: BadgeHooks, timers: Timers = realTimers, ms = BADGE_POLL_MS) {
    this.poll = new Poll(async () => {
      const b = await load();
      if (!this.live) return;
      this.seen(b);
      if (this.win.document.visibilityState !== "visible") this.poll.stop();
    }, timers, ms);
  }

  /** The last count, or null before the first. */
  get count(): number | null {
    return this.last?.waiting ?? null;
  }

  start(): void {
    if (this.live) return;
    this.live = true;
    this.win.document.addEventListener("visibilitychange", this.wake);
    this.win.addEventListener("focus", this.wake);
    if (this.win.document.visibilityState === "visible") this.poll.start();
  }

  /** Asks now (the reporter just read or gave a verdict on a report). */
  now(): void {
    if (!this.live) return;
    this.poll.stop();
    this.poll.start();
  }

  stop(): void {
    this.live = false;
    this.poll.stop();
    this.win.document.removeEventListener("visibilitychange", this.wake);
    this.win.removeEventListener("focus", this.wake);
  }

  private readonly wake = () => {
    if (this.win.document.visibilityState === "visible") this.now();
    else this.poll.stop();
  };

  private seen(b: Badge): void {
    const before = this.last;
    this.last = b;
    if (before?.waiting === b.waiting) return;
    this.hooks.changed(b.waiting);
    if (before && b.waiting > before.waiting) this.hooks.rose(b.report_ids.filter(id => !before.report_ids.includes(id)));
  }
}

// ---- The pure lifecycle: an Elm-style model, messages and update ----

/** Whether reports or the badge have reached Clax: `"idle"` before either
 * starts, `"loading"` until the first answer (success or failure),
 * `"ready"` once one has ever succeeded, `"error"` (with `authError` set)
 * when the latest attempt failed and none before it ever did. This is
 * never a window into the reporter's own key or token: `ReporterClient`
 * keeps those itself (a mint failure there surfaces here only once it
 * gives up and a call fails outright). */
export type AuthStatus = "idle" | "loading" | "ready" | "error";

/** The reporter lifecycle's state: nothing of the DOM, nothing of Svelte,
 * every field plain data so two states compare and print like any other
 * value. */
export interface ReporterLifecycleState {
  auth: AuthStatus;
  authError: unknown;
  /** The reporter's reports, as "Your reports" last loaded them; `null`
   * until the first load. A `report/updated` message is dropped while
   * this is `null` (nothing to update), as the overlay always did. */
  reports: Report[] | null;
  reportsError: unknown;
  /** Whether "Your reports" is open: a `reports/loaded` or
   * `reports/failed` message that arrives after `reports/closed` changes
   * nothing (the overlay's own guard against a straggling answer). */
  reportsOpen: boolean;
  /** `Badge.waiting` from the last ask: a report of this reporter's that
   * waits on them, a fix proposed or a reply they have not read — not
   * only `fix_proposed` reports (`pendingFixCount` below counts those
   * alone, from `reports`, with no poll of its own). `null` before the
   * first ask. */
  waiting: number | null;
  /** The badge's count rising since the ask before it: a new `seq` each
   * time (never 0) with the reports newly waiting, for a host to act on
   * once (the overlay's pulse, or a host's own); `null` before the first
   * rise. */
  rise: { seq: number; ids: string[] } | null;
}

/** The state before anything has loaded. */
export const initialLifecycleState: ReporterLifecycleState = {
  auth: "idle", authError: null, reports: null, reportsError: null, reportsOpen: false, waiting: null, rise: null,
};

/** What changes the lifecycle's state: an answer from Clax (through
 * `ReporterLifecycle`'s `Poll` and `BadgeWatch`), a command's report, or
 * "Your reports" opening or closing. */
export type ReporterLifecycleMsg =
  | { type: "reports/opened" }
  | { type: "reports/closed" }
  | { type: "reports/loaded"; page: ReportPage }
  | { type: "reports/failed"; error: unknown }
  | { type: "report/updated"; report: Report; first?: boolean }
  | { type: "badge/started" }
  | { type: "badge/changed"; waiting: number }
  | { type: "badge/rose"; ids: string[] }
  | { type: "reset" };

/** `state` with `report` put in place of its old self, or first when
 * `first` and new; a no-op while `state.reports` is `null` (nothing
 * loaded to put it in). */
function updated(reports: Report[], report: Report, first: boolean): Report[] {
  return reports.some(r => r.id === report.id) ? reports.map(r => (r.id === report.id ? report : r)) : first ? [report, ...reports] : reports;
}

/** The pure reducer: `state` and a message in, the next state out. Every
 * branch is total and returns `state` itself (not a copy) when nothing
 * changes, so a caller can skip notifying on no-op messages with `===`. */
export function update(state: ReporterLifecycleState, msg: ReporterLifecycleMsg): ReporterLifecycleState {
  switch (msg.type) {
    case "reports/opened":
      return state.reportsOpen ? state : { ...state, reportsOpen: true, auth: state.auth === "idle" ? "loading" : state.auth };
    case "reports/closed":
      return state.reportsOpen ? { ...state, reportsOpen: false } : state;
    case "reports/loaded":
      if (!state.reportsOpen) return state;
      return { ...state, reports: msg.page.reports, reportsError: null, auth: "ready" };
    case "reports/failed":
      if (!state.reportsOpen) return state;
      return state.auth === "ready"
        ? { ...state, reportsError: msg.error }
        : { ...state, reportsError: msg.error, auth: "error", authError: msg.error };
    case "report/updated": {
      if (state.reports === null) return state;
      const reports = updated(state.reports, msg.report, msg.first ?? false);
      return reports === state.reports ? state : { ...state, reports };
    }
    case "badge/started":
      return state.auth === "idle" ? { ...state, auth: "loading" } : state;
    case "badge/changed":
      return state.waiting === msg.waiting && state.auth === "ready" ? state : { ...state, waiting: msg.waiting, auth: "ready" };
    case "badge/rose":
      return { ...state, rise: { seq: (state.rise?.seq ?? 0) + 1, ids: msg.ids } };
    case "reset":
      return state === initialLifecycleState ? state : initialLifecycleState;
  }
}

/** How many of `reports` are `fix_proposed` with a fix still waiting on a
 * verdict (not Clax's own `Badge.waiting`, which also counts an unread
 * reply); `null` before `reports` has ever loaded, so a host can tell none
 * waiting apart from not asked yet. */
export function pendingFixCount(reports: Report[] | null): number | null {
  if (reports === null) return null;
  return reports.filter(r => r.state === "fix_proposed" && (r.fixes?.at(-1)?.outcome ?? "pending") === "pending").length;
}

/** Options `ReporterLifecycle` runs its polls on; every field optional so
 * a host that wants reports alone, with no badge (no `win`), gets one. */
export interface ReporterLifecycleOptions {
  timers?: Timers;
  /** Where the badge watch reads page visibility; without it the badge is
   * never asked for (`startBadge` does nothing), same as a host with no
   * launcher of its own choosing not to show one. */
  win?: Visibility;
  pollMs?: number;
  badgePollMs?: number;
}

/** The headless reporter lifecycle: a `Poll` of "Your reports" and a
 * `BadgeWatch` of the waiting count, turned into the pure `update` above,
 * behind a Svelte-store `subscribe`. `client` is called only from here:
 * `openReports`/`closeReports`/`startBadge`/`stopBadge`/`refresh` start and
 * stop the polls, and `file`/`reply`/`verify`/`reopen`/`seen` are its
 * commands, each a call to `client` and the message its answer makes. */
export class ReporterLifecycle {
  private state: ReporterLifecycleState = initialLifecycleState;
  private readonly subs = new Set<(s: ReporterLifecycleState) => void>();
  private readonly pollReports: Poll;
  private readonly badgeWatch: BadgeWatch | null;

  constructor(private readonly client: ReporterApi, opts: ReporterLifecycleOptions = {}) {
    this.pollReports = new Poll(
      async () => { const page = await client.myReports(); this.dispatch({ type: "reports/loaded", page }); },
      opts.timers, opts.pollMs,
      e => this.dispatch({ type: "reports/failed", error: e }),
    );
    this.badgeWatch = opts.win
      ? new BadgeWatch(() => client.badge(), opts.win, {
        changed: n => this.dispatch({ type: "badge/changed", waiting: n }),
        rose: ids => this.dispatch({ type: "badge/rose", ids }),
      }, opts.timers, opts.badgePollMs)
      : null;
  }

  /** The current state, without subscribing. */
  get(): ReporterLifecycleState {
    return this.state;
  }

  /** A Svelte store's `subscribe`: `fn` is called at once with the
   * current state, and again on every change, until the returned
   * function is called. */
  subscribe(fn: (s: ReporterLifecycleState) => void): () => void {
    this.subs.add(fn);
    fn(this.state);
    return () => { this.subs.delete(fn); };
  }

  private dispatch(msg: ReporterLifecycleMsg): void {
    const next = update(this.state, msg);
    if (next === this.state) return;
    this.state = next;
    for (const fn of [...this.subs]) fn(next);
  }

  /** Starts polling "Your reports" (`POLL_MS`), as while its panel is
   * open; a `reports/loaded` from a load already in flight when
   * `closeReports` is called is dropped (`update`'s own guard). */
  openReports(): void {
    this.dispatch({ type: "reports/opened" });
    this.pollReports.start();
  }

  closeReports(): void {
    this.pollReports.stop();
    this.dispatch({ type: "reports/closed" });
  }

  /** Loads "Your reports" now (a reply was just sent); does nothing while
   * closed. */
  refresh(): void {
    this.pollReports.now();
  }

  /** Starts watching the waiting badge; does nothing without `win`
   * (`ReporterLifecycleOptions`). */
  startBadge(): void {
    if (!this.badgeWatch) return;
    this.dispatch({ type: "badge/started" });
    this.badgeWatch.start();
  }

  stopBadge(): void {
    this.badgeWatch?.stop();
  }

  /** Files `r`, and merges it into `reports` as new (first). */
  async file(r: NewReport): Promise<Report> {
    const report = await this.client.fileReport(r);
    this.dispatch({ type: "report/updated", report, first: true });
    return report;
  }

  /** Comments `body` on report `id` as this reporter, and merges the
   * answer. */
  async reply(id: string, body: string): Promise<Report> {
    const report = await this.client.comment(id, body);
    this.dispatch({ type: "report/updated", report });
    return report;
  }

  /** Verifies the fix waiting on report `id`, merges the answer, and asks
   * the badge again at once (it no longer waits on this one). */
  async verify(id: string, v?: Verification): Promise<Report> {
    const report = await this.client.verify(id, v);
    this.dispatch({ type: "report/updated", report });
    this.badgeWatch?.now();
    return report;
  }

  /** Says report `id` is still broken (a new capture, `r`), merges the
   * answer, and asks the badge again. */
  async reopen(id: string, r: Reopening): Promise<Report> {
    const report = await this.client.reopen(id, r);
    this.dispatch({ type: "report/updated", report });
    this.badgeWatch?.now();
    return report;
  }

  /** Marks report `id` read (its comments leave the badge), and asks the
   * badge again. A refusal is the caller's: nothing here catches it, and
   * the badge is not asked again on one. */
  async seen(id: string): Promise<void> {
    await this.client.seen(id);
    this.badgeWatch?.now();
  }

  /** Stops both polls, for a teardown (the overlay's `onDestroy`). */
  stop(): void {
    this.pollReports.stop();
    this.badgeWatch?.stop();
  }

  /** Stops both polls and drops every held state, as a host does on
   * sign-out (next to `ReporterClient.forget()` / `handle.forget()`):
   * the next reporter starts from `initialLifecycleState`. */
  reset(): void {
    this.stop();
    this.state = initialLifecycleState;
    for (const fn of [...this.subs]) fn(this.state);
  }
}
