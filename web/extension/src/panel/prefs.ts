// What the side panel remembers per site, in chrome.storage.local (owner
// decision 2026-10-06; a convenience only): the status filter and the
// "Elsewhere" groups the person collapsed. Storage that fails or holds
// something else reads as the defaults.
import { FILTERS, type Filter } from "./site-model";

export type Prefs = { filter: Filter; collapsed: string[] };
export const DEFAULT_PREFS: Prefs = { filter: "all", collapsed: [] };
type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
const key = (origin: string) => `site-prefs:${origin}`;
/** The most collapsed groups kept per site. */
const MAX_COLLAPSED = 200;

export async function loadPrefs(area: Area | undefined, origin: string): Promise<Prefs> {
  try {
    const v = (await area!.get(key(origin)))[key(origin)] as Partial<Prefs> | undefined;
    return {
      filter: FILTERS.some(f => f.value === v?.filter) ? v!.filter! : DEFAULT_PREFS.filter,
      collapsed: Array.isArray(v?.collapsed) ? v.collapsed.filter((c): c is string => typeof c === "string").slice(0, MAX_COLLAPSED) : [],
    };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(area: Area | undefined, origin: string, p: Prefs): void {
  try {
    void area?.set({ [key(origin)]: { filter: p.filter, collapsed: p.collapsed.slice(-MAX_COLLAPSED) } }).catch(() => {});
  } catch { /* not kept */ }
}
