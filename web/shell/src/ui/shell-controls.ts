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
