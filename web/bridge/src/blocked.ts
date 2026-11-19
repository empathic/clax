// What Clax's lockdown refused the page from other origins
// (`crates/clax-http/src/content_policy.rs`, `lockdown`), reported to the
// shell as `clax:blocked`: the directive and the blocked URL's origin only.
// In the eager bridge, so kept small.
import { BLOCKED_DIRECTIVE, BLOCKED_MAX, BLOCKED_ORIGIN_MAX } from "./protocol";

/** One refusal as the shell hears it. */
export type Blocked = { directive: string; origin: string };

/** The directives every lockdown carries after its `frame-ancestors`, in
 * its order. A page's `<meta>` policy cannot set `frame-ancestors` (the
 * browser ignores it there), so a policy that starts with it and carries
 * these is taken for the lockdown: only a `<meta>` written to read exactly
 * so is mistaken for it. */
const LOCKDOWN_TAIL = /connect-src 'self' ?; ?object-src 'none' ?; ?base-uri 'none' ?; ?form-action 'none'/;

/** Whether `policy` (a violation's `originalPolicy`) is Clax's lockdown. */
export function isLockdown(policy: string): boolean {
  const p = policy.toLowerCase().replace(/\s+/g, " ").trim();
  return p.startsWith("frame-ancestors") && LOCKDOWN_TAIL.test(p);
}

/** The refusal `e` stands for, or null: it must be the browser's
 * (`isTrusted`), enforced, of the lockdown, by a directive the lockdown
 * refuses other origins by, and of an `http`, `https`, `ws` or `wss` URL of
 * an origin other than `own`, the page's. */
export function lockdownRefusal(e: SecurityPolicyViolationEvent, own: string): Blocked | null {
  if (!e.isTrusted || e.disposition !== "enforce" || !isLockdown(String(e.originalPolicy))) return null;
  const directive = String(e.effectiveDirective);
  if (!BLOCKED_DIRECTIVE.test(directive)) return null;
  let u: URL;
  try { u = new URL(String(e.blockedURI)); } catch { return null; }
  const origin = u.origin;
  return /^(https?|wss?):$/.test(u.protocol) && origin !== own && origin.length <= BLOCKED_ORIGIN_MAX ? { directive, origin } : null;
}

/** The refusals a document has seen, each directive and origin once, at
 * most `BLOCKED_MAX`. `add` answers whether it is new. */
export class BlockedLog {
  readonly all: Blocked[] = [];
  private seen = new Set<string>();
  add(b: Blocked): boolean {
    const k = b.directive + " " + b.origin;
    if (this.seen.has(k) || this.all.length >= BLOCKED_MAX) return false;
    this.seen.add(k);
    this.all.push(b);
    return true;
  }
}
