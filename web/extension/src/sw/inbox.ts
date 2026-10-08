// The owner's questions and inbox for the side panels (spec
// 2026-10-06-agent-questions-and-inbox §9.6). While any panel is open the
// worker holds one hub client, `inbox`, subscribed to `questions` and
// `inbox`; it keeps the open questions and the unread count and tells every
// panel. The extension's own owner check comes first: its credential is its
// identity, and the daemon may not take it as the owner. Only once
// `/api/inbox/summary` answered 200 are the topics subscribed, as one
// refused topic fails the whole stream change that carries it, the tabs'
// topics with it. Then, as the contract asks, each time the topics go live
// or resync the summary is fetched again, so no change falls between a
// fetch and the subscription. A refused topic (the daemon no longer takes
// the extension as the owner) drops the client. The check never starts a
// native host itself: it joins the pairing stored or under way. Without one,
// or when the daemon or host cannot be reached, it waits for a pairing
// (`kick`: a panel watching a tab or asking to retry, or a new pairing);
// other failures are retried a few times with a backoff.
import type { InboxFilter, InboxItem, InboxPage, QuestionView } from "../../../shell/src/api";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import { RETRYABLE, type WorkerToPanel } from "../messages";
import type { Api } from "./api";

/** The worker's hub client for the questions and the inbox. */
export const HUB_ID = "inbox";
export const TOPICS = ["questions", "inbox"];
/** The longest wait before a failed owner check is made again. */
export const RETRY_MAX_MS = 30_000;
/** How many times a failed owner check is made again before it waits for a `kick`. */
export const RETRY_LIMIT = 6;

type Port = Pick<chrome.runtime.Port, "postMessage" | "onDisconnect">;
export type InboxDeps = {
  /** `inboxSummary(true)`: only on a pairing stored or under way. */
  api: Pick<Api, "inboxSummary" | "inboxPage">;
  hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void };
  /** Runs `fn` in `ms` (a failed check's retry); returns what `cancel` takes. */
  after?(fn: () => void, ms: number): unknown;
  cancel?(h: unknown): void;
};

const byCreated = (a: QuestionView, b: QuestionView) => a.created_at.localeCompare(b.created_at);
const forbidden = (e: unknown) => (e as { status?: unknown })?.status === 403;
/** A failure only a pairing (or a reachable daemon) mends: no timer retries it. */
const unpaired = (e: unknown) => { const c = (e as { code?: unknown })?.code; return typeof c === "string" && (c === "not_paired" || RETRYABLE.has(c)); };

export class WorkerInbox {
  /** The open questions, oldest first. */
  questions: QuestionView[] = [];
  /** Unread items, as the daemon last counted them. */
  unread = 0;
  /** Whether the daemon takes the extension as the owner; null until it answered. */
  owner: boolean | null = null;
  private ports = new Set<Port>();
  private subscribed = false;
  private checking: Promise<void> | null = null;
  private wait = 0;
  private tries = 0;
  /** A kick came while a check ran: once it ends, it runs again unless it is done. */
  private again = false;
  private timer: unknown = null;

  constructor(private readonly d: InboxDeps) {}

  /** The open questions about artifact (or live page) `aid`. */
  forPage(aid: string): QuestionView[] {
    return this.questions.filter(q => q.artifact?.id === aid);
  }

  /** A panel's port: it hears the inbox now and on every change; the first
   * one starts the owner check and the subscription, the last one to go
   * stops them. */
  attach(port: Port): void {
    this.ports.add(port);
    this.post(port, this.state());
    port.onDisconnect.addListener(() => {
      this.ports.delete(port);
      if (!this.ports.size) this.stop();
    });
    if (this.ports.size === 1 || this.owner === null) void this.start();
  }

  /** A pairing may have landed (any pairing the worker made, or a panel
   * watched a tab or asked to retry): an owner check that is not done runs
   * again, after the one running now if there is one. */
  kick(): void {
    if (this.checking) { this.again = true; return; }
    if (!this.ports.size || this.subscribed || this.owner === false || this.timer !== null) return;
    this.tries = 0;
    void this.start();
  }

  private state(): WorkerToPanel {
    return { t: "inbox", owner: this.owner, unread: this.unread, questions: this.questions };
  }

  private post(port: Port, m: WorkerToPanel): void {
    try { (port as chrome.runtime.Port).postMessage(m); } catch { /* the panel closed */ }
  }

  private tell(m: WorkerToPanel = this.state()): void {
    for (const p of this.ports) this.post(p, m);
  }

  /** Checks the owner (once at a time), then subscribes the topics. */
  private start(): Promise<void> {
    this.checking ??= this.check().finally(() => {
      this.checking = null;
      if (this.again) { this.again = false; this.kick(); }
    });
    return this.checking;
  }

  private async check(): Promise<void> {
    if (this.timer !== null) { (this.d.cancel ?? clearTimeout)(this.timer as never); this.timer = null; }
    try {
      const r = await this.d.api.inboxSummary(true);
      this.owner = true;
      this.wait = 0;
      this.tries = 0;
      // The last panel closed meanwhile: what it answered would go stale.
      if (!this.ports.size) return;
      this.take(r);
    } catch (e) {
      if (forbidden(e)) {
        this.owner = false;
      } else if (this.ports.size && !unpaired(e) && this.tries < RETRY_LIMIT) {
        // Tried again after a backoff (1 s, doubling, at most RETRY_MAX_MS) while a panel is open.
        this.tries++;
        this.wait = Math.min(RETRY_MAX_MS, this.wait ? this.wait * 2 : 1000);
        this.timer = (this.d.after ?? setTimeout)(() => { this.timer = null; if (this.ports.size) void this.start(); }, this.wait);
      }
      if (this.ports.size) this.tell();
      return;
    }
    this.tell();
    if (this.ports.size && !this.subscribed) {
      this.subscribed = true;
      this.d.hub.receive(HUB_ID, { t: "topics", topics: TOPICS });
    }
  }

  private stop(): void {
    if (this.timer !== null) { (this.d.cancel ?? clearTimeout)(this.timer as never); this.timer = null; }
    if (this.subscribed) this.d.hub.detach(HUB_ID);
    this.subscribed = false;
    this.again = false;
    this.tries = 0;
    this.wait = 0;
    // Nothing is announced while no stream holds the topics: what is kept would go stale.
    this.questions = [];
  }

  private take(r: { unread: number; questions: QuestionView[] }): void {
    this.unread = r.unread ?? 0;
    this.questions = (r.questions ?? []).filter(q => q.status === "open").sort(byCreated);
  }

  /** Fetches the summary again (the topics went live or resynced). */
  async load(): Promise<void> {
    try {
      this.take(await this.d.api.inboxSummary(true));
    } catch (e) {
      if (forbidden(e)) this.refused();
      // Otherwise the next `live` or `resync` fetches it again.
      return;
    }
    this.tell();
  }

  private refused(): void {
    this.owner = false;
    this.stop();
    this.tell();
  }

  /** One event of the topics: a question is upserted (an open one listed,
   * a closed one dropped); an item or a bulk mark updates the count. The
   * panels hear each. */
  apply(name: string, data: Record<string, unknown>): void {
    if (typeof data.unread === "number" && Number.isSafeInteger(data.unread) && data.unread >= 0) this.unread = data.unread;
    if (name === "question") {
      const q = data.question as QuestionView | undefined;
      if (!q?.id) return;
      const rest = this.questions.filter(x => x.id !== q.id);
      this.questions = q.status === "open" ? [...rest, q].sort(byCreated) : rest;
      this.tell({ t: "q-event", question: q });
    } else if (name === "inbox_item" || name === "inbox_read") {
      const item = name === "inbox_item" ? (data.item as InboxItem | undefined) : undefined;
      // `inbox_read` (ids null): what the panels show is fetched again.
      this.tell({ t: "inbox-event", item: item?.id ? item : null, unread: this.unread });
    }
  }

  /** One hub message for the client `inbox`. */
  fromHub(msg: HubMsg): void {
    if (msg.t === "ping") this.d.hub.receive(HUB_ID, msg);
    else if (msg.t === "live" || msg.t === "resync") void this.load();
    else if (msg.t === "event") this.apply(msg.name, msg.data);
    else if (msg.t === "refused") this.refused();
  }

  /** One page of items matching `filter`, newest first, after `before`; its count is the panels' too. */
  async page(filter: InboxFilter, before: string | null): Promise<InboxPage> {
    const r = await this.d.api.inboxPage(filter, before);
    this.count(r.unread);
    return r;
  }

  /** A count an answer carried (a page, a mark): the panels hear it when it changed. */
  count(unread: number): void {
    if (!Number.isSafeInteger(unread) || unread < 0 || unread === this.unread) return;
    this.unread = unread;
    this.tell();
  }
}
