// Whether the viewer's latest input went to the content frame, for capability
// calls that may act only on the viewer's own recent gesture in the page
// (opening the composer, sending to the agent, a pick's start, a page's
// republish).
//
// The shell window's transient user activation alone is not enough: it is
// also set by the viewer's input to the shell itself (a click on the Comment
// button, the "Your name" field, Cancel or Post in the composer, Allow in the
// consent dialog, a pin), which a page calling on a timer could ride. Focus in
// the frame is not enough either: the page can move focus into itself with
// `window.focus()`, without any input of the viewer's. Nothing the page or the
// bridge reports counts (they share a realm). So a call counts only when all
// of these hold, from the shell's own trusted events:
//
// 1. the shell window has transient user activation;
// 2. focus is in the content frame (`iframe.frame`), and no trusted pointer
//    press or counted key press has reached the shell document since it
//    entered;
// 3. the viewer, not the page, can have moved focus there: either
//    - the pointer arrived on the frame after the viewer's latest input to
//      the shell, by a real move (below), and was on it when focus entered
//      (a click or tap in the frame, or the bridge's `window.focus()` on an
//      area drag's press) or is on it now;
//    - or focus entered after a Tab or Shift+Tab key press in the shell that
//      was the latest shell input, before focus reached any shell element
//      and within 500 ms (keyboard entry; Shift+Tab into a cross-origin frame
//      lands some tasks later).
//
// A real move. The shell sees the pointer arrive on the frame as a `mouseover`
// on the iframe element (also sent before a touch tap's press), and leave as a
// `mouseover` on another element or a `mouseout` to outside the window. But
// Chromium also sends that `mouseover`, with unchanged coordinates, when a
// shell control over the frame vanishes from under a resting pointer (the
// composer closing on Cancel or Post, the consent dialog on Allow or Enter, a
// banner's Dismiss, a pin that moves away). So an arrival counts only when its
// coordinates differ by more than 2 px from where the pointer was at the
// viewer's latest shell input. That position comes from trusted `pointermove`,
// `mousemove` and `pointerdown` in the shell (never from `mouseover`, whose
// coordinates prove no movement), and is taken for key presses too. Before any
// shell input every arrival counts; after a key press with no known position,
// an arrival counts only after a trusted move since that key.
//
// The shield. A mouse at rest over the frame after the viewer's click on a
// control there (Cancel, Post, Allow, a pin, Dismiss) would otherwise have to
// leave the frame and come back before a click in the page counted: when the
// control vanishes the browser moves its hover to the frame with no real
// arrival, and moves inside a cross-origin frame never reach the shell. So at
// such a press, or a key press with the pointer resting over the frame's box
// on a shell element (the consent dialog's backdrop), the shell puts a small
// transparent square (`registerShield`) under the pointer, beneath every
// shell control. When the control vanishes the pointer is over the square,
// not the frame; the viewer's first move out of it arrives on the frame at a
// new position, which counts. A click on the square without such a move is
// spent on it. Touch input raises no square (it would take the next tap): a
// tap's arrival always comes at the tap's own coordinates, and Chromium sends
// no arrival when a control vanishes after a tap. Keys typed in the shell with
// the pointer resting on the page itself (its position there unknown) raise
// none either: the pointer then has to leave the frame and come back.
//
// Keys the shell forwards to the page (Option, Option+Up/Down and Escape in
// comment mode with the pointer over the frame, `setForwardedKeys`) are not
// input to the shell; every other key press in the shell is.
//
// Residual (what a hostile page can still do): within the activation window
// (about five seconds) after the viewer's latest input to the shell, once the
// viewer moves the pointer onto or over the frame by more than 2 px, the page
// can pull focus into itself and have a call counted without a click in it.
// The same holds, without a further key, after the viewer Tabs into the frame.
// A pointer that has not moved since the viewer's latest shell input never
// counts, whatever the shell's layout does around it.

// Event order, not time: a counter bumped by each recorded event.
let seq = 0;
let lastShellInput = -1;
let frameEnteredAt = -1;
/** Whether the viewer can have made the latest frame entry (condition 3). */
let entryByViewer = false;
/** When the pointer's counted arrival on the frame was; -1 while it is
 * elsewhere or arrived without a real move. */
let pointerOnFrameSince = -1;
/** Whether the latest `mouseover` targeted the frame (counted or not). */
let overFrame = false;
/** The shell input that was a Tab key press, until the task after it. */
let tabAt = -1;
type Pos = { x: number; y: number };
/** Where the pointer last was, from trusted moves and presses in the shell. */
let lastPos: Pos | null = null;
let lastPointerType = "";
/** When the latest trusted move was. */
let lastMoveAt = -1;
/** Where the pointer was at the latest shell input. */
let inputPos: Pos | null = null;
let shield: HTMLElement | null = null;
/** Where the shield is centred while it is up. */
let shieldAt: Pos | null = null;
let forwarded: (e: KeyboardEvent) => boolean = () => false;

/** How long a Tab press may take to move focus into the frame. Shift+Tab
 * into a cross-origin frame lands after later tasks have run; a Tab that moves
 * focus to a shell element ends the wait at once. */
const TAB_MS = 500;
/** Farther than this from the input position, in CSS pixels, is a real move. */
const MOVE_PX = 2;
/** The shield reaches this far from the input position, in CSS pixels. */
const SHIELD_PX = 4;
const isFrame = (el: EventTarget | null) => el instanceof HTMLIFrameElement && el.classList.contains("frame");
const pointerFresh = () => pointerOnFrameSince > lastShellInput;
const far = (a: Pos, x: number, y: number) => Math.abs(x - a.x) > MOVE_PX || Math.abs(y - a.y) > MOVE_PX;
const outside = (a: Pos, x: number, y: number) => Math.abs(x - a.x) > SHIELD_PX || Math.abs(y - a.y) > SHIELD_PX;
/** Whether an arrival at (x, y) is a real move since the latest shell input. */
const moved = (x: number, y: number) => lastShellInput < 0 || (inputPos ? far(inputPos, x, y) : lastMoveAt > lastShellInput);

/** Records trusted shell input (the watcher does this; tests call it);
 * `tab`: a Tab or Shift+Tab key press, whose default action may move focus
 * into the frame before the next task. */
export function noteShellInput(tab = false): void {
  const at = ++seq;
  lastShellInput = at;
  inputPos = lastPos && { ...lastPos };
  tabAt = tab ? at : -1;
  if (tab) setTimeout(() => { if (tabAt === at) tabAt = -1; }, TAB_MS);
}

/** Records a trusted pointer move or press at (x, y) in the shell (the watcher
 * does this; tests call it); hides the shield after a real move. */
export function notePointerAt(x: number, y: number, pointerType = "mouse"): void {
  lastPos = { x, y };
  lastPointerType = pointerType;
  lastMoveAt = ++seq;
  if (shieldAt && outside(shieldAt, x, y)) lowerShield();
}

/** Records the pointer moving onto `target` at (x, y) (the watcher does this
 * for trusted `mouseover`s; tests call it); null: it left the window. An
 * arrival on the frame counts only after a real move. */
export function notePointerOver(target: EventTarget | null, x = NaN, y = NaN): void {
  // The pointer left the shield's square.
  if (shieldAt && outside(shieldAt, x, y)) lowerShield();
  overFrame = isFrame(target);
  if (!overFrame) pointerOnFrameSince = -1;
  else if (pointerOnFrameSince < 0 && moved(x, y)) pointerOnFrameSince = ++seq;
}

/** The element `watchGestures` shows over the content frame (see the
 * header); null when it is unmounted. */
export function registerShield(el: HTMLElement | null): void {
  if (shield && shield !== el) shield.style.display = "";
  shield = el;
  if (!el) shieldAt = null;
}

/** Whether the shield is up (tests). */
export function shielded(): boolean {
  return shieldAt !== null;
}

function lowerShield(): void {
  shieldAt = null;
  if (shield) shield.style.display = "";
}

/** Raises the shield under the pointer when a mouse press or key press in
 * the shell happens over the frame's box on a shell element (the watcher calls
 * it at that input). */
export function raiseShieldIfOverFrame(doc: Document = document): void {
  lowerShield();
  if (lastPointerType === "touch" || !lastPos || overFrame) return;
  const frame = doc.querySelector("iframe.frame");
  if (!frame) return;
  const r = frame.getBoundingClientRect();
  const { x, y } = lastPos;
  if (x < r.left || x >= r.right || y < r.top || y >= r.bottom) return;
  shieldAt = { x, y };
  if (!shield) return;
  const box = shield.parentElement?.getBoundingClientRect() ?? r;
  Object.assign(shield.style, { display: "block", left: `${x - box.left - SHIELD_PX}px`, top: `${y - box.top - SHIELD_PX}px`, width: `${2 * SHIELD_PX + 1}px`, height: `${2 * SHIELD_PX + 1}px` });
}

/** Sets which key events the shell forwards to the page (not input to the
 * shell); returns the reset. */
export function setForwardedKeys(pred: (e: KeyboardEvent) => boolean): () => void {
  forwarded = pred;
  return () => { if (forwarded === pred) forwarded = () => false; };
}

/** Records a trusted key press in the shell (the watcher does this; tests
 * call it); whether it counted as shell input (not a forwarded key). */
export function noteShellKey(e: KeyboardEvent): boolean {
  if (forwarded(e)) return false;
  noteShellInput(e.key === "Tab");
  return true;
}

function enterFrame(): void {
  frameEnteredAt = ++seq;
  entryByViewer = pointerFresh() || (tabAt >= 0 && tabAt === lastShellInput);
}

/** Watches `doc` (the shell document) for trusted input, for the pointer
 * moving onto and off the content frame, and for focus moving into the frame;
 * returns the unwatcher. */
export function watchGestures(doc: Document = document): () => void {
  // The shield sits beneath every shell control, so raising it at the press
  // leaves the press's click to its control.
  const onPress = (e: PointerEvent) => {
    if (!e.isTrusted) return;
    notePointerAt(e.clientX, e.clientY, e.pointerType);
    noteShellInput();
    raiseShieldIfOverFrame(doc);
  };
  const onMove = (e: MouseEvent) => { if (e.isTrusted) notePointerAt(e.clientX, e.clientY, e instanceof PointerEvent ? e.pointerType : lastPointerType || "mouse"); };
  const onKey = (e: KeyboardEvent) => {
    if (!e.isTrusted || !noteShellKey(e)) return;
    raiseShieldIfOverFrame(doc);
    // A Tab from outside the document (the browser's own controls) can land
    // in the frame with no blur here; the entry shows as focus in the frame.
    const at = lastShellInput;
    if (e.key === "Tab") setTimeout(() => { if (tabAt === at && frameEnteredAt < at && isFrame(doc.activeElement)) enterFrame(); }, 0);
  };
  const onOver = (e: MouseEvent) => { if (e.isTrusted) notePointerOver(e.target, e.clientX, e.clientY); };
  const onOut = (e: MouseEvent) => { if (e.isTrusted && e.relatedTarget === null) notePointerOver(null); };
  const onWheel = (e: WheelEvent) => { if (e.isTrusted && shieldAt) lowerShield(); };
  const onFocus = (e: FocusEvent) => { if (isFrame(e.target)) enterFrame(); else tabAt = -1; };
  // Focus moving into a cross-origin frame (a click, Tab, or the page's own
  // window.focus()) fires no focus event on the iframe element in Chromium;
  // the shell window's blur, with the frame focused, marks it.
  const onBlur = () => { if (isFrame(doc.activeElement)) enterFrame(); };
  const win = doc.defaultView;
  const on: [string, EventListener][] = [
    ["pointerdown", onPress as EventListener],
    ["pointermove", onMove as EventListener], ["mousemove", onMove as EventListener],
    ["keydown", onKey as EventListener], ["mouseover", onOver as EventListener],
    ["mouseout", onOut as EventListener], ["wheel", onWheel as EventListener], ["focusin", onFocus as EventListener],
  ];
  for (const [t, f] of on) doc.addEventListener(t, f, true);
  win?.addEventListener("blur", onBlur);
  return () => {
    for (const [t, f] of on) doc.removeEventListener(t, f, true);
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
  overFrame = false;
  tabAt = -1;
  lastPos = null;
  lastPointerType = "";
  lastMoveAt = -1;
  inputPos = null;
  forwarded = () => false;
  lowerShield();
}

if (typeof document !== "undefined") watchGestures(document);
