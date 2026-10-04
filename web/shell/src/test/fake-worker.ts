// A stand-in for the shared worker that holds the page's event stream:
// install it with `vi.stubGlobal("SharedWorker", FakeWorker)`, then read the
// topics the page holds and hand it hub messages (`live`, `event`, ...).
import type { HubMsg, TabMsg } from "../stream-hub";

export class FakeWorker {
  static all: FakeWorker[] = [];
  static get last(): FakeWorker | undefined { return FakeWorker.all.at(-1); }
  /** Every message the page sent, in order. */
  msgs: TabMsg[] = [];
  /** The page's latest topics. */
  topics: string[] = [];
  closed = false;
  readonly port = {
    onmessage: null as ((e: { data: HubMsg }) => void) | null,
    postMessage: (m: TabMsg) => { this.msgs.push(m); if (m.t === "topics") this.topics = m.topics; if (m.t === "bye") this.closed = true; },
    start() {},
    close: () => { this.closed = true; },
    addEventListener() {},
  };
  constructor(public url: URL | string, public opts?: { name?: string }) { FakeWorker.all.push(this); }
  addEventListener() {}
  /** Hands the page a hub message. */
  send(m: HubMsg): void { this.port.onmessage?.({ data: m }); }
  /** The page's topics (or `topics`) went live. */
  live(topics: string[] = this.topics): void { this.send({ t: "live", topics }); }
  /** One stream event of `topic`. */
  emit(topic: string, name: string, data: Record<string, unknown>): void { this.send({ t: "event", topic, name, data: { topic, ...data } }); }
}

/** Waits until the page holds a fake worker whose topics include every one of `topics`. */
export async function workerWith(...topics: string[]): Promise<FakeWorker> {
  const deadline = Date.now() + 3000;
  for (;;) {
    const w = [...FakeWorker.all].reverse().find((x: FakeWorker) => !x.closed && topics.every(t => x.topics.includes(t)));
    if (w) return w;
    if (Date.now() > deadline) throw new Error(`no stream with ${topics.join(", ")}; topics: ${FakeWorker.all.map(x => x.topics.join(",")).join(" | ")}`);
    await new Promise(r => setTimeout(r, 5));
  }
}

/** An artifact view's stream, driven by event name with the daemon's full
 * events (as `/api/events` carried them), handed to the view as the hub
 * hands it the deltas of the topic each event belongs to. */
export class ArtifactDriver {
  constructor(readonly w: FakeWorker, readonly id: string) {}
  get topics(): string[] { return this.w.topics; }
  emit(name: string, data: Record<string, unknown>): void {
    const { type: _type, ...fields } = data;
    if (name === "ready") return this.w.live();
    if (name === "resync") return this.w.send({ t: "resync", topic: `artifact:${this.id}` });
    if (name === "presence") return this.w.emit(`presence:${this.id}`, name, { ...fields, gone: [] });
    if (name === "working") return this.w.emit(`working:${this.id}`, name, fields);
    if (name === "thread") {
      const { comments = [], ...rest } = fields.thread as { comments?: unknown[] };
      return this.w.emit(`artifact:${this.id}`, name, { ...fields, thread: { ...rest, comment_count: comments.length, last_comment: comments.at(-1) ?? null } });
    }
    this.w.emit(`artifact:${this.id}`, name, fields);
  }
}

/** Stands in for the old `EventSource` fake: `last` is artifact `id`'s open
 * stream, and setting it forgets every fake worker. */
export function artifactStreams(id: string): { last: ArtifactDriver | undefined } {
  return {
    get last() {
      const w = [...FakeWorker.all].reverse().find((x: FakeWorker) => !x.closed && x.topics.includes(`artifact:${id}`));
      return w ? new ArtifactDriver(w, id) : undefined;
    },
    set last(_: ArtifactDriver | undefined) { FakeWorker.all = []; },
  };
}
