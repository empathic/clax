// What Clax's lockdown blocked the greeted page from (`clax:blocked`, from
// the bridge's `blocked.ts`): the other sites' origins the artifact page's
// notice names. The page can post the message itself, so each is checked
// for its shape and length, kept as text, at most `BLOCKED_MAX` a document,
// and nothing is done because of it but saying so.
import { BLOCKED_DIRECTIVE, BLOCKED_MAX, BLOCKED_ORIGIN_MAX } from "../../../bridge/src/protocol";

/** A report as the shell takes it: a known directive, and an origin alone
 * (`http`, `https`, `ws` or `wss`; no path, query, fragment or user)
 * other than `own`, the page's own site. Null for anything else. */
export function blockedReport(m: unknown, own: string): { directive: string; origin: string } | null {
  if (!m || typeof m !== "object") return null;
  const { directive, origin } = m as { directive?: unknown; origin?: unknown };
  if (typeof directive !== "string" || !BLOCKED_DIRECTIVE.test(directive)) return null;
  if (typeof origin !== "string" || !origin || origin.length > BLOCKED_ORIGIN_MAX || origin === own) return null;
  let u: URL;
  try { u = new URL(origin); } catch { return null; }
  if (u.origin !== origin || !/^(https?|wss?):$/.test(u.protocol)) return null;
  return { directive, origin };
}

/** The reports of one document: each directive and origin once, at most
 * `BLOCKED_MAX`; `origins` the distinct origins, first reported first. */
export class BlockedSites {
  private seen = new Set<string>();
  origins: string[] = [];
  /** Takes `m` (a `clax:blocked` from a page on `own`): true when it
   * named an origin not named before. */
  add(m: unknown, own: string): boolean {
    const r = blockedReport(m, own);
    if (!r || this.seen.size >= BLOCKED_MAX) return false;
    const k = `${r.directive} ${r.origin}`;
    if (this.seen.has(k)) return false;
    this.seen.add(k);
    if (this.origins.includes(r.origin)) return false;
    this.origins = [...this.origins, r.origin];
    return true;
  }
  clear(): void { this.seen.clear(); this.origins = []; }
}
