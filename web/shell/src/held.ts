// A decision the daemon holds for the owner's confirmation (docs/contract.md,
// "Browser confirmations"): while an agent is live, a decision made from a
// browser that may not decide alone answers 202 `confirmation_pending` and
// changes nothing. The shell's decision calls read it here, and say where
// to confirm instead of taking the body for the decision's result. The
// owner's browser's own calls go through `browserCall`, which reads it for
// each and presents a confirmation's ID when the call is made again.

/** A held decision: its confirmation, where to confirm it, and the line that says so. */
export type Held = { id: string; url: string; message: string };

/** The code a held decision's refusal carries. */
export const HELD = "confirmation_pending";

/** `res` read as a held decision, or null for any other answer. Reads a clone, so `res` stays unread. */
export async function heldOf(res: Response): Promise<Held | null> {
  if (res.status !== 202) return null;
  let b: { status?: unknown; confirmation_id?: unknown; confirm_url?: unknown } = {};
  try { b = await res.clone().json(); } catch { return null; }
  if (b.status !== HELD) return null;
  const url = typeof b.confirm_url === "string" ? b.confirm_url : "";
  return { id: typeof b.confirmation_id === "string" ? b.confirmation_id : "", url, message: `Confirm in your browser: ${url}` };
}

/** A route's refusal: its status, the daemon's message and code; `held`
 * when the decision waits for the owner's confirmation (status 202, code
 * `confirmation_pending`, the message saying where to confirm). */
export class Refused extends Error {
  constructor(public status: number, message: string, public code: string | null, public held: Held | null = null) {
    super(message);
    this.name = "Refused";
  }
}

/** The header that presents a confirmation's ID with the call it confirms:
 * an ID the daemon handed this shell, never a page's. */
const CONFIRMATION_HEADER = "x-clax-confirmation";

/** The owner's browser's call to `path` (its cookie, never the token),
 * with `body` as JSON and `confirm`, a held decision's confirmation ID,
 * presented with the call it confirms. Answers the body (`{}` for a 204);
 * a held decision or any other refusal throws `Refused`. */
export async function browserCall<T>(path: string, method = "GET", body?: unknown, confirm?: string): Promise<T> {
  const headers: Record<string, string> = body === undefined ? {} : { "content-type": "application/json" };
  if (confirm) headers[CONFIRMATION_HEADER] = confirm;
  const res = await fetch(path, { method, headers, cache: "no-store", ...body !== undefined && { body: JSON.stringify(body) } });
  const held = await heldOf(res);
  if (held) throw new Refused(202, held.message, HELD, held);
  if (res.ok) return (res.status === 204 ? {} : await res.json()) as T;
  let b: { error?: { code?: string; message?: string } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}
