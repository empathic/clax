// db.d.ts in the shell: calls go to the Docs routes as this viewer (the cookie
// always; the token in the owner shell) and every write is last-writer-wins
// (`lww: true`). Subscriptions refetch, debounced, on each SSE `doc` event that
// touches them and on `resync` or `ready`, and push the result to the frame.
// The page is untrusted: paths and query specs are checked here before they
// become URLs, whatever the bridge already checked.
import type { ArtifactEvent } from "../events";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export type WireDoc = { path: string; id: string; data: Record<string, unknown>; version: number };
type QuerySpec = { kind: "query"; collection: string; where: unknown[]; orderBy: string | null; desc: boolean; limit: number | null };
type Spec = { kind: "doc"; path: string } | QuerySpec;
type ApiDoc = WireDoc & { collection: string; updated_at: string };

export const SNAPSHOT_DEBOUNCE_MS = 25;
export const MAX_SUBSCRIPTIONS = 64;

/** A daemon error as the page sees it (db.d.ts `DbErrorCode`). A refused
 * write reads as not found in the daemon; the page gets `invalid_argument`. */
export function dbError(status: number, err: { code?: string; message?: string }, write: boolean): CapError {
  const message = err.message ?? `HTTP ${status}`;
  if (err.code === "quota_exceeded" || err.code === "resource_exhausted") return new CapError(err.code, message);
  if (status === 404 && write) return new CapError("invalid_argument", "this document does not exist, or this viewer cannot write it");
  if ([400, 403, 404, 409].includes(status)) return new CapError("invalid_argument", message);
  if (status === 408 || status === 429) return new CapError("resource_exhausted", message);
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

function checkSpec(v: unknown): Spec {
  const s = (v ?? {}) as Record<string, unknown>;
  if (s.kind === "doc") return { kind: "doc", path: checkPath(s.path, 0) };
  if (s.kind === "query" && Array.isArray(s.where) && (s.orderBy === null || typeof s.orderBy === "string")
    && (s.limit === null || Number.isInteger(s.limit))) {
    return { kind: "query", collection: checkPath(s.collection, 1), where: s.where, orderBy: s.orderBy as string | null, desc: s.desc === true, limit: s.limit as number | null };
  }
  throw new CapError("invalid_argument", "a subscription is a document path or a query");
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
  const docUrl = (path: string) => `${base}/${path.split("/").map(encodeURIComponent).join("/")}`;

  async function request<T>(method: string, url: string, body?: unknown, missingIsNull = false): Promise<T | null> {
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (env.token) headers.authorization = `Bearer ${env.token}`;
    let res: Response;
    try {
      res = await fetch(url, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    } catch {
      throw new CapError("unavailable", "the Artifax daemon could not be reached");
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
      q.set("limit", String(spec.limit ?? 1000));
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
    const live = () => subs.get(sub) === spec && fetches.get(sub) === n;
    try {
      const docs = await current(spec);
      if (live()) env.post({ type: "artifax:event", ns: "db", topic: "snapshot", data: { sub, docs } });
    } catch (e) {
      // A daemon that cannot be reached is retried by the next change, `resync` or `ready`.
      if (!live() || (e instanceof CapError && e.code === "unavailable")) return;
      subs.delete(sub);
      fetches.delete(sub);
      env.post({ type: "artifax:event", ns: "db", topic: "snapshot-error", data: { sub, code: e instanceof CapError ? e.code : "unavailable", message: e instanceof Error ? e.message : String(e) } });
    }
  }

  function schedule(sub: string): void {
    if (timers.has(sub)) return;
    timers.set(sub, setTimeout(() => { timers.delete(sub); void push(sub); }, SNAPSHOT_DEBOUNCE_MS));
  }

  const touches = (spec: Spec, path: string) => (spec.kind === "doc" ? spec.path === path : parentOf(path) === spec.collection);

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
          await request("PUT", docUrl(path()), { data: args[1], lww: true });
          return null;
        case "update":
          await request("PATCH", docUrl(path()), { data: args[1], lww: true });
          return null;
        case "delete":
          await request("DELETE", `${docUrl(path())}?lww=true`);
          return null;
        case "query": {
          const spec = checkSpec(arg);
          if (spec.kind !== "query") throw new CapError("invalid_argument", "query takes a query");
          return query(spec);
        }
        case "acquire": {
          const o = (args[1] ?? {}) as { holder?: string; ttlMs?: number; data?: unknown };
          const r = (await request<{ acquired: boolean; version: number | null; expires_at: string | null; holder: string | null }>(
            "POST", `${base}:acquire`, { path: path(), holder: o.holder, ttl_ms: o.ttlMs, data: o.data },
          ))!;
          const out: Record<string, unknown> = { acquired: r.acquired };
          if (r.version !== null && r.version !== undefined) out.version = r.version;
          if (r.expires_at) out.expiresAt = r.expires_at;
          if (r.holder) out.holder = r.holder;
          return out;
        }
        case "subscribe": {
          const sub = String(arg);
          const spec = checkSpec(args[1]);
          if (!subs.has(sub) && subs.size >= MAX_SUBSCRIPTIONS) throw new CapError("resource_exhausted", `at most ${MAX_SUBSCRIPTIONS} subscriptions per view`);
          subs.set(sub, spec);
          await push(sub);
          return null;
        }
        case "unsubscribe":
          subs.delete(String(arg));
          fetches.delete(String(arg));
          return null;
        default:
          throw new CapError("capability_removed", `db.${method} is not part of this runtime`);
      }
    },
    onEvent(e: ArtifactEvent) {
      if (e.type === "doc") {
        for (const [sub, spec] of subs) if (touches(spec, e.path)) schedule(sub);
      } else if (e.type === "resync" || e.type === "ready") {
        for (const sub of subs.keys()) schedule(sub);
      }
    },
    reset() {
      subs.clear();
      fetches.clear();
      for (const t of timers.values()) clearTimeout(t);
      timers.clear();
    },
  };
};
