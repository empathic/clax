// A decision the daemon holds for the owner's confirmation (docs/contract.md,
// "Browser confirmations"): while an agent is live, a decision made from a
// browser that may not decide alone answers 202 `confirmation_pending` and
// changes nothing. The shell's decision calls read it here, and say where
// to confirm instead of taking the body for the decision's result.

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
