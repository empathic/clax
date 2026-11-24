// The pages that moved into Settings, and where each is now. Apart from the
// screens' list so the gallery's first chunk carries only this.

const ADMIN_TABS: Record<string, string> = { people: "roles", provider: "provider", web: "web", pricing: "pricing", usage: "usage" };

/** Where a page that moved into Settings is now, with the query it keeps:
 * `/people`, `/admin` (by its `?tab=`), `/me` and `/audit`. Null for any other path. */
export function moved(pathname: string, search = ""): string | null {
  const q = new URLSearchParams(search);
  const rest = () => { const s = q.toString(); return s ? `?${s}` : ""; };
  switch (pathname) {
    case "/people": return `/settings/people${rest()}`;
    case "/me": return `/settings/claxbot${rest()}`;
    case "/audit": return `/settings/audit${rest()}`;
    case "/admin": {
      const tab = ADMIN_TABS[q.get("tab") ?? "people"] ?? "roles";
      q.delete("tab");
      return `/settings/${tab}${rest()}`;
    }
    default: return null;
  }
}
