// The client core the queue helpers share: signed requests to an instance,
// the retries every signing client makes, and the one error shape.
import type { ApiErrorBody, ClaxErrorCode } from "./types";
import { newNonce, signature, type Signer } from "./sign";

/** A failed call: the error code the instance answered (`ApiError`'s
 * `error.code`), its message and the HTTP status; `network` with status 0 when
 * no answer came, `bad_response` for an answer that is not the JSON
 * expected. `retryAfterS` is the wait in seconds the answer asked for (its
 * `Retry-After`, else the error's `retry_after_s`), when it named one: a
 * caller passing a refusal on, such as a backend's token route answering
 * 429 `limit_reached`, passes it on too. */
export class ClaxError extends Error {
  declare readonly retryAfterS?: number;

  constructor(readonly code: ClaxErrorCode, message: string, readonly status: number, retryAfterS?: number) {
    super(message);
    this.name = "ClaxError";
    if (retryAfterS !== undefined) this.retryAfterS = retryAfterS;
  }
}

/** The `fetch` the clients call; injected in tests. */
export type Fetch = (input: string, init: RequestInit) => Promise<Response>;

/** The authorization schemes signed with `clax-ed25519 v2` on the queue routes. */
export type Scheme = "Clax-Queue" | "Clax-Reporter";

/** A token and the key it is bound to. */
export interface SignedCredential {
  scheme: Scheme;
  token: string;
  signer: Signer;
}

/** One signed request. */
export interface SignedRequest {
  method: string;
  /** The instance's origin, such as `https://clax.example` (`checkInstance`). */
  instance: string;
  /** The path and query, starting with `/`. */
  path: string;
  body?: Uint8Array;
  contentType?: string;
  credential: SignedCredential;
  /** Milliseconds since the Unix epoch. */
  now: () => number;
  fetch: Fetch;
}

/** Throws a `TypeError` naming `who` unless `instance` is an origin: an
 * `http` or `https` URL with no user, path (but `/`), query or fragment.
 * Clax is served at the root of its own host, and a request is signed
 * over its path as the instance receives it, so an instance behind a path
 * prefix cannot be reached. */
export function checkInstance(instance: string, who: string): void {
  let ok = false;
  try {
    const u = new URL(instance);
    ok = /^https?:$/.test(u.protocol) && !u.username && !u.password && u.pathname === "/" && !/[?#]/.test(instance);
  } catch {
    // Not a URL.
  }
  if (!ok) throw new TypeError(`${who}: instance must be an origin, such as https://clax.example.com, not ${JSON.stringify(instance)}`);
}

/** Backoffs, in seconds, before each retry of 429 `limit_reached` and 503
 * `busy` (`clax_core::remote::BACKOFF_S`): three retries, each waiting up
 * to a quarter more as jitter. */
export const BACKOFF_S = [1, 2, 4];
/** Seconds the instance's clock may be learned off by
 * (`clax_core::remote::MAX_OFFSET_S`); a `stale_request` whose `Date` is
 * further off is not retried. */
export const MAX_OFFSET_S = 600;

/** A client's time: its `clock`, `offsetMs` the instance's clock is ahead
 * of it as learned, `now()` the instance's time as best known (both in
 * milliseconds), `sleep` that waits out a backoff and `random`, a number in
 * [0, 1) that scales a backoff's jitter. `learn` takes an answer's `Date`
 * as the instance's clock: true when it is readable and within
 * [`MAX_OFFSET_S`] of `clock`. (No comment in its body: the package ships
 * class bodies' comments, which count toward its size ceilings.) */
export class Timing {
  offsetMs = 0;

  constructor(
    private readonly clock: () => number,
    readonly sleep: (ms: number) => Promise<void> = ms => new Promise(resolve => setTimeout(resolve, ms)),
    readonly random: () => number = Math.random,
  ) {}

  now(): number {
    return this.clock() + this.offsetMs;
  }

  learn(date: string | null): boolean {
    const at = date ? Date.parse(date) : NaN;
    if (Number.isNaN(at)) return false;
    const offset = at - this.clock();
    if (Math.abs(offset) > MAX_OFFSET_S * 1000) return false;
    this.offsetMs = offset;
    return true;
  }
}

/** `/api/queues/<queue>` and `rest`, each segment percent-encoded. */
export function queuePath(queue: string, ...rest: string[]): string {
  return ["/api/queues", queue, ...rest].map((s, i) => (i === 0 ? s : encodeURIComponent(s))).join("/");
}

/** Sends `r` with its `Authorization` and `Clax-Signature`, signed over
 * the path and query and `Host` the URL resolves to. The answer comes back
 * whatever its status; a request that gets no answer throws `network`. */
export async function signedFetch(r: SignedRequest): Promise<Response> {
  const url = new URL(r.path, r.instance);
  const pathAndQuery = url.pathname + url.search;
  const body = r.body ?? new Uint8Array();
  const headers: Record<string, string> = {
    authorization: `${r.credential.scheme} ${r.credential.token}`,
    "clax-signature": await signature(r.credential.signer, {
      method: r.method, pathAndQuery, host: url.host, t: Math.floor(r.now() / 1000), nonce: newNonce(), body, token: r.credential.token,
    }),
  };
  if (r.contentType) headers["content-type"] = r.contentType;
  const init: RequestInit = { method: r.method, headers, credentials: "omit" };
  if (r.body) init.body = body as BodyInit;
  try {
    return await r.fetch(url.href, init);
  } catch (e) {
    throw new ClaxError("network", e instanceof Error ? e.message : String(e), 0);
  }
}

/** The error code of a refusal, read from a copy so that the answer's own
 * body stays unread; `""` for one without the error shape. */
export async function errorCode(res: Response): Promise<string> {
  try {
    const body = await res.clone().json() as Partial<ApiErrorBody> | null;
    return typeof body?.error?.code === "string" ? body.error.code : "";
  } catch {
    return "";
  }
}

/** Sends `r` as `signedFetch` does, at `timing`'s time, and retries as
 * every signing client does (`docs/contract.md`, "Signed requests"),
 * signing each attempt anew: once on 401 `stale_request` when its `Date`
 * gives an offset within [`MAX_OFFSET_S`], which the calls after it keep,
 * and up to three times on 429 `limit_reached` and 503 `busy`, after
 * [`BACKOFF_S`]. The last answer comes back whatever its status. */
export async function sendSigned(r: Omit<SignedRequest, "now">, timing: Timing): Promise<Response> {
  let staleRetried = false;
  let backoffs = 0;
  for (;;) {
    const res = await signedFetch({ ...r, now: () => timing.now() });
    if (res.status !== 401 && res.status !== 429 && res.status !== 503) return res;
    const code = await errorCode(res);
    if (res.status === 401 && code === "stale_request" && !staleRetried) {
      staleRetried = true;
      if (timing.learn(res.headers.get("date"))) continue;
      return res;
    }
    if (((res.status === 429 && code === "limit_reached") || (res.status === 503 && code === "busy")) && backoffs < BACKOFF_S.length) {
      const base = BACKOFF_S[backoffs++] * 1000;
      await timing.sleep(base + base * Math.min(Math.max(timing.random(), 0), 1) / 4);
      continue;
    }
    return res;
  }
}

/** The JSON of a 2xx answer; a `ClaxError` from any other. */
export async function readJson<T>(res: Response): Promise<T> {
  if (!res.ok) throw await failure(res);
  try {
    return await res.json() as T;
  } catch {
    throw new ClaxError("bad_response", "the answer is not JSON", res.status);
  }
}

/** The `ClaxError` a non-2xx answer stands for: its wait is the seconds of
 * its `Retry-After` (Clax answers seconds, never a date), else of the
 * error's `retry_after_s`. */
export async function failure(res: Response): Promise<ClaxError> {
  let body: { error?: Partial<ApiErrorBody["error"]> & { retry_after_s?: unknown } } = {};
  try {
    body = (await res.json() as typeof body | null) ?? {};
  } catch {
    // An answer without the error shape keeps its status alone.
  }
  const header = res.headers.get("retry-after")?.trim() ?? "";
  const inBody = body.error?.retry_after_s;
  const after = /^\d+$/.test(header) ? Number(header) : typeof inBody === "number" && inBody >= 0 ? inBody : undefined;
  return new ClaxError(body.error?.code ?? `http_${res.status}`, body.error?.message ?? res.statusText, res.status, after);
}

const utf8 = new TextEncoder();

/** One multipart field: text, or bytes with a file name and type. */
export type Part = string | { bytes: Uint8Array; filename: string; type: string };

/** A `multipart/form-data` body of `parts` in their order, built here
 * rather than by `FormData` so the signed body is the bytes sent. */
export function multipart(parts: [string, Part][], boundary: string): { body: Uint8Array; contentType: string } {
  const chunks: Uint8Array[] = [];
  for (const [name, part] of parts) {
    if (typeof part === "string") {
      chunks.push(utf8.encode(`--${boundary}\r\ncontent-disposition: form-data; name="${name}"\r\n\r\n`), utf8.encode(part), utf8.encode("\r\n"));
    } else {
      chunks.push(utf8.encode(`--${boundary}\r\ncontent-disposition: form-data; name="${name}"; filename="${part.filename}"\r\ncontent-type: ${part.type}\r\n\r\n`), part.bytes, utf8.encode("\r\n"));
    }
  }
  chunks.push(utf8.encode(`--${boundary}--\r\n`));
  const body = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    body.set(c, at);
    at += c.length;
  }
  return { body, contentType: `multipart/form-data; boundary=${boundary}` };
}
