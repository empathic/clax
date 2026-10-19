// The thread drawer's place and size for its controls (DrawerChrome.svelte,
// with the sidebar's lazy chunk): the prefs as this browser stored them
// (which the artifact page's head script applied before the first paint),
// checked, and each change applied to the page and remembered.
import { type DrawerPrefs, DRAWER_KEY, applyPrefs, normal, readPrefs } from "../view/drawer-model";

let prefs: DrawerPrefs = $state.raw(normal(readPrefs()));

export const drawer = {
  get prefs(): DrawerPrefs { return prefs; },
  /** Shows `p`; `keep` also remembers it (a drag remembers where it ends). */
  set(p: DrawerPrefs, keep = true): void {
    prefs = p;
    applyPrefs(p);
    if (keep) try { localStorage.setItem(DRAWER_KEY, JSON.stringify(p)); } catch { /* this page only */ }
  },
};
