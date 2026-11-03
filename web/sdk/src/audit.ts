// The SDK package's audit entry (`@empathic/clax-sdk/audit`): the owner's
// reading of a Clax instance's audit trail, `GET /api/audit` and
// `/api/audit/<seq>` (docs/contract.md, "The audit view"). Its types are
// the API contract's (contract.gen.ts). The routes answer the owner (a
// browser of theirs, or the daemon token by hand) and the people they
// connect, each their own scope; an agent or a grant is refused (403
// `not_for_agents`, `forbidden`). Nothing here asks for more.

import { ClaxError, failure, readJson, type Fetch } from "./client";
import type { AuditEvent, AuditEventDetail, AuditPage, Operations } from "./contract.gen";

export type { AuditActor, AuditEvent, AuditEventDetail, AuditPage } from "./contract.gen";

/** `GET /api/audit`'s filters: `q` words, `actor` (`claxbot` or a person
 * key), `group`, `kind` (one kind, or a prefix such as `bot.*`),
 * `artifact`, `from` and `to` (RFC 3339 or `YYYY-MM-DD`), `cursor` (a
 * page's `next_cursor`) and `limit` (1 to 200). */
export type AuditFilter = Operations["list_audit_events"]["query"];
/** A kind group the view filters by. */
export type AuditGroup = NonNullable<AuditFilter["group"]>;

export interface AuditOptions {
  /** The instance's base URL (`https://clax.example`); by default this
   * page's own origin, as the Clax shell is. */
  instance?: string;
  /** The daemon token, for the owner by hand (`Authorization: Bearer`), or
   * a function that gives it. Without one, the browser's own session (its
   * cookies, from the instance's origin). */
  token?: string | (() => Promise<string | null | undefined>);
  /** Default: the global `fetch`. */
  fetch?: Fetch;
}

export interface FollowOptions {
  /** How long to wait between polls, in milliseconds. Default 2000. */
  intervalMs?: number;
  /** Ends the follow. */
  signal?: AbortSignal;
  /** Start after this `seq`; by default after the newest event now. */
  after?: number;
}

/** The query string of `f`'s given parameters, with its `?`. */
export function auditQuery(f: AuditFilter = {}): string {
  const s = new URLSearchParams();
  for (const [k, v] of Object.entries(f)) if (v !== undefined && v !== null && v !== "") s.set(k, String(v));
  const q = s.toString();
  return q ? `?${q}` : "";
}

/** The events of `page` after `seq`, oldest first. */
export const newer = (page: AuditPage, seq: number): AuditEvent[] => page.events.filter(e => e.seq > seq).sort((a, b) => a.seq - b.seq);

export class AuditClient {
  private readonly fetch: Fetch;

  constructor(private readonly opts: AuditOptions = {}) {
    this.fetch = opts.fetch ?? ((input, init) => globalThis.fetch(input, init));
  }

  /** A page of the events the caller may read, newest first. */
  async list(f: AuditFilter = {}): Promise<AuditPage> {
    return readJson<AuditPage>(await this.get(`/api/audit${auditQuery(f)}`));
  }

  /** One event, with its stored body; `ClaxError` `not_found` for one the
   * caller may not read, as for none. */
  async event(seq: number): Promise<AuditEventDetail> {
    return readJson<AuditEventDetail>(await this.get(`/api/audit/${encodeURIComponent(String(seq))}`));
  }

  /** The events `f` matches as they are recorded, oldest first, until
   * `signal` aborts. Every poll asks the same query (no cursor), so the
   * view is recorded once a window, as any reader's is; only when a whole
   * page is new does it page back, by cursor, to the last event it gave. */
  async *follow(f: AuditFilter = {}, o: FollowOptions = {}): AsyncGenerator<AuditEvent> {
    const query: AuditFilter = { ...f, cursor: undefined };
    let last = o.after ?? Math.max(0, ...(await this.list(query)).events.map(e => e.seq));
    while (!o.signal?.aborted) {
      await sleep(o.intervalMs ?? 2000, o.signal);
      if (o.signal?.aborted) return;
      let page = await this.list(query);
      const fresh = newer(page, last);
      while (fresh.length && page.next_cursor !== null && page.events.every(e => e.seq > last)) {
        page = await this.list({ ...query, cursor: page.next_cursor });
        fresh.unshift(...newer(page, last));
      }
      for (const e of fresh) {
        last = Math.max(last, e.seq);
        yield e;
      }
    }
  }

  private async get(path: string): Promise<Response> {
    const headers: Record<string, string> = {};
    const t = typeof this.opts.token === "function" ? await this.opts.token() : this.opts.token;
    if (t) headers.authorization = `Bearer ${t}`;
    const url = this.opts.instance ? new URL(path, this.opts.instance).href : path;
    let res: Response;
    try {
      res = await this.fetch(url, { method: "GET", headers, cache: "no-store" });
    } catch (e) {
      throw new ClaxError("network", e instanceof Error ? e.message : String(e), 0);
    }
    if (!res.ok) throw await failure(res);
    return res;
  }
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise(resolve => {
    const t = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => { clearTimeout(t); resolve(); }, { once: true });
  });
}
