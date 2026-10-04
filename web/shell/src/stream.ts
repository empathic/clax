// The page's one event stream to the daemon (`GET /api/events`). Every view
// on the page watches it with its topics; the stream carries their union and
// hands each watcher the events its own topics name. It reopens with other
// topics when a watcher needs more, and after a failure with backoff, each
// time resuming after the last event it saw (`last_event_id`), so nothing is
// refetched unless the daemon could not resume. It closes as the page is
// hidden (a navigation, a reload, the back/forward cache) and resumes when
// the back/forward cache restores the page. While it is down the page shows
// a quiet notice. The token never goes in its URL: the daemon reads the
// events cookie `GET /api/token` sets for the shell.
import { STREAM_DOWN, connTrouble } from "./conn-notice";
import type { ArtifactEvent } from "./events";
import { backoff } from "./lifecycle";

/** Every event the daemon sends by name, besides `ready` and `resync`. */
export const EVENT_NAMES = ["version", "artifact_deleted", "thread", "comment", "thread_resolved", "thread_deleted", "feedback_state", "doc", "working", "presence"] as const;

/** What a watcher hears: one artifact's events (all when absent), of these
 * names (all when absent). `ready`, `resync` and `stream_down` reach every
 * watcher. */
export type Topics = { artifact?: string; types?: readonly string[] };

/** How long a connection may take to say `ready` before it counts as stuck. */
export const CONNECT_MS = 5000;
/** How long the stream may be down before the notice shows. */
export const NOTICE_MS = 1500;

type Timer = ReturnType<typeof setTimeout>;
type Watcher = { topics: Topics; on: (e: ArtifactEvent) => void; fresh: boolean };

const unique = (xs: string[]) => [...new Set(xs)];

/** Whether `e`, a named event, is one `t` asks for. */
function wants(t: Topics, e: ArtifactEvent): boolean {
  if (t.types && !t.types.includes(e.type)) return false;
  return !t.artifact || ("artifact_id" in e && e.artifact_id === t.artifact);
}

export class EventStream {
  private watchers: Watcher[] = [];
  private es: EventSource | null = null;
  /** The topics part of the open (or next) stream's query. */
  private query = "";
  /** The ID of the last event heard: the resume point. */
  private lastId = "";
  private failures = 0;
  /** The open stream said `ready`. */
  private up = false;
  /** Watchers were told `stream_down` for this outage. */
  private down = false;
  /** The page is hidden (leaving, or in the back/forward cache). */
  private hidden = false;
  private retry: Timer | undefined;
  private watchdog: Timer | undefined;
  private noticeTimer: Timer | undefined;
  private pageHooked = false;

  constructor(private readonly win: Window = window) {}

  /** The URL of the open stream, or null (tests). */
  get url(): string | null { return this.es?.url ?? null; }

  /** Hands `on` the events `topics` name, until the returned function runs.
   * A watcher's first `ready` comes once the stream is up; after that,
   * `ready` comes only when a reconnect could not resume. */
  watch(topics: Topics, on: (e: ArtifactEvent) => void): () => void {
    const w: Watcher = { topics, on, fresh: true };
    this.watchers.push(w);
    this.hookPage();
    if (this.hidden) return () => this.unwatch(w);
    if (!this.es && this.retry === undefined) this.open();
    else if (this.es && this.topicsQuery() !== this.query) this.open();
    else if (this.up) {
      queueMicrotask(() => {
        if (!w.fresh || !this.up || !this.watchers.includes(w)) return;
        w.fresh = false;
        w.on({ type: "ready" });
      });
    }
    return () => this.unwatch(w);
  }

  /** Reopens now, resuming after the last event (the subscriber's level is
   * fixed when a stream opens: a new viewer cookie takes effect so). */
  reconnect(): void {
    if (this.watchers.length && !this.hidden) this.open();
  }

  /** Closes the stream and forgets every watcher. */
  close(): void {
    this.watchers = [];
    this.stop();
    this.lastId = "";
    if (this.pageHooked) {
      this.win.removeEventListener("pagehide", this.onHide);
      this.win.removeEventListener("pageshow", this.onShow);
      this.pageHooked = false;
    }
  }

  private unwatch(w: Watcher): void {
    const i = this.watchers.indexOf(w);
    if (i < 0) return;
    this.watchers.splice(i, 1);
    // A narrower set of topics keeps the open stream: the watchers filter.
    if (!this.watchers.length) this.close();
  }

  private hookPage(): void {
    if (this.pageHooked) return;
    this.pageHooked = true;
    this.win.addEventListener("pagehide", this.onHide);
    this.win.addEventListener("pageshow", this.onShow);
  }

  private onHide = () => {
    this.hidden = true;
    this.stop();
  };

  private onShow = (e: Event) => {
    if (!(e as PageTransitionEvent).persisted) return;
    this.hidden = false;
    this.down = false;
    if (this.watchers.length) this.open();
  };

  private topicsQuery(): string {
    const ws = this.watchers;
    const parts: string[] = [];
    if (ws.length && ws.every(w => w.topics.artifact)) parts.push(`artifact=${unique(ws.map(w => w.topics.artifact!)).map(encodeURIComponent).join(",")}`);
    if (ws.length && ws.every(w => w.topics.types)) parts.push(`types=${unique(ws.flatMap(w => [...w.topics.types!])).map(encodeURIComponent).join(",")}`);
    return parts.join("&");
  }

  /** Ends the open stream and every timer; the watchers stay. */
  private stop(): void {
    this.closeSource();
    clearTimeout(this.retry);
    this.retry = undefined;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = undefined;
    connTrouble("stream", false, STREAM_DOWN, this.win.document);
  }

  private closeSource(): void {
    clearTimeout(this.watchdog);
    this.watchdog = undefined;
    this.es?.close();
    this.es = null;
    this.up = false;
  }

  private open(): void {
    this.closeSource();
    clearTimeout(this.retry);
    this.retry = undefined;
    if (this.hidden || !this.watchers.length || typeof EventSource === "undefined") return;
    this.query = this.topicsQuery();
    const q = [this.query, this.lastId ? `last_event_id=${encodeURIComponent(this.lastId)}` : ""].filter(Boolean).join("&");
    const es = new EventSource(`/api/events${q ? `?${q}` : ""}`);
    this.es = es;
    const mine = (fn: (e: MessageEvent) => void) => (e: Event) => { if (this.es === es) fn(e as MessageEvent); };
    for (const name of EVENT_NAMES) es.addEventListener(name, mine(e => this.event(e)));
    es.addEventListener("ready", mine(e => this.ready(e)));
    es.addEventListener("resync", mine(e => {
      let dropped = 0;
      try { dropped = Number(JSON.parse(e.data).dropped) || 0; } catch { /* malformed: 0 */ }
      this.tell({ type: "resync", dropped });
    }));
    es.addEventListener("error", mine(() => this.fail()));
    this.watchdog = setTimeout(() => { if (this.es === es && !this.up) this.fail(); }, CONNECT_MS);
  }

  private ready(e: MessageEvent): void {
    clearTimeout(this.watchdog);
    this.watchdog = undefined;
    if (e.lastEventId) this.lastId = e.lastEventId;
    let resumed = false;
    try { resumed = JSON.parse(e.data)?.resumed === true; } catch { /* not resumed */ }
    this.up = true;
    this.down = false;
    this.failures = 0;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = undefined;
    connTrouble("stream", false, STREAM_DOWN, this.win.document);
    for (const w of [...this.watchers]) {
      if (!w.fresh && resumed) continue;
      w.fresh = false;
      w.on({ type: "ready" });
    }
  }

  private event(e: MessageEvent): void {
    if (e.lastEventId) this.lastId = e.lastEventId;
    let ev: ArtifactEvent;
    try { ev = JSON.parse(e.data); } catch { return; }
    for (const w of [...this.watchers]) if (wants(w.topics, ev)) w.on(ev);
  }

  private tell(e: ArtifactEvent): void {
    for (const w of [...this.watchers]) w.on(e);
  }

  /** The stream failed or is stuck: closed, retried after a backoff, and the
   * notice shown once it has been down a while. */
  private fail(): void {
    this.closeSource();
    if (!this.down) {
      this.down = true;
      this.tell({ type: "stream_down" });
    }
    if (this.hidden || !this.watchers.length) return;
    this.retry = setTimeout(() => { this.retry = undefined; this.open(); }, backoff(this.failures++));
    this.noticeTimer ??= setTimeout(() => {
      this.noticeTimer = undefined;
      if (!this.up && !this.hidden && this.watchers.length) connTrouble("stream", true, STREAM_DOWN, this.win.document);
    }, NOTICE_MS);
  }
}

let page: EventStream | null = null;
/** This page's stream. */
export function pageStream(): EventStream {
  return (page ??= new EventStream());
}
