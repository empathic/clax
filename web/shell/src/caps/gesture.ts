// Whether the viewer's latest input went to the content frame, for capability
// calls that may act only on the viewer's own recent gesture in the page. Two
// tiers:
//
// - `frameGesture()`, for calls whose worst case is a composer the viewer
//   sees (`openComposer`, `compose`, a pick's start): the page can prefill
//   what it shows, and nothing is posted without the viewer.
// - `frameGestureStrict()`, for calls that act beyond the composer
//   (`sendToClaude`, a reply into a thread sent to the agent,
//   `artifact.publish`): `frameGesture()` and, in addition, no trusted input
//   to the shell for `SHELL_QUIET_MS`, which uses no pointer heuristics.
//
// Nothing the page or the bridge reports counts (they share a realm); every
// rule below reads only the shell's own trusted events.
//
// `frameGesture()` holds when all of these hold:
//
// 1. the shell window has transient user activation;
// 2. focus is in the content frame (`iframe.frame`), and no trusted pointer
//    press or counted key press has reached the shell document since it
//    entered;
// 3. the viewer, not the page, can have moved focus there: either
//    - the pointer arrived on the frame by a real move after the viewer's
//      latest input to the shell, and was on it when focus entered (a click
//      or tap in the frame, or the bridge's `window.focus()` on an area
//      drag's press) or is on it now;
//    - or focus entered after a Tab or Shift+Tab key press in the shell that
//      was the latest shell input, before focus reached any shell element
//      and within `TAB_MS` (keyboard entry).
//
// A real move. The shell sees the pointer arrive on the frame as a `mouseover`
// on the iframe element (also sent before a touch tap's press). Chromium also
// sends that `mouseover`, with unchanged coordinates, whenever the layout
// changes under a resting pointer: a shell control over the frame vanishing
// (the composer on Cancel or Post, the consent dialog, a banner's Dismiss),
// or an element the page moves (a pin) coming and going. So the shell records
// the pointer's position from every trusted pointer event it gets (moves,
// presses, and the boundary events `mouseover` and a `mouseout` leaving the
// window), and an arrival counts only when both hold:
// - its coordinates differ from those of the previous boundary event, when
//   that was the latest pointer event (a boundary event at the same spot is a
//   layout change, never a move; a `mousemove` there is a move);
// - they are more than 2 px from where the pointer was at the viewer's latest
//   shell input (for key presses too). After a key press with no known
//   position, an arrival counts only after a trusted move since that key.
// Before any shell input every arrival counts.
//
// The shield. Moves inside a cross-origin frame never reach the shell, so a
// pointer resting over the frame after shell input (a click on Cancel, Post,
// Allow, a pin or Dismiss over the page, or keys typed with the pointer on the
// page) would have to leave the frame and come back before a click in the page
// counted. So at such input the shell covers the frame with transparent bands
// (`registerShield`) that leave a 9 px hole under the pointer, beneath every
// shell control. A press or wheel in the hole reaches the page. The viewer's
// first move out of the hole lands on a band: that trusted `mousemove` lowers
// the bands, and the pointer's arrival on the frame then counts. A press on a
// band (possible only after a move the shell could not see, inside the page)
// does nothing at all: it is not shell input, it does not reach the page, the
// bands come down, and the shell shows its hint (`onShieldPress`). Touch input
// raises no bands: a tap's arrival always comes at the tap's own coordinates.
//
// Keys the shell forwards to the page (Option, Option+Up/Down and Escape in
// comment mode with the pointer over the frame, `setForwardedKeys`) are not
// input to the shell for `frameGesture()`; every other key press is.
//
// Residual of `frameGesture()` (what a hostile page can still do): within the
// activation window (about five seconds) after the viewer's latest input to
// the shell, once the viewer moves the pointer onto or over the frame by more
// than 2 px, or Tabs into it, the page can pull focus into itself and have a
// low-tier call counted without a click or key in the page: it can open the
// composer, prefilled, or forge a pick. The viewer sees that composer, and
// nothing is posted without them.
//
// `frameGestureStrict()` adds: no trusted pointer press or release, key press
// or release (forwarded keys included), or wheel in the shell for the last
// `SHELL_QUIET_MS`. The shell's own transient activation from such input has
// then expired, so an active `navigator.userActivation` can only come from
// input to the frame. Its residual: a page can act within the activation
// window after the viewer's own click or key in the page, whatever that input
// was meant for. The cost: a viewer who clicks a page's control within that
// time after using the shell is refused (`shell_input_recent`) and must click
// again.

/** Chromium's transient user activation lasts 5 s from the input that
 * granted it; half a second more so a shell input's activation has surely
 * expired before a strict call may rely on `isActive`. */
export const SHELL_QUIET_MS = 5_500;
/** How long a Tab press may take to move focus into the frame; a Tab that
 * moves focus to a shell element ends the wait at once. */
const TAB_MS = 500;
/** Farther than this from the input position, in CSS pixels, is a real move. */
const MOVE_PX = 2;
/** The shield's hole reaches this far from the pointer, in CSS pixels. */
const HOLE_PX = 4;

// Event order, not time: a counter bumped by each recorded event.
let seq = 0;
let lastShellInput = -1;
/** When (`performance.now()`) the latest trusted shell input of any kind was,
 * for `frameGestureStrict`. */
let lastShellInputAt = -Infinity;
let frameEnteredAt = -1;
/** Whether the viewer can have made the latest frame entry (condition 3). */
let entryByViewer = false;
/** When the pointer's counted arrival on the frame was; -1 while it is
 * elsewhere or arrived without a real move. */
let pointerOnFrameSince = -1;
/** Whether the latest `mouseover` targeted the frame (counted or not). */
let overFrame = false;
/** The shell input that was a Tab key press, until focus lands or `TAB_MS`. */
let tabAt = -1;
type Pos = { x: number; y: number };
/** Where the pointer last was, from any trusted pointer event in the shell. */
let lastPos: Pos | null = null;
/** Whether `lastPos` came from a boundary event (`mouseover`, `mouseout`). */
let lastWasBoundary = false;
let lastPointerType = "";
/** When the latest trusted move was. */
let lastMoveAt = -1;
/** Where the pointer was at the latest shell input. */
let inputPos: Pos | null = null;
let shield: HTMLElement | null = null;
/** The shield's hole, while it is up. */
let holeAt: Pos | null = null;
let forwarded: (e: KeyboardEvent) => boolean = () => false;
let shieldPressed: () => void = () => {};

const isFrame = (el: EventTarget | null) => el instanceof HTMLIFrameElement && el.classList.contains("frame");
const isShield = (el: EventTarget | null) => !!shield && el instanceof Node && el !== shield && shield.contains(el);
const pointerFresh = () => pointerOnFrameSince > lastShellInput;
const far = (a: Pos, x: number, y: number) => Math.abs(x - a.x) > MOVE_PX || Math.abs(y - a.y) > MOVE_PX;
const now = () => performance.now();

/** Records trusted shell input (the watcher does this; tests call it);
 * `tab`: a Tab or Shift+Tab key press, whose default action may move focus
 * into the frame. */
export function noteShellInput(tab = false): void {
  const at = ++seq;
  lastShellInput = at;
  lastShellInputAt = now();
  inputPos = lastPos && { ...lastPos };
  tabAt = tab ? at : -1;
  if (tab) setTimeout(() => { if (tabAt === at) tabAt = -1; }, TAB_MS);
}

/** Records trusted shell input that `frameGesture` does not count (a key the
 * shell forwards to the page, a key release, a wheel): only
 * `frameGestureStrict` waits for it. */
export function noteQuietBreak(): void {
  lastShellInputAt = now();
}

/** Records a trusted pointer move or press at (x, y) in the shell (the watcher
 * does this; tests call it). A move lowers the shield. */
export function notePointerAt(x: number, y: number, pointerType = "mouse"): void {
  lastPos = { x, y };
  lastWasBoundary = false;
  lastPointerType = pointerType;
  lastMoveAt = ++seq;
  if (holeAt) lowerShield();
}

/** Records the pointer moving onto `target` at (x, y) (the watcher does this
 * for trusted `mouseover`s, and for a `mouseout` leaving the window with a
 * null target; tests call it). An arrival on the frame counts only after a
 * real move (see the header). */
export function notePointerOver(target: EventTarget | null, x = NaN, y = NaN): void {
  const known = Number.isFinite(x) && Number.isFinite(y);
  const layout = known && lastWasBoundary && !!lastPos && lastPos.x === x && lastPos.y === y;
  if (known) { lastPos = { x, y }; lastWasBoundary = true; }
  overFrame = isFrame(target);
  if (!overFrame) { pointerOnFrameSince = -1; return; }
  if (pointerOnFrameSince >= 0 || !known) return;
  const moved = lastShellInput < 0 || (!layout && (inputPos ? far(inputPos, x, y) : lastMoveAt > lastShellInput));
  if (moved) pointerOnFrameSince = ++seq;
}

/** The element whose four children are the shield's bands (see the header);
 * null when it is unmounted. */
export function registerShield(el: HTMLElement | null): void {
  if (shield && shield !== el) shield.style.display = "";
  shield = el;
  if (!el) holeAt = null;
}

/** Sets what the shell does when a press lands on the shield (its hint);
 * returns the reset. */
export function onShieldPress(f: () => void): () => void {
  shieldPressed = f;
  return () => { if (shieldPressed === f) shieldPressed = () => {}; };
}

/** Whether the shield is up (tests). */
export function shielded(): boolean {
  return holeAt !== null;
}

function lowerShield(): void {
  holeAt = null;
  if (shield) shield.style.display = "";
}

/** Raises the shield, with its hole under the pointer, when a mouse rests
 * over the frame's box at shell input (the watcher calls it at that input). */
export function raiseShieldIfOverFrame(doc: Document = document): void {
  lowerShield();
  if (lastPointerType === "touch" || !lastPos) return;
  const frame = doc.querySelector("iframe.frame");
  if (!frame) return;
  const r = frame.getBoundingClientRect();
  // The pointer rests on the frame, or on a shell element over it. (Chromium
  // may deliver the last move over the shell after the frame's `mouseover`,
  // so `lastPos` can lie just outside the frame while the pointer is on it;
  // the hole is then outside, and the bands cover the whole frame.)
  const { x, y } = lastPos;
  if (!overFrame && (x < r.left || x >= r.right || y < r.top || y >= r.bottom)) return;
  holeAt = { x, y };
  if (!shield) return;
  // The shield covers the frame's box exactly (both fill the stage).
  const hx = x - r.left;
  const hy = y - r.top;
  const [top, bottom, left, right] = Array.from(shield.children) as HTMLElement[];
  const band = (el: HTMLElement | undefined, l: string, t: string, w: string, h: string) => { if (el) Object.assign(el.style, { left: l, top: t, width: w, height: h }); };
  band(top, "0", "0", "100%", `${Math.max(0, hy - HOLE_PX)}px`);
  band(bottom, "0", `${hy + HOLE_PX + 1}px`, "100%", `calc(100% - ${hy + HOLE_PX + 1}px)`);
  band(left, "0", `${hy - HOLE_PX}px`, `${Math.max(0, hx - HOLE_PX)}px`, `${2 * HOLE_PX + 1}px`);
  band(right, `${hx + HOLE_PX + 1}px`, `${hy - HOLE_PX}px`, `calc(100% - ${hx + HOLE_PX + 1}px)`, `${2 * HOLE_PX + 1}px`);
  shield.style.display = "block";
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
  if (forwarded(e)) { noteQuietBreak(); return false; }
  noteShellInput(e.key === "Tab");
  return true;
}

/** Records a trusted press on the shield's band at (x, y) (the watcher does
 * this; tests call it). It reaches neither the page nor any shell control, is
 * not shell input for `frameGesture`, lowers the shield, and the viewer is
 * told why (`onShieldPress`). It still grants the shell activation, so the
 * strict tier waits it out. */
export function noteShieldPress(x: number, y: number, pointerType = "mouse"): void {
  notePointerAt(x, y, pointerType);
  noteQuietBreak();
  shieldPressed();
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
    if (isShield(e.target)) { noteShieldPress(e.clientX, e.clientY, e.pointerType); return; }
    notePointerAt(e.clientX, e.clientY, e.pointerType);
    noteShellInput();
    raiseShieldIfOverFrame(doc);
  };
  const onRelease = (e: PointerEvent) => { if (e.isTrusted) noteQuietBreak(); };
  // Nor does a press on a band move focus.
  const onMouseDown = (e: MouseEvent) => { if (e.isTrusted && isShield(e.target)) e.preventDefault(); };
  const onMove = (e: MouseEvent) => { if (e.isTrusted) notePointerAt(e.clientX, e.clientY, e instanceof PointerEvent ? e.pointerType : lastPointerType || "mouse"); };
  const onKey = (e: KeyboardEvent) => {
    if (!e.isTrusted || !noteShellKey(e)) return;
    raiseShieldIfOverFrame(doc);
    // A Tab from outside the document (the browser's own controls) can land
    // in the frame with no blur here; the entry shows as focus in the frame.
    const at = lastShellInput;
    if (e.key === "Tab") setTimeout(() => { if (tabAt === at && frameEnteredAt < at && isFrame(doc.activeElement)) enterFrame(); }, 0);
  };
  const onKeyUp = (e: KeyboardEvent) => { if (e.isTrusted) noteQuietBreak(); };
  const onOver = (e: MouseEvent) => { if (e.isTrusted) notePointerOver(e.target, e.clientX, e.clientY); };
  const onOut = (e: MouseEvent) => { if (e.isTrusted && e.relatedTarget === null) notePointerOver(null, e.clientX, e.clientY); };
  const onWheel = (e: WheelEvent) => {
    if (!e.isTrusted) return;
    noteQuietBreak();
    if (isShield(e.target)) lowerShield();
  };
  const onFocus = (e: FocusEvent) => { if (isFrame(e.target)) enterFrame(); else tabAt = -1; };
  // Focus moving into a cross-origin frame (a click, Tab, or the page's own
  // window.focus()) fires no focus event on the iframe element in Chromium;
  // the shell window's blur, with the frame focused, marks it.
  const onBlur = () => { if (isFrame(doc.activeElement)) enterFrame(); };
  const win = doc.defaultView;
  const on: [string, EventListener][] = [
    ["pointerdown", onPress as EventListener], ["pointerup", onRelease as EventListener], ["mousedown", onMouseDown as EventListener],
    ["pointermove", onMove as EventListener], ["mousemove", onMove as EventListener],
    ["keydown", onKey as EventListener], ["keyup", onKeyUp as EventListener],
    ["mouseover", onOver as EventListener], ["mouseout", onOut as EventListener],
    ["wheel", onWheel as EventListener], ["focusin", onFocus as EventListener],
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

/** The strict tier (see the header): "ok", "no_gesture" when
 * `frameGesture()` fails, or "shell_input_recent" when it holds but the shell
 * had trusted input within `SHELL_QUIET_MS`. */
export function frameGestureStrict(doc: Document = document): "ok" | "no_gesture" | "shell_input_recent" {
  if (!frameGesture(doc)) return "no_gesture";
  return now() - lastShellInputAt < SHELL_QUIET_MS ? "shell_input_recent" : "ok";
}

/** Forgets what was recorded (tests). */
export function forgetGestures(): void {
  lastShellInput = -1;
  lastShellInputAt = -Infinity;
  frameEnteredAt = -1;
  entryByViewer = false;
  pointerOnFrameSince = -1;
  overFrame = false;
  tabAt = -1;
  lastPos = null;
  lastWasBoundary = false;
  lastPointerType = "";
  lastMoveAt = -1;
  inputPos = null;
  forwarded = () => false;
  shieldPressed = () => {};
  lowerShield();
}

if (typeof document !== "undefined") watchGestures(document);
