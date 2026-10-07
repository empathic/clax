// What the worker knows about each tab with Clax (spec 2026-10-05 §9.4,
// §9.5): its URL, live page, route, threads, working list and comment mode;
// the stream's deltas applied to it; the overlay and the side panels told
// of every change. A tab's record says whether Clax is on in it, and for
// which origin (spec O4). What a restarted worker needs to pick a tab up
// again (its URL, its origin, whether the overlay is in it, comment mode,
// activeTab) is kept in session storage; the tab's page and threads are
// fetched again at its next message.
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Participants, Version } from "../../../shell/src/api";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import { type FeedbackState, type Thread, type Viewer, upsert } from "../../../shell/src/threads";
import { type ThreadDelta, applyPresence, applyThread } from "../../../shell/src/view/deltas";
import type { PresenceView } from "../../../shell/src/view/presence-model";
import { type ThreadChange, ThreadSync } from "../../../shell/src/view/thread-sync";
import type { Working } from "../../../shell/src/view/working-model";
import { MAX_FAR, MAX_URL, type OverlayToWorker, type PageView, type PanelState, type PanelToWorker, URL_TOO_LONG, type WorkerToOverlay, type WorkerToPanel, isFromPanel, overlayThread, waitsForSnapshot } from "../messages";
import type { Api } from "./api";
import { originOf } from "./origins";
import type { Sites } from "./site";

type Owner = { public_id: string; display_name: string | null };

export type TabState = {
  tabId: number; url: string; page: PageView | null; route: string | null; threads: Thread[]; working: Working[];
  resolved: Record<string, AnchorResult>; commentMode: boolean; overlay: boolean; pending: boolean; selected: string | null;
  versions: Version[]; participants: Participants | null;
  /** A gesture granted the tab activeTab. Chrome keeps the grant through
   * reloads and navigations within the origin, and withdraws it at a
   * navigation to another origin, which turns Clax off in the tab. */
  active: boolean;
  /** The origin Clax is on for in the tab; null when it is off there. */
  on: string | null;
  /** The person refused the origin's permission when turning Clax on: a reload will turn it off. */
  declined: boolean;
  error: { code: string; message: string } | null;
};

export const emptyTab = (tabId: number, url: string): TabState => ({
  tabId, url, page: null, route: null, threads: [], working: [], resolved: {}, commentMode: false, overlay: false, pending: false, selected: null,
  versions: [], participants: null, active: false, on: null, declined: false, error: null,
});

/** A thread of a live page has `addressed_pending` while an agent's address waits for the page's next snapshot. */
const waiting = waitsForSnapshot;
const pendingOf = (threads: Thread[]) => threads.some(waiting);

const FEEDBACK = ["thread_id", "state", "tier", "since", "resends", "exhausted"] as const;

/** The change a thread event makes to a thread list (idempotent and keyed
 * by thread ID, as `ThreadSync` replays it); null for other events. */
function threadChange(name: string, data: Record<string, unknown>): ThreadChange | null {
  switch (name) {
    case "thread": return ts => applyThread(ts, data.thread as ThreadDelta).threads;
    case "thread_deleted": return ts => ts.filter(t => t.id !== data.thread_id);
    case "feedback_state": {
      const fs = Object.fromEntries(FEEDBACK.map(k => [k, data[k]])) as FeedbackState;
      return ts => ts.map(t => (t.id === data.thread_id ? { ...t, feedback_state: fs } : t));
    }
    default: return null;
  }
}

/** `s` with one stream event of its live page applied. */
export function applyEvent(s: TabState, name: string, data: Record<string, unknown>): TabState {
  const change = threadChange(name, data);
  if (change) {
    const threads = change(s.threads);
    return { ...s, threads, pending: pendingOf(threads) };
  }
  switch (name) {
    case "working": return { ...s, working: Array.isArray(data.working) ? (data.working as Working[]) : [] };
    case "version": {
      const n = Number(data.n);
      if (!s.page || data.artifact_id !== s.page.artifact_id || !Number.isSafeInteger(n) || n <= s.page.current_version) return s;
      return { ...s, page: { ...s.page, current_version: n } };
    }
    default: return s;
  }
}

/** The daemon calls the tabs make. */
export type TabsApi = Pick<Api, "lookup" | "threads" | "thread" | "working" | "artifact" | "me" | "presence" | "presenceOf">;

/** How often a visible panel's owner is reported here (spec §9.5). */
export const PRESENCE_MS = 30_000;

/** A side panel's port and what the worker keeps for it: the tab it shows,
 * its window, whether it is visible, its hub client (the page's presence
 * topic) and the presence it heard, and its 30 s report. */
type PanelEntry = {
  port: chrome.runtime.Port; tabId: number | null; windowId: number; visible: boolean;
  hubId: string; aid: string | null; presence: PresenceView[]; beat: ReturnType<typeof setInterval> | null;
};

type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
type Saved = { url: string; overlay: boolean; commentMode: boolean; active: boolean; on: string | null };
const STORE_KEY = "tabs";
const saved = (v: unknown): v is Saved => {
  const o = v as Partial<Saved> | null;
  return typeof o === "object" && o !== null && typeof o.url === "string" && /^https?:\/\//.test(o.url)
    && typeof o.overlay === "boolean" && typeof o.commentMode === "boolean" && typeof o.active === "boolean"
    && (o.on === null || (typeof o.on === "string" && /^https?:\/\/[^/?#]+$/.test(o.on)));
};

type Deps = {
  api: TabsApi;
  hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void };
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  /** Injects the overlay into the tab's document, if it is of `origin`,
   * unless it has it: true when it injected it now, false when the
   * document has it, null when the document is of another origin. */
  inject(tabId: number, origin: string): Promise<boolean | null>;
  /** Whether the tab's current document has the overlay (false when the worker cannot reach it). */
  present?(tabId: number): Promise<boolean>;
  /** Where the tabs are kept across worker restarts (chrome.storage.session). */
  store?: Area;
  /** The window the tab is in now (a tab can move to another): where its panel's owner is reported. */
  windowOf?(tabId: number): Promise<number>;
  /** Every thread of the sites the tabs are on for (spec §7.1). */
  sites?: Pick<Sites, "view" | "follow" | "fromHub" | "origins">;
  now?(): number;
};

/** How long a closed tab's ID is kept, so an answer still in flight for it writes no record. */
export const CLOSED_MS = 5 * 60_000;

const failure = (e: unknown) => {
  const err = e as { code?: unknown; message?: unknown };
  return { code: typeof err?.code === "string" ? err.code : "failed", message: typeof err?.message === "string" ? err.message : String(e) };
};
const hubId = (tabId: number) => `tab:${tabId}`;
/** A URL's origin and path: the page it names, whatever its route. */
const pageOf = (url: string) => { try { const u = new URL(url); return u.origin + u.pathname; } catch { return null; } };
const tabOf = (id: string) => (id.startsWith("tab:") ? Number(id.slice(4)) : Number.NaN);
const presenceList = (v: unknown): PresenceView[] => (Array.isArray(v) ? (v as PresenceView[]) : []);

export class Tabs {
  private tabs = new Map<number, TabState>();
  /** Each tab's latest lookup: an older one that answers late is dropped. */
  private seqs = new Map<number, number>();
  /** Each tab's thread list, ordered against the loads in flight (per live page). */
  private syncs = new Map<number, { aid: string; sync: ThreadSync }>();
  /** Each tab's count of working events, so a refetch never replaces a newer list. */
  private workingAt = new Map<number, number>();
  /** Tabs restored from storage whose page has not been fetched again yet. */
  private stale = new Set<number>();
  /** Tabs with a refetch queued for this turn. */
  private refreshing = new Set<number>();
  private panels = new Set<PanelEntry>();
  /** Per tab, a thread the panel selected on another route: scrolled to once the overlay finds it there. */
  private scrollAfter = new Map<number, { id: string; at: string | null }>();
  private panelSeq = 0;
  /** Per tab, what the overlay was last told (`state`): a change to nothing it shows is not sent. */
  private told = new Map<number, string>();
  /** Per tab, a count of the documents and injections the record has seen: a presence probe answered after either is stale. */
  private gens = new Map<number, number>();
  /** Tabs turned off (null) or closed (when): an answer still in flight
   * for one writes no record. A closed tab's ID is dropped after CLOSED_MS
   * (Chrome does not reuse it); a tab turned off keeps it until it is
   * turned on again or closes. */
  private closed = new Map<number, number | null>();
  private viewer: Owner | null = null;
  private streamUp = true;
  private saving = false;
  private readonly loaded: Promise<void>;
  /** Whether the tabs kept before a worker restart are back. */
  isReady = false;

  constructor(private readonly d: Deps) {
    this.loaded = this.restore().finally(() => { this.isReady = true; this.followSites(); });
  }

  /** Resolves once the tabs kept before a worker restart are back. */
  ready(): Promise<void> { return this.loaded; }

  /** The origin Clax is on for in the tab; null when it is off there. */
  onOrigin(tabId: number): string | null { return this.tabs.get(tabId)?.on ?? null; }

  /** The tabs Clax is on in. */
  onTabs(): number[] { return [...this.tabs.values()].filter(s => s.on !== null).map(s => s.tabId); }

  /** A gesture turned Clax on in the tab for `origin` (or kept it on), and
   * granted it activeTab. A tab on for another origin starts afresh. */
  turnOn(tabId: number, url: string, origin: string): void {
    const s = this.tabs.get(tabId);
    if (s && s.on !== origin) this.close(tabId);
    this.closed.delete(tabId);
    this.set(tabId, { ...(s && s.on === origin ? s : emptyTab(tabId, url)), on: origin, active: true, declined: false });
  }

  /** The person refused the origin's permission while turning Clax on in the tab. */
  declined(tabId: number): void {
    const s = this.tabs.get(tabId);
    if (s?.on) this.set(tabId, { ...s, declined: true });
  }

  state(tabId: number): TabState | undefined { return this.tabs.get(tabId); }

  /** Whether a gesture granted the tab activeTab (kept within its origin). */
  admits(tabId: number): boolean { return !!this.tabs.get(tabId)?.active; }

  /** The open threads of the tab's page waiting for a snapshot: what a snapshot taken now covers. */
  pendingIds(tabId: number): string[] {
    return (this.tabs.get(tabId)?.threads ?? []).filter(waiting).map(t => t.id);
  }

  private async restore(): Promise<void> {
    if (!this.d.store) return;
    try {
      const all = (await this.d.store.get(STORE_KEY))[STORE_KEY];
      if (typeof all !== "object" || all === null) return;
      for (const [k, v] of Object.entries(all)) {
        const tabId = Number(k);
        if (!Number.isSafeInteger(tabId) || tabId < 0 || !saved(v) || this.tabs.has(tabId)) continue;
        this.tabs.set(tabId, { ...emptyTab(tabId, v.url), overlay: v.overlay, commentMode: v.commentMode, active: v.active, on: v.on });
        this.stale.add(tabId);
      }
    } catch {
      // Nothing kept: tabs start afresh at their next message.
    }
  }

  /** Keeps the tabs' essentials in storage, once per turn. */
  private persist(): void {
    if (!this.d.store || this.saving) return;
    this.saving = true;
    queueMicrotask(() => {
      this.saving = false;
      const out: Record<string, Saved> = {};
      for (const [id, s] of this.tabs) if (s.url) out[id] = { url: s.url, overlay: s.overlay, commentMode: s.commentMode, active: s.active, on: s.on };
      void this.d.store!.set({ [STORE_KEY]: out }).catch(() => {});
    });
  }

  /** Tabs whose URL the daemon has answered a lookup for: the daemon is there. */
  private looked = new Set<number>();
  /** The site topics followed are those of the origins Clax is on for in tabs the daemon answered for. */
  private followSites(): void {
    this.d.sites?.follow(this.onTabs().filter(id => this.looked.has(id)).map(id => this.tabs.get(id)!.on!));
  }

  /** The origin's site changed: the overlays of its tabs pin its threads, and their panels list them. */
  siteChanged(origin: string): void {
    for (const s of this.tabs.values()) if (s.on === origin) this.tellOverlay(s.tabId, s);
    for (const p of this.panels) if (p.tabId !== null && this.tabs.get(p.tabId)?.on === origin) this.tellSite(p);
  }

  private tellSite(p: PanelEntry): void {
    const on = p.tabId === null ? null : (this.tabs.get(p.tabId)?.on ?? null);
    p.port.postMessage({ t: "site", site: this.d.sites?.view(on) ?? null } satisfies WorkerToPanel);
  }

  /** Where a thread of the tab's site is: its path and route, and the
   * site's origins it may be opened on, the most recently used first (spec
   * §7.2: any of a joined site's); null when the site has no such thread.
   * `opening` then names the URL the tab goes to for it. */
  openThread(tabId: number, threadId: string): { path: string; origins: string[] } | null {
    const s = this.tabs.get(tabId);
    const t = this.d.sites?.view(s?.on ?? null)?.pages.flatMap(p => p.threads).find(x => x.id === threadId);
    if (!s?.on || !t?.page_url) return null;
    const origins = this.d.sites?.origins(s.on) ?? [s.on];
    let u: URL;
    try { u = new URL(t.page_url); } catch { return null; }
    if (!origins.includes(u.origin)) return null;
    return { path: u.pathname + u.search + u.hash, origins };
  }

  /** The tab goes to `url` for the thread (the panel opened it): the
   * overlay scrolls to it once it finds it there, on that page only (the
   * document the tab leaves may find it first). */
  opening(tabId: number, threadId: string, url: string): void {
    this.scrollAfter.set(tabId, { id: threadId, at: pageOf(url) });
  }

  /** The tab went to `origin`, another origin of the site Clax is on for
   * there (spec §7.2): Clax stays on in it, now for `origin`. False, and
   * nothing changed, for an origin of another site. */
  moveOn(tabId: number, origin: string): boolean {
    const s = this.tabs.get(tabId);
    if (!s?.on || s.on === origin || !(this.d.sites?.origins(s.on) ?? []).includes(origin)) return false;
    this.set(tabId, { ...s, on: origin });
    return true;
  }

  private set(tabId: number, next: TabState): void {
    if (this.closed.has(tabId)) return;
    const prev = this.tabs.get(tabId);
    this.tabs.set(tabId, next);
    if (prev?.on !== next.on) this.followSites();
    if (!prev || prev.url !== next.url || prev.overlay !== next.overlay || prev.commentMode !== next.commentMode || prev.active !== next.active || prev.on !== next.on) this.persist();
    this.tellOverlay(tabId, next);
    this.tellPanels(tabId);
  }

  /** The overlay's `state`: the page, the route, comment mode, and each
   * thread's anchor and status only (spec L7: thread text stays in extension
   * pages); sent when any of it changed since the overlay was last told. */
  private tellOverlay(tabId: number, s: TabState): void {
    // The site's open threads of other pages follow the page's own (so the
    // pins number those first), each with the path it was left at; their
    // pending addresses are their own pages' to settle.
    // At most MAX_FAR, the newest.
    const far = (this.d.sites?.view(s.on)?.pages ?? []).filter(p => p.page.artifact_id !== s.page?.artifact_id)
      .flatMap(p => p.threads.filter(t => t.status === "open").map(t => [t, p.page.path] as const))
      .sort(([a], [b]) => (a.created_at < b.created_at ? 1 : -1)).slice(0, MAX_FAR)
      .map(([t, path]) => ({ ...overlayThread(t), addressed_pending: false, from: t.page_path ?? path }));
    const m: WorkerToOverlay = { t: "state", page: s.page, route: s.route, threads: [...s.threads.map(overlayThread), ...far].slice(0, 1000), commentMode: s.commentMode, pending: s.pending };
    const key = JSON.stringify(m);
    if (this.told.get(tabId) === key) return;
    this.told.set(tabId, key);
    this.d.toOverlay(tabId, m);
  }

  /** A new overlay (or a new document) hears the whole state again. */
  private retell(tabId: number): void {
    this.told.delete(tabId);
    this.gens.set(tabId, (this.gens.get(tabId) ?? 0) + 1);
  }

  private tellPanels(tabId: number): void {
    for (const p of this.panels) {
      if (p.tabId !== tabId) continue;
      this.followPage(p);
      this.tellPanel(p);
    }
  }

  private tellPanel(p: PanelEntry): void {
    p.port.postMessage({ t: "tab", state: { ...this.panelState(p.tabId), presence: p.presence } } satisfies WorkerToPanel);
  }

  /** The panel's hub client follows the presence of the live page its tab
   * shows, and its owner is reported here while it is visible there. */
  private followPage(p: PanelEntry): void {
    const aid = p.tabId === null ? null : (this.tabs.get(p.tabId)?.page?.artifact_id ?? null);
    if (aid === p.aid) return;
    p.aid = aid;
    p.presence = [];
    this.d.hub.receive(p.hubId, { t: "topics", topics: aid ? [`presence:${aid}`] : [] });
    this.report(p);
  }

  /** Starts or stops the panel's presence report: `here` now and every
   * 30 s while it is visible and shows a live page; never `away`, so a
   * hidden panel's report lapses rather than hiding a shell tab's `here`. */
  private report(p: PanelEntry): void {
    if (p.beat !== null) { clearInterval(p.beat); p.beat = null; }
    const aid = p.aid;
    if (!aid || !p.visible || !this.panels.has(p)) return;
    const once = () => {
      const tabId = p.tabId;
      const where = tabId !== null && this.d.windowOf ? this.d.windowOf(tabId).catch(() => p.windowId) : Promise.resolve(p.windowId);
      void where.then(w => this.d.api.presence(aid, w)).then(r => {
        if (p.aid === aid && r?.people) this.takePresence(p, presenceList(r.people));
      }, () => {});
    };
    once();
    p.beat = setInterval(once, PRESENCE_MS);
  }

  private takePresence(p: PanelEntry, people: PresenceView[]): void {
    p.presence = people;
    if (this.panels.has(p)) this.tellPanel(p);
  }

  private async fetchPresence(p: PanelEntry): Promise<void> {
    const aid = p.aid;
    if (!aid) return;
    const r = await this.d.api.presenceOf(aid).catch(() => null);
    if (r && p.aid === aid) this.takePresence(p, presenceList(r.people));
  }

  /** One hub message for a panel's client. */
  private fromHubPanel(id: string, msg: HubMsg): void {
    const p = [...this.panels].find(x => x.hubId === id);
    if (!p) return;
    switch (msg.t) {
      case "ping": this.d.hub.receive(id, { t: "ping" }); break;
      case "live": case "resync": void this.fetchPresence(p); break;
      case "event":
        if (msg.name === "presence" && p.aid && msg.data.artifact_id === p.aid) {
          const gone = Array.isArray(msg.data.gone) ? msg.data.gone.filter((g): g is string => typeof g === "string") : [];
          this.takePresence(p, applyPresence(p.presence, presenceList(msg.data.people), gone));
        }
        break;
      default: break;
    }
  }

  private watched(tabId: number): boolean {
    for (const p of this.panels) if (p.tabId === tabId) return true;
    return false;
  }

  private syncFor(tabId: number, aid: string): ThreadSync {
    let e = this.syncs.get(tabId);
    if (!e || e.aid !== aid) {
      e = { aid, sync: new ThreadSync(f => this.changeThreads(tabId, aid, f)) };
      this.syncs.set(tabId, e);
    }
    return e.sync;
  }

  /** Applies `f` to the tab's threads while it still shows page `aid`. */
  private changeThreads(tabId: number, aid: string, f: ThreadChange): void {
    const s = this.tabs.get(tabId);
    if (!s || s.page?.artifact_id !== aid) return;
    const threads = f(s.threads);
    this.set(tabId, { ...s, threads, pending: pendingOf(threads) });
  }

  /** The tab's overlay shows open threads its record has no results for
   * (the worker restarted, or the page changed): the overlay is asked to
   * send them again. Only a lookup asks, never a `resolved`, so the two
   * cannot loop. */
  private askResults(tabId: number): void {
    const s = this.tabs.get(tabId);
    if (s?.overlay && Object.keys(s.resolved).length === 0 && s.threads.some(t => t.status === "open")) this.d.toOverlay(tabId, { t: "resend" });
  }

  /** The page's URL changed (a load or a route change), or its topics went
   * live (`fresh`): look its live page up, load its threads and working
   * list when the page changed or `fresh`, and follow its topics. Stream
   * deltas heard meanwhile are kept: on the same page the lists are not
   * replaced, and a refetch's answer is ordered against them. */
  async route(tabId: number, url: string | null, fresh = false): Promise<TabState> {
    const seq = (this.seqs.get(tabId) ?? 0) + 1;
    this.seqs.set(tabId, seq);
    if (url === null || url.length > MAX_URL) return this.tooLong(tabId);
    const prev = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    const latest = () => this.seqs.get(tabId) === seq;
    const now = () => this.tabs.get(tabId) ?? prev;
    try {
      const { page, route } = await this.d.api.lookup(url);
      if (!latest()) return now();
      this.stale.delete(tabId);
      if (!this.looked.has(tabId)) { this.looked.add(tabId); this.followSites(); }
      const cur = now();
      const topics = page ? [`artifact:${page.artifact_id}`, `working:${page.artifact_id}`] : [];
      if (page && page.artifact_id === cur.page?.artifact_id) {
        const shown = { ...page, current_version: Math.max(page.current_version, cur.page.current_version) };
        this.d.hub.receive(hubId(tabId), { t: "topics", topics });
        this.set(tabId, { ...cur, url, page: shown, route, error: null });
        this.askResults(tabId);
        if (fresh) await this.refetch(tabId, page.artifact_id);
        if (fresh && this.watched(tabId)) void this.details(tabId);
        return now();
      }
      let threads: Thread[] = [];
      let working: Working[] = [];
      if (page) [threads, working] = await Promise.all([this.d.api.threads(page.artifact_id), this.d.api.working(page.artifact_id)]);
      if (!latest()) return now();
      // Another page: its deltas start with its topics, after this.
      this.syncs.delete(tabId);
      const next: TabState = {
        ...now(), url, page, route, threads, working, pending: pendingOf(threads), error: null, versions: [], participants: null, resolved: {},
      };
      this.d.hub.receive(hubId(tabId), { t: "topics", topics });
      this.set(tabId, next);
      this.askResults(tabId);
      if (page && this.watched(tabId)) void this.details(tabId);
      return next;
    } catch (e) {
      if (!latest()) return now();
      const next = { ...now(), url, error: failure(e) };
      this.set(tabId, next);
      return next;
    }
  }

  /** The tab shows a page whose address is over MAX_URL, which the daemon
   * refuses (spec §7): no live page, and the panel says why. The record
   * keeps no URL, so nothing looks it up again. */
  private tooLong(tabId: number): TabState {
    this.stale.delete(tabId);
    this.syncs.delete(tabId);
    this.d.hub.receive(hubId(tabId), { t: "topics", topics: [] });
    const next: TabState = {
      ...(this.tabs.get(tabId) ?? emptyTab(tabId, "")), url: "", page: null, route: null, threads: [], working: [], pending: false,
      resolved: {}, versions: [], participants: null, error: { code: "url_too_long", message: URL_TOO_LONG },
    };
    this.set(tabId, next);
    return next;
  }

  /** The page's threads and working list again, ordered against the deltas heard meanwhile. */
  private async refetch(tabId: number, aid: string): Promise<void> {
    const done = this.syncFor(tabId, aid).begin();
    const at = this.workingAt.get(tabId) ?? 0;
    try {
      const [threads, working] = await Promise.all([this.d.api.threads(aid), this.d.api.working(aid)]);
      done(threads);
      const s = this.tabs.get(tabId);
      if (s && s.page?.artifact_id === aid && (this.workingAt.get(tabId) ?? 0) === at) this.set(tabId, { ...s, working });
    } catch (e) {
      done(undefined);
      const s = this.tabs.get(tabId);
      if (s) this.set(tabId, { ...s, error: failure(e) });
    }
  }

  /** The tab's record for a new document of its origin: no overlay and
   * comment mode off (spec §11 "Page navigates"). The activeTab grant
   * stays: Chrome withdraws it only at a navigation to another origin,
   * which turns Clax off in the tab, so the side panel's Comment keeps
   * working after a reload or a hot reload. */
  private newDocument(tabId: number, url: string): void {
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    this.retell(tabId);
    this.set(tabId, { ...s, overlay: false, commentMode: false, resolved: {}, selected: null });
  }

  /** Chrome reported a tab Clax is on loading, or done loading (`loaded`),
   * within its origin. Only a new document (its overlay is gone) resets the
   * record; an in-page navigation, which Chrome may report the same way,
   * keeps everything, and the overlay reports its route. A probe at
   * `loading` can still see the old document: the one at `complete` sees
   * the new one, which then gets the overlay again (Clax stays on in the
   * tab, and `ensureOverlay` probes again, so it injects once). A probe
   * answered after a new document or an injection the record saw meanwhile
   * is stale and resets nothing. */
  async navigated(tabId: number, loaded: boolean): Promise<void> {
    const s = this.tabs.get(tabId);
    if (!s?.on) return;
    if (s.overlay) {
      const gen = this.gens.get(tabId) ?? 0;
      if (this.d.present && (await this.d.present(tabId))) return;
      if (this.onOrigin(tabId) !== s.on) return;
      // A probe answered after a new document or an injection is stale: the record already follows the document.
      if ((this.gens.get(tabId) ?? 0) === gen) this.newDocument(tabId, s.url);
    }
    if (!loaded) return;
    try {
      await this.ensureOverlay(tabId);
    } catch (e) {
      this.fail(tabId, e);
    }
  }

  /** A gesture granted the tab activeTab. */
  activate(tabId: number, url = ""): void {
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    this.set(tabId, { ...s, active: true });
  }

  /** The tab's error, for the panel (a failed step of turning Clax on). */
  fail(tabId: number, e: unknown): void {
    const s = this.tabs.get(tabId);
    if (s) this.set(tabId, { ...s, error: failure(e) });
  }

  /** Makes sure the tab's document has the overlay, if it is of the
   * origin Clax is on for there: true when it was injected now, false when
   * it had it, null when nothing was (the document is of another origin, or
   * the tab was turned off or closed meanwhile; an overlay that landed in
   * such a tab hears `off` at its first message). */
  private async ensureOverlay(tabId: number): Promise<boolean | null> {
    const s = this.tabs.get(tabId);
    const origin = s ? (s.on ?? originOf(s.url)) : null;
    if (!s || !origin || this.closed.has(tabId)) return null;
    const fresh = await this.d.inject(tabId, origin);
    const now = this.tabs.get(tabId);
    if (fresh === null || !now || now.on !== s.on || this.closed.has(tabId)) return null;
    if (fresh) this.retell(tabId);
    this.set(tabId, { ...now, overlay: true });
    return fresh;
  }

  /** The icon, the command or the context menu: the overlay is made sure
   * of, and comment mode turns on when the overlay was just injected (a
   * new document, whatever the record said), else flips. */
  async toggle(tabId: number, url: string): Promise<void> {
    await this.loaded;
    if (this.closed.has(tabId)) return;
    if (!this.tabs.has(tabId)) this.tabs.set(tabId, emptyTab(tabId, url));
    let fresh: boolean | null;
    try {
      fresh = await this.ensureOverlay(tabId);
    } catch (e) {
      this.fail(tabId, e);
      return;
    }
    const s = this.tabs.get(tabId);
    if (fresh === null || !s) return;
    this.set(tabId, { ...s, commentMode: fresh || !s.commentMode });
    void this.route(tabId, url, this.stale.has(tabId));
  }

  /** A thread was posted from the tab on `page`: a page the tab did not
   * show yet (the first thread created it) is looked up, so the tab follows
   * its topics; on the page it shows, the stream brings the thread. */
  posted(tabId: number, page: PageView): void {
    const s = this.tabs.get(tabId);
    if (s && s.page?.artifact_id !== page.artifact_id) void this.route(tabId, s.url);
  }

  /** The tab closed (`removed`), or Clax turned off in it: its state and
   * its topics go, and nothing still in flight for it writes a record again. */
  close(tabId: number, removed = false): void {
    const now = (this.d.now ?? Date.now)();
    for (const [id, at] of this.closed) if (at !== null && now - at > CLOSED_MS) this.closed.delete(id);
    this.closed.set(tabId, removed ? now : null);
    this.tabs.delete(tabId);
    this.seqs.delete(tabId);
    this.syncs.delete(tabId);
    this.workingAt.delete(tabId);
    this.scrollAfter.delete(tabId);
    this.stale.delete(tabId);
    this.looked.delete(tabId);
    this.told.delete(tabId);
    this.gens.delete(tabId);
    this.d.hub.detach(hubId(tabId));
    this.persist();
    this.followSites();
    this.tellPanels(tabId);
  }

  /** A request paired again: the stream belongs to the old credential, so
   * the hub opens a new one, whose topics go live and refetch each tab. */
  repaired(): void {
    const first = this.tabs.keys().next();
    if (!first.done) this.d.hub.receive(hubId(first.value), { t: "reconnect" });
  }

  /** One event or notice from the stream hub, for the clients `ids`. */
  fromHub(ids: string[], msg: HubMsg): void {
    if (msg.t === "status") { this.status(msg.up); return; }
    for (const id of ids) {
      if (id.startsWith("panel:")) { this.fromHubPanel(id, msg); continue; }
      if (id.startsWith("site:")) { this.d.sites?.fromHub(id, msg); continue; }
      const tabId = tabOf(id);
      const s = this.tabs.get(tabId);
      if (!s) continue;
      switch (msg.t) {
        // The hub drops a client it has not heard from in a while; each tab is the worker's to keep until it closes.
        case "ping": this.d.hub.receive(id, { t: "ping" }); break;
        case "event": this.event(tabId, s, msg.name, msg.data); break;
        case "live": case "resync": this.refresh(tabId); break;
        case "refused": this.set(tabId, { ...s, error: { code: msg.code, message: `The daemon refused ${msg.topic}.` } }); break;
      }
    }
  }

  private event(tabId: number, s: TabState, name: string, data: Record<string, unknown>): void {
    const aid = s.page?.artifact_id;
    if (!aid || (data.artifact_id !== undefined && data.artifact_id !== aid)) return;
    const change = threadChange(name, data);
    if (change) {
      const incomplete = name === "thread" && !applyThread(s.threads, data.thread as ThreadDelta).complete;
      this.syncFor(tabId, aid).change(change);
      if (incomplete) void this.refetchThread(tabId, aid, (data.thread as ThreadDelta).id);
      return;
    }
    if (name === "working") this.workingAt.set(tabId, (this.workingAt.get(tabId) ?? 0) + 1);
    this.set(tabId, applyEvent(s, name, data));
    if (name === "version" && this.watched(tabId)) void this.details(tabId);
  }

  /** One refetch per tab for the notices of one turn (a `live` and a `resync` together). */
  private refresh(tabId: number): void {
    if (this.refreshing.has(tabId)) return;
    this.refreshing.add(tabId);
    queueMicrotask(() => {
      this.refreshing.delete(tabId);
      const s = this.tabs.get(tabId);
      if (s) void this.route(tabId, s.url, true);
    });
  }

  /** The stream went up or down: the side panels say so (the overlay shows nothing of it). */
  private status(up: boolean): void {
    this.streamUp = up;
    const m = { t: "stream-status", up } as const;
    for (const p of this.panels) p.port.postMessage(m satisfies WorkerToPanel);
  }

  /** A thread whose comments no longer add up, fetched whole, ordered against the deltas heard meanwhile. */
  private async refetchThread(tabId: number, aid: string, tid: string): Promise<void> {
    const done = this.syncFor(tabId, aid).beginThread(tid);
    try {
      done((await this.d.api.thread(aid, tid)).thread);
    } catch {
      // The next `live` or `resync` refetches the list.
      done(undefined);
    }
  }

  /** The panel selected a thread (null: none): the overlay pins it and scrolls to it. */
  select(tabId: number, threadId: string | null): void {
    const s = this.tabs.get(tabId);
    if (!s) return;
    this.set(tabId, { ...s, selected: threadId });
    this.scrollAfter.delete(tabId);
    const t = s.threads.find(x => x.id === threadId);
    // A thread on another route: the panel navigates the tab there first.
    if (t && (t.anchor.route ?? null) !== s.route) { this.scrollAfter.set(tabId, { id: t.id, at: null }); return; }
    this.d.toOverlay(tabId, threadId === null ? { t: "focus", threadId: null } : { t: "scroll-to", threadId });
  }

  /** The panel turned comment mode on or off; the overlay follows the tab's state. */
  setCommentMode(tabId: number, on: boolean): void {
    const s = this.tabs.get(tabId);
    if (s) this.set(tabId, { ...s, commentMode: on });
  }

  /** A thread an action answered with, applied at once (the stream brings it too). */
  applied(tabId: number, thread: Thread): void {
    const aid = this.tabs.get(tabId)?.page?.artifact_id;
    if (aid && thread.artifact_id === aid) this.syncFor(tabId, aid).change(ts => upsert(ts, thread));
  }

  /** The owner as the daemon answered (its name set): every panel shows it. */
  setViewer(v: Viewer): void {
    this.viewer = { public_id: v.public_id, display_name: v.display_name };
    for (const p of this.panels) this.tellPanel(p);
  }

  /** The owner, and the page's versions and participants, for a panel. */
  private async details(tabId: number): Promise<void> {
    const aid = this.tabs.get(tabId)?.page?.artifact_id;
    const [me, art] = await Promise.all([
      this.d.api.me().catch(() => null),
      aid ? this.d.api.artifact(aid).catch(() => null) : Promise.resolve(null),
    ]);
    if (me) this.viewer = { public_id: me.viewer.public_id, display_name: me.viewer.display_name };
    const s = this.tabs.get(tabId);
    if (s && art && s.page?.artifact_id === aid) this.tabs.set(tabId, { ...s, versions: art.versions, participants: art.artifact.participants ?? null });
    this.tellPanels(tabId);
  }

  /** A message from the tab's overlay. `senderUrl` is the
   * sender's URL: a tab the worker does not know (it restarted) is looked
   * up there again before the message is acted on. */
  async fromOverlay(tabId: number, _windowId: number, m: OverlayToWorker, senderUrl?: string): Promise<unknown> {
    await this.loaded;
    // An overlay at its start (its first route): it hears the whole state.
    if (m.t === "route") { this.told.delete(tabId); await this.route(tabId, m.url, this.stale.has(tabId)); return; }
    const known = this.tabs.get(tabId);
    if ((!known || this.stale.has(tabId)) && (senderUrl ?? known?.url)) await this.route(tabId, (senderUrl ?? known?.url)!, true);
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, senderUrl ?? "");
    switch (m.t) {
      case "resolved": {
        const after = this.scrollAfter.get(tabId);
        const wanted = after && (after.at === null || pageOf(s.url) === after.at) ? after.id : undefined;
        const found = wanted !== undefined && m.results.some(r => r.id === wanted && r.found);
        this.set(tabId, { ...s, resolved: Object.fromEntries(m.results.map(r => [r.id, r])), ...(found ? { selected: wanted } : {}) });
        if (found) { this.scrollAfter.delete(tabId); this.d.toOverlay(tabId, { t: "scroll-to", threadId: wanted }); }
        return;
      }
      case "comment-mode": this.set(tabId, { ...s, commentMode: m.on }); return;
      case "pin": this.set(tabId, { ...s, selected: m.threadId }); return;
      case "removed": this.set(tabId, { ...s, overlay: false, error: { code: "overlay_removed", message: "The page removed Clax's overlay." } }); return;
      default: return; // capture, pick, quiet and cancel are the pick flow's (`Picks`); ping keeps the worker alive
    }
  }

  panelState(tabId: number | null): PanelState {
    const s = tabId === null ? undefined : this.tabs.get(tabId);
    return {
      tabId, url: s?.url ?? null, page: s?.page ?? null, route: s?.route ?? null, threads: s?.threads ?? [], resolved: s?.resolved ?? {},
      versions: s?.versions ?? [], working: s?.working ?? [], participants: s?.participants ?? null, viewer: this.viewer,
      commentMode: s?.commentMode ?? false, enabled: !!s?.on, declined: !!s?.declined, selected: s?.selected ?? null, error: s?.error ?? null,
    };
  }

  /** A side panel's port: `watch-tab` picks the tab it shows, which it then
   * hears of on every change, with the stream's status. Every message it
   * sends passes `isFromPanel` before anything acts on it, and then goes to
   * `onMessage`. */
  attachPanel(port: chrome.runtime.Port, onMessage: (tabId: number | null, m: PanelToWorker, windowId: number) => void): void {
    const name = port.name.slice("panel:".length);
    const windowId = Number(name);
    if (!/^\d{1,15}$/.test(name) || !Number.isSafeInteger(windowId)) { port.disconnect(); return; }
    const entry: PanelEntry = {
      port, tabId: null, windowId, visible: false,
      hubId: `panel:${++this.panelSeq}`, aid: null, presence: [], beat: null,
    };
    this.panels.add(entry);
    port.onMessage.addListener((m: unknown) => {
      if (!isFromPanel(m)) return;
      if (m.t === "watch-tab") {
        entry.tabId = m.tabId;
        this.followPage(entry);
        this.tellPanel(entry);
        this.tellSite(entry);
        port.postMessage({ t: "stream-status", up: this.streamUp } satisfies WorkerToPanel);
        void this.details(m.tabId);
      } else if (m.t === "visible") {
        if (entry.visible !== m.on) { entry.visible = m.on; this.report(entry); }
      }
      onMessage(entry.tabId, m, entry.windowId);
    });
    port.onDisconnect.addListener(() => {
      this.panels.delete(entry);
      if (entry.beat !== null) clearInterval(entry.beat);
      this.d.hub.detach(entry.hubId);
    });
  }
}
