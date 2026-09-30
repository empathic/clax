// Whether the viewer's latest input went to the content frame, for capability
// calls that may act only on the viewer's own recent gesture in the page
// (opening the composer, sending to the agent, a pick's start).
//
// The shell window's transient user activation alone is not enough: it is
// also set by the viewer's input to the shell itself (a click on the Comment
// button or the "Your name" field, typing a reply), which a page calling on a
// timer could ride. Focus in the frame is not enough either: the page can move
// focus into itself with `window.focus()`, without any input of the viewer's.
// Nothing the page or the bridge reports counts (they share a realm). So a call
// counts only when all of these hold, from the shell's own trusted events:
//
// 1. the shell window has transient user activation;
// 2. focus is in the content frame (`iframe.frame`), and no trusted pointer
//    press or counted key press has reached the shell document since it entered;
// 3. the viewer, not the page, can have moved focus there: either
//    - the pointer was over the frame when focus entered it, having moved onto
//      the frame after the viewer's latest input to the shell (a click or tap
//      in the frame, or the bridge's `window.focus()` on an area drag's press);
//    - focus entered before the task after a Tab or Shift+Tab key press in
//      the shell, which was the latest shell input (keyboard entry);
//    - or the pointer is over the frame now, having moved onto it after the
//      viewer's latest input to the shell.
//
// A click in a cross-origin or sandboxed frame reaches the frame's document,
// never the shell's, so it leaves the record of shell input alone; the shell
// sees the pointer arrive as a `mouseover` on the iframe element (also sent
// before a touch tap's press), and leave as a `mouseover` on another element
// or a `mouseout` to outside the window.
//
// Keys that act on the page while the pointer is over it are not counted as
// input to the shell: modifier keys alone, Escape (which grants no
// activation), and Option+Up/Down, which the shell forwards to the page.
//
// Residual (what a hostile page can still do): within the activation window
// (about five seconds) after the viewer's latest input to the shell, once the
// viewer moves the pointer onto the frame, the page can pull focus into itself
// and have a call counted without a click in it. The same holds, without a
// further key, after the viewer Tabs into the frame. Keys typed in the shell
// with the pointer resting on the frame, and a pointer that has not moved onto
// the frame since the viewer's latest shell input, never count. The pointer's
// position is the browser's hit test in the shell, so this relies on the
// shell's layout not moving the frame under a resting pointer.

// Event order, not time: a counter bumped by each recorded event.
let seq = 0;
let lastShellInput = -1;
let frameEnteredAt = -1;
/** Whether the viewer can have made the latest frame entry (condition 3). */
let entryByViewer = false;
/** When the pointer moved onto the frame; -1 while it is elsewhere. */
let pointerOnFrameSince = -1;
/** The shell input that was a Tab key press, until the task after it. */
let tabAt = -1;
const isFrame = (el: EventTarget | null) => el instanceof HTMLIFrameElement && el.classList.contains("frame");
const PAGE_KEYS = new Set(["Alt", "AltGraph", "Control", "Meta", "Shift", "Escape"]);
/** Whether a key pressed in the shell counts as input to the shell: not a
 * modifier alone, Escape, or Option+Up/Down (see above). */
export function shellKey(e: Pick<KeyboardEvent, "key" | "altKey">): boolean {
  return !PAGE_KEYS.has(e.key) && !(e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown"));
}
const pointerFresh = () => pointerOnFrameSince > lastShellInput;

/** Records trusted shell input (the watcher does this; tests call it);
 * `tab`: a Tab or Shift+Tab key press, whose default action may move focus
 * into the frame before the next task. */
export function noteShellInput(tab = false): void {
  const at = ++seq;
  lastShellInput = at;
  tabAt = tab ? at : -1;
  if (tab) setTimeout(() => { if (tabAt === at) tabAt = -1; }, 0);
}

/** Records the pointer moving onto `target` (the watcher does this for
 * trusted `mouseover`s; tests call it); null: it left the window. */
export function notePointerOver(target: EventTarget | null): void {
  if (!isFrame(target)) pointerOnFrameSince = -1;
  else if (pointerOnFrameSince < 0) pointerOnFrameSince = ++seq;
}

function enterFrame(): void {
  frameEnteredAt = ++seq;
  entryByViewer = pointerFresh() || (tabAt >= 0 && tabAt === lastShellInput);
}

/** Watches `doc` (the shell document) for trusted input, for the pointer
 * moving onto and off the content frame, and for focus moving into the frame;
 * returns the unwatcher. */
export function watchGestures(doc: Document = document): () => void {
  const onPress = (e: Event) => { if (e.isTrusted) noteShellInput(); };
  const onKey = (e: KeyboardEvent) => { if (e.isTrusted && shellKey(e)) noteShellInput(e.key === "Tab"); };
  const onOver = (e: MouseEvent) => { if (e.isTrusted) notePointerOver(e.target); };
  const onOut = (e: MouseEvent) => { if (e.isTrusted && e.relatedTarget === null) notePointerOver(null); };
  const onFocus = (e: FocusEvent) => { if (isFrame(e.target)) enterFrame(); };
  // Focus moving into a cross-origin frame (a click, Tab, or the page's own
  // window.focus()) fires no focus event on the iframe element in Chromium;
  // the shell window's blur, with the frame focused, marks it.
  const onBlur = () => { if (isFrame(doc.activeElement)) enterFrame(); };
  const win = doc.defaultView;
  doc.addEventListener("pointerdown", onPress, true);
  doc.addEventListener("keydown", onKey, true);
  doc.addEventListener("mouseover", onOver, true);
  doc.addEventListener("mouseout", onOut, true);
  doc.addEventListener("focusin", onFocus, true);
  win?.addEventListener("blur", onBlur);
  return () => {
    doc.removeEventListener("pointerdown", onPress, true);
    doc.removeEventListener("keydown", onKey, true);
    doc.removeEventListener("mouseover", onOver, true);
    doc.removeEventListener("mouseout", onOut, true);
    doc.removeEventListener("focusin", onFocus, true);
    win?.removeEventListener("blur", onBlur);
  };
}

/** True when the viewer's latest input went to the content frame (see the
 * conditions above). */
export function frameGesture(doc: Document = document): boolean {
  const ua = (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation;
  if (ua?.isActive !== true) return false;
  if (!isFrame(doc.activeElement)) return false;
  if (lastShellInput >= frameEnteredAt) return false;
  return entryByViewer || pointerFresh();
}

/** Forgets what was recorded (tests). */
export function forgetGestures(): void {
  lastShellInput = -1;
  frameEnteredAt = -1;
  entryByViewer = false;
  pointerOnFrameSince = -1;
  tabAt = -1;
}

if (typeof document !== "undefined") watchGestures(document);
