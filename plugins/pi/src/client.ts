// HTTP client for the Clax daemon's REST API.
//
// Requests go through node:http rather than fetch because fetch cannot bound
// connection setup separately from the whole request.
import { request as httpRequest, type IncomingHttpHeaders } from "node:http";

/** Deadline for ordinary requests. */
export const REQUEST_TIMEOUT_MS = 30_000;
/** Deadline for publishes and asset uploads. */
export const PUBLISH_TIMEOUT_MS = 120_000;
/** Deadline for ending the session, so a hung daemon cannot hold up the harness. */
export const END_TIMEOUT_MS = 3_000;
/** Deadline for establishing a connection; a live daemon on loopback accepts at once. */
export const CONNECT_TIMEOUT_MS = 2_000;

/** Why a daemon call failed:
 * - `unreachable`: no connection (refused, not established within the connect
 *   timeout, or dropped before a response);
 * - `timeout`: connected, but the request did not complete within its deadline,
 *   so a write may or may not have taken effect;
 * - `bad_response`: a success status with a body that is not the expected JSON;
 * - `api`: an error status; `error` is the body's `error` object (`code`,
 *   `message`, and any extra fields such as `current`). */
export type ClientErrorKind = "unreachable" | "timeout" | "bad_response" | "api";

export class ClientError extends Error {
  constructor(
    readonly kind: ClientErrorKind,
    message: string,
    readonly status = 0,
    readonly error: Record<string, unknown> = {},
  ) {
    super(message);
    this.name = "ClientError";
  }
}

/** Where a daemon answers: `base` (`http://127.0.0.1:<port>`) for API calls,
 * `browserBase` (`http://localhost:<port>`) for URLs shown to the person, and
 * its bearer token. */
export interface Endpoint {
  base: string;
  browserBase: string;
  token: string;
}

/** The body of `POST /api/sessions`. */
export interface Registration {
  harness: string;
  harness_session_id: string | null;
  cwd: string;
  pid: number;
  parent_pid: number;
}

/** A harness session as the daemon reports it. */
export interface Session {
  id: string;
  harness: string;
  harness_session_id: string | null;
  cwd: string;
  pid: number | null;
  parent_pid: number | null;
  started_at: string;
  last_seen_at: string;
  ended_at: string | null;
}

export interface RawResponse {
  status: number;
  headers: IncomingHttpHeaders;
  body: Buffer;
}

/** A failed attempt, and whether a refresh may cure it: the connection was not
 * established (so the request was never sent), the token was refused, or the
 * session was ended under the client (`unknown_session`). */
class Failure {
  constructor(readonly error: ClientError, readonly refreshable: boolean) {}
}

interface RequestOptions {
  method: string;
  headers?: Record<string, string>;
  body?: Buffer | string;
  timeoutMs?: number;
  /** Cancels the request; a cancelled request fails `unreachable` and is not retried. */
  signal?: AbortSignal;
}

/** Sends one request. Transport failures reject with a [`Failure`]: a request
 * cancelled through `opts.signal` is `unreachable` and not retried, a
 * connection that was never established is `unreachable` and refreshable, a
 * request past its deadline is `timeout`, anything else (such as a connection
 * dropped mid-request) is `unreachable` but not retried, since the request may
 * have taken effect. Proxies are never used. */
function send(url: string, opts: RequestOptions): Promise<RawResponse> {
  const deadline = AbortSignal.timeout(opts.timeoutMs ?? REQUEST_TIMEOUT_MS);
  const signal = opts.signal ? AbortSignal.any([deadline, opts.signal]) : deadline;
  return new Promise<RawResponse>((resolve, reject) => {
    let connected = false;
    let connectTimer: NodeJS.Timeout | undefined;
    const fail = (e: unknown) => {
      clearTimeout(connectTimer);
      const msg = e instanceof Error ? e.message : String(e);
      if (opts.signal?.aborted) reject(new Failure(new ClientError("unreachable", "request cancelled"), false));
      else if (deadline.aborted) reject(new Failure(new ClientError("timeout", `request to ${url} timed out`), false));
      else if (!connected) reject(new Failure(new ClientError("unreachable", `${url}: ${msg}`), true));
      else reject(new Failure(new ClientError("unreachable", `${url}: ${msg}`), false));
    };
    const req = httpRequest(url, { method: opts.method, headers: opts.headers, signal }, res => {
      const chunks: Buffer[] = [];
      res.on("data", (c: Buffer) => chunks.push(c));
      res.on("error", fail);
      res.on("end", () => {
        if (!res.complete) return fail(new Error("connection closed mid-response"));
        resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks) });
      });
    });
    req.on("socket", socket => {
      if (!socket.connecting) {
        connected = true;
        return;
      }
      connectTimer = setTimeout(
        () => req.destroy(new Error(`not connected within ${CONNECT_TIMEOUT_MS} ms`)),
        CONNECT_TIMEOUT_MS,
      );
      socket.once("connect", () => {
        connected = true;
        clearTimeout(connectTimer);
      });
    });
    req.on("error", fail);
    req.end(opts.body);
  });
}

/** Sends a request; a non-success status becomes an `api` failure carrying the
 * body's `error` object, refreshable when the token was refused (401) or the
 * session named by `X-Clax-Session` is no longer live (400
 * `unknown_session`). */
async function attempt(url: string, opts: RequestOptions): Promise<RawResponse> {
  const res = await send(url, opts);
  if (res.status >= 200 && res.status < 300) return res;
  let error: Record<string, unknown> | undefined;
  try {
    const body = JSON.parse(res.body.toString("utf8"));
    if (body && typeof body.error === "object" && body.error !== null && !Array.isArray(body.error)) error = body.error;
  } catch { /* not JSON */ }
  error ??= { code: "http_error", message: `daemon answered HTTP ${res.status}` };
  const unknownSession = res.status === 400 && error.code === "unknown_session";
  throw new Failure(new ClientError("api", `HTTP ${res.status}: ${JSON.stringify(error)}`, res.status, error), res.status === 401 || unknownSession);
}

function bodyJson(res: RawResponse): any {
  if (res.status === 204) return {};
  try {
    return JSON.parse(res.body.toString("utf8"));
  } catch (e) {
    throw new ClientError("bad_response", e instanceof Error ? e.message : String(e));
  }
}

function sessionOf(res: any): Session {
  const s = res?.session;
  if (!s || typeof s.id !== "string") throw new ClientError("bad_response", "session: missing");
  return s as Session;
}

/** A GET to `url` with a 1 s deadline: true when it answers with a success status. */
export async function probe(url: string): Promise<boolean> {
  try {
    const res = await send(url, { method: "GET", timeoutMs: 1_000 });
    return res.status >= 200 && res.status < 300;
  } catch {
    return false;
  }
}

/** `fallback` ms, or less when `deadline` (epoch ms) comes sooner; at least 1. */
function remaining(deadline: number | undefined, fallback: number): number {
  return deadline === undefined ? fallback : Math.max(1, Math.min(fallback, deadline - Date.now()));
}

/** A request path, or a function giving it per attempt (for paths naming the
 * session, which a refresh may replace). */
type Path = string | (() => string);

/** Finds (and, for `refresh`, may start) the daemon. */
export type Find = () => Promise<Endpoint>;

/** Percent-encodes every byte of `path` outside the RFC 3986 unreserved set,
 * keeping `/` as the segment separator. */
export function encodePath(path: string): string {
  let out = "";
  for (const b of Buffer.from(path, "utf8")) {
    const c = String.fromCharCode(b);
    out += /[A-Za-z0-9\-._~/]/.test(c) ? c : `%${b.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}

/** What a failed request saw: the endpoint and session it was sent with. A
 * refresh is skipped when the client has since moved on from either. */
interface Stale {
  endpoint: Endpoint | undefined;
  sessionId: string | undefined;
}

/**
 * A client of one daemon's REST API that finds its daemon with `refresh`
 * (which may start one) or `discover` (which must not), and registers a harness
 * session there: lazily on first use, and again whenever a request cannot
 * connect or is refused with 401 (the daemon restarted, on a new port or with a
 * new token) or with `unknown_session` (the session was ended under it), after
 * which the request is retried once. Tool requests may start a daemon; ending
 * the session only finds one. Every request carries `X-Clax-Session` once a
 * session is registered.
 */
export class DaemonClient {
  private endpoint: Endpoint | undefined;
  private sessionId: string | undefined;
  private registeredSession: Session | undefined;
  /** True when the session is registered with the daemon at `endpoint`. */
  private registered = false;
  /** The refresh in flight, so concurrent failures refresh once. */
  private refreshing: Promise<void> | undefined;

  constructor(
    private readonly refreshFn: Find,
    private readonly discoverFn: Find,
    private readonly registration: Registration,
  ) {}

  /** The browser base URL of the current daemon, once known. */
  browserBase(): string | undefined {
    return this.endpoint?.browserBase;
  }

  /** The session last registered, once registered. */
  session(): Session | undefined {
    return this.registeredSession;
  }

  /** Finds (or starts) the daemon and registers the session unless both are
   * done. With `timeoutMs`, the registration request is cut off at that long
   * after the call (finding the daemon is bounded by its own timeouts). */
  ensureSession(timeoutMs?: number): Promise<void> {
    return this.ensure(this.refreshFn, timeoutMs === undefined ? undefined : Date.now() + timeoutMs);
  }

  private async ensure(find: Find, deadline?: number): Promise<void> {
    if (this.endpoint && this.registered) return;
    await this.refresh(this.stale(), find, deadline);
  }

  private stale(): Stale {
    return { endpoint: this.endpoint, sessionId: this.sessionId };
  }

  /** Re-discovers the daemon and registers the session there, unless another
   * caller already moved on from `stale` (the endpoint and session the failure
   * was seen with). Registering again after the session was ended inserts a
   * new live row with the same harness session ID. */
  private async refresh(stale: Stale, find: Find, deadline?: number): Promise<void> {
    while (this.refreshing) await this.refreshing.catch(() => {});
    if (this.endpoint && this.registered && (this.endpoint !== stale.endpoint || this.sessionId !== stale.sessionId)) return;
    const run = (async () => {
      let endpoint: Endpoint;
      try {
        endpoint = await find();
      } catch (e) {
        throw new ClientError("unreachable", e instanceof Error ? e.message : String(e));
      }
      this.endpoint = endpoint;
      this.registered = false;
      let res: RawResponse;
      try {
        res = await attempt(`${endpoint.base}/api/sessions`, {
          method: "POST",
          headers: { authorization: `Bearer ${endpoint.token}`, "content-type": "application/json" },
          body: JSON.stringify(this.registration),
          timeoutMs: remaining(deadline, REQUEST_TIMEOUT_MS),
        });
      } catch (e) {
        throw e instanceof Failure ? e.error : e;
      }
      const session = sessionOf(bodyJson(res));
      this.sessionId = session.id;
      this.registeredSession = session;
      this.registered = true;
    })();
    this.refreshing = run;
    try {
      await run;
    } finally {
      this.refreshing = undefined;
    }
  }

  private headers(endpoint: Endpoint, extra: Record<string, string> = {}): Record<string, string> {
    const h: Record<string, string> = { authorization: `Bearer ${endpoint.token}`, ...extra };
    if (this.sessionId) h["x-clax-session"] = this.sessionId;
    return h;
  }

  /** Sends a request after ensuring the session; on a refreshable failure,
   * refreshes and retries once. When the refresh fails, the original error is
   * thrown. */
  private async request(path: Path, opts: RequestOptions, find: Find = this.refreshFn, deadline?: number): Promise<RawResponse> {
    await this.ensure(find, deadline);
    const endpoint = this.endpoint;
    if (!endpoint) throw new ClientError("unreachable", "no daemon found");
    const stale = this.stale();
    const go = (ep: Endpoint) =>
      attempt(`${ep.base}${typeof path === "function" ? path() : path}`, {
        ...opts,
        headers: this.headers(ep, opts.headers),
        timeoutMs: remaining(deadline, opts.timeoutMs ?? REQUEST_TIMEOUT_MS),
      });
    try {
      return await go(endpoint);
    } catch (e) {
      if (!(e instanceof Failure)) throw e;
      if (!e.refreshable) throw e.error;
      try {
        await this.refresh(stale, find, deadline);
      } catch {
        throw e.error;
      }
      try {
        return await go(this.endpoint!);
      } catch (e2) {
        throw e2 instanceof Failure ? e2.error : e2;
      }
    }
  }

  private async json(path: Path, opts: RequestOptions, find?: Find, deadline?: number): Promise<any> {
    return bodyJson(await this.request(path, opts, find, deadline));
  }

  private jsonBody(method: string, body: unknown, timeoutMs = REQUEST_TIMEOUT_MS): RequestOptions {
    return { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body), timeoutMs };
  }

  /** `GET /healthz`: `{version, pid, started_at}`. */
  healthz(): Promise<any> {
    return this.json("/healthz", { method: "GET" });
  }

  /** `GET /api/artifacts`: `{artifacts: [..]}`, pinned first, then most recently updated. */
  list(): Promise<any> {
    return this.json("/api/artifacts", { method: "GET" });
  }

  /** `GET /api/artifacts/<id>`: `{artifact, versions}`; `artifact` carries
   * `owner_session_id`, `owner_live` and `owner_harness`. */
  get(id: string): Promise<any> {
    return this.json(`/api/artifacts/${id}`, { method: "GET" });
  }

  /** `POST /api/artifacts`: creates an artifact; `{artifact, version, url}`. */
  create(body: unknown): Promise<any> {
    return this.json("/api/artifacts", this.jsonBody("POST", body, PUBLISH_TIMEOUT_MS));
  }

  /** `POST /api/artifacts/<id>/versions`: publishes a new version; `{artifact, version, url}`. */
  publishVersion(id: string, body: unknown): Promise<any> {
    return this.json(`/api/artifacts/${id}/versions`, this.jsonBody("POST", body, PUBLISH_TIMEOUT_MS));
  }

  /** `PATCH /api/artifacts/<id>` with metadata fields; `{artifact}`. */
  patch(id: string, body: unknown): Promise<any> {
    return this.json(`/api/artifacts/${id}`, this.jsonBody("PATCH", body));
  }

  /** `DELETE /api/artifacts/<id>`. */
  async delete(id: string): Promise<void> {
    await this.request(`/api/artifacts/${id}`, { method: "DELETE" });
  }

  /** The stored bytes of `path` in version `n`, unwrapped
   * (`GET /api/artifacts/<id>/versions/<n>/files/<path>`). */
  async fileBytes(id: string, n: number, path: string): Promise<Buffer> {
    return (await this.request(`/api/artifacts/${id}/versions/${n}/files/${encodePath(path)}`, { method: "GET" })).body;
  }

  /** `POST /api/artifacts/<id>/assets` (multipart field `file`): `{asset, url}`.
   * Without `contentType` the part carries none and the daemon infers it from
   * `filename`. */
  uploadAsset(id: string, filename: string, contentType: string | undefined, bytes: Buffer): Promise<any> {
    if (contentType !== undefined && /[\r\n"]/.test(contentType)) {
      throw new ClientError("api", "invalid content type", 400, { code: "invalid_content_type", message: `invalid content type '${contentType}'` });
    }
    const boundary = `clax-${Math.random().toString(16).slice(2)}${Date.now().toString(16)}`;
    const name = filename.replace(/["\r\n\\]/g, "_");
    const body = Buffer.concat([
      Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="${name}"\r\n${contentType === undefined ? "" : `Content-Type: ${contentType}\r\n`}\r\n`),
      bytes,
      Buffer.from(`\r\n--${boundary}--\r\n`),
    ]);
    return this.json(`/api/artifacts/${id}/assets`, {
      method: "POST",
      headers: { "content-type": `multipart/form-data; boundary=${boundary}` },
      body,
      timeoutMs: PUBLISH_TIMEOUT_MS,
    });
  }

  private docsPath(id: string, path: string, query: Record<string, string | undefined> = {}): string {
    const q = new URLSearchParams(Object.entries(query).filter((e): e is [string, string] => e[1] !== undefined));
    const qs = q.toString();
    return `/api/artifacts/${id}/docs/${encodePath(path)}${qs ? `?${qs}` : ""}`;
  }

  /** `GET /api/artifacts/<id>/docs/<path>`: `{doc}` (404 when absent or unreadable). */
  docGet(id: string, path: string, asLevel?: string): Promise<any> {
    return this.json(this.docsPath(id, path, { as_level: asLevel }), { method: "GET" });
  }

  /** `PUT /api/artifacts/<id>/docs/<path>` with `{data, if_version?}`: `{doc, created}`. */
  docPut(id: string, path: string, body: unknown, asLevel?: string): Promise<any> {
    return this.json(this.docsPath(id, path, { as_level: asLevel }), this.jsonBody("PUT", body));
  }

  /** `PATCH /api/artifacts/<id>/docs/<path>` with `{data, if_version?}`: `{doc}`. */
  docPatch(id: string, path: string, body: unknown, asLevel?: string): Promise<any> {
    return this.json(this.docsPath(id, path, { as_level: asLevel }), this.jsonBody("PATCH", body));
  }

  /** `DELETE /api/artifacts/<id>/docs/<path>?if_version=`: `{deleted}`. */
  docDelete(id: string, path: string, ifVersion?: number, asLevel?: string): Promise<any> {
    return this.json(this.docsPath(id, path, { if_version: ifVersion === undefined ? undefined : String(ifVersion), as_level: asLevel }), { method: "DELETE" });
  }

  /** `GET /api/artifacts/<id>/docs?<query>`: `{docs, next_cursor}`. */
  docList(id: string, query: [string, string][]): Promise<any> {
    return this.json(`/api/artifacts/${id}/docs?${new URLSearchParams(query)}`, { method: "GET" });
  }

  /** `POST /api/artifacts/<id>/docs:batch`: `{results}`. */
  docBatch(id: string, body: unknown, asLevel?: string): Promise<any> {
    const q = new URLSearchParams(asLevel === undefined ? {} : { as_level: asLevel }).toString();
    return this.json(`/api/artifacts/${id}/docs:batch${q ? `?${q}` : ""}`, this.jsonBody("POST", body));
  }

  /** `POST /api/artifacts/<id>/docs:str_replace`: `{doc}`. */
  docStrReplace(id: string, body: unknown, asLevel?: string): Promise<any> {
    const q = new URLSearchParams(asLevel === undefined ? {} : { as_level: asLevel }).toString();
    return this.json(`/api/artifacts/${id}/docs:str_replace${q ? `?${q}` : ""}`, this.jsonBody("POST", body));
  }

  /** `GET /api/artifacts/<id>/threads`: `{threads, next_cursor}`. */
  threads(id: string, includeResolved: boolean, cursor?: string): Promise<any> {
    const q = new URLSearchParams({ include_resolved: String(includeResolved) });
    if (cursor !== undefined) q.set("cursor", cursor);
    return this.json(`/api/artifacts/${id}/threads?${q}`, { method: "GET" });
  }

  /** `GET /api/artifacts/<id>/threads/<tid>`: `{thread}`. */
  thread(id: string, tid: string): Promise<any> {
    return this.json(`/api/artifacts/${id}/threads/${tid}`, { method: "GET" });
  }

  /**
   * An agent reply: `{comment, thread}` (with `addressed: "pending"` when `addressed` marked a live page's
   * thread), or `{guidance}` on a thread not sent to the agent. `addressed` is sent only when true.
   */
  reply(id: string, tid: string, text: string, addressed = false): Promise<any> {
    const body: Record<string, unknown> = { body: text, author_kind: "agent" };
    if (addressed) body.addressed = true;
    return this.json(`/api/artifacts/${id}/threads/${tid}/comments`, this.jsonBody("POST", body));
  }

  /** Resolves a thread as the agent: `{thread}`, or `{guidance}` on a thread not sent to the agent. */
  resolve(id: string, tid: string): Promise<any> {
    return this.json(`/api/artifacts/${id}/threads/${tid}/resolve`, this.jsonBody("POST", { as: "agent" }));
  }

  /** `PUT /api/sessions/<sid>/watches/<id>`: `{watch}`. */
  watch(id: string, replies: boolean): Promise<any> {
    return this.json(() => `${this.sessionPath()}/watches/${id}`, this.jsonBody("PUT", { replies_armed: replies }));
  }

  /** `DELETE /api/sessions/<sid>/watches/<id>`. */
  async unwatch(id: string): Promise<void> {
    await this.request(() => `${this.sessionPath()}/watches/${id}`, { method: "DELETE" });
  }

  /** `PUT /api/sessions/<sid>/live-watches`: a scope watch on the page `url`; `{live_watch, page, site, covered}`. */
  liveWatch(url: string, replies: boolean): Promise<any> {
    return this.json(() => `${this.sessionPath()}/live-watches`, this.jsonBody("PUT", { url, replies_armed: replies }));
  }

  /** `DELETE /api/sessions/<sid>/live-watches?url=`: `{page_url, removed}`. */
  liveUnwatch(url: string): Promise<any> {
    return this.json(() => `${this.sessionPath()}/live-watches?${new URLSearchParams({ url })}`, { method: "DELETE" });
  }

  /** `GET /api/live/pages?url=`: `{page, route}`, `page` null when the URL has no live page. */
  livePage(url: string): Promise<any> {
    return this.json(`/api/live/pages?${new URLSearchParams({ url })}`, { method: "GET" });
  }

  /** `PUT /api/sessions/<sid>/working/<id>`: `{working, message_truncated}`. */
  setWorking(id: string, body: { thread_ids?: string[]; message?: string }): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/${id}`, this.jsonBody("PUT", body));
  }

  /** `DELETE /api/sessions/<sid>/working/<id>`: `{cleared, working}`. */
  clearWorking(id: string, threadIds?: string[]): Promise<any> {
    const q = threadIds ? `?${new URLSearchParams({ thread_ids: threadIds.join(",") })}` : "";
    return this.json(() => `${this.sessionPath()}/working/${id}${q}`, { method: "DELETE" });
  }

  /** `POST /api/sessions/<sid>/working/renew`: `{renewed}`. Never starts a daemon. */
  renewWorking(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/renew`, this.jsonBody("POST", {}), this.discoverFn);
  }

  /** `POST /api/sessions/<sid>/working/end`: `{cleared}`. Never starts a daemon. */
  endWorking(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/end`, this.jsonBody("POST", {}), this.discoverFn);
  }

  /** `GET /api/sessions/<sid>/watches`: `{watches}`. */
  watches(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/watches`, { method: "GET" });
  }

  /** `GET /api/sessions/<sid>`: `{session, push}`. */
  sessionInfo(): Promise<any> {
    return this.json(this.sessionPath, { method: "GET" });
  }

  /** `GET /api/sessions/<sid>/feedback` for `tier`, waiting up to `waitS`
   * seconds (`artifact` narrows it to one artifact): `{feedback, text,
   * waited_s}`. The deadline is `waitS` plus 10 s; `signal` cancels it. */
  feedback(tier: string, waitS: number, artifact?: string, signal?: AbortSignal): Promise<any> {
    const q = new URLSearchParams({ tier, wait: String(waitS) });
    if (artifact !== undefined) q.set("artifact", artifact);
    return this.json(() => `${this.sessionPath()}/feedback?${q}`, { method: "GET", timeoutMs: (waitS + 10) * 1000, signal });
  }

  /** [`feedback`] for a background poll: it only discovers a running daemon
   * (registering the session there again when needed) and never starts one,
   * so a daemon the person stopped stays stopped. */
  pollFeedback(tier: string, waitS: number, signal: AbortSignal): Promise<any> {
    const q = new URLSearchParams({ tier, wait: String(waitS) });
    return this.json(() => `${this.sessionPath()}/feedback?${q}`, { method: "GET", timeoutMs: (waitS + 10) * 1000, signal }, this.discoverFn);
  }

  /** `POST /api/sessions/<sid>/feedback/ack` `{thread_ids}`: acknowledges every
   * row of the session on these threads. */
  ack(threadIds: string[]): Promise<any> {
    return this.json(() => `${this.sessionPath()}/feedback/ack`, this.jsonBody("POST", { thread_ids: threadIds }));
  }

  /** `POST /api/sessions/<sid>/feedback/ack` `{comment_ids}`: acknowledges the
   * session's rows for these comments only. */
  ackComments(commentIds: string[]): Promise<any> {
    return this.json(() => `${this.sessionPath()}/feedback/ack`, this.jsonBody("POST", { comment_ids: commentIds }));
  }

  /** `PATCH /api/sessions/<id>` `{"ended": true}` for the registered session,
   * within `timeoutMs` (default [`END_TIMEOUT_MS`]) for the requests.
   * Never starts a daemon. `undefined` when no session was registered. */
  async endSession(timeoutMs = END_TIMEOUT_MS): Promise<Session | undefined> {
    if (!this.registeredSession) return undefined;
    return sessionOf(await this.json(this.sessionPath, this.sessionPatch({ ended: true }), this.discoverFn, Date.now() + timeoutMs));
  }

  private sessionPatch(body: unknown): RequestOptions {
    return this.jsonBody("PATCH", body);
  }

  /** The path of this client's session (`/api/sessions/<id>`), as of the attempt. */
  private readonly sessionPath = () => `/api/sessions/${this.sessionId ?? ""}`;
}
