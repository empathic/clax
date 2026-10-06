// What the worker knows about each tab with Clax (spec 2026-10-05 §9.4,
// §9.5): its URL, live page, route, threads, working list and comment mode;
// the stream's deltas applied to it; the overlay and the side panels told
// of every change.
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Participants, Version } from "../../../shell/src/api";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { FeedbackState, Thread } from "../../../shell/src/threads";
import { type ThreadDelta, applyThread } from "../../../shell/src/view/deltas";
import type { Working } from "../../../shell/src/view/working-model";
import { type OverlayToWorker, type PageView, type PanelState, type WorkerToOverlay, type WorkerToPanel, isFromPanel } from "../messages";
import type { Api } from "./api";

type Owner = { public_id: string; display_name: string | null };

export type TabState = {
  tabId: number; url: string; page: PageView | null; route: string | null; threads: Thread[]; working: Working[];
  resolved: Record<string, AnchorResult>; commentMode: boolean; overlay: boolean; pending: boolean; selected: string | null;
  versions: Version[]; participants: Participants | null;
  error: { code: string; message: string } | null;
};

export const emptyTab = (tabId: number, url: string): TabState => ({
  tabId, url, page: null, route: null, threads: [], working: [], resolved: {}, commentMode: false, overlay: false, pending: false, selected: null,
  versions: [], participants: null, error: null,
});

/** A thread as the daemon sends it for a live page: `addressed_pending`
 * while an agent's address waits for the page's next snapshot. */
type LiveThread = Thread & { addressed_pending?: { harness: string; at: string } | null };
const waiting = (t: Thread) => t.status === "open" && !!(t as LiveThread).addressed_pending;
const pendingOf = (threads: Thread[]) => threads.some(waiting);

const FEEDBACK = ["thread_id", "state", "tier", "since", "resends", "exhausted"] as const;

/** `s` with one stream event of its live page applied. */
export function applyEvent(s: TabState, name: string, data: Record<string, unknown>): TabState {
  let threads = s.threads;
  switch (name) {
    case "thread": threads = applyThread(threads, data.thread as ThreadDelta).threads; break;
    case "thread_deleted": threads = threads.filter(t => t.id !== data.thread_id); break;
    case "feedback_state": {
      const fs = Object.fromEntries(FEEDBACK.map(k => [k, data[k]])) as FeedbackState;
      threads = threads.map(t => (t.id === data.thread_id ? { ...t, feedback_state: fs } : t));
      break;
    }
    case "working": return { ...s, working: Array.isArray(data.working) ? (data.working as Working[]) : [] };
    case "version": {
      const n = Number(data.n);
      if (!s.page || data.artifact_id !== s.page.artifact_id || !Number.isSafeInteger(n) || n <= s.page.current_version) return s;
      return { ...s, page: { ...s.page, current_version: n } };
    }
    default: return s;
  }
  return { ...s, threads, pending: pendingOf(threads) };
}

/** The daemon calls the tabs make. */
export type TabsApi = Pick<Api, "lookup" | "threads" | "thread" | "working" | "artifact" | "me">;

type Deps = {
  api: TabsApi;
  hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void };
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  inject(tabId: number): Promise<void>;
};

const failure = (e: unknown) => {
  const err = e as { code?: unknown; message?: unknown };
  return { code: typeof err?.code === "string" ? err.code : "failed", message: typeof err?.message === "string" ? err.message : String(e) };
};
const hubId = (tabId: number) => `tab:${tabId}`;

export class Tabs {
  private tabs = new Map<number, TabState>();
  /** Each tab's latest lookup: an older one that answers late is dropped. */
  private seqs = new Map<number, number>();
  private panels = new Set<{ port: chrome.runtime.Port; tabId: number | null }>();
  private viewer: Owner | null = null;
  constructor(private readonly d: Deps) {}

  state(tabId: number): TabState | undefined { return this.tabs.get(tabId); }

  /** The open threads of the tab's page waiting for a snapshot: what a snapshot taken now covers. */
  pendingIds(tabId: number): string[] {
    return (this.tabs.get(tabId)?.threads ?? []).filter(waiting).map(t => t.id);
  }

  private set(tabId: number, next: TabState): void {
    this.tabs.set(tabId, next);
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

  /** The page's URL changed (a load or a route change), or its topics went
   * live (`fresh`): look its live page up, load its threads and working
   * list when the page changed or `fresh`, and follow its topics. */
  async route(tabId: number, url: string, fresh = false): Promise<TabState> {
    const seq = (this.seqs.get(tabId) ?? 0) + 1;
    this.seqs.set(tabId, seq);
    const prev = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    const latest = () => this.seqs.get(tabId) === seq;
    try {
      const { page, route } = await this.d.api.lookup(url);
      const same = !!page && page.artifact_id === prev.page?.artifact_id;
      let threads = page ? prev.threads : [];
      let working = page ? prev.working : [];
      if (page && (!same || fresh)) [threads, working] = await Promise.all([this.d.api.threads(page.artifact_id), this.d.api.working(page.artifact_id)]);
      if (!latest()) return this.tabs.get(tabId) ?? prev;
      // What changed meanwhile (comment mode, the overlay) is kept.
      const cur = this.tabs.get(tabId) ?? prev;
      const keep = same && cur.page?.artifact_id === page?.artifact_id;
      const next: TabState = {
        ...cur, url, page, route, threads, working, pending: pendingOf(threads), error: null,
        versions: keep ? cur.versions : [], participants: keep ? cur.participants : null, resolved: keep ? cur.resolved : {},
      };
      this.d.hub.receive(hubId(tabId), { t: "topics", topics: page ? [`artifact:${page.artifact_id}`, `working:${page.artifact_id}`] : [] });
      this.set(tabId, next);
      if (page && (!keep || fresh) && this.watched(tabId)) void this.details(tabId);
      return next;
    } catch (e) {
      if (!latest()) return this.tabs.get(tabId) ?? prev;
      const next = { ...(this.tabs.get(tabId) ?? prev), url, error: failure(e) };
      this.set(tabId, next);
      return next;
    }
  }

  /** The loader greeted from a page load: the overlay comes when the page has open threads. */
  async hello(tabId: number, url: string): Promise<void> {
    const s = await this.route(tabId, url);
    if (s.threads.some(t => t.status === "open") && !s.overlay) await this.injectOnce(tabId);
  }

  private async injectOnce(tabId: number): Promise<void> {
    if (this.tabs.get(tabId)?.overlay) return;
    await this.d.inject(tabId);
    this.set(tabId, { ...(this.tabs.get(tabId) ?? emptyTab(tabId, "")), overlay: true });
  }

  /** The icon, the command or the context menu: the overlay is injected and comment mode flips. */
  async toggle(tabId: number, url: string): Promise<void> {
    if (!this.tabs.has(tabId)) this.tabs.set(tabId, emptyTab(tabId, url));
    try {
      await this.injectOnce(tabId);
    } catch (e) {
      this.set(tabId, { ...this.tabs.get(tabId)!, error: failure(e) });
      return;
    }
    const s = this.tabs.get(tabId)!;
    this.set(tabId, { ...s, commentMode: !s.commentMode });
    void this.route(tabId, url);
  }

  /** The tab closed: its state and its topics go. */
  close(tabId: number): void {
    this.tabs.delete(tabId);
    this.seqs.delete(tabId);
    this.d.hub.detach(hubId(tabId));
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
    for (const id of ids) {
      if (!id.startsWith("tab:")) continue;
      const tabId = Number(id.slice(4));
      const s = this.tabs.get(tabId);
      if (!s) continue;
      if (msg.t === "event") {
        if (msg.data.artifact_id !== undefined && msg.data.artifact_id !== s.page?.artifact_id) continue;
        this.set(tabId, applyEvent(s, msg.name, msg.data));
        if (msg.name === "thread" && s.page && !applyThread(s.threads, msg.data.thread as ThreadDelta).complete) void this.refetchThread(tabId, s.page.artifact_id, (msg.data.thread as ThreadDelta).id);
        if (msg.name === "version" && this.watched(tabId)) void this.details(tabId);
      } else if (msg.t === "live" || msg.t === "resync") {
        void this.route(tabId, s.url, true);
      } else if (msg.t === "refused") {
        this.set(tabId, { ...s, error: { code: msg.code, message: `The daemon refused ${msg.topic}.` } });
      }
    }
  }

  /** A thread whose comments no longer add up, fetched whole. */
  private async refetchThread(tabId: number, aid: string, tid: string): Promise<void> {
    try {
      const { thread } = await this.d.api.thread(aid, tid);
      const s = this.tabs.get(tabId);
      if (!s || s.page?.artifact_id !== aid) return;
      const threads = s.threads.map(t => (t.id === tid ? thread : t));
      this.set(tabId, { ...s, threads, pending: pendingOf(threads) });
    } catch {
      // The next `live` or `resync` refetches the list.
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

  async fromOverlay(tabId: number, _windowId: number, m: OverlayToWorker): Promise<unknown> {
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, "");
    switch (m.t) {
      case "hello": return this.hello(tabId, m.url);
      case "route": await this.route(tabId, m.url); return;
      case "resolved": this.set(tabId, { ...s, resolved: Object.fromEntries(m.results.map(r => [r.id, r])) }); return;
      case "comment-mode": this.set(tabId, { ...s, commentMode: m.on }); return;
      case "pin": this.set(tabId, { ...s, selected: m.threadId }); return;
      case "removed": this.set(tabId, { ...s, overlay: false, error: { code: "overlay_removed", message: "The page removed Clax's overlay." } }); return;
      default: return; // capture, pick, quiet and cancel are the pick flow's (Task 13); ping keeps the worker alive
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
   * hears of on every change. Every message it sends passes `isFromPanel`
   * before anything acts on it, and then goes to `onMessage`. */
  attachPanel(port: chrome.runtime.Port, onMessage: (tabId: number | null, m: unknown) => void): void {
    const entry = { port, tabId: null as number | null };
    this.panels.add(entry);
    port.onMessage.addListener((m: unknown) => {
      if (!isFromPanel(m)) return;
      if (m.t === "watch-tab") {
        entry.tabId = m.tabId;
        port.postMessage({ t: "tab", state: this.panelState(entry.tabId) } satisfies WorkerToPanel);
        void this.details(m.tabId);
      }
      onMessage(entry.tabId, m);
    });
    port.onDisconnect.addListener(() => this.panels.delete(entry));
  }
}
