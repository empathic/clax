// The daemon's API as the extension uses it (spec 2026-10-05 §9.2): every
// request carries the credential and no cookie; a 401 or an unreachable
// daemon pairs again (at most once per request) and retries once. A request
// the daemon may have carried out before its answer was lost (a comment, a
// send) is not sent again after a network error: the person sees the
// failure and retries. A new thread is sent again: it names its pick, which
// the daemon makes one thread of.
import type { AnswerBody, Artifact, InboxFilter, InboxItem, InboxPage, InboxSummary, QuestionView, Version } from "../../../shell/src/api";
import { inboxQuery } from "../../../shell/src/q/api";
import type { Thread, Viewer } from "../../../shell/src/threads";
import type { PresenceView } from "../../../shell/src/view/presence-model";
import type { Working } from "../../../shell/src/view/working-model";
import type { PageView, SiteInfo, SiteRule, SiteView } from "../messages";
import { bytesDataUrl } from "../data-url";
import { MAX_CLIP } from "./capture";
import { PairError, type Pairer, type Pairing } from "./pairing";

export class ApiFailure extends Error {
  constructor(readonly code: string, message: string, readonly status = 0) { super(message); }
}

const AID = /^[0-9a-hjkmnp-tv-z]{12}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
function ids(aid: string, tids: readonly string[] = []): void {
  if (!AID.test(aid) || !tids.every(t => ULID.test(t))) throw new ApiFailure("invalid_id", "not an artifact or thread ID");
}
function threadIds(tids: readonly string[]): void {
  if (!tids.every(t => ULID.test(t))) throw new ApiFailure("invalid_id", "not a thread ID");
}

export class Api {
  /** Called with each pairing a request made: the old credential's stream cannot be changed or resumed by the new one. */
  onRepair: ((p: Pairing) => void) | null = null;
  /** The credential `onRepair` last heard of. */
  private told: string | null = null;
  /** Requests not yet answered, and what waits for there to be none. */
  private active = 0;
  private idlers: (() => void)[] = [];

  constructor(private readonly pairer: Pick<Pairer, "current" | "pair">, private readonly fetchFn: typeof fetch = (...a) => fetch(...a)) {}

  /** `path` on the daemon, with the credential. After a 401 or a network
   * error the request is retried once: with a pairing another request
   * renewed meanwhile, else with a new one. When pairing again is refused
   * (`paired_recently`), the 401 is the answer, or `daemon_unreachable`.
   * With `once`, a network error pairs again but is the answer
   * (`daemon_unreachable`): the request is not sent twice. */
  async request(path: string, init: RequestInit = {}, once = false): Promise<Response> {
    this.active++;
    try {
      return await this.attempt(path, init, once);
    } finally {
      this.active--;
      queueMicrotask(() => this.drain());
    }
  }

  /** Runs `fn` once no request waits for its answer (at once when none
   * does): a stream's request counts until its response starts. */
  whenIdle(fn: () => void): void {
    this.idlers.push(fn);
    this.drain();
  }
  private drain(): void {
    if (this.active) return;
    for (const fn of this.idlers.splice(0)) fn();
  }

  private async attempt(path: string, init: RequestInit, once: boolean): Promise<Response> {
    let p = await this.pairer.current();
    for (let attempt = 0; ; attempt++) {
      const headers = new Headers(init.headers);
      headers.set("authorization", `Clax-Extension ${p.credential}`);
      let res: Response | null = null;
      let failed: unknown = null;
      try {
        res = await this.fetchFn(p.daemon + path, { ...init, headers, credentials: "omit" });
        if (res.status !== 401 || attempt > 0) return res;
      } catch (e) {
        if (attempt > 0 || init.signal?.aborted) throw new ApiFailure("daemon_unreachable", String(e));
        failed = e;
      }
      const used = p;
      p = await this.pairer.current();
      if (p.credential === used.credential && p.daemon === used.daemon) {
        try {
          p = await this.pairer.pair();
        } catch (e) {
          if (!(e instanceof PairError && e.code === "paired_recently")) throw e;
          if (res) return res;
          throw new ApiFailure("daemon_unreachable", String(failed));
        }
        // Requests that shared one pairing tell of it once.
        if (p.credential !== this.told) {
          this.told = p.credential;
          this.onRepair?.(p);
        }
      }
      if (failed !== null && once) throw new ApiFailure("daemon_unreachable", String(failed));
    }
  }

  async json<T>(path: string, init?: RequestInit, once = false): Promise<T> {
    const res = await this.request(path, init, once);
    const body = await res.json().catch(() => null) as { error?: { code?: string; message?: string } } | null;
    if (!res.ok) throw new ApiFailure(body?.error?.code ?? `http_${res.status}`, body?.error?.message ?? res.statusText, res.status);
    return body as T;
  }

  private send<T>(method: string, path: string, body?: unknown, once = false): Promise<T> {
    return this.json<T>(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) }, once);
  }

  lookup(url: string) { return this.json<{ page: PageView | null; route: string | null }>(`/api/live/pages?url=${encodeURIComponent(url)}`); }
  async artifact(aid: string) { ids(aid); return this.json<{ artifact: Artifact; versions: Version[] }>(`/api/artifacts/${aid}`); }
  async threads(aid: string) { ids(aid); return (await this.json<{ threads: Thread[] }>(`/api/artifacts/${aid}/threads?include_resolved=true&limit=200`)).threads; }
  async thread(aid: string, tid: string) { ids(aid, [tid]); return this.json<{ thread: Thread }>(`/api/artifacts/${aid}/threads/${tid}`); }
  async working(aid: string) { ids(aid); return (await this.json<{ working: Working[] }>(`/api/artifacts/${aid}/working`)).working; }
  /** A comment with its page's snapshot. `pending` names the threads the
   * page state showed waiting for a snapshot when it was serialized (`[]`
   * when none); the daemon links only those. The form names its pick
   * (`pick_id`), so a retry makes no second thread. */
  async postThread(form: FormData, pending: string[]) {
    threadIds(pending);
    form.set("pending", JSON.stringify(pending));
    return this.json<{ thread: Thread; page: PageView; version: number; clip_error?: string }>("/api/live/threads", { method: "POST", body: form });
  }
  /** A snapshot for the pending addresses of `pending`, as `postThread`. */
  async postSnapshot(form: FormData, pending: string[]) {
    threadIds(pending);
    form.set("pending", JSON.stringify(pending));
    return this.json<{ page: PageView; version: number; linked: string[] }>("/api/live/snapshots", { method: "POST", body: form });
  }
  async comment(aid: string, tid: string, body: string) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/comments`, { body }, true); }
  async sendThread(aid: string, tid: string, to: string | null) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/send`, to ? { to } : {}, true); }
  async sendBatch(aid: string, tids: string[], note: string | null, to: string | null) { ids(aid, tids); return this.send<{ threads: Thread[] }>("POST", `/api/artifacts/${aid}/threads:send`, { thread_ids: tids, note, to }, true); }
  /** A thread's clip as a `data:image/png` URL; null when it has none, or it is not a PNG of at most `MAX_CLIP` bytes. */
  async clip(aid: string, tid: string): Promise<string | null> {
    ids(aid, [tid]);
    const res = await this.request(`/api/artifacts/${aid}/threads/${tid}/clip`);
    if (!res.ok || res.headers.get("content-type") !== "image/png") return null;
    const bytes = new Uint8Array(await res.arrayBuffer());
    return bytes.length <= MAX_CLIP ? bytesDataUrl(bytes, "image/png") : null;
  }
  async resolve(aid: string, tid: string) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/resolve`, {}); }
  async reopen(aid: string, tid: string) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/reopen`, {}); }
  /** The owner viewer: the extension acts as the owner. */
  me() { return this.json<{ viewer: Viewer }>("/api/viewers/me"); }
  /** Sets the owner's name. */
  setName(name: string) { return this.send<{ viewer: Viewer }>("PUT", "/api/viewers/me", { display_name: name }); }
  /** The owner's marks of the threads it has looked at. */
  async looked(aid: string, tids: string[]) { ids(aid, tids); return this.send<unknown>("PUT", "/api/viewers/me/looked", { artifact_id: aid, thread_ids: tids }); }
  /** Reports the owner `here` on the page from window `windowId`'s side
   * panel, as its own presence tab. Never `away`: presence combines the
   * owner's tabs, and an `away` must not hide a shell tab's `here` (§9.5). */
  async presence(aid: string, windowId: number) {
    ids(aid);
    if (!Number.isSafeInteger(windowId)) throw new ApiFailure("invalid_id", "not a window ID");
    return this.send<{ people?: PresenceView[] }>("PUT", "/api/viewers/me/presence", { artifact_id: aid, state: "here", tab: `clax-ext:${windowId}` });
  }
  /** Every live page of the origin that has threads, with its threads and merge rules (spec §7.1). */
  site(origin: string) { return this.json<SiteView>(`/api/live/site?origin=${encodeURIComponent(origin)}`); }
  /** Moves a live page's thread to the live page `pageUrl` names. */
  async move(tid: string, pageUrl: string) { threadIds([tid]); return this.send<{ moved: boolean }>("POST", `/api/live/threads/${tid}/move`, { page_url: pageUrl }); }
  /** One batch of a new merge rule; `remaining` says how many threads are left to merge. */
  addRule(origin: string, pattern: string) { return this.send<{ rule: SiteRule; moved: string[]; remaining: number }>("POST", "/api/live/rules", { origin, pattern }); }
  /** One batch of deleting a merge rule; `remaining` says how many threads are left to un-merge. */
  async deleteRule(id: string) { threadIds([id]); return this.send<{ moved: string[]; remaining: number }>("DELETE", `/api/live/rules/${id}`); }
  /** The sites the page's origin may be the same app as (spec §7.2). */
  suggest(url: string, title: string) {
    return this.json<{ origin: string; site: SiteInfo; suggestions: { origin: string; site: SiteInfo; reason: "path" | "title"; path: string | null }[] }>(
      `/api/live/sites/suggest?url=${encodeURIComponent(url)}&title=${encodeURIComponent(title)}`);
  }
  /** Every site Clax has live pages of, one entry per joined site. */
  sites() { return this.json<{ sites: { site: SiteInfo }[] }>("/api/live/sites"); }
  /** One batch of joining `origin` to the site of `with`; `remaining` says how many threads are left to merge. */
  join(origin: string, withOrigin: string) { return this.send<{ site: SiteInfo; moved: string[]; remaining: number }>("POST", "/api/live/sites/join", { origin, with: withOrigin }); }
  /** Splits `origin` off its site. */
  split(origin: string) { return this.send<{ split: boolean; site: SiteInfo }>("POST", "/api/live/sites/split", { origin }); }
  /** The owner's answer to a suggested join: never, or not now. */
  answer(origin: string, withOrigin: string, answer: "never" | "later") { return this.send<unknown>("POST", "/api/live/sites/answer", { origin, with: withOrigin, answer }); }
  /** Who is on the live page now. */
  async presenceOf(aid: string) { ids(aid); return this.json<{ people?: PresenceView[] }>(`/api/artifacts/${aid}/presence`); }

  // The owner's questions and inbox (spec 2026-10-06-agent-questions-and-inbox
  // §6.2, §8.1): the daemon answers 403 `forbidden` to a caller it does not take as the owner.

  /** The unread count, the open questions oldest first, and the newest unread items. */
  inboxSummary() { return this.json<InboxSummary>("/api/inbox/summary"); }
  /** One page of items matching `f`, newest first, after cursor `before`. */
  inboxPage(f: InboxFilter, before: string | null) { return this.json<InboxPage>(`/api/inbox${inboxQuery(f, before)}`); }
  /** Marks one item read or unread. */
  markItem(id: string, read: boolean) { return this.send<{ item: InboxItem; unread: number }>("POST", `/api/inbox/${encodeURIComponent(id)}/${read ? "read" : "unread"}`, {}); }
  /** Marks items read: these IDs, or every unread item matching `filter` up to `upto`. */
  markItems(m: { ids: string[] } | { all: true; filter: InboxFilter | null; upto: number | null }) {
    if ("ids" in m) return this.send<{ marked: number; unread: number }>("POST", "/api/inbox/read", { ids: m.ids });
    const f = m.filter ?? {};
    const filter = Object.fromEntries(Object.entries({ q: f.q?.trim() || undefined, kind: f.kind?.length ? f.kind : undefined, artifact: f.artifact, agent: f.agent, since: f.since, until: f.until }).filter(([, v]) => v !== undefined));
    return this.send<{ marked: number; unread: number }>("POST", "/api/inbox/read", { all: true, ...(Object.keys(filter).length ? { filter } : {}), ...(m.upto !== null ? { upto: m.upto } : {}) });
  }
  /** Answers, skips (`decline`) or moves to the terminal (`release`) question
   * `qid`: the question after it, or, when something closed it first (409),
   * the question as it closed. Sent once: a lost answer is not sent again. */
  async closeQuestion(qid: string, verb: "answer" | "decline" | "release", body?: AnswerBody): Promise<{ question: QuestionView; closed: boolean }> {
    threadIds([qid]);
    const res = await this.request(`/api/questions/${qid}/${verb}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body ?? {}) }, true);
    const r = await res.json().catch(() => null) as { question?: QuestionView; error?: { code?: string; message?: string } } | null;
    if (res.ok && r?.question) return { question: r.question, closed: false };
    if (res.status === 409 && r?.question) return { question: r.question, closed: true };
    throw new ApiFailure(r?.error?.code ?? `http_${res.status}`, r?.error?.message ?? res.statusText, res.status);
  }
}
