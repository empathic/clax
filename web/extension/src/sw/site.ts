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
  const at = v.pages.findIndex(p => p.page.artifact_id === d.artifact_id);
  const on = (i: number, f: (ts: Thread[]) => Thread[], w = v) => ({ ...w, pages: w.pages.map((p, k) => (k === i ? { ...p, threads: f(p.threads) } : p)) });
  const drop = (ts: Thread[]) => ts.filter(t => t.id !== d.thread_id);
  if (at < 0) return name === "thread" || name === "thread_moved" ? null : v;
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
      if (to < 0 || !t) return null;
      return on(to, ts => [...drop(ts), { ...t, artifact_id: d.to_artifact_id as string }], on(at, drop));
    }
    case "artifact_deleted": return null;
    default: return v;
  }
}

type Entry = { view: SiteView | null; heard: Ev[] | null; seq: number };
export type SitesDeps = {
  api: { site(origin: string): Promise<SiteView> };
  hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void };
  /** The origin's listing changed. */
  changed(origin: string): void;
};

export class Sites {
  private m = new Map<string, Entry>();
  constructor(private readonly d: SitesDeps) {}

  view(origin: string | null): SiteView | null { return (origin && this.m.get(origin)?.view) || null; }

  /** Follows the site topics of `origins`, and only those. */
  follow(origins: Iterable<string>): void {
    const want = new Set(origins);
    for (const o of this.m.keys()) if (!want.has(o)) { this.m.delete(o); this.d.hub.detach(`site:${o}`); }
    for (const o of want) {
      if (this.m.has(o)) continue;
      this.m.set(o, { view: null, heard: null, seq: 0 });
      this.d.hub.receive(`site:${o}`, { t: "topics", topics: [`site:${o}`] });
    }
  }

  /** Fetches the origin's listing again (its topic went live, a delta did not add up, or a rule changed it). */
  async load(origin: string): Promise<void> {
    const e = this.m.get(origin);
    if (!e) return;
    const seq = ++e.seq;
    e.heard = [];
    let v: SiteView | null;
    try { v = await this.d.api.site(origin); } catch { return; }
    if (this.m.get(origin) !== e || e.seq !== seq) return;
    for (const [n, data] of e.heard) if (v) v = applySite(v, n, data);
    e.heard = null;
    // A delta heard meanwhile is newer than the listing: ask again.
    if (!v) { void this.load(origin); return; }
    e.view = v;
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
