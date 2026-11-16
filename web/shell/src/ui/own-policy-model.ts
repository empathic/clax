// The content policy switch's words (spec 2026-11-15-site-own-content-policy):
// its label, the explanation shown before it is turned on, and the line an
// artifact that sets its own policy shows; and the words of the notice the
// artifact page shows when Clax's lockdown blocked the page from other sites
// (`view/blocked.ts`). One wording wherever the switch sits: the hosted Share
// dialog and the Permissions panel. Pure: the components render it.

export const OWN_POLICY_LABEL = "Let this clax set its own content policy";
export const OWN_POLICY_EXPLAIN = "Its pages may then load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website.";
export const OWN_POLICY_NOTE = "Sets its own content policy: its pages load and send where they choose.";
/** The Permissions panel's heading over the switch. */
export const OWN_POLICY_HEADING = "Content policy";
/** What the Permissions panel says under the switch while it is off. */
export const LOCKED_NOTE = "Locked: its pages load and connect to its own site only.";

/** How many origins the notice names before it says how many more. */
export const NAMED_MAX = 3;

/** The blocked notice's words for `origins` (distinct, as the page reported
 * them, at least one): `admin`, for whoever may turn the switch, says how
 * many sites and names them; anyone else gets one line. `own`: the switch
 * was turned on since, so the page loads them once reloaded (said to an
 * administrator only; null for anyone else, who sees nothing then). */
export function blockedNotice(origins: readonly string[], admin: boolean, own: boolean): { text: string; sites: string | null } | null {
  if (!origins.length) return null;
  if (!admin) return own ? null : { text: "This page tried to load from other sites, and Clax blocked it.", sites: null };
  const n = origins.length;
  const named = origins.slice(0, NAMED_MAX).join(", ") + (n > NAMED_MAX ? `, and ${n - NAMED_MAX} more` : "");
  if (own) return { text: "This clax now sets its own content policy. Reload to let it reach these sites.", sites: named };
  return { text: `This clax tried to reach ${n === 1 ? "another site" : `${n} other sites`}, and Clax blocked it.`, sites: named };
}
