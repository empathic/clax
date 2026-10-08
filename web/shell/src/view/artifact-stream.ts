// The artifact view's watch of the page's event stream, loaded off the
// artifact entry once the view knows its viewer: the `artifact:<id>`,
// `presence:<id>` and `working:<id>` topics, and `docs:<id>` for a page that
// declares `db`. Thread and presence deltas are applied to what the view
// holds, and the view handles the full events they amount to. It also starts
// the owner's question surfaces (artifact-questions.ts, a module of their own).
import { ApiError, getArtifact } from "../api";
import type { ArtifactEvent } from "../events";
import { nav } from "../nav";
import { type StreamEvent, pageStream } from "../stream";
import { type Thread, getViewer, renamedViewer, upsert } from "../threads";
import type { ArtifactController } from "./artifact-controller";
import { holdKeysAcrossLoad } from "./keys";
import { type ThreadDelta, applyPresence, applyThread } from "./deltas";
import type { PresenceView } from "./presence-model";

/** What the stream reads from the view and hands it. */
export type ArtifactStreamView = {
  threads(): Thread[];
  presence(): PresenceView[];
  changeThreads(f: (ts: Thread[]) => Thread[]): void;
  /** Starts a load of one thread; the result takes its answer (undefined
   * when it failed), which applies in order with the changes made meanwhile. */
  beginThread(tid: string): (answer: Thread | undefined) => void;
  /** A full event for the view (and its page). */
  event(e: ArtifactEvent): void;
  /** An event for the page's capabilities alone (the `docs` topic). */
  page(e: ArtifactEvent): void;
  disposed(): boolean;
};

/** One thread of the artifact, with all its comments. */
async function getThread(aid: string, tid: string): Promise<Thread> {
  const r = await fetch(`/api/artifacts/${aid}/threads/${tid}`);
  if (!r.ok) throw new Error(`thread request failed: ${r.status}`);
  return (await r.json()).thread as Thread;
}

const DOCS_EVENTS = new Set(["doc", "ready", "resync", "stream_down", "stream_up"]);

/** Opens a new stream for every tab: the viewer changed. */
export const reconnect = () => pageStream().reconnect();

export class ArtifactStream {
  private readonly main: () => void;
  private docs: (() => void) | null = null;
  private questions: (() => void) | null = null;
  private stopped = false;

  constructor(private readonly id: string, private readonly v: ArtifactStreamView) {
    this.main = pageStream().watch([`artifact:${id}`, `presence:${id}`, `working:${id}`], e => this.on(e));
    void import("./artifact-questions").then(m => { if (!this.stopped) this.questions = m.artifactQuestions(id); }, () => {});
  }

  /** Also watches the `docs` topic (once). */
  watchDocs(): void {
    this.docs ??= pageStream().watch([`docs:${this.id}`], e => {
      if (!this.v.disposed() && DOCS_EVENTS.has(e.type)) this.v.page(e as unknown as ArtifactEvent);
    });
  }

  stop(): void {
    this.stopped = true;
    this.questions?.();
    this.questions = null;
    this.main();
    this.docs?.();
    this.docs = null;
  }

  private on(e: StreamEvent): void {
    const v = this.v;
    if (v.disposed()) return;
    if (e.type === "refused") {
      // The daemon would not subscribe the artifact's topic (it was deleted
      // before the view subscribed, say): the view refetches, and learns why.
      if (e.topic === `artifact:${this.id}`) v.event({ type: "resync", topic: e.topic });
      return;
    }
    if (e.type === "thread") {
      const d = (e as unknown as { thread: ThreadDelta }).thread;
      const r = applyThread(v.threads(), d);
      v.changeThreads(ts => applyThread(ts, d).threads);
      v.page({ type: "thread", artifact_id: this.id, thread: r.thread });
      // A comment this view never saw (an edit or a delete before the
      // newest): the thread is fetched whole, and the page hears it again.
      if (!r.complete) {
        const done = v.beginThread(d.id);
        getThread(this.id, d.id).then(t => {
          if (v.disposed()) return;
          done(t);
          const now = v.threads().find(x => x.id === d.id);
          if (now) v.page({ type: "thread", artifact_id: this.id, thread: now });
        }, () => done(undefined));
      }
      return;
    }
    if (e.type === "presence") {
      const d = e as unknown as { people?: PresenceView[]; gone?: string[] };
      v.event({ type: "presence", artifact_id: this.id, people: applyPresence(v.presence(), d.people ?? [], d.gone ?? []) });
      return;
    }
    v.event(e as unknown as ArtifactEvent);
  }
}

/** The artifact view's handling of one full event from its stream. It
 * lives here, off the artifact entry, as it runs only once the stream is
 * open. Working deltas and the refetches on `ready` and `resync` are
 * ordered by the view's clock: an answer never replaces working state a
 * delta set after its request started. */
export function onArtifactEvent(c: ArtifactController, e: ArtifactEvent): void {
  if (c.disposed) return;
  c.host?.onEvent(e);
  if (e.type === "version" && e.by_page && e.n > c.shown()) {
    // The page republished itself (artifact.publish): every unpinned view
    // follows at once, on the page it shows (the new version carries every
    // file forward); a pinned view is offered Reload in the top bar. The
    // publishing view reloads itself after its call result is posted; while
    // one of its own publishes is in flight, another view's publish waits
    // for it to settle.
    if (c.ownPublish.active > 0) { c.deferredPublish = Math.max(c.deferredPublish ?? 0, e.n); return; }
    if (c.pinnedVersion === null) {
      c.latestKnown = Math.max(c.latestKnown, e.n);
      holdKeysAcrossLoad();
      nav.assign(c.here(null));
      return;
    }
  }
  if (e.type === "version" && e.n > c.latestKnown) { c.latestKnown = e.n; c.set({ newer: e.n }); }
  if (e.type === "artifact_deleted") c.set({ deleted: true });
  if (e.type === "working") { c.workingAt = ++c.clock; c.set({ working: e.working }); }
  // A name set in another view of this viewer (the owner's other browser,
  // the CLI) arrives in presence.
  if (e.type === "presence") { c.set({ presence: e.people }); adoptName(e.people); }
  // An agent may have started or ended: the Send target and the roster follow.
  if (e.type === "working" || e.type === "version") c.refreshAgents();
  if (e.type === "thread") c.changeThreads(ts => upsert(ts, e.thread));
  if (e.type === "thread_deleted") { c.changeThreads(ts => ts.filter(t => t.id !== e.thread_id)); c.set(s => ({ selected: s.selected === e.thread_id ? null : s.selected })); }
  if (e.type === "feedback_state") c.changeThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: { thread_id: e.thread_id, state: e.state, tier: e.tier, since: e.since, resends: e.resends, exhausted: e.exhausted } } : t));
  // A (re)connect may follow a daemon restart that dropped events without a
  // resync; reload like a resync. The first one also covers anything
  // published between the initial load and the stream opening.
  if (e.type === "resync" || e.type === "ready") {
    c.loadThreads();
    c.reporter?.fetch();
    const t = ++c.clock;
    getArtifact(c.id).then(d => {
      const n = d.artifact.current_version;
      // A working delta heard since the request started is newer than its answer.
      c.set(s => ({ working: c.workingAt > t ? s.working : d.artifact.working ?? [], attention: d.attention ?? s.attention, looked: { ...s.looked, ...d.attention?.looked } }));
      c.agentsFetched(t, d.artifact.participants?.agents ?? []);
      if (n > c.latestKnown) { c.latestKnown = n; c.set({ newer: n }); }
    }, err => { if (err instanceof ApiError && err.status === 404) c.set({ deleted: true }); });
  }
}

/** This viewer's name as `people` (a presence list) gives it, taken when it
 * differs: it was set in another view of this viewer (the owner's other
 * browser, or the CLI). */
function adoptName(people: PresenceView[]): void {
  void getViewer().then(v => {
    const mine = people.find(p => p.public_id === v.public_id);
    if (mine && mine.display_name !== v.display_name) renamedViewer({ ...v, display_name: mine.display_name });
  }, () => {});
}
