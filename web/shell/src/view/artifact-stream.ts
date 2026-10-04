// The artifact view's watch of the page's event stream, loaded off the
// artifact entry once the view knows its viewer: the `artifact:<id>`,
// `presence:<id>` and `working:<id>` topics, and `docs:<id>` for a page that
// declares `db`. Thread and presence deltas are applied to what the view
// holds, and the view handles the full events they amount to.
import type { ArtifactEvent } from "../events";
import { type StreamEvent, pageStream } from "../stream";
import type { Thread } from "../threads";
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

  constructor(private readonly id: string, private readonly v: ArtifactStreamView) {
    this.main = pageStream().watch([`artifact:${id}`, `presence:${id}`, `working:${id}`], e => this.on(e));
  }

  /** Also watches the `docs` topic (once). */
  watchDocs(): void {
    this.docs ??= pageStream().watch([`docs:${this.id}`], e => {
      if (!this.v.disposed() && DOCS_EVENTS.has(e.type)) this.v.page(e as unknown as ArtifactEvent);
    });
  }

  stop(): void {
    this.main();
    this.docs?.();
    this.docs = null;
  }

  private on(e: StreamEvent): void {
    const v = this.v;
    if (v.disposed() || e.type === "refused") return;
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
