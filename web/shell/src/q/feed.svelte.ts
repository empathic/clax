// The page's open questions and inbox count (spec
// 2026-10-06-agent-questions-and-inbox §9.2–§9.5, §9.7), following the
// `questions` and `inbox` topics. Each feed first asks an owner route: a
// viewer who is not the owner gets 403, and the feed stays empty and never
// subscribes, since the hub sends every tab's topics in one request (ruling
// R8). An owner's feed then subscribes, as a background watcher (kept while
// the page is hidden), and fetches again each time its topic goes live or
// resyncs, so no change falls between a fetch and the subscription.
import { ApiError, type AnswerBody, type InboxFilter, type InboxItem, type QuestionView } from "../api";
import { after } from "../clock";
import type { EventStream, StreamEvent } from "../stream";
import { type Closing, type Forbidden, answerQuestion, declineQuestion, inboxSummary, listInbox, listQuestions, markInbox, markInboxMany, releaseQuestion } from "./api";

/** How long a closed question's card stays, showing what closed it. */
export const LEAVE_MS = 4000;
/** How long the inbox feed waits after an item changed before fetching `latest`. */
export const SUMMARY_MS = 100;

type Stream = Pick<EventStream, "watch">;
type After = (ms: number, fn: () => void) => () => void;

const byCreated = (a: QuestionView, b: QuestionView) => a.created_at.localeCompare(b.created_at);
const forbidden = (): ApiError => new ApiError(403, "Only the owner may do this", "forbidden");

/** A deeply reactive record. The feeds keep their state in one, not in
 * `$state` class fields, which compile to private fields the build lowers
 * to helper calls on every access. */
function reactive<T extends object>(v: T): T {
  const r = $state(v);
  return r;
}

export class QuestionFeed {
  declare private s: { open: QuestionView[]; recent: QuestionView[]; owner: boolean };
  declare private changed: Set<() => void>;
  declare private leave: Map<string, () => void>;
  declare private unwatch: (() => void) | null;
  declare private readonly after: After;
  declare private runs: number;

  constructor(opts: { after?: After } = {}) {
    this.runs = 0;
    this.s = reactive({ open: [], recent: [], owner: false });
    this.changed = new Set();
    this.leave = new Map();
    this.unwatch = null;
    this.after = opts.after ?? after;
  }

  /** Open questions, oldest first. */
  get open(): QuestionView[] { return this.s.open; }
  set open(v: QuestionView[]) { this.s.open = v; this.tell(); }
  /** Questions that closed while shown, for `LEAVE_MS`, so their cards say what closed them. */
  get recent(): QuestionView[] { return this.s.recent; }
  set recent(v: QuestionView[]) { this.s.recent = v; this.tell(); }
  /** The caller is the owner (the first fetch answered). */
  get owner(): boolean { return this.s.owner; }

  /** Calls `f` after each change of `open` or `recent`, until the returned function runs. */
  listen(f: () => void): () => void {
    this.changed.add(f);
    return () => { this.changed.delete(f); };
  }

  private tell(): void {
    for (const f of [...this.changed]) f();
  }

  /** Open and recently closed questions, oldest first: the cards to show. */
  get cards(): QuestionView[] {
    return [...this.open, ...this.recent].sort(byCreated);
  }

  /** The cards about artifact `aid`. */
  byArtifact(aid: string): QuestionView[] {
    return this.cards.filter(q => q.artifact?.id === aid);
  }

  /** The question `id` as last heard, open or recently closed. */
  get(id: string): QuestionView | undefined {
    return this.open.find(q => q.id === id) ?? this.recent.find(q => q.id === id);
  }

  /** Fetches the open questions and, for the owner, follows the topic. Resolves false for anyone else. */
  async start(stream: Stream): Promise<boolean> {
    if (this.unwatch) return true;
    const run = this.runs;
    if (!(await this.refetch()) || run !== this.runs) return false;
    this.unwatch ??= stream.watch(["questions"], e => this.on(e), { background: true });
    return true;
  }

  /** Stops following the topic (a start still asking stops too). */
  stop(): void {
    this.runs++;
    this.unwatch?.();
    this.unwatch = null;
    for (const c of this.leave.values()) c();
    this.leave.clear();
  }

  /** Answers question `id`; rejects when the answer was not sent. */
  answer(id: string, b: AnswerBody): Promise<void> { return this.close(answerQuestion(id, b)); }
  /** Skips question `id`. */
  decline(id: string): Promise<void> { return this.close(declineQuestion(id)); }
  /** Moves question `id` to the terminal (mirrored questions). */
  release(id: string): Promise<void> { return this.close(releaseQuestion(id)); }

  /** Takes a question view: an open one is listed (or replaced); one that
   * closed leaves the open list and stays in `recent` for `LEAVE_MS`. A
   * closed question never opens again, whatever order views arrive in. */
  upsert(q: QuestionView): void {
    const was = this.open.findIndex(x => x.id === q.id);
    if (q.status === "open") {
      if (this.recent.some(x => x.id === q.id)) return;
      if (was >= 0) this.open = this.open.map(x => (x.id === q.id ? q : x));
      else this.open = [...this.open, q].sort(byCreated);
      return;
    }
    const r = this.recent.findIndex(x => x.id === q.id);
    if (r >= 0) { this.recent = this.recent.map(x => (x.id === q.id ? q : x)); return; }
    if (was < 0) return;
    this.open = this.open.filter(x => x.id !== q.id);
    this.recent = [...this.recent, q];
    this.leave.set(q.id, this.after(LEAVE_MS, () => {
      this.leave.delete(q.id);
      this.recent = this.recent.filter(x => x.id !== q.id);
    }));
  }

  private async close(p: Promise<Closing | Forbidden>): Promise<void> {
    const r = await p;
    if (r === "forbidden") throw forbidden();
    this.upsert("question" in r ? r.question : r.closed);
  }

  /** Fetches the open list; false when the caller is not the owner. */
  private async refetch(): Promise<boolean> {
    let r: Awaited<ReturnType<typeof listQuestions>>;
    try { r = await listQuestions("open", 200); } catch { return this.s.owner; }
    if (r === "forbidden") { this.s.owner = false; this.open = []; return false; }
    this.s.owner = true;
    // Those that closed unheard leave at once; a card closing now keeps its time.
    this.open = (r.questions ?? []).filter(q => !this.recent.some(x => x.id === q.id)).sort(byCreated);
    return true;
  }

  private on(e: StreamEvent): void {
    if (e.type === "question") { const q = e.question as QuestionView | undefined; if (q?.id) this.upsert(q); }
    else if (e.type === "ready" || e.type === "resync") void this.refetch();
    else if (e.type === "refused") this.stop();
  }
}

/** What the inbox feed tells its listeners: an item as it is now (made,
 * marked, or its source changed), or that what they show must be fetched again. */
export type InboxChange = { item: InboxItem } | { refetch: true };

export class InboxFeed {
  declare private s: { unread: number; latest: InboxItem[]; owner: boolean };
  declare private counted: Set<(n: number) => void>;
  declare private unwatch: (() => void) | null;
  declare private listeners: Set<(c: InboxChange) => void>;
  declare private later: (() => void) | null;
  declare private readonly after: After;

  declare private runs: number;

  constructor(opts: { after?: After } = {}) {
    this.runs = 0;
    this.s = reactive({ unread: 0, latest: [], owner: false });
    this.counted = new Set();
    this.unwatch = null;
    this.listeners = new Set();
    this.later = null;
    this.after = opts.after ?? after;
  }

  /** Unread items, as the daemon last counted them. */
  get unread(): number { return this.s.unread; }
  set unread(n: number) {
    if (n === this.s.unread) return;
    this.s.unread = n;
    for (const f of [...this.counted]) f(n);
  }
  /** The five newest unread items other than questions. */
  get latest(): InboxItem[] { return this.s.latest; }
  set latest(v: InboxItem[]) { this.s.latest = v; }
  /** The caller is the owner (the summary answered). */
  get owner(): boolean { return this.s.owner; }

  /** Calls `f` with each new unread count, until the returned function runs. */
  onCount(f: (n: number) => void): () => void {
    this.counted.add(f);
    return () => { this.counted.delete(f); };
  }

  /** Fetches the summary and, for the owner, follows the topic. Resolves false for anyone else. */
  async start(stream: Stream): Promise<boolean> {
    if (this.unwatch) return true;
    const run = this.runs;
    if (!(await this.summary()) || run !== this.runs) return false;
    this.unwatch ??= stream.watch(["inbox"], e => this.on(e), { background: true });
    return true;
  }

  /** Stops following the topic (a start still asking stops too). */
  stop(): void {
    this.runs++;
    this.unwatch?.();
    this.unwatch = null;
    this.later?.();
    this.later = null;
  }

  /** Hears each change until the returned function runs. */
  listen(f: (c: InboxChange) => void): () => void {
    this.listeners.add(f);
    return () => { this.listeners.delete(f); };
  }

  /** Fetches the summary (the count and `latest`); false when the caller is not the owner. */
  async summary(): Promise<boolean> {
    let r: Awaited<ReturnType<typeof inboxSummary>>;
    try { r = await inboxSummary(); } catch { return this.s.owner; }
    if (r === "forbidden") { this.s.owner = false; return false; }
    this.s.owner = true;
    this.unread = r.unread ?? 0;
    this.latest = r.latest ?? [];
    return true;
  }

  /** One page of items matching `filter`, newest first, after cursor `before`. */
  async page(filter: InboxFilter, before?: string | null, limit?: number) {
    const r = await listInbox(filter, before, limit);
    if (r === "forbidden") throw forbidden();
    this.unread = r.unread;
    return r;
  }

  /** Marks item `id` read or unread; resolves to the item as it is now. */
  async mark(id: string, read: boolean): Promise<InboxItem> {
    const r = await markInbox(id, read);
    if (r === "forbidden") throw forbidden();
    this.unread = r.unread;
    this.take(r.item);
    return r.item;
  }

  /** Marks every unread item read, or every one matching `filter`, up to
   * `upto` (the newest `seq` shown) when given. */
  async markAll(filter?: InboxFilter, upto?: number): Promise<number> {
    const r = await markInboxMany({ all: true, filter, upto });
    if (r === "forbidden") throw forbidden();
    this.unread = r.unread;
    return r.marked;
  }

  /** Takes an item as it is now: `latest` is fetched again (soon, so a
   * burst is one fetch) when the item may join or leave it; the listeners hear it. */
  private take(item: InboxItem): void {
    if (item.kind !== "question" && (!item.read || this.latest.some(x => x.id === item.id))) {
      this.later ??= this.after(SUMMARY_MS, () => { this.later = null; void this.summary(); });
    }
    this.tell({ item });
  }

  private tell(c: InboxChange): void {
    for (const f of [...this.listeners]) f(c);
  }

  private on(e: StreamEvent): void {
    if (typeof e.unread === "number") this.unread = e.unread;
    if (e.type === "inbox_item") {
      const item = e.item as InboxItem | undefined;
      if (item?.id) this.take(item);
    } else if (e.type === "inbox_read" || e.type === "ready" || e.type === "resync") {
      void this.summary();
      this.tell({ refetch: true });
    } else if (e.type === "refused") this.stop();
  }
}
