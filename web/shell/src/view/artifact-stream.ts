// The artifact view's watch of the page's event stream, loaded off the
// artifact entry once the view knows its viewer: the `artifact:<id>`,
// `presence:<id>` and `working:<id>` topics, and `docs:<id>` for a page that
// declares `db`. Thread and presence deltas are applied to what the view
// holds, and the view handles the full events they amount to. It also starts
// the owner's question surfaces (artifact-questions.ts, a module of their own).
import { ApiError, getArtifact, getToken, onToken } from "../api";
import type { ArtifactEvent } from "../events";
import { nav } from "../nav";
import { type StreamEvent, pageStream } from "../stream";
import { type FeedbackState, type Thread, getViewer, renamedViewer, upsert } from "../threads";
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
  /** The daemon had no live seat for this viewer (`no_live_seat`), or
   * released it for want of input (`seat_idle`): the view turns spectator. */
  spectate(code: string): void;
  /** The live topics are live (again): the view holds a live seat, or needs none. */
  seated(): void;
  disposed(): boolean;
};

/** The codes of a refused or released live seat (spec
 * 2026-10-11-clax-dev-hosted-design §16.1). */
export const SEAT_CODES: ReadonlySet<string> = new Set(["no_live_seat", "seat_idle"]);

/** One thread of the artifact, with all its comments. */
async function getThread(aid: string, tid: string): Promise<Thread> {
  const r = await fetch(`/api/artifacts/${aid}/threads/${tid}`);
  if (!r.ok) throw new Error(`thread request failed: ${r.status}`);
  return (await r.json()).thread as Thread;
}

const DOCS_EVENTS = new Set(["doc", "ready", "resync", "stream_down", "stream_up"]);

/** The caller the daemon fixes for this browser's stream when it opens, as
 * a key (the hub keys a stream's `ready` the same way, `callerKey`): with
 * the token (the events cookie) the owner's browser, at `admin` named or
 * not; without it the viewer, and whether it is named (a named viewer is at
 * `interact`, an unnamed one at `view`). */
const caller = () => Promise.all([getToken(), getViewer()]).then(([t, v]) => t ? OWNER : v.public_id + (v.display_name != null));
const OWNER = "admin";

/** Asks the hub for a new stream for every tab when the caller changed
 * (another viewer, one named or unnamed, the token gained): it reopens once
 * per caller, so another name for a named viewer, the owner's rename, and
 * every tab's announcement of one change after the first reopen nothing. */
export const reconnect = () => void caller().then(c => pageStream().reconnect(c), () => {});
// A token that arrives late (its first request failed) makes this browser
// the owner's: the caller changed. Asked at once, before the owner's topics
// the token starts reach the hub.
onToken(() => pageStream().reconnect(OWNER));

export class ArtifactStream {
  private readonly main: () => void;
  private docs: (() => void) | null = null;
  private questions: (() => void) | null = null;
  private stopped = false;
  /** Delivery states heard for threads this view does not hold yet, kept
   * until the thread arrives (a state may come before its thread). One is
   * given to its thread only when a thread event adds it: once the view
   * holds the thread (a load brought it, with its own newer state), the
   * kept one is dropped unused. */
  private readonly early = new Map<string, FeedbackState | null>();

  /** The artifact's live topics, which hold its live seat. */
  private readonly live: string[];

  constructor(private readonly id: string, private readonly v: ArtifactStreamView) {
    this.live = [`artifact:${id}`, `presence:${id}`, `working:${id}`];
    this.main = pageStream().watch(this.live, e => this.on(e));
    void import("./artifact-questions").then(m => { if (!this.stopped) this.questions = m.artifactQuestions(id); }, () => {});
  }

  /** Also watches the `docs` topic (once). */
  watchDocs(): void {
    this.docs ??= pageStream().watch([`docs:${this.id}`], e => {
      if (!this.v.disposed() && DOCS_EVENTS.has(e.type)) this.v.page(e as unknown as ArtifactEvent);
    });
  }

  /** Asks for the live topics again (Join live): `ready` if a seat is
   * free, else `refused` once more. */
  rejoin(): void {
    pageStream().retry(this.live);
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
      // No live seat: a spectator polls the snapshot instead.
      if (typeof e.topic === "string" && this.live.includes(e.topic) && SEAT_CODES.has(String(e.code))) {
        v.spectate(String(e.code));
        return;
      }
      // The daemon would not subscribe the artifact's topic (it was deleted
      // before the view subscribed, say): the view refetches, and learns why.
      if (e.topic === `artifact:${this.id}`) v.event({ type: "resync", topic: e.topic });
      return;
    }
    if (e.type === "resync" || e.type === "ready") this.early.clear();
    if (e.type === "feedback_state") {
      const f = e as unknown as { thread_id: string } & FeedbackState;
      if (!v.threads().some(t => t.id === f.thread_id)) { this.early.set(f.thread_id, stateOf(f)); return; }
      this.early.delete(f.thread_id);
    }
    if (e.type === "ready") v.seated();
    if (e.type === "thread") {
      const raw = (e as unknown as { thread: ThreadDelta }).thread;
      // A thread event carries no delivery state: the thread keeps the one
      // this viewer read (`feedback_state` events bring each its own), or
      // takes one heard before the thread itself when this event adds it.
      const early = this.early.get(raw.id);
      this.early.delete(raw.id);
      const added = !v.threads().some(t => t.id === raw.id);
      const d = added && early !== undefined && raw.feedback_state === undefined ? { ...raw, feedback_state: early } : raw;
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
  // Its tags changed, or the list renamed, recoloured or deleted one it
  // carries. An answer older than one shown is dropped (`setTags`).
  if (e.type === "tags") {
    const tt = c.tagTicket();
    getArtifact(c.id).then(d => { if (!c.disposed) c.setTags(tt, { tags: d.artifact.tags ?? [], may: !!d.artifact.may_tag }); }, () => {});
  }
  // A sharer turned the Comments switch: off, comment mode ends here too.
  if (e.type === "comments") c.noteComments(e.comments);
  // An approval or a withdrawal, here or in another view: what is in force changed.
  if (e.type === "permissions") c.permissionsChanged();
  if (e.type === "working") { c.workingAt = ++c.clock; c.set({ working: e.working }); }
  if (e.type === "drafts") { c.draftsAt = ++c.clock; c.draftsChanged(e.drafts); }
  // A name set in another view of this viewer (the owner's other browser,
  // the CLI) arrives in presence.
  if (e.type === "presence") { c.presenceAt = ++c.clock; c.set({ presence: e.people }); adoptName(e.people); }
  // An agent may have started or ended: the Send target and the roster follow.
  if (e.type === "working" || e.type === "version" || e.type === "agents") c.refreshAgents();
  if (e.type === "thread") c.changeThreads(ts => upsert(ts, keepState(ts.find(t => t.id === e.thread.id), e.thread)));
  if (e.type === "thread_deleted") { c.changeThreads(ts => ts.filter(t => t.id !== e.thread_id)); c.set(s => ({ selected: s.selected === e.thread_id ? null : s.selected })); }
  if (e.type === "feedback_state") c.changeThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: stateOf(e) } : t));
  // A (re)connect may follow a daemon restart that dropped events without a
  // resync; reload like a resync. The first one also covers anything
  // published between the initial load and the stream opening.
  if (e.type === "resync" || e.type === "ready") {
    c.loadThreads();
    c.loadDrafts();
    c.reporter?.fetch();
    const t = ++c.clock;
    const tt = c.tagTicket();
    getArtifact(c.id).then(d => {
      const n = d.artifact.current_version;
      c.setTags(tt, { tags: d.artifact.tags ?? [], may: !!d.artifact.may_tag });
      c.takeApproval(d.artifact);
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

/** `next`, keeping `prev`'s delivery state when `next` carries none: a
 * thread event reaches every viewer, so it says nothing of delivery (spec
 * 2026-10-08 §10); each viewer hears its own in `feedback_state` events. */
export function keepState(prev: Thread | undefined, next: Thread): Thread {
  return next.feedback_state === undefined ? { ...next, feedback_state: prev?.feedback_state ?? null } : next;
}

/** The delivery state a `feedback_state` event carries: this viewer's own
 * (whose approval and `mine` included, for those who read them); `null`
 * (`state: null`) once the thread has no delivery left. */
export function stateOf(e: { thread_id: string } & FeedbackState): FeedbackState | null {
  if (e.state === null) return null;
  const { thread_id, state, tier, since, resends, exhausted, approver, mine } = e;
  return { thread_id, state, tier, since, resends, exhausted, ...(approver === undefined ? {} : { approver }), ...(mine ? { mine } : {}) };
}
