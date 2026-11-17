// Why a request failed, and what the person is told. Every request helper
// reads a refusal through `readError` and a body through `readJson`, so the
// daemon's code is never dropped and an answer that is not Clax's (a proxy's
// or a CDN's page: a 530 "error code: 1016", an HTML 502, a 200 HTML page
// where JSON was asked for) is told apart from Clax's own refusal. Every
// entry loads it (and the stream's worker): when Clax is gone, a lazy chunk
// would not load either. The artifact view's prefixes are failure.ts's.

/** Why a request failed: `offline` (the browser says so), `unreachable` (no
 * answer), `not_clax` (an answer that is not Clax's), and for Clax's own
 * refusals `signed_out`, `gone` (deleted), `limited` (too many requests),
 * `server` (Clax failed) and `refused` (any other refusal). */
export type Trouble = "offline" | "unreachable" | "not_clax" | "signed_out" | "gone" | "limited" | "server" | "refused";

/** A non-OK answer; `status` is the HTTP status code, `code` the daemon's
 * error code and `reason` its message (without the status); `clax` whether
 * the answer was Clax's (a JSON body naming `error.code`). */
export class ApiError extends Error {
  readonly reason: string;
  constructor(public status: number, message: string, public code: string | null = null, readonly clax = code !== null) {
    super(`${status} ${message}`);
    this.name = "ApiError";
    this.reason = message;
  }
}

/** `res`, a refusal, as an `ApiError`: Clax's when its body is JSON naming
 * `error.code` (its message the reason), or when it has no body at all (a
 * local daemon's 404 for a route it does not have: a refusal, never a sign
 * Clax is gone); else one that is not Clax's (a CDN's or a proxy's page). */
export async function readError(res: Response): Promise<ApiError> {
  let e: { code?: unknown; message?: unknown } | undefined;
  const body = await res.text().catch(() => "");
  try { e = JSON.parse(body)?.error; } catch { /* not JSON: not Clax's */ }
  const code = typeof e?.code === "string" ? e.code : null;
  return new ApiError(res.status, code !== null && typeof e?.message === "string" ? e.message : "", code, code !== null || !body.trim());
}

/** `res`'s JSON body (`{}` for a 204); a refusal throws `readError`'s, and
 * an answer that is not JSON (a page served with 200) throws one that is
 * not Clax's. */
export async function readJson<T>(res: Response): Promise<T> {
  if (!res.ok) throw await readError(res);
  if (res.status === 204) return {} as T;
  try { return (await res.json()) as T; } catch { throw new ApiError(res.status, ""); }
}

/** What an answer to `path` says of reaching Clax: null for one of Clax's,
 * `not_clax` for one that is not (a refusal with a body that is not Clax's
 * JSON, or an HTML page where the API answers JSON), and undefined outside
 * the API and for the files a version serves (which may be HTML pages),
 * which tell nothing. Reads a clone, so `res` stays unread. */
export async function answered(res: Response, path: string): Promise<Trouble | null | undefined> {
  if (!path.startsWith("/api/") && path !== "/healthz") return undefined;
  if (path.includes("/files/")) return undefined;
  if (res.ok) return /html/.test(res.headers.get("content-type") ?? "") ? "not_clax" : null;
  return (await readError(res.clone())).clax ? null : "not_clax";
}

/** Why `e`, anything a request threw, failed; `online` is the browser's
 * `navigator.onLine`. A request with no answer is fetch's `TypeError` or a
 * `DOMException` (aborted, timed out); an error that is no request's is
 * `refused`. */
export function classify(e: unknown, online = typeof navigator === "undefined" || navigator.onLine !== false): Trouble {
  if (e instanceof ApiError) {
    if (!e.clax) return online ? "not_clax" : "offline";
    if (e.status === 401 && e.code !== "guest_name_required") return "signed_out";
    if (e.status === 404 && e.code === "not_found") return "gone";
    if (e.status === 429) return "limited";
    return e.status >= 500 ? "server" : "refused";
  }
  if (e instanceof TypeError || e instanceof DOMException) return online ? "unreachable" : "offline";
  return e instanceof SyntaxError ? "not_clax" : "refused";
}

/** Whether `e` says Clax was not reached (offline, no answer, an answer that
 * is not Clax's), rather than that Clax refused: worth asking again. */
export const unreached = (e: unknown): boolean => ["offline", "unreachable", "not_clax"].includes(classify(e));

/** What the shell says while the artifact's Comments switch is off (spec
 * 2026-10-21-app-push-pull §5.6): the Comment button's title, the comments
 * sidebar's note, a page's refused `comments` call, and after its prefix a
 * refused write (`comments_off`, [`failureText`]). */
export const COMMENTS_OFF = "Comments are off for this page";

/** What the person is told after a failure's prefix, by kind; `server` and
 * `refused` say the daemon's own message instead. */
export const REASON: Partial<Record<Trouble, string>> = {
  not_clax: "this Clax is not answering. It may have been taken down or moved.",
  offline: "you are offline.",
  unreachable: "Clax did not answer.",
  signed_out: "you are signed out.",
  gone: "it was deleted.",
  limited: "too many requests. Try again in a moment.",
};

/** Why `e` failed, in the person's words: by its kind, else the daemon's
 * message (or the error's own, for one that is no request's); never a
 * status number or a browser's error string. A `comments_off` refusal says
 * comments are off ([`COMMENTS_OFF`]). */
export function reasonText(e: unknown): string {
  if (e instanceof ApiError && e.code === "comments_off") return `${COMMENTS_OFF[0].toLowerCase()}${COMMENTS_OFF.slice(1)}`;
  const said = REASON[classify(e)];
  if (said) return said;
  return (e instanceof ApiError ? e.reason : e instanceof Error ? e.message : String(e)) || "Clax could not do it.";
}

/** `<prefix>: <reason>` for anything a request threw (`reasonText`). */
export function failureText(prefix: string, e: unknown): string {
  return `${prefix}: ${reasonText(e)}`;
}
