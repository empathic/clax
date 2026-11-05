// The shell's own controls a page may point at: each is marked in its
// component with `data-shell-control="<name>"`, and one is found by name,
// among the elements the viewer can see now.

export const SHELL_CONTROLS = ["comment", "threads", "versions", "publish", "fork", "send"] as const;
export type ShellControl = (typeof SHELL_CONTROLS)[number];

export const isShellControl = (v: unknown): v is ShellControl => typeof v === "string" && (SHELL_CONTROLS as readonly string[]).includes(v);

/** `el`'s own visibility and opacity: `checkVisibility` where the browser has it, else the computed style. */
function visible(el: Element): boolean {
  if (typeof el.checkVisibility === "function") return el.checkVisibility({ opacityProperty: true, visibilityProperty: true });
  const cs = getComputedStyle(el);
  return cs.visibility !== "hidden" && cs.visibility !== "collapse" && cs.opacity !== "0";
}

/** Whether `el` is shown now: connected, laid out with a box, visible
 * (opacity and visibility included), not `disabled`, and under no `[inert]`
 * or `[aria-hidden="true"]` ancestor. */
export function isShown(el: Element): boolean {
  if (!el.isConnected || el.getClientRects().length === 0) return false;
  if (!visible(el)) return false;
  if (el.matches(":disabled")) return false;
  return el.closest('[inert], [aria-hidden="true"]') === null;
}

/** The first `[data-shell-control="<c>"]` in document order that `shown` accepts, or `null`. */
export function findControl(c: ShellControl, doc: Document = document, shown: (el: Element) => boolean = isShown): HTMLElement | null {
  for (const el of doc.querySelectorAll<HTMLElement>(`[data-shell-control="${c}"]`)) if (shown(el)) return el;
  return null;
}

const CLIPS = new Set(["auto", "scroll", "hidden", "clip", "overlay"]);

/** Whether `el`'s box is in view: inside the window and inside every
 * ancestor that scrolls or clips it (up to a fixed one, which leaves the rest
 * behind). It counts when any of the box shows, or with `whole` only when all
 * of it does. */
export function inView(el: Element, whole = false): boolean {
  const r = el.getBoundingClientRect();
  let clip = { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
  for (let a = el.parentElement; a; a = a.parentElement) {
    const cs = getComputedStyle(a);
    if (a !== document.body && a !== document.documentElement && (CLIPS.has(cs.overflowX) || CLIPS.has(cs.overflowY))) {
      const b = a.getBoundingClientRect();
      clip = { left: Math.max(clip.left, b.left), top: Math.max(clip.top, b.top), right: Math.min(clip.right, b.right), bottom: Math.min(clip.bottom, b.bottom) };
    }
    if (cs.position === "fixed") break;
  }
  return whole
    ? r.left >= clip.left && r.top >= clip.top && r.right <= clip.right && r.bottom <= clip.bottom
    : r.left < clip.right && r.right > clip.left && r.top < clip.bottom && r.bottom > clip.top;
}

/** Scrolls `el` into view only when it is not wholly in view, so a page that
 * points at a control again and again cannot hold the viewer's scroll. */
export function revealIfOutside(el: HTMLElement): void {
  if (!inView(el, true)) el.scrollIntoView?.({ block: "nearest" });
}
