// Presentation plugins: how the composer is shown. `floating` (the corner
// dock, the overlay's own), `dialog` (prod Changeling's Send-feedback layout:
// a modal dialog in the middle of the page), `drawer` (a panel along the
// right edge), `sheet` (rising from the bottom), `window` (its own browser
// window; a full-screen page on a phone) and `extension` (the installed Clax
// extension takes the report; a dialog when it is not there).
import { definePlugin, type HandoffRequest, PLUGIN_API_VERSION, type PresentationContribution } from "../../../sdk/src/plugin";

/** The layouts every page can show. */
export const presentations = () => definePlugin({
  name: "presentations", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
  contributes: { presentations: [
    { id: "floating", label: "Floating in the corner", layout: "dock" },
    { id: "dialog", label: "Dialog", layout: "dialog" },
    { id: "drawer", label: "Side drawer", layout: "drawer" },
    { id: "sheet", label: "Bottom sheet", layout: "sheet" },
  ] },
});

/** The composer in its own window: opened by the overlay in the click or
 * key that starts the report (a popup blocker allows that), drawn there by
 * the overlay itself, so the anchor and screenshot never leave the page's
 * memory. A dialog where the page cannot open windows. */
export const windowPresentation = () => definePlugin({
  name: "presentation-window", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
  contributes: { presentations: [{ id: "window", label: "Its own window", layout: "window", fallback: "dialog", available: (win: Window) => typeof win.open === "function" }] },
});

/** The attribute the Clax extension sets on the page's root while it is
 * injected in the tab (its announced presence). */
export const EXTENSION_ATTR = "data-clax-extension";
/** The event the overlay dispatches on the page's document to hand a report
 * to the extension; the extension takes it by cancelling it. */
export const EXTENSION_EVENT = "clax:extension-report";

/** Whether the extension announced itself on `win`'s page. */
export const extensionPresent = (win: Window): boolean => !!win.document?.documentElement?.hasAttribute(EXTENSION_ATTR);

/** Hands the report to the extension: dispatches `EXTENSION_EVENT` with the
 * request; the extension's page script takes it with `preventDefault()`.
 * No data comes back to the page, and nothing is sent when no one takes it. */
export function handToExtension(req: HandoffRequest, win: Window): boolean {
  const Ev = (win as Window & typeof globalThis).CustomEvent ?? CustomEvent;
  const e = new Ev(EXTENSION_EVENT, { detail: { ...req }, cancelable: true });
  win.document.dispatchEvent(e);
  return e.defaultPrevented;
}

/** The installed Clax extension takes the report (its side panel, pinned to
 * this tab); `detect` and `handoff` default to the extension's announced
 * presence and its page event. A dialog when it is absent or declines. */
export const extensionPresentation = (opts: { detect?: (win: Window) => boolean; handoff?: (req: HandoffRequest, win: Window) => boolean } = {}) => {
  const p: PresentationContribution = {
    id: "extension", label: "The Clax extension", layout: "dialog", fallback: "dialog",
    available: opts.detect ?? extensionPresent,
    handoff: opts.handoff ?? handToExtension,
  };
  return definePlugin({ name: "presentation-extension", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [], contributes: { presentations: [p] } });
};

/** The site's look: `--clax-*` custom properties (see web/sdk/tokens). */
export const theme = (tokens: Record<string, string>) => definePlugin({
  name: "theme", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
  contributes: { theme: { tokens } },
});
