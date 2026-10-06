// The daemon's API as the extension uses it (spec 2026-10-05 §9.2): every
// request carries the credential and no cookie; a 401 or an unreachable
// daemon pairs again (at most once per request) and retries once.
import type { Artifact, Version } from "../../../shell/src/api";
import type { Thread, Viewer } from "../../../shell/src/threads";
import type { PresenceView } from "../../../shell/src/view/presence-model";
import type { Working } from "../../../shell/src/view/working-model";
import type { PageView } from "../messages";
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

  constructor(private readonly pairer: Pick<Pairer, "current" | "pair">, private readonly fetchFn: typeof fetch = (...a) => fetch(...a)) {}

  /** `path` on the daemon, with the credential. After a 401 or a network
   * error the request is retried once: with a pairing another request
   * renewed meanwhile, else with a new one. When pairing again is refused
   * (`paired_recently`), the 401 is the answer, or `daemon_unreachable`. */
  async request(path: string, init: RequestInit = {}): Promise<Response> {
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
    }
  }

  async json<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await this.request(path, init);
    const body = await res.json().catch(() => null) as { error?: { code?: string; message?: string } } | null;
    if (!res.ok) throw new ApiFailure(body?.error?.code ?? `http_${res.status}`, body?.error?.message ?? res.statusText, res.status);
    return body as T;
  }

  private send<T>(method: string, path: string, body?: unknown): Promise<T> {
    return this.json<T>(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  }

  lookup(url: string) { return this.json<{ page: PageView | null; route: string | null }>(`/api/live/pages?url=${encodeURIComponent(url)}`); }
  async artifact(aid: string) { ids(aid); return this.json<{ artifact: Artifact; versions: Version[] }>(`/api/artifacts/${aid}`); }
  async threads(aid: string) { ids(aid); return (await this.json<{ threads: Thread[] }>(`/api/artifacts/${aid}/threads?include_resolved=true&limit=200`)).threads; }
  async thread(aid: string, tid: string) { ids(aid, [tid]); return this.json<{ thread: Thread }>(`/api/artifacts/${aid}/threads/${tid}`); }
  async working(aid: string) { ids(aid); return (await this.json<{ working: Working[] }>(`/api/artifacts/${aid}/working`)).working; }
  /** A comment with its page's snapshot. `pending` names the threads the
   * page state showed waiting for a snapshot when it was serialized (`[]`
   * when none); the daemon links only those. */
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
  async comment(aid: string, tid: string, body: string) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/comments`, { body }); }
  async sendThread(aid: string, tid: string, to: string | null) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/send`, to ? { to } : {}); }
  async sendBatch(aid: string, tids: string[], note: string | null, to: string | null) { ids(aid, tids); return this.send<{ threads: Thread[] }>("POST", `/api/artifacts/${aid}/threads:send`, { thread_ids: tids, note, to }); }
  async resolve(aid: string, tid: string) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/resolve`, {}); }
  async reopen(aid: string, tid: string) { ids(aid, [tid]); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/reopen`, {}); }
  async remove(aid: string, tid: string) { ids(aid, [tid]); return this.send<unknown>("DELETE", `/api/artifacts/${aid}/threads/${tid}`); }
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
  /** Who is on the live page now. */
  async presenceOf(aid: string) { ids(aid); return this.json<{ people?: PresenceView[] }>(`/api/artifacts/${aid}/presence`); }
}
