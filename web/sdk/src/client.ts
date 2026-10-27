// The client core the queue helpers share: signed requests to an instance
// and the one error shape.
import type { ApiErrorBody } from "./types";
import { newNonce, signature, type Signer } from "./sign";

/** A failed call: the error code the instance answered (`ApiError`'s
 * `error`), its message and the HTTP status; `network` with status 0 when
 * no answer came, `bad_response` for an answer that is not the JSON
 * expected. */
export class ClaxError extends Error {
  constructor(readonly code: string, message: string, readonly status: number) {
    super(message);
    this.name = "ClaxError";
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
  /** The instance's base URL, such as `https://clax.example`. */
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

/** The JSON of a 2xx answer; a `ClaxError` from any other. */
export async function readJson<T>(res: Response): Promise<T> {
  if (!res.ok) throw await failure(res);
  try {
    return await res.json() as T;
  } catch {
    throw new ClaxError("bad_response", "the answer is not JSON", res.status);
  }
}

/** The `ClaxError` a non-2xx answer stands for. */
export async function failure(res: Response): Promise<ClaxError> {
  let body: Partial<ApiErrorBody> = {};
  try {
    body = await res.json() as Partial<ApiErrorBody>;
  } catch {
    // An answer without the error shape keeps its status alone.
  }
  return new ClaxError(body.error ?? `http_${res.status}`, body.message ?? res.statusText, res.status);
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
