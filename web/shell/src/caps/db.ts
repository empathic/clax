// db.d.ts in the shell: calls go to the Docs routes as this viewer (the cookie
// always; the token in the owner shell) and every write is last-writer-wins
// (`lww: true`). Subscriptions refetch, debounced, on each SSE `doc` event that
// touches them, on `resync`, `ready` and `version`, and right after this
// shell's own writes, and push the result to the frame. While the event stream
// is down every subscription is refreshed every [`POLL_MS`]; a refetch the
// daemon could not answer is retried after [`RETRY_MS`]. The page is
// untrusted: every argument is checked here before any request, whatever the
// bridge already checked.
import type { ArtifactEvent } from "../events";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export type WireDoc = { path: string; id: string; data: Record<string, unknown>; version: number };
type QuerySpec = { kind: "query"; collection: string; where: unknown[]; orderBy: string | null; desc: boolean; limit: number | null };
type Spec = { kind: "doc"; path: string } | QuerySpec;
type ApiDoc = WireDoc & { collection: string; updated_at: string };

export const SNAPSHOT_DEBOUNCE_MS = 25;
export const MAX_SUBSCRIPTIONS = 64;
/** Refresh period for every subscription while the event stream is down. */
export const POLL_MS = 30_000;
/** Delays before retrying a refetch that failed `unavailable`: the first, then every later one. */
export const RETRY_MS = [5_000, 30_000] as const;
export const MAX_DOC_BYTES = 256 * 1024;
const MAX_FILTERS = 10;
const MAX_IN = 30;
const MAX_FIELD = 200;
const MAX_HOLDER = 200;
const MAX_SUB_ID = 64;
const MIN_LEASE_MS = 1_000;
const MAX_LEASE_MS = 600_000;
const OPS = new Set(["==", "!=", "<", "<=", ">", ">=", "in", "not-in", "array-contains"]);

/** A daemon error as the page sees it (db.d.ts `DbErrorCode`). A refused
 * write reads as not found in the daemon; the page gets `invalid_argument`.
 * An artifact whose current declaration no longer includes `db` is
 * `revoked`; a daemon timeout is transient (`unavailable`). */
export function dbError(status: number, err: { code?: string; message?: string }, write: boolean): CapError {
  const message = err.message ?? `HTTP ${status}`;
  if (err.code === "quota_exceeded" || err.code === "resource_exhausted") return new CapError(err.code, message);
  if (err.code === "not_declared") return new CapError("revoked", message);
  if (status === 413 || status === 414 || status === 431) return new CapError("invalid_argument", `the request is too large: ${message}`);
  if (status === 404 && write) return new CapError("invalid_argument", "this document does not exist, or this viewer cannot write it");
  if ([400, 403, 404, 409].includes(status)) return new CapError("invalid_argument", message);
  if (status === 429) return new CapError("resource_exhausted", message);
  return new CapError("unavailable", message);
}

const SEGMENT = /^[A-Za-z0-9_\-.~:@+]{1,200}$/;

/** `path` when it is a document path (`parity` 0) or a collection path
 * (`parity` 1) in the contract's grammar, else `invalid_argument`. A checked
 * path has no `.` or `..` segment, so it cannot leave the Docs routes. */
export function checkPath(path: unknown, parity: 0 | 1): string {
  const segs = typeof path === "string" ? path.split("/") : [];
  const ok = typeof path === "string" && path.length <= 1000 && segs.length <= 16 && segs.length % 2 === parity
    && segs.every(s => SEGMENT.test(s) && s !== "." && s !== "..");
  if (!ok) throw new CapError("invalid_argument", `'${String(path)}' is not a ${parity === 0 ? "document" : "collection"} path`);
  return path as string;
}

const invalid = (message: string) => new CapError("invalid_argument", message);

/** `v` serialized, when it is plain JSON (not undefined, no BigInt, no cycles). */
function jsonOf(v: unknown): string | null {
  try {
    const s = JSON.stringify(v);
    return typeof s === "string" ? s : null;
  } catch {
    return null;
  }
}

/** Whether `v` holds only plain objects (prototype `Object.prototype` or
 * null), arrays, and scalars at every level, without cycles: a `Map`, `Set`, `Date`, `RegExp`
 * or class instance would otherwise serialize as something else, or `{}`. */
function plainJson(v: unknown, path: Set<object> = new Set()): boolean {
  if (v === null || typeof v !== "object") return true;
  if (path.has(v)) return false; // a cycle
  path.add(v);
  const proto = Object.getPrototypeOf(v);
  const ok = Array.isArray(v)
    ? v.every(x => plainJson(x, path))
    : (proto === Object.prototype || proto === null) && Object.values(v).every(x => plainJson(x, path));
  path.delete(v);
  return ok;
}

/** `v` when it is a plain JSON object within [`MAX_DOC_BYTES`]. */
function checkBody(v: unknown): Record<string, unknown> {
  if (v === null || typeof v !== "object" || Array.isArray(v)) throw invalid("a document body is a plain JSON object");
  if (!plainJson(v)) throw invalid("a document body holds only plain objects, arrays, strings, numbers, booleans and null");
  const s = jsonOf(v);
  if (s === null) throw invalid("a document body is plain JSON");
  if (new TextEncoder().encode(s).length > MAX_DOC_BYTES) throw invalid(`a document is at most ${MAX_DOC_BYTES} bytes as JSON`);
  return v as Record<string, unknown>;
}

function checkWhere(v: unknown): [string, string, unknown][] {
  if (!Array.isArray(v) || v.length > MAX_FILTERS) throw invalid(`where is a list of at most ${MAX_FILTERS} filters`);
  return v.map(w => {
    if (!Array.isArray(w) || w.length !== 3) throw invalid("a filter is [field, operator, value]");
    const [field, op, value] = w as unknown[];
    if (typeof field !== "string" || !field || field.length > MAX_FIELD) throw invalid(`a filter's field is 1 to ${MAX_FIELD} characters`);
    if (typeof op !== "string" || !OPS.has(op)) throw invalid(`'${String(op)}' is not a query operator`);
    if (jsonOf(value) === null) throw invalid("a filter's value is plain JSON");
    if ((op === "in" || op === "not-in") && !(Array.isArray(value) && value.length <= MAX_IN)) throw invalid(`in and not-in take an array of at most ${MAX_IN} values`);
    return [field, op, value];
  });
}

function checkSpec(v: unknown): Spec {
  const s = (v !== null && typeof v === "object" ? v : {}) as Record<string, unknown>;
  if (s.kind === "doc") return { kind: "doc", path: checkPath(s.path, 0) };
  if (s.kind !== "query") throw invalid("a subscription is a document path or a query");
  const collection = checkPath(s.collection, 1);
  const where = checkWhere(s.where);
  if (!(s.orderBy === null || (typeof s.orderBy === "string" && s.orderBy && s.orderBy.length <= MAX_FIELD))) throw invalid("orderBy is a field name or null");
  if (typeof s.desc !== "boolean") throw invalid("desc is a boolean");
  if (!(s.limit === null || (Number.isInteger(s.limit) && (s.limit as number) >= 1 && (s.limit as number) <= 1000))) throw invalid("limit is an integer from 1 to 1000, or null");
  return { kind: "query", collection, where, orderBy: s.orderBy as string | null, desc: s.desc, limit: s.limit as number | null };
}

function checkSubId(v: unknown): string {
  if (typeof v !== "string" || !v || v.length > MAX_SUB_ID) throw invalid(`a subscription ID is 1 to ${MAX_SUB_ID} characters`);
  return v;
}

/** The acquire body: `holder` 1 to 200 characters; `ttlMs` floored and clamped
 * to the daemon's bounds (absent or 0 is its default); `data` a body. */
function checkAcquire(v: unknown): { holder: string; ttl_ms?: number; data?: Record<string, unknown> } {
  const o = (v !== null && typeof v === "object" ? v : {}) as Record<string, unknown>;
  if (typeof o.holder !== "string" || !o.holder || [...o.holder].length > MAX_HOLDER) throw invalid(`holder is 1 to ${MAX_HOLDER} characters`);
  const out: { holder: string; ttl_ms?: number; data?: Record<string, unknown> } = { holder: o.holder };
  if (o.ttlMs !== undefined) {
    if (typeof o.ttlMs !== "number" || !Number.isFinite(o.ttlMs) || o.ttlMs < 0) throw invalid("ttlMs is a finite number of milliseconds, 0 or more");
    const ms = Math.floor(o.ttlMs);
    if (ms > 0) out.ttl_ms = Math.min(MAX_LEASE_MS, Math.max(MIN_LEASE_MS, ms));
  }
  if (o.data !== undefined) out.data = checkBody(o.data);
  return out;
}

const wire = (d: ApiDoc): WireDoc => ({ path: d.path, id: d.id, data: d.data, version: d.version });
const parentOf = (path: string) => path.slice(0, path.lastIndexOf("/"));

export const dbHandler: HandlerFactory = env => {
  const base = `/api/artifacts/${env.aid}/docs`;
  const subs = new Map<string, Spec>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  // The latest fetch started per subscription: only it may deliver, so a slow
  // earlier fetch never overwrites a newer snapshot.
  const fetches = new Map<string, number>();
  let fetchSeq = 0;
  // Retry timers and consecutive `unavailable` failures per subscription.
  const retries = new Map<string, ReturnType<typeof setTimeout>>();
  const failures = new Map<string, number>();
  // Whether the event stream is down, and the poll that stands in for it.
  let streamDown = false;
  let poll: ReturnType<typeof setInterval> | null = null;
  // The highest version delivered per document subscription: an older
  // result is never delivered after it (versions are never reused).
  const delivered = new Map<string, number>();
  let disposed = false;
  const docUrl = (path: string) => `${base}/${path.split("/").map(encodeURIComponent).join("/")}`;

  async function request<T>(method: string, url: string, body?: unknown, missingIsNull = false): Promise<T | null> {
    if (disposed) throw new CapError("unavailable", "this view has closed");
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (env.token) headers.authorization = `Bearer ${env.token}`;
    let res: Response;
    try {
      res = await fetch(url, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    } catch {
      throw new CapError("unavailable", "the Clax daemon could not be reached");
    }
    if (res.ok) return (await res.json()) as T;
    if (missingIsNull && res.status === 404) return null;
    const err = ((await res.json().catch(() => ({}))) as { error?: { code?: string; message?: string } }).error ?? {};
    throw dbError(res.status, err, method !== "GET");
  }

  async function query(spec: QuerySpec): Promise<WireDoc[]> {
    const q = new URLSearchParams({ collection: spec.collection });
    if (spec.where.length) q.set("where", JSON.stringify(spec.where));
    if (spec.orderBy) {
      q.set("order_by", spec.orderBy);
      if (spec.desc) q.set("direction", "desc");
    }
    if (spec.orderBy || spec.limit !== null) {
      // Without a limit the daemon answers an ordered query with every match.
      if (spec.limit !== null) q.set("limit", String(spec.limit));
      return (await request<{ docs: ApiDoc[] }>("GET", `${base}?${q}`))!.docs.map(wire);
    }
    const out: WireDoc[] = [];
    let cursor: string | null = null;
    q.set("limit", "1000");
    do {
      if (cursor) q.set("cursor", cursor);
      const page: { docs: ApiDoc[]; next_cursor: string | null } = (await request<{ docs: ApiDoc[]; next_cursor: string | null }>("GET", `${base}?${q}`))!;
      out.push(...page.docs.map(wire));
      cursor = page.next_cursor;
    } while (cursor);
    return out;
  }

  async function current(spec: Spec): Promise<WireDoc[]> {
    if (spec.kind === "query") return query(spec);
    const r = await request<{ doc: ApiDoc }>("GET", docUrl(spec.path), undefined, true);
    return r ? [wire(r.doc)] : [];
  }

  async function push(sub: string): Promise<void> {
    const spec = subs.get(sub);
    if (!spec) return;
    const n = ++fetchSeq;
    fetches.set(sub, n);
    const live = () => !disposed && subs.get(sub) === spec && fetches.get(sub) === n;
    try {
      const docs = await current(spec);
      if (!live()) return;
      failures.delete(sub);
      clearTimeout(retries.get(sub));
      retries.delete(sub);
      if (spec.kind === "doc" && docs.length) {
        if (docs[0].version < (delivered.get(sub) ?? 0)) return;
        delivered.set(sub, docs[0].version);
      }
      env.post({ type: "clax:event", ns: "db", topic: "snapshot", data: { sub, docs } });
    } catch (e) {
      if (!live()) return;
      if (e instanceof CapError && e.code === "unavailable") {
        // Retried after a backoff (and by any change, `resync` or `ready` before then).
        const k = (failures.get(sub) ?? 0) + 1;
        failures.set(sub, k);
        clearTimeout(retries.get(sub));
        retries.set(sub, setTimeout(() => { retries.delete(sub); void push(sub); }, RETRY_MS[Math.min(k, RETRY_MS.length) - 1]));
        return;
      }
      drop(sub);
      env.post({ type: "clax:event", ns: "db", topic: "snapshot-error", data: { sub, code: e instanceof CapError ? e.code : "unavailable", message: e instanceof Error ? e.message : String(e) } });
    }
  }

  function schedule(sub: string): void {
    if (timers.has(sub)) return;
    timers.set(sub, setTimeout(() => { timers.delete(sub); void push(sub); }, SNAPSHOT_DEBOUNCE_MS));
  }

  /** Forgets `sub` and its timers. */
  function drop(sub: string): void {
    subs.delete(sub);
    fetches.delete(sub);
    failures.delete(sub);
    delivered.delete(sub);
    clearTimeout(timers.get(sub));
    timers.delete(sub);
    clearTimeout(retries.get(sub));
    retries.delete(sub);
    if (subs.size === 0) stopPoll();
  }

  function stopPoll(): void {
    if (poll !== null) clearInterval(poll);
    poll = null;
  }

  function startPoll(): void {
    if (poll !== null || disposed || !streamDown || subs.size === 0) return;
    poll = setInterval(() => { for (const sub of subs.keys()) schedule(sub); }, POLL_MS);
  }

  const touches = (spec: Spec, path: string) => (spec.kind === "doc" ? spec.path === path : parentOf(path) === spec.collection);

  /** This shell wrote `path`: the page's matching subscriptions refetch now,
   * without waiting for the stream's `doc` event. */
  function echo(path: string): void {
    for (const [sub, spec] of subs) {
      if (!touches(spec, path)) continue;
      clearTimeout(timers.get(sub));
      timers.delete(sub);
      void push(sub);
    }
  }

  return {
    async call(method, args) {
      const arg = args[0];
      const path = () => checkPath(arg, 0);
      switch (method) {
        case "get": {
          const r = await request<{ doc: ApiDoc }>("GET", docUrl(path()), undefined, true);
          return r ? wire(r.doc) : null;
        }
        case "set":
        case "update": {
          const p = path();
          await request(method === "set" ? "PUT" : "PATCH", docUrl(p), { data: checkBody(args[1]), lww: true });
          echo(p);
          return null;
        }
        case "delete": {
          const p = path();
          await request("DELETE", `${docUrl(p)}?lww=true`);
          echo(p);
          return null;
        }
        case "query": {
          const spec = checkSpec(arg);
          if (spec.kind !== "query") throw new CapError("invalid_argument", "query takes a query");
          return query(spec);
        }
        case "acquire": {
          const p = path();
          const body = checkAcquire(args[1]);
          const r = (await request<{ acquired: boolean; version: number | null; expires_at: string | null; holder: string | null }>(
            "POST", `${base}:acquire`, { path: p, ...body },
          ))!;
          if (r.acquired) echo(p);
          const out: Record<string, unknown> = { acquired: r.acquired };
          if (r.version !== null && r.version !== undefined) out.version = r.version;
          if (r.expires_at) out.expiresAt = r.expires_at;
          if (r.holder) out.holder = r.holder;
          return out;
        }
        case "subscribe": {
          const sub = checkSubId(arg);
          const spec = checkSpec(args[1]);
          if (!subs.has(sub) && subs.size >= MAX_SUBSCRIPTIONS) throw new CapError("resource_exhausted", `at most ${MAX_SUBSCRIPTIONS} subscriptions per view`);
          drop(sub);
          subs.set(sub, spec);
          startPoll();
          await push(sub);
          return null;
        }
        case "unsubscribe":
          drop(checkSubId(arg));
          return null;
        default:
          throw new CapError("capability_removed", `db.${method} is not part of this runtime`);
      }
    },
    onEvent(e: ArtifactEvent) {
      if (disposed) return;
      if (e.type === "doc") {
        for (const [sub, spec] of subs) if (touches(spec, e.path)) schedule(sub);
      } else if (e.type === "resync" || e.type === "ready" || e.type === "version") {
        // A republish may change the rules; `ready` ends a stream outage.
        if (e.type === "ready") {
          streamDown = false;
          stopPoll();
        }
        for (const sub of subs.keys()) schedule(sub);
      } else if (e.type === "stream_up") {
        // Back and resumed: nothing was missed, so the polling stops.
        streamDown = false;
        stopPoll();
      } else if (e.type === "stream_down") {
        streamDown = true;
        startPoll();
      }
    },
    reset() {
      for (const sub of [...subs.keys()]) drop(sub);
      stopPoll();
    },
    dispose() {
      disposed = true;
      for (const sub of [...subs.keys()]) drop(sub);
      stopPoll();
    },
  };
};
