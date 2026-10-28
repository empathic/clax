// The page runtime's backend on the page's own origin: `/_clax/api/…` of
// the artifact's site (docs/contract.md), which its host alone scopes to one
// artifact. A call names a capability's method and its arguments, never an
// artifact: the browser's site cookie (or a world visitor's cookie) says who
// calls, and the site answers for its own artifact alone. Requests go with
// `credentials: "same-origin"` and no `Authorization`; the site counts every
// write as the page's. Daemon errors become the contract's codes
// (web/contract/0.3.0 db.d.ts, assets.d.ts). A docs read sends the `ETag`
// of its last answer back as `If-None-Match`, and the site's 304 (nothing
// changed since) stands for that answer. Another app's documents (`db.artifact(id)`)
// are read through `read/<id>/docs…`, which the site serves for this app
// alone, as the viewer the cookie names (docs/contract.md, "Cross-app reads").
// With a `relay` (in the Clax app, whose WebViews give their scheme no
// request body), every request but a read goes to the framing shell instead
// (`site.fetch`, web/shell/src/caps/site.ts), which relays it to this same
// site with this frame's site session and origin; reads still go by fetch.
import { CapabilityError } from "../rpc";
import { checkUpload } from "./asset-checks";

export type WireDoc = { path: string; id: string; data: Record<string, unknown>; version: number };
type ApiDoc = WireDoc & { collection?: string; updated_at?: string };
type ApiError = { code?: string; message?: string };
type QuerySpec = { collection: string; where: unknown[]; orderBy: string | null; desc: boolean; limit: number | null };
/** An asset as the site lists it. */
type ApiAsset = { id: string; content_type: string; size: number; created_at: string; url?: string };

export type OriginClient = {
  /** `cap.method(...args)` as one request (a query's pages as several). */
  call(cap: string, method: string, args: unknown): Promise<unknown>;
  /** The WebSocket at `/_clax/api/<path>` on the site. */
  socket(path: string): WebSocket;
};

/** A site request the framing shell makes for the page (its `site.fetch`):
 * the path under the site's origin, the request's headers and its body. */
export type SiteRelay = (r: { method: string; path: string; headers: [string, string][]; body: ArrayBuffer | null }) => Promise<{ status: number; headers: [string, string][]; body: ArrayBuffer | null }>;

/** Statuses whose response has no body. */
const NULL_BODY = new Set([101, 103, 204, 205, 304]);

/** Where the site serves the runtime's backend. */
export const API = "/_clax/api/";
const MIN_LEASE_MS = 1_000;
const MAX_LEASE_MS = 600_000;
/** Most docs reads whose answers a client keeps for `If-None-Match`. */
export const MAX_VALIDATED = 128;
type Validated = { tag: string; status: number; body: unknown };
/** Asset IDs as the site issues them (ULIDs). */
const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;

/** A daemon error as the page sees it (db.d.ts `DbErrorCode`). A refused
 * write reads as not found in the daemon; the page gets `invalid_argument`.
 * An artifact whose current declaration no longer includes `db` is
 * `revoked`; a timeout, or no answer (status 0), is transient (`unavailable`). */
export function dbError(status: number, err: ApiError, write: boolean): CapabilityError {
  const message = err.message ?? `HTTP ${status}`;
  if (err.code === "quota_exceeded" || err.code === "resource_exhausted") return new CapabilityError(err.code, message);
  if (err.code === "not_declared") return new CapabilityError("revoked", message);
  // A spectator's write (spec 2026-10-11-clax-dev-hosted-design §16.1):
  // transient, as the viewer may join live.
  if (err.code === "spectator") return new CapabilityError("unavailable", message);
  if (status === 413 || status === 414 || status === 431) return new CapabilityError("invalid_argument", `the request is too large: ${message}`);
  if (status === 404 && write) return new CapabilityError("invalid_argument", "this document does not exist, or this viewer cannot write it");
  if ([400, 403, 404, 409].includes(status)) return new CapabilityError("invalid_argument", message);
  if (status === 429) return new CapabilityError("resource_exhausted", message);
  return new CapabilityError("unavailable", message);
}

/** A daemon error of an asset call (assets.d.ts `UploadErrorCode`); no
 * answer (status 0) is `store_unavailable`, the one code worth a retry. */
export function assetsError(status: number, err: ApiError): CapabilityError {
  const message = err.message ?? `HTTP ${status}`;
  if (status === 0) return new CapabilityError("store_unavailable", "the site could not be reached");
  if (status === 413 || err.code === "asset_too_large" || err.code === "body_too_large") return new CapabilityError("too_large", message);
  if (err.code === "unsupported_type") return new CapabilityError("unsupported_type", message);
  if (status === 404) return new CapabilityError("quota_or_state", "the artifact cannot take assets now");
  if (status === 401 || status === 403) return new CapabilityError("upstream_auth", "the site did not accept this view; reload the page");
  if (status === 400) return new CapabilityError("invalid_request", message);
  return new CapabilityError("upstream_error", message);
}

const otherError = (status: number, err: ApiError) => new CapabilityError("unavailable", err.message ?? (status ? `HTTP ${status}` : "the site could not be reached"));
const invalid = (message: string) => new CapabilityError("invalid_argument", message);

/** `docs/<path>`, each segment encoded; a segment that could leave the
 * documents (empty, `.`, `..`) is refused before any request. */
function docPath(path: unknown): string {
  const segs = typeof path === "string" ? path.split("/") : [""];
  if (segs.some(s => s === "" || s === "." || s === "..")) throw invalid(`'${String(path)}' is not a document path`);
  return `docs/${segs.map(encodeURIComponent).join("/")}`;
}

const wire = (d: ApiDoc): WireDoc => ({ path: d.path, id: d.id, data: d.data, version: d.version });

const ARTIFACT_ID = /^[0-9a-hjkmnp-tv-z]{12}$/;

/** `v` when it is an artifact ID, else `invalid_argument` before any request. */
function sourceOf(v: unknown): string {
  if (typeof v !== "string" || !ARTIFACT_ID.test(v)) throw invalid(`'${String(v)}' is not an artifact ID`);
  return v;
}

/** A cross-app read's refusal: `not_granted` (the source does not share
 * with this app and the owner granted nothing) and `not_declared` (this app
 * does not list the source in `db.read`, or the source keeps no documents)
 * keep their codes; anything else is as a read's. */
function readError(status: number, err: ApiError): CapabilityError {
  if (err.code === "not_granted" || err.code === "not_declared") return new CapabilityError(err.code, err.message ?? err.code);
  return dbError(status, err, false);
}

/** `url` with `init` as the shell relays it: its method, path, headers and
 * body's bytes (a form's with its boundary in its `Content-Type`). */
async function relayed(relay: SiteRelay, url: string, init: RequestInit): Promise<Response> {
  const req = new Request(url, init);
  const u = new URL(url);
  const body = await req.arrayBuffer();
  const r = await relay({ method: req.method, path: u.pathname + u.search, headers: [...req.headers], body: body.byteLength ? body : null });
  return new Response(NULL_BODY.has(r.status) ? null : r.body, { status: r.status, headers: r.headers });
}

/** The client of the site at `base` (its origin; the path is ignored);
 * with `relay`, its writes go by the framing shell, and with `relayAll`
 * its reads too (a frame whose opaque origin keeps no site cookie). */
export function originClient(base: URL, relay?: SiteRelay, relayAll = false): OriginClient {
  const at = (path: string) => new URL(API + path, base).href;

  /** The docs reads' last answers by URL, with their `ETag`: sent again as
   * `If-None-Match`, a 304 stands for the answer kept. */
  const validated = new Map<string, Validated>();

  /** The answer's JSON (null for 204), or the rejection `fail` makes of it;
   * `missing` answers a 404 instead. */
  async function send(path: string, init: RequestInit, fail: (status: number, err: ApiError) => CapabilityError, missing?: unknown): Promise<any> {
    const url = at(path);
    const read = (init.method ?? "GET") === "GET" && path.startsWith("docs");
    const kept = read ? validated.get(url) : undefined;
    const headers = new Headers(init.headers);
    if (kept) headers.set("if-none-match", kept.tag);
    let res: Response;
    try {
      const go: RequestInit = { credentials: "same-origin", cache: "no-store", ...init, headers };
      res = relay && (relayAll || !["GET", "HEAD"].includes((init.method ?? "GET").toUpperCase())) ? await relayed(relay, url, go) : await fetch(url, go);
    } catch {
      throw fail(0, {});
    }
    let status = res.status;
    let body: any;
    if (status === 304 && kept) {
      ({ status, body } = kept);
    } else {
      body = status === 204 ? null : await res.json().catch(() => (res.ok ? undefined : {}));
      if (res.ok && body === undefined) throw fail(status, { message: "the site's answer was not JSON" });
      const tag = res.headers.get("etag");
      if (read && tag && (status === 200 || status === 404)) keep(url, { tag, status, body });
    }
    if (status >= 200 && status < 300) return body;
    if (missing !== undefined && status === 404) return missing;
    throw fail(status, (body as { error?: ApiError } | null)?.error ?? {});
  }

  function keep(url: string, v: Validated) {
    validated.delete(url);
    validated.set(url, v);
    // The oldest go first: a page polls at most its subscriptions' reads.
    for (const old of validated.keys()) {
      if (validated.size <= MAX_VALIDATED) break;
      validated.delete(old);
    }
  }

  const db = (method: string, path: string, body?: unknown, missing?: unknown) =>
    send(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) }, (s, e) => dbError(s, e, method !== "GET"), missing);

  async function query(spec: QuerySpec, under = "docs", get: (path: string) => Promise<unknown> = path => db("GET", path)): Promise<WireDoc[]> {
    const q = new URLSearchParams({ collection: spec.collection });
    if (spec.where.length) q.set("where", JSON.stringify(spec.where));
    if (spec.orderBy) {
      q.set("order_by", spec.orderBy);
      if (spec.desc) q.set("direction", "desc");
    }
    if (spec.orderBy || spec.limit !== null) {
      // Without a limit the daemon answers an ordered query with every match.
      if (spec.limit !== null) q.set("limit", String(spec.limit));
      return ((await get(`${under}?${q}`)) as { docs: ApiDoc[] }).docs.map(wire);
    }
    const out: WireDoc[] = [];
    let cursor: string | null = null;
    q.set("limit", "1000");
    do {
      if (cursor) q.set("cursor", cursor);
      const page = (await get(`${under}?${q}`)) as { docs: ApiDoc[]; next_cursor: string | null };
      out.push(...page.docs.map(wire));
      cursor = page.next_cursor;
    } while (cursor);
    return out;
  }

  /** The acquire body: `ttlMs` floored and clamped to the daemon's bounds
   * (absent or 0 is its default). */
  function acquireBody(path: string, o: { holder?: unknown; ttlMs?: unknown; data?: unknown }) {
    const body: Record<string, unknown> = { path, holder: o.holder };
    if (o.ttlMs !== undefined) {
      if (typeof o.ttlMs !== "number" || !Number.isFinite(o.ttlMs) || o.ttlMs < 0) throw invalid("ttlMs is a finite number of milliseconds, 0 or more");
      const ms = Math.floor(o.ttlMs);
      if (ms > 0) body.ttl_ms = Math.min(MAX_LEASE_MS, Math.max(MIN_LEASE_MS, ms));
    }
    if (o.data !== undefined) body.data = o.data;
    return body;
  }

  async function upload(arg: { blob?: unknown; type?: unknown } | undefined) {
    const { blob, type } = arg ?? {};
    if (!(blob instanceof Blob) || blob.size === 0) throw new CapabilityError("invalid_request", "upload takes a non-empty Blob or File");
    if (type !== undefined && typeof type !== "string") throw new CapabilityError("invalid_request", "options.type is a string");
    const ct = await checkUpload(blob, type);
    const form = new FormData();
    form.set("file", new Blob([blob], { type: ct }), "upload");
    const { asset, url } = (await send("assets", { method: "POST", body: form }, assetsError)) as { asset: ApiAsset; url: string };
    return { id: asset.id, url, sizeBytes: asset.size, contentType: asset.content_type };
  }

  async function call(cap: string, method: string, a: unknown): Promise<unknown> {
    const args = Array.isArray(a) ? a : [];
    switch (`${cap}.${method}`) {
      case "db.get": {
        const r = await db("GET", docPath(args[0]), undefined, null);
        return r ? wire(r.doc) : null;
      }
      case "db.set":
      case "db.update":
        await db(method === "set" ? "PUT" : "PATCH", docPath(args[0]), { data: args[1], lww: true });
        return null;
      case "db.delete":
        await db("DELETE", `${docPath(args[0])}?lww=true`);
        return null;
      case "db.query":
        return query(args[0] as QuerySpec);
      case "db.read_get": {
        const r = await send(`read/${sourceOf(args[0])}/${docPath(args[1])}`, {}, readError, null);
        return r ? wire(r.doc) : null;
      }
      case "db.read_query": {
        return query(args[1] as QuerySpec, `read/${sourceOf(args[0])}/docs`, path => send(path, {}, readError));
      }
      case "db.acquire": {
        docPath(args[0]);
        const r = (await db("POST", "docs:acquire", acquireBody(args[0] as string, (args[1] ?? {}) as object))) as { acquired: boolean; version?: number | null; expires_at?: string | null; holder?: string | null };
        const out: Record<string, unknown> = { acquired: r.acquired };
        if (r.version !== null && r.version !== undefined) out.version = r.version;
        if (r.expires_at) out.expiresAt = r.expires_at;
        if (r.holder) out.holder = r.holder;
        return out;
      }
      case "assets.upload":
        return upload(args[0] as { blob?: unknown; type?: unknown });
      case "assets.list": {
        const { assets } = (await send("assets", {}, assetsError)) as { assets: ApiAsset[] };
        return {
          assets: assets.map(x => ({ id: x.id, url: x.url ?? `/_clax/blob/${x.id}`, contentType: x.content_type, sizeBytes: x.size, createdAt: x.created_at })),
          // The asset store has no per-artifact quota; the maxima say so.
          usage: { files: assets.length, bytes: assets.reduce((n, x) => n + x.size, 0), maxFiles: Number.MAX_SAFE_INTEGER, maxBytes: Number.MAX_SAFE_INTEGER },
        };
      }
      case "assets.delete": {
        const id = args[0];
        // Nothing is stored under an ID the store never issues.
        if (typeof id !== "string" || !ULID.test(id)) return { deleted: false };
        const r = await send(`assets/${id}`, { method: "DELETE" }, assetsError, false);
        return { deleted: r !== false };
      }
      case "user.whoami":
        // A 401 is the site's `site_session_required`: no session reached it.
        return send("whoami", {}, (s, e) => (s === 401 ? new CapabilityError("unauthenticated", e.message ?? "no site session") : otherError(s, e)));
      case "artifact.get":
        return ((await send("artifact", {}, otherError)) as { artifact: unknown }).artifact;
      default:
        throw new CapabilityError("capability_removed", `${cap}.${method} is not part of this runtime`);
    }
  }

  return {
    call,
    socket(path) {
      const u = new URL(API + path, base);
      u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
      return new WebSocket(u.href);
    },
  };
}
