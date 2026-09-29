// Whether the viewer's latest gesture was made inside the content frame, for
// capability calls that may act only on the viewer's own recent gesture in
// the page (opening the composer, sending to the agent). The shell window's
// transient user activation alone is not enough: it is also set by the
// viewer's input to the shell itself (typing a reply, the consent dialog),
// which a page calling on a timer could ride. So the activation counts only
// while focus is in the content frame (`iframe.frame`) and no trusted
// pointer or key input has reached the shell document since focus last
// entered the frame; a click inside a cross-origin frame reaches the frame's
// document, never the shell's, so it leaves that record alone.

// Event order, not time: a counter bumped by each recorded event.
let seq = 0;
let lastShellInput = -1;
let frameEnteredAt = -1;
const isFrame = (el: Element | null) => el instanceof HTMLIFrameElement && el.classList.contains("frame");

/** Records trusted shell input (the watcher does this; tests call it). */
export function noteShellInput(): void {
  lastShellInput = ++seq;
}

/** Watches `doc` (the shell document) for trusted input and for focus moving
 * into the content frame; returns the unwatcher. */
export function watchGestures(doc: Document = document): () => void {
  const onInput = (e: Event) => { if (e.isTrusted) noteShellInput(); };
  const onFocus = (e: FocusEvent) => { if (isFrame(e.target as Element | null)) frameEnteredAt = ++seq; };
  // A click into a cross-origin frame fires no focus event on the iframe
  // element in Chromium; the shell window's blur, with the frame focused, marks it.
  const onBlur = () => { if (isFrame(doc.activeElement)) frameEnteredAt = ++seq; };
  const win = doc.defaultView;
  doc.addEventListener("pointerdown", onInput, true);
  doc.addEventListener("keydown", onInput, true);
  doc.addEventListener("focusin", onFocus, true);
  win?.addEventListener("blur", onBlur);
  return () => {
    doc.removeEventListener("pointerdown", onInput, true);
    doc.removeEventListener("keydown", onInput, true);
    doc.removeEventListener("focusin", onFocus, true);
    win?.removeEventListener("blur", onBlur);
  };
}

/** True when the viewer has just acted inside the content frame: transient
 * activation, focus in the frame, and no shell input since it entered. */
export function frameGesture(doc: Document = document): boolean {
  const ua = (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation;
  if (ua?.isActive !== true) return false;
  if (!isFrame(doc.activeElement)) return false;
  return lastShellInput < frameEnteredAt;
}

/** Forgets what was recorded (tests). */
export function forgetGestures(): void {
  lastShellInput = -1;
  frameEnteredAt = -1;
}

if (typeof document !== "undefined") watchGestures(document);
