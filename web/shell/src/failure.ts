// Why a request failed, and what the person is told. Every request helper
// reads a refusal through `readError` and a body through `readJson`, so the
// daemon's code is never dropped and an answer that is not Clax's (a proxy's
// or a CDN's page: a 530 "error code: 1016", an HTML 502, a 200 HTML page
// where JSON was asked for) is told apart from Clax's own refusal.

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
 * `error.code` (its message the reason), else one that is not Clax's. */
export async function readError(res: Response): Promise<ApiError> {
  let e: { code?: unknown; message?: unknown } | undefined;
  try { e = JSON.parse(await res.text())?.error; } catch { /* not JSON: not Clax's */ }
  const code = typeof e?.code === "string" ? e.code : null;
  return new ApiError(res.status, code !== null && typeof e?.message === "string" ? e.message : "", code);
}

/** `res`'s JSON body (`{}` for a 204); a refusal throws `readError`'s, and
 * an answer that is not JSON (a page served with 200) throws one that is
 * not Clax's. */
export async function readJson<T>(res: Response): Promise<T> {
  if (!res.ok) throw await readError(res);
  if (res.status === 204) return {} as T;
  try { return (await res.json()) as T; } catch { throw new ApiError(res.status, ""); }
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

export const SEND_FAILED = "Could not send to the agent";
export const RESOLVE_FAILED = "Could not resolve";
export const REOPEN_FAILED = "Could not reopen";
export const POST_FAILED = "Could not post";
export const NAME_FAILED = "Could not save your name";
export const NAME_LOAD_FAILED = "Could not load your name";
export const LOAD_FAILED = "Could not load comments";
export const OPEN_FAILED = "Could not open";
export const APPROVE_FAILED = "Could not approve";
export const SHEET_FAILED = "Could not open the keyboard shortcuts";
/** What the shell says when a lazy part of the bridge could not load in the page. */
export const PART_FAILED: Record<"comment" | "clip" | "caps" | "origin" | "room" | "sample" | "test", string> = {
  comment: "Comment mode could not load in this page",
  clip: "Screenshots could not load in this page",
  caps: "This page's capabilities could not load",
  origin: "This page's data could not load",
  room: "This page's live room could not load",
  sample: "This page's Claude calls could not load",
  test: "This page's tests could not load",
};

/** What the shell says while the artifact's Comments switch is off (spec
 * 2026-10-21-app-push-pull §5.6): the Comment button's title, the comments
 * sidebar's note, a page's refused `comments` call, and after its prefix a
 * refused write (`comments_off`, [`failureText`]). */
export const COMMENTS_OFF = "Comments are off for this page";

/** `<prefix>: <message>` for an API error, a network failure, or anything
 * thrown; a `comments_off` refusal says comments are off ([`COMMENTS_OFF`]). */
export function failureText(prefix: string, e: unknown): string {
  if (e instanceof ApiError && e.code === "comments_off") return `${prefix}: ${COMMENTS_OFF[0].toLowerCase()}${COMMENTS_OFF.slice(1)}`;
  const message = e instanceof ApiError || e instanceof Error ? e.message : String(e);
  return `${prefix}: ${message}`;
}

/** Awaits `p`. On success clears the notice and returns the value; on failure
 * shows `failureText(prefix, e)` and returns `undefined`. Nothing is swallowed. */
export async function report<T>(p: Promise<T>, prefix: string, setNotice: (text: string | null) => void): Promise<T | undefined> {
  try {
    const v = await p;
    setNotice(null);
    return v;
  } catch (e) {
    setNotice(failureText(prefix, e));
    return undefined;
  }
}

/** A notice setter for one kind of call: a failure text always shows, while a
 * success (`null`) clears the notice only when one of `prefixes` raised it, so
 * an unrelated success never hides a failure. `set` takes a value or updater. */
export function scopedNotice(set: (u: string | null | ((prev: string | null) => string | null)) => void, ...prefixes: string[]): (text: string | null) => void {
  return text => set(cur => (text !== null ? text : cur !== null && prefixes.some(p => cur.startsWith(`${p}:`)) ? null : cur));
}
