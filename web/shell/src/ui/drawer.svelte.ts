// The thread drawer's place and size for its controls (DrawerChrome.svelte,
// with the sidebar's lazy chunk): the prefs as this browser stored them
// (which the artifact page's head script applied before the first paint),
// checked and applied in place of what the head script applied, and each
// change applied to the page and remembered.
//
// Its layout is also Clax's own app dev setting (web/sdk/src/devtools.ts),
// the hook's first client in Clax itself (Clax native before the SDK or a
// site): on a dev or local origin the Clax DevTools panel shows it and sets
// it. The viewer's override is the layout this browser stored; its base,
// the default (docked right). The hook's module loads only on such an
// origin, so no other page pays for it.
import type { DevSettingsHandle } from "../../../sdk/src/devtools";
import { type DrawerLayout, type DrawerPrefs, DEFAULTS, DRAWER_KEY, LAYOUTS, applyPrefs, normal, readPrefs } from "../view/drawer-model";

let prefs: DrawerPrefs = $state.raw(normal(readPrefs()));
applyPrefs(prefs);
let dev: DevSettingsHandle | null = null;

/** Whether this browser stored a layout of its own (the viewer's override in the DevTools panel). */
const chosen = () => LAYOUTS.includes(readPrefs().layout as DrawerLayout);

export const drawer = {
  get prefs(): DrawerPrefs { return prefs; },
  /** Shows `p`; `keep` also remembers it (a drag remembers where it ends). */
  set(p: DrawerPrefs, keep = true): void {
    const was = prefs.layout;
    prefs = p;
    applyPrefs(p);
    if (keep) try { localStorage.setItem(DRAWER_KEY, JSON.stringify(p)); } catch { /* this page only */ }
    // The DevTools panel reads it again.
    if (keep && p.layout !== was) dev?.setBase("layout", { value: DEFAULTS.layout, source: "site" });
  },
};

const LABELS: Record<DrawerLayout, string> = { right: "Docked right", left: "Docked left", bottom: "Docked at the bottom", float: "Floating window" };
const KEY = "drawer.layout";

/** Registers the layout with the dev settings hook (`register`: the SDK's `registerDevSettings`). */
export function registerDrawer(register: typeof import("../../../sdk/src/devtools").registerDevSettings): DevSettingsHandle | null {
  dev = register({
    id: "drawer", title: "Clax thread drawer", description: "Where an artifact page's threads open, wider than a phone; kept in this browser.",
    fields: [{
      id: "layout", label: "Layout", kind: "radio", applies: "live", base: { value: DEFAULTS.layout, source: "site" },
      options: LAYOUTS.map(l => ({ value: l, label: LABELS[l] })),
    }],
  }, {
    // The drawer's own stored prefs are the override: nothing is kept twice.
    store: {
      get: k => (k === KEY && chosen() ? prefs.layout : undefined),
      set: (k, v) => {
        if (k !== KEY || !LAYOUTS.includes(v as DrawerLayout)) return false;
        drawer.set({ ...prefs, layout: v as DrawerLayout });
        return chosen();
      },
      delete: k => {
        if (k !== KEY) return;
        try { const o = readPrefs(); delete o.layout; localStorage.setItem(DRAWER_KEY, JSON.stringify(o)); } catch { /* this page only */ }
        prefs = { ...prefs, layout: DEFAULTS.layout };
        applyPrefs(prefs);
      },
    },
  });
  return dev;
}

// Loopback, `.localhost`, `.local` and `.test` names (the SDK's `isDevOrigin`, which decides again).
if (typeof location !== "undefined" && /^(localhost|127(\.\d+){3}|\[::1\])$|\.(localhost|local|test)$/.test(location.hostname)) {
  void import("../../../sdk/src/devtools").then(m => registerDrawer(m.registerDevSettings), () => {});
}
