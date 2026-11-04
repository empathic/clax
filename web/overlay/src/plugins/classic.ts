// The overlay as it was before plugins: what `overlay()` runs without a
// `plugins` option. Its own module, so the overlay entry imports these
// plugins alone, not every first-party plugin.
import type { ClaxPlugin } from "../../../sdk/src/plugin";
import { annotate } from "./annotate";
import { displayCapture, domCapture } from "./capture";
import { areaPicker, elementPicker, pagePicker } from "./pickers";
import { presentations } from "./presentations";

/** The DOM render and the exact screenshot, Box/Arrow/Redact/Text,
 * Element, Area and Screen, the corner dock. */
export function classic(): ClaxPlugin[] {
  return [domCapture(), displayCapture(), annotate({ tools: ["box", "arrow", "redact", "text"] }), elementPicker(), areaPicker(), pagePicker(), presentations()];
}
