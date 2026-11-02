// Capture plugins: the private DOM render of the page (no permission asked),
// and the browser's exact screenshot of this tab (`getDisplayMedia`, after the
// reporter allows it). The overlay offers a screenshot only when a capture
// plugin is in; without one a report is its note and where it points.
import { definePlugin, PLUGIN_API_VERSION } from "../../../sdk/src/plugin";
import { displayCaptureSupported } from "../../../bridge/src/display-capture";
import { type CaptureDeps, type Captured, type Mode, captureExact, captureFor } from "../capture";
import type { Picked } from "../picker";

/** What the overlay asks of a capture plugin. */
export interface CaptureRequest {
  mode: Mode;
  pick: Picked | null;
  win: Window;
  deps: CaptureDeps;
}

/** The DOM render: `captureFor` (bridge/src/clip.ts, with its privacy: typed values blanked, cross-origin images and frames left out and counted). */
export const domCapture = () => definePlugin({
  name: "capture-dom", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["dom:read", "dom:render"],
  contributes: { capture: [{ id: "dom-render", label: "A screenshot of this page", method: "dom", take: (r: unknown): Promise<Captured> => { const q = r as CaptureRequest; return captureFor(q.mode, q.pick, q.win, q.deps); } }] },
});

/** The browser's exact screenshot of this tab, cropped to the pick; the DOM render (`deps.fallback`) when it cannot be had. */
export const displayCapture = () => definePlugin({
  name: "capture-display", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["media:display"],
  contributes: { capture: [{ id: "display-media", label: "Use exact screenshot", method: "display", supported: displayCaptureSupported, take: (r: unknown): Promise<Captured> => { const q = r as CaptureRequest; return captureExact(q.mode, q.pick, q.win, q.deps); } }] },
});
