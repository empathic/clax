// Where the picking hint docks: never over what the reporter is pointing at
// (BRIEF clax-feedback-plugins Addendum 4).

/** How close, in CSS px, the pointer may come to the chip before it moves to the other edge. */
export const KEEP_CLEAR_PX = 96;

/** How long, in ms, the pointer stays near the chip without reaching it before
 * the chip moves: a pointer on its way to the chip's buttons gets there first. */
export const KEEP_CLEAR_DWELL_MS = 500;

/** The edge the chip docks to: the one the pointer is farther from, so the
 * chip is never over what the reporter is pointing at. It stays where it is
 * until the pointer comes within `KEEP_CLEAR_PX` of it. */
export function edgeFor(current: "top" | "bottom", pointerY: number, viewportH: number, chipH: number): "top" | "bottom" {
  const near = current === "top" ? pointerY <= chipH + KEEP_CLEAR_PX : pointerY >= viewportH - chipH - KEEP_CLEAR_PX;
  if (!near) return current;
  return current === "top" ? "bottom" : "top";
}
