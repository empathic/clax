// What a modal dialog in the shell needs beyond `aria-modal`: the rest of the
// document inert while it is open, and Tab kept among its own controls.

/** Makes every element outside `el` inert (each sibling of `el` and of its
 * ancestors that is not inert already); returns the undo. */
export function inertOutside(el: Element): () => void {
  const made: Element[] = [];
  for (let n: Element = el; n.parentElement && n !== document.body; n = n.parentElement) {
    for (const sib of n.parentElement.children) if (sib !== n && !sib.hasAttribute("inert")) { sib.setAttribute("inert", ""); made.push(sib); }
  }
  return () => { for (const sib of made) sib.removeAttribute("inert"); };
}

const FOCUSABLE = "button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])";

/** For a keydown in `box`: a Tab or Shift+Tab cycles among the box's enabled
 * controls and never leaves it (with none, focus stays on the box). */
export function trapTab(e: KeyboardEvent, box: HTMLElement): void {
  if (e.key !== "Tab") return;
  const all = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)];
  const first = all[0], last = all[all.length - 1];
  if (!first) { e.preventDefault(); box.focus(); return; }
  const at = document.activeElement;
  if (e.shiftKey && (at === first || at === box || !box.contains(at))) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && (at === last || !box.contains(at))) { e.preventDefault(); first.focus(); }
}
