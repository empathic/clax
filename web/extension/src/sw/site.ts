// Every thread of each site a tab has Clax on for (spec 2026-10-05 §7.1,
// owner decision 2026-10-06): the listing `GET /api/live/site` answers,
// kept up to date by the `site:<origin>` topic, which the worker alone
// follows. A delta that does not add up (a page the listing lacks, a
// comment count that does not match) fetches the listing again; deltas
// heard while it is fetched are applied to it again when it answers.
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { Thread } from "../../../shell/src/threads";
import { type ThreadDelta, applyThread } from "../../../shell/src/view/deltas";
import type { SiteView } from "../messages";

type Ev = [string, Record<string, unknown>];
/** `v` with one event of the site's topic applied; null when it does not
 * add up (fetch the listing again). The listing keeps what the panel's site
 * list and the pins read (threads, their comments, status and anchors), so
 * other events (feedback, versions) change nothing; a deleted page fetches it again. */
export function applySite(v: SiteView, name: string, d: Record<string, unknown>): SiteView | null {
  // An origin joined the site or was split off it (spec §7.2): its pages and origins change.
  if (name === "site") return null;
  const at = v.pages.findIndex(p => p.page.artifact_id === d.artifact_id);
  const on = (i: number, f: (ts: Thread[]) => Thread[], w = v) => ({ ...w, pages: w.pages.map((p, k) => (k === i ? { ...p, threads: f(p.threads) } : p)) });
  const drop = (ts: Thread[]) => ts.filter(t => t.id !== d.thread_id);
  if (at < 0) {
    // A move from a page the listing no longer holds is applied already when its thread is on the page it went to.
    const there = name === "thread_moved" && v.pages.some(p => p.page.artifact_id === d.to_artifact_id && p.threads.some(t => t.id === d.thread_id));
    return name === "thread" || (name === "thread_moved" && !there) ? null : v;
  }
  switch (name) {
    case "thread": {
      const r = applyThread(v.pages[at].threads, d.thread as ThreadDelta);
      return r.complete ? on(at, () => r.threads) : null;
    }
    case "thread_deleted": return on(at, drop);
    case "thread_moved": {
      // The card goes with the thread, comments and all; its `thread` on the new page follows.
      const to = v.pages.findIndex(p => p.page.artifact_id === d.to_artifact_id);
      const t = v.pages[at].threads.find(x => x.id === d.thread_id);
      if (to < 0) return null;
      // Applied already (a replay), or a thread the listing never had: its `thread` there follows.
      if (!t) return v;
      return on(to, ts => [...drop(ts), { ...t, artifact_id: d.to_artifact_id as string }], on(at, drop));
    }
    case "artifact_deleted": return null;
    default: return v;
  }
}

/** An origin's listing; `heard`: the deltas heard while a fetch is in
 * flight; `again`: fetch once more when it answers; `wait`: the backoff
 * after a failed fetch. */
type Entry = { view: SiteView | null; heard: Ev[] | null; busy: Promise<void> | null; again: boolean; wait: number; timer: unknown };
export type SitesDeps = {
  api: { site(origin: string): Promise<SiteView> };
  hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void };
  /** The origin's listing changed. */
  changed(origin: string): void;
  /** Runs `fn` in `ms` (a failed fetch's retry); returns what `cancel` takes. */
  after?(fn: () => void, ms: number): unknown;
  cancel?(h: unknown): void;
};
/** The longest wait before a failed listing is fetched again. */
export const RETRY_MAX_MS = 30_000;

export class Sites {
  private m = new Map<string, Entry>();
  constructor(private readonly d: SitesDeps) {}

  view(origin: string | null): SiteView | null { return (origin && this.m.get(origin)?.view) || null; }

  /** The origins of `origin`'s site, the most recently used first (spec
   * §7.2): only `origin` until its listing is loaded, or when it joined none. */
  origins(origin: string): string[] {
    const o = this.view(origin)?.site?.origins.map(x => x.origin) ?? [];
    return o.includes(origin) ? o : [origin, ...o];
  }

  /** Follows the site topics of `origins`, and only those. */
  follow(origins: Iterable<string>): void {
    const want = new Set(origins);
    for (const [o, e] of this.m) {
      if (want.has(o)) continue;
      this.m.delete(o);
      if (e.timer !== null) (this.d.cancel ?? clearTimeout)(e.timer as never);
      this.d.hub.detach(`site:${o}`);
    }
    for (const o of want) {
      if (this.m.has(o)) continue;
      this.m.set(o, { view: null, heard: null, busy: null, again: false, wait: 0, timer: null });
      this.d.hub.receive(`site:${o}`, { t: "topics", topics: [`site:${o}`] });
    }
  }

  /** Fetches the origin's listing again (its topic went live, a delta did
   * not add up, or a rule changed it): at most one fetch in flight per
   * origin, and one more after it however often it is asked meanwhile. */
  load(origin: string): Promise<void> {
    const e = this.m.get(origin);
    if (!e) return Promise.resolve();
    if (e.busy) { e.again = true; return e.busy; }
    if (e.timer !== null) { (this.d.cancel ?? clearTimeout)(e.timer as never); e.timer = null; }
    e.again = false;
    e.busy = this.fetch(origin, e).finally(() => {
      e.busy = null;
      if (e.again && this.m.get(origin) === e) void this.load(origin);
    });
    return e.busy;
  }

  private async fetch(origin: string, e: Entry): Promise<void> {
    e.heard = [];
    let v: SiteView | null;
    try {
      v = await this.d.api.site(origin);
    } catch {
      // Tried again after a backoff (1 s, doubling, at most RETRY_MAX_MS), not at the next delta.
      e.heard = null;
      e.again = false;
      e.wait = Math.min(RETRY_MAX_MS, e.wait ? e.wait * 2 : 1000);
      e.timer = (this.d.after ?? setTimeout)(() => { e.timer = null; if (this.m.get(origin) === e) void this.load(origin); }, e.wait);
      return;
    }
    e.wait = 0;
    for (const [n, data] of e.heard) if (v) v = applySite(v, n, data);
    e.heard = null;
    if (this.m.get(origin) !== e) return;
    // A delta heard meanwhile is newer than the listing: ask again.
    if (!v) { e.again = true; return; }
    e.view = v;
    this.d.changed(origin);
  }

  /** A thread an action on another page of the site answered with,
   * applied at once (the site's topic brings it too); the listing's own
   * fields (`page_path`, `page_url`, `moves`) stay when the answer lacks them. */
  applied(origin: string, thread: Thread): void {
    const e = this.m.get(origin);
    const at = e?.view?.pages.findIndex(p => p.page.artifact_id === thread.artifact_id) ?? -1;
    if (!e?.view || at < 0) return;
    e.view = { ...e.view, pages: e.view.pages.map((p, k) => (k === at ? { ...p, threads: p.threads.map(t => (t.id === thread.id ? { ...t, ...thread } : t)) } : p)) };
    this.d.changed(origin);
  }

  /** One hub message for the client `site:<origin>`. */
  fromHub(id: string, msg: HubMsg): void {
    const origin = id.slice(5);
    const e = this.m.get(origin);
    if (!e) return;
    if (msg.t === "ping") this.d.hub.receive(id, msg);
    else if (msg.t === "live" || msg.t === "resync") void this.load(origin);
    else if (msg.t === "event") {
      e.heard?.push([msg.name, msg.data]);
      if (!e.view) return;
      const v = applySite(e.view, msg.name, msg.data);
      if (!v) { void this.load(origin); return; }
      if (v === e.view) return;
      e.view = v;
      this.d.changed(origin);
    }
  }
}
