// The capabilities of the page's own origin (`db`, `assets`, `user`, `room`):
// the site's client, what `use()` resolves of them, their members and the
// room's socket. Loaded on the page's first claude.use() of one of them,
// framed or top-level, never for the shell's capabilities (parts/caps.ts).
import { type Site, siteOf } from "../caps/origin";
import { type SiteRelay, originClient } from "../caps/origin-client";
import type { Rpc } from "../rpc";

export { originUse } from "../caps/origin";

let relayed: Site | null = null;

/** The page's site, its writes relayed by the framing shell (`site.fetch`):
 * what a page the Clax app serves uses (its bridge tag says
 * `data-api="shell"`), as the app's WebViews give their scheme no request
 * body. One per page. */
export function relayedSite(rpc: Pick<Rpc, "call">, noCookie: () => void): Site {
  const relay: SiteRelay = r => rpc.call("site", "fetch", [r]) as ReturnType<SiteRelay>;
  return relayed ??= siteOf(originClient(new URL(location.origin), relay), noCookie);
}
