// The side panel's link to the worker (spec 2026-10-05 §9.4, §9.5): one
// port per window, named `panel:<windowId>`; the state of the window's
// active tab; whether the worker's stream is up; whether the panel is
// visible (the worker reports the owner here only while it is); and a ping
// every 20 s that keeps the worker up while the panel is open. Only
// messages `isToPanel` takes are acted on. When the worker goes away (Chrome
// stopped it), the link connects again and watches the tab again. An
// action's failure stays shown through the worker's later pushes until the
// person acts again or watches another tab. The owner's questions and
// inbox come over the same port (spec 2026-10-06-agent-questions-and-inbox
// §9.6): the unread count, the open questions (kept by `questions`, whose
// cards answer through the worker), and each item as it changes.
import type { InboxItem } from "../../../shell/src/api";
import { type FarPage, type PanelState, type PanelToWorker, type SiteChoice, type SiteView, type Suggestion, type WorkerToPanel, isToPanel } from "../messages";
import { PanelQuestions } from "./questions";

/** A request the worker answers with `step` (its `req` is the link's to give). */
export type Ask = { t: "rule"; origin: string; pattern: string } | { t: "unrule"; origin: string; ruleId: string }
  | { t: "join"; origin: string; with: string } | { t: "split"; origin: string };
/** A question or inbox request, without its `req`. */
type Req<M = PanelToWorker> = M extends { req: number } ? Omit<M, "req"> : never;
export type InboxReq = Exclude<Req, Ask | { t: "clip" | "far-page" }>;
/** How long a request waits for the worker's answer (one batch of at most 200 threads). */
export const REQUEST_MS = 120_000;
type Step = { moved: number; remaining: number };
type Failure = Error & { code: string };
const failure = (code: string, message: string): Failure => Object.assign(new Error(message), { code });

type Port = Pick<chrome.runtime.Port, "postMessage" | "onMessage" | "onDisconnect" | "disconnect">;
export type LinkEnv = {
  runtime: { connect(info: { name: string }): Port };
  tabs: Pick<typeof chrome.tabs, "query" | "onActivated" | "onUpdated">;
  /** The panel page's query: `?tab=<id>` pins it to one tab. */
  search: string;
  doc: Pick<Document, "visibilityState" | "addEventListener" | "removeEventListener">;
};
const chromeEnv = (): LinkEnv => ({ runtime: chrome.runtime, tabs: chrome.tabs, search: location.search, doc: document });

export const PING_MS = 20_000;
/** How long the link waits before connecting again after the worker went away. */
export const RECONNECT_MS = 500;

export class PanelLink {
  state = $state<PanelState | null>(null);
  /** Whether the worker's event stream is up. */
  up = $state(true);
  /** Every thread of the tab's site, as the worker last told it. */
  site = $state<SiteView | null>(null);
  /** Whether the tab's origin may be the same app as another site, as the worker last answered (spec §7.2). */
  suggestion = $state<{ origin: string; suggestion: Suggestion | null } | null>(null);
  /** Every site Clax has live pages of, once asked for (`list-sites`). */
  sites = $state<SiteChoice[] | null>(null);
  /** How many of the panel's actions have failed: a failure the panel was
   * told to hide shows again when an action fails again, even the same way. */
  failures = $state(0);
  /** Whether the daemon takes the extension as the owner (null until it answered), and the unread count. */
  inbox = $state<{ owner: boolean | null; unread: number }>({ owner: null, unread: 0 });
  /** The open questions, and those closing here for a moment. */
  readonly questions = new PanelQuestions(this);
  private items = new Set<(item: InboxItem | null) => void>();
  private reqs = 0;
  private asked = new Map<number, { ok(m: WorkerToPanel): void; fail(e: Failure): void }>();
  /** The lookups (`clip`, `far-page`) asked for and not yet answered, by request number: null when it failed. */
  private lookups = new Map<number, (m: WorkerToPanel | null) => void>();
  private port: Port;
  private tabId: number | null = null;
  /** The last action's failure, shown until the next action. */
  private failure: { code: string; message: string } | null = null;
  private closed = false;
  /** The port is new: its first watch says whether the panel is visible. */
  private fresh = true;
  private readonly pinned: number | null;
  private readonly beat: ReturnType<typeof setInterval>;
  private readonly onVisibility = () => this.post({ t: "visible", on: this.env.doc.visibilityState === "visible" });
  private readonly onActivated = (i: { windowId: number }) => { if (i.windowId === this.windowId) void this.follow(); };
  private readonly onUpdated = (_id: number, c: { url?: string }, tab: { active: boolean; windowId: number }) => {
    if (tab.active && tab.windowId === this.windowId && c.url) void this.follow();
  };

  constructor(private readonly windowId: number, private readonly env: LinkEnv = chromeEnv()) {
    // `?tab=<id>` pins the panel to one tab (the browser test opens the
    // panel's page in a tab of its own); otherwise it follows the window's
    // active tab.
    const pinned = Number(new URLSearchParams(env.search).get("tab"));
    this.pinned = Number.isSafeInteger(pinned) && pinned > 0 ? pinned : null;
    if (this.pinned === null) {
      env.tabs.onActivated.addListener(this.onActivated);
      env.tabs.onUpdated.addListener(this.onUpdated as never);
    }
    env.doc.addEventListener("visibilitychange", this.onVisibility);
    this.beat = setInterval(() => this.post({ t: "ping" }), PING_MS);
    this.port = this.connect();
    void this.follow();
  }

  private connect(): Port {
    const port = this.env.runtime.connect({ name: `panel:${this.windowId}` });
    this.fresh = true;
    port.onMessage.addListener((m: unknown) => {
      if (!isToPanel(m)) return;
      const lookup = "req" in m && m.req !== undefined ? this.lookups.get(m.req) : undefined;
      if (lookup) {
        this.lookups.delete((m as { req: number }).req);
        lookup(m.t === "failed" ? null : m);
        return;
      }
      const ask = "req" in m && m.req !== undefined ? this.asked.get(m.req) : undefined;
      if (ask) {
        this.asked.delete((m as { req: number }).req);
        if (m.t === "failed") ask.fail(failure(m.code, m.message));
        else ask.ok(m);
        return;
      }
      if (m.t === "tab") this.state = this.failure && !m.state.error ? { ...m.state, error: this.failure } : m.state;
      else if (m.t === "site") this.site = m.site;
      else if (m.t === "suggestion") this.suggestion = { origin: m.origin, suggestion: m.suggestion };
      else if (m.t === "sites") this.sites = m.sites;
      else if (m.t === "failed" && m.req === undefined && this.state) {
        this.failure = { code: m.code, message: m.message };
        this.state = { ...this.state, error: this.failure };
        this.failures++;
      }
      else if (m.t === "stream-status") this.up = m.up;
      else if (m.t === "inbox") { this.inbox = { owner: m.owner, unread: m.unread }; this.questions.list(m.questions); }
      else if (m.t === "q-event") this.questions.upsert(m.question);
      else if (m.t === "inbox-event") {
        this.inbox = { ...this.inbox, unread: m.unread };
        for (const f of [...this.items]) f(m.item);
      }
    });
    port.onDisconnect.addListener(() => {
      for (const a of this.asked.values()) a.fail(failure("worker_restarted", "Clax restarted. Try again."));
      this.asked.clear();
      for (const c of this.lookups.values()) c(null);
      this.lookups.clear();
      if (this.closed) return;
      setTimeout(() => {
        if (this.closed) return;
        this.port = this.connect();
        if (this.tabId !== null) this.watch(this.tabId);
        else void this.follow();
      }, RECONNECT_MS);
    });
    return port;
  }

  private watch(tabId: number): void {
    if (tabId !== this.tabId) this.failure = null;
    this.tabId = tabId;
    this.post({ t: "watch-tab", tabId });
    if (this.fresh) { this.fresh = false; this.onVisibility(); }
  }

  private async follow(): Promise<void> {
    if (this.pinned !== null) { this.watch(this.pinned); return; }
    const [tab] = await this.env.tabs.query({ active: true, windowId: this.windowId });
    if (tab?.id !== undefined && !this.closed) this.watch(tab.id);
  }

  post(m: PanelToWorker): void {
    if (this.closed) return;
    if (m.t !== "ping" && m.t !== "visible" && m.t !== "watch-tab" && m.t !== "clip" && m.t !== "far-page") this.failure = null;
    try { this.port.postMessage(m); } catch { /* the port closed; the link connects again */ }
  }

  /** Sends `m` with a request number of its own; answered by its `step`, or rejected with its failure. */
  request(m: Ask): Promise<Step> {
    return this.send(m).then(r => {
      if (r.t !== "step") throw failure("bad_reply", "Clax answered something else.");
      return { moved: r.moved, remaining: r.remaining };
    });
  }

  /** Sends a question or inbox request; resolves to the worker's answer, or rejects with its failure. */
  ask(m: InboxReq): Promise<WorkerToPanel> { return this.send(m); }

  /** Hears each inbox item as it changes (null: fetch what is shown again), until the returned function runs. */
  onItem(f: (item: InboxItem | null) => void): () => void {
    this.items.add(f);
    return () => { this.items.delete(f); };
  }

  private send(m: Ask | InboxReq): Promise<WorkerToPanel> {
    const req = ++this.reqs;
    return new Promise((ok, fail) => {
      const timer = setTimeout(() => {
        if (this.asked.delete(req)) fail(failure("timeout", "Clax did not answer. Try again."));
      }, REQUEST_MS);
      this.asked.set(req, { ok: r => { clearTimeout(timer); ok(r); }, fail: e => { clearTimeout(timer); fail(e); } });
      this.post({ ...m, req } as PanelToWorker);
    });
  }

  /** Asks the worker `m` with a request number of its own; answered by its
   * answer, or null on a failure, a lost worker or no answer in `REQUEST_MS`. */
  private lookup(m: { t: "clip" | "far-page"; threadId: string }): Promise<WorkerToPanel | null> {
    const req = ++this.reqs;
    return new Promise(ok => {
      const timer = setTimeout(() => { if (this.lookups.delete(req)) ok(null); }, REQUEST_MS);
      this.lookups.set(req, a => { clearTimeout(timer); ok(a); });
      this.post({ ...m, req });
    });
  }

  /** Thread `threadId`'s clip as a `data:` URL, which the worker fetches
   * (the panel holds no credential); null when it has none, or no answer
   * came. */
  async clip(threadId: string): Promise<string | null> {
    const a = await this.lookup({ t: "clip", threadId });
    return a?.t === "clip" ? a.url : null;
  }

  /** The live agents and versions of the page of thread `threadId`, another page of the tab's site; null when unknown. */
  async farPage(threadId: string): Promise<FarPage | null> {
    const a = await this.lookup({ t: "far-page", threadId });
    return a?.t === "far-page" ? a.page : null;
  }

  /** Stops the link (tests). */
  close(): void {
    this.closed = true;
    clearInterval(this.beat);
    this.env.tabs.onActivated.removeListener?.(this.onActivated);
    this.env.tabs.onUpdated.removeListener?.(this.onUpdated as never);
    this.env.doc.removeEventListener("visibilitychange", this.onVisibility);
    this.port.disconnect();
  }
}
