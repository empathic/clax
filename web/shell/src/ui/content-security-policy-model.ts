// The Content Security Policy setting's words (spec
// 2026-11-15-site-own-content-policy): its button's two labels, the
// explanation shown before `unrestricted` is sent, and the line an artifact
// served unrestricted shows; and the words of the notice the artifact page shows when Clax's
// lockdown blocked the page from other sites (`view/blocked.ts`). One
// wording wherever the setting sits: the hosted Share dialog and the
// Permissions panel. Pure: the components render it.

/** The setting's button while the clax is `self`: it opens the explanation. */
export const CSP_UNRESTRICT = "Serve without Clax's Content Security Policy…";
/** The setting's button while the clax is `unrestricted`: it sends `self`. */
export const CSP_RESTORE = "Restore Clax's Content Security Policy";
export const CSP_EXPLAIN = "Clax then sends no Content-Security-Policy holding its pages to their own site: they may load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website.";
export const CSP_NOTE = "Served without Clax's Content Security Policy: its pages load and send where they choose.";
/** The Permissions panel's heading over the setting. */
export const CSP_HEADING = "Content Security Policy";
/** What the Permissions panel says under the setting while it is `self`. */
export const SELF_NOTE = "Held to its own site: its pages load and connect to it alone.";

/** How many origins the notice names before it says how many more. */
export const NAMED_MAX = 3;

/** The blocked notice's words for `origins` (distinct, as the page reported
 * them, at least one): `admin`, for whoever may change the setting, says
 * how many sites and names them; anyone else gets one line.
 * `unrestricted`: the setting was changed since, so the page loads them
 * once reloaded (said to an administrator only; null for anyone else, who
 * sees nothing then). */
export function blockedNotice(origins: readonly string[], admin: boolean, unrestricted: boolean): { text: string; sites: string | null } | null {
  if (!origins.length) return null;
  if (!admin) return unrestricted ? null : { text: "This page tried to load from other sites, and Clax blocked it.", sites: null };
  const n = origins.length;
  const named = origins.slice(0, NAMED_MAX).join(", ") + (n > NAMED_MAX ? `, and ${n - NAMED_MAX} more` : "");
  if (unrestricted) return { text: "This clax is now served without Clax's Content Security Policy. Reload to let it reach these sites.", sites: named };
  return { text: `This clax tried to reach ${n === 1 ? "another site" : `${n} other sites`}, and Clax blocked it.`, sites: named };
}
