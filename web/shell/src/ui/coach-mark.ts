// The coach mark: a ring drawn around one of the shell's controls (spec
// 2026-10-26 §8). One at a time in the window; it goes after `COACH_MS`, on
// any press or Escape, and when its control stops being shown or scrolls out of
// view. It follows the control on `resize`, on a scroll of the window or any
// element, and on the control's own resizes.
import { after } from "../clock";
import { inView, isShown } from "./shell-controls";

export const COACH_MS = 6_000;
/** The ring's distance from the control's box, in px. */
export const COACH_PAD = 4;

const STYLE = `
.coach-mark { position: fixed; z-index: 45; box-sizing: border-box; border-radius: 10px; pointer-events: none;
  box-shadow: 0 0 0 2px var(--accent), 0 0 0 6px color-mix(in srgb, var(--accent) 30%, transparent);
  animation: coach-mark-pulse 1.2s ease-in-out 2; }
@keyframes coach-mark-pulse { 50% { box-shadow: 0 0 0 2px var(--accent), 0 0 0 10px color-mix(in srgb, var(--accent) 12%, transparent); } }
@media (prefers-reduced-motion: reduce) { .coach-mark { animation: none; } }
`;

let current: (() => void) | null = null;

/** Draws the ring around `el`, replacing any ring already drawn; returns its idempotent dismiss. */
export function showCoachMark(el: HTMLElement, shown: (el: Element) => boolean = isShown): () => void {
  current?.();
  if (!document.head.querySelector("style[data-coach-mark]")) {
    const style = document.createElement("style");
    style.setAttribute("data-coach-mark", "");
    style.textContent = STYLE;
    document.head.appendChild(style);
  }
  const ring = document.createElement("div");
  ring.className = "coach-mark";
  ring.setAttribute("aria-hidden", "true");
  document.body.appendChild(ring);

  const place = () => {
    if (!shown(el) || !inView(el)) { dismiss(); return; }
    const r = el.getBoundingClientRect();
    ring.style.left = `${r.left - COACH_PAD}px`;
    ring.style.top = `${r.top - COACH_PAD}px`;
    ring.style.width = `${r.width + 2 * COACH_PAD}px`;
    ring.style.height = `${r.height + 2 * COACH_PAD}px`;
  };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") dismiss(); };
  const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);

  let done = false;
  const cancel = after(COACH_MS, () => dismiss());
  function dismiss() {
    if (done) return;
    done = true;
    if (current === dismiss) current = null;
    cancel();
    observer?.disconnect();
    window.removeEventListener("resize", place);
    window.removeEventListener("scroll", place, true);
    window.removeEventListener("pointerdown", dismiss, true);
    window.removeEventListener("keydown", onKey, true);
    ring.remove();
  }
  current = dismiss;
  window.addEventListener("resize", place);
  window.addEventListener("scroll", place, true);
  window.addEventListener("pointerdown", dismiss, true);
  window.addEventListener("keydown", onKey, true);
  observer?.observe(el);
  place();
  return dismiss;
}
