// What the worker knows about each tab with Clax (spec 2026-10-05 §9.4,
// §9.5): its URL, live page, route, threads, working list and comment mode;
// the stream's deltas applied to it; the overlay and the side panels told
// of every change. What a restarted worker needs to pick a tab up again
// (its URL, whether the overlay is in it, comment mode, activeTab) is kept
// in session storage; the tab's page and threads are fetched again at its
// next message.
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Participants, Version } from "../../../shell/src/api";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { FeedbackState, Thread } from "../../../shell/src/threads";
import { type ThreadDelta, applyThread } from "../../../shell/src/view/deltas";
import { type ThreadChange, ThreadSync } from "../../../shell/src/view/thread-sync";
import type { Working } from "../../../shell/src/view/working-model";
import { type OverlayToWorker, type PageView, type PanelState, type WorkerToOverlay, type WorkerToPanel, isFromPanel, waitsForSnapshot } from "../messages";
import type { Api } from "./api";

type Owner = { public_id: string; display_name: string | null };

export type TabState = {
  tabId: number; url: string; page: PageView | null; route: string | null; threads: Thread[]; working: Working[];
  resolved: Record<string, AnchorResult>; commentMode: boolean; overlay: boolean; pending: boolean; selected: string | null;
  versions: Version[]; participants: Participants | null;
  /** A gesture granted activeTab since the tab's last full load. */
  active: boolean;
  error: { code: string; message: string } | null;
};

export const emptyTab = (tabId: number, url: string): TabState => ({
  tabId, url, page: null, route: null, threads: [], working: [], resolved: {}, commentMode: false, overlay: false, pending: false, selected: null,
  versions: [], participants: null, active: false, error: null,
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
export type TabsApi = Pick<Api, "lookup" | "threads" | "thread" | "working" | "artifact" | "me">;

type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
type Saved = { url: string; overlay: boolean; commentMode: boolean; active: boolean };
const STORE_KEY = "tabs";
const saved = (v: unknown): v is Saved => {
  const o = v as Partial<Saved> | null;
  return typeof o === "object" && o !== null && typeof o.url === "string" && /^https?:\/\//.test(o.url)
    && typeof o.overlay === "boolean" && typeof o.commentMode === "boolean" && typeof o.active === "boolean";
};

type Deps = {
  api: TabsApi;
  hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void };
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  /** Injects the overlay unless the tab's document has it; true when it injected it now. */
  inject(tabId: number): Promise<boolean>;
  /** Whether the tab's current document has the overlay (false when the worker cannot reach it). */
  present?(tabId: number): Promise<boolean>;
  /** Where the tabs are kept across worker restarts (chrome.storage.session). */
  store?: Area;
};

const failure = (e: unknown) => {
  const err = e as { code?: unknown; message?: unknown };
  return { code: typeof err?.code === "string" ? err.code : "failed", message: typeof err?.message === "string" ? err.message : String(e) };
};
const hubId = (tabId: number) => `tab:${tabId}`;
const tabOf = (id: string) => (id.startsWith("tab:") ? Number(id.slice(4)) : Number.NaN);

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
  private panels = new Set<{ port: chrome.runtime.Port; tabId: number | null }>();
  private viewer: Owner | null = null;
  private streamUp = true;
  private saving = false;
  private readonly loaded: Promise<void>;

  constructor(private readonly d: Deps) {
    this.loaded = this.restore();
  }

  /** Resolves once the tabs kept before a worker restart are back. */
  ready(): Promise<void> { return this.loaded; }

  state(tabId: number): TabState | undefined { return this.tabs.get(tabId); }

  /** Whether a gesture granted the tab activeTab since its last full load. */
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
        this.tabs.set(tabId, { ...emptyTab(tabId, v.url), overlay: v.overlay, commentMode: v.commentMode, active: v.active });
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
      for (const [id, s] of this.tabs) if (s.url) out[id] = { url: s.url, overlay: s.overlay, commentMode: s.commentMode, active: s.active };
      void this.d.store!.set({ [STORE_KEY]: out }).catch(() => {});
    });
  }

  private set(tabId: number, next: TabState): void {
    const prev = this.tabs.get(tabId);
    this.tabs.set(tabId, next);
    if (!prev || prev.url !== next.url || prev.overlay !== next.overlay || prev.commentMode !== next.commentMode || prev.active !== next.active) this.persist();
    this.d.toOverlay(tabId, { t: "state", page: next.page, route: next.route, threads: next.threads, commentMode: next.commentMode, pending: next.pending });
    this.tellPanels(tabId);
  }

  private tellPanels(tabId: number): void {
    for (const p of this.panels) if (p.tabId === tabId) p.port.postMessage({ t: "tab", state: this.panelState(tabId) } satisfies WorkerToPanel);
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
  async route(tabId: number, url: string, fresh = false): Promise<TabState> {
    const seq = (this.seqs.get(tabId) ?? 0) + 1;
    this.seqs.set(tabId, seq);
    const prev = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    const latest = () => this.seqs.get(tabId) === seq;
    const now = () => this.tabs.get(tabId) ?? prev;
    try {
      const { page, route } = await this.d.api.lookup(url);
      if (!latest()) return now();
      this.stale.delete(tabId);
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

  /** The loader greeted from a page load: a new document, so no overlay
   * and comment mode off (spec §11); the overlay comes when the page has
   * open threads. */
  async hello(tabId: number, url: string): Promise<void> {
    this.newDocument(tabId, url);
    const s = await this.route(tabId, url);
    if (s.threads.some(t => t.status === "open") && !s.overlay) await this.ensureOverlay(tabId);
  }

  /** The tab's record for a new document: no overlay, comment mode off.
   * The click's activeTab grant is kept: after a cross-document load no
   * Clax script runs where the origin is not on, and the next click grants
   * it again. */
  private newDocument(tabId: number, url: string): void {
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    this.set(tabId, { ...s, overlay: false, commentMode: false, resolved: {}, selected: null });
  }

  /** Chrome reported the tab loading. Only a new document (its overlay is
   * gone) resets the record; an in-page navigation, which Chrome may report
   * the same way, keeps everything, and the overlay reports its route. */
  async navigated(tabId: number): Promise<void> {
    const s = this.tabs.get(tabId);
    if (!s?.overlay) return;
    if (this.d.present && (await this.d.present(tabId))) return;
    if (this.tabs.has(tabId)) this.newDocument(tabId, s.url);
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

  /** Makes sure the tab's document has the overlay; true when it was injected now. */
  private async ensureOverlay(tabId: number): Promise<boolean> {
    const fresh = await this.d.inject(tabId);
    this.set(tabId, { ...(this.tabs.get(tabId) ?? emptyTab(tabId, "")), overlay: true });
    return fresh;
  }

  /** The icon, the command or the context menu: the overlay is made sure
   * of, and comment mode turns on when the overlay was just injected (a
   * new document, whatever the record said), else flips. */
  async toggle(tabId: number, url: string): Promise<void> {
    await this.loaded;
    if (!this.tabs.has(tabId)) this.tabs.set(tabId, emptyTab(tabId, url));
    let fresh: boolean;
    try {
      fresh = await this.ensureOverlay(tabId);
    } catch (e) {
      this.fail(tabId, e);
      return;
    }
    const s = this.tabs.get(tabId)!;
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

  /** The tab closed: its state and its topics go. */
  close(tabId: number): void {
    this.tabs.delete(tabId);
    this.seqs.delete(tabId);
    this.syncs.delete(tabId);
    this.workingAt.delete(tabId);
    this.stale.delete(tabId);
    this.d.hub.detach(hubId(tabId));
    this.persist();
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
    if (msg.t === "status") { this.status(ids, msg.up); return; }
    for (const id of ids) {
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

  private status(ids: string[], up: boolean): void {
    this.streamUp = up;
    const m = { t: "stream-status", up } as const;
    for (const p of this.panels) p.port.postMessage(m satisfies WorkerToPanel);
    for (const id of ids) {
      const tabId = tabOf(id);
      if (this.tabs.has(tabId)) this.d.toOverlay(tabId, m);
    }
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

  /** A message from the tab's loader or overlay. `senderUrl` is the
   * sender's URL: a tab the worker does not know (it restarted) is looked
   * up there again before the message is acted on. */
  async fromOverlay(tabId: number, _windowId: number, m: OverlayToWorker, senderUrl?: string): Promise<unknown> {
    await this.loaded;
    if (m.t === "hello") return this.hello(tabId, m.url);
    if (m.t === "route") { await this.route(tabId, m.url, this.stale.has(tabId)); return; }
    const known = this.tabs.get(tabId);
    if ((!known || this.stale.has(tabId)) && (senderUrl ?? known?.url)) await this.route(tabId, (senderUrl ?? known?.url)!, true);
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, senderUrl ?? "");
    switch (m.t) {
      case "resolved": this.set(tabId, { ...s, resolved: Object.fromEntries(m.results.map(r => [r.id, r])) }); return;
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
      commentMode: s?.commentMode ?? false, enabled: !!s?.overlay, selected: s?.selected ?? null, error: s?.error ?? null,
    };
  }

  /** A side panel's port: `watch-tab` picks the tab it shows, which it then
   * hears of on every change, with the stream's status. Every message it
   * sends passes `isFromPanel` before anything acts on it, and then goes to
   * `onMessage`. */
  attachPanel(port: chrome.runtime.Port, onMessage: (tabId: number | null, m: unknown) => void): void {
    const entry = { port, tabId: null as number | null };
    this.panels.add(entry);
    port.onMessage.addListener((m: unknown) => {
      if (!isFromPanel(m)) return;
      if (m.t === "watch-tab") {
        entry.tabId = m.tabId;
        port.postMessage({ t: "tab", state: this.panelState(entry.tabId) } satisfies WorkerToPanel);
        port.postMessage({ t: "stream-status", up: this.streamUp } satisfies WorkerToPanel);
        void this.details(m.tabId);
      }
      onMessage(entry.tabId, m);
    });
    port.onDisconnect.addListener(() => this.panels.delete(entry));
  }
}
