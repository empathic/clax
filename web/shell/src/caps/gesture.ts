// Whether the viewer's latest input went to the content frame, for capability
// calls that may act only on the viewer's own recent gesture in the page. Two
// tiers:
//
// - `frameGesture()`, the composer tier, for calls whose worst case is a
//   composer the viewer sees (`openComposer`, `compose`, a pick's start): the
//   page can prefill what it shows, and nothing is posted without the viewer.
// - `frameGestureStrict()`, the strict tier, for calls that act as the viewer
//   beyond that composer (the comments write verbs `create`, `reply`,
//   `resolve`, `delete` and `sendToClaude`; `artifact.publish`):
//   `frameGesture()` and, in addition, no trusted input to the shell for
//   `SHELL_QUIET_MS`. That added rule uses no pointer position; the
//   `frameGesture()` it includes does.
//
// Nothing the page or the bridge reports counts (they share a realm); every
// rule below reads only the shell's own trusted events.
//
// Shell input is recorded allow-all: every trusted event of the types in
// `SHELL_INPUT_EVENTS` (each type through which Chromium or the HTML spec
// lets input grant a document activation, or that marks the viewer's
// interaction with it: presses, releases, clicks, keys, a drop, a drag's
// start and end, a cancelled pointer, a wheel, and the text events an input
// method, the emoji picker or dictation dispatch with no key press), captured
// on the shell window. Also shell input: the shell window losing focus to
// anything but the content frame (read on the next tick: at blur time
// `activeElement` can still name the content frame); focus sitting on
// anything foreign, that is outside the element the shell renders into
// (`#app`) and not the content frame, `body` or `html`, such as an
// extension's frame, directly or inside an open or closed shadow root whose
// host then holds focus (checked every `FOCUS_POLL_MS`, since focus moving
// there from the content frame fires nothing here); and the pointer leaving
// something foreign while the shell has activation. The keys
// the shell forwards to the page (Option, Option+Up/Down and Escape in
// comment mode with the pointer over the frame, `setForwardedKeys`) are the
// one exception: they are not shell input for `frameGesture()`, though the
// strict tier still waits them out.
//
// `frameGesture()` holds when all of these hold:
//
// 1. the shell window has transient user activation;
// 2. focus is in the content frame (`iframe.frame`);
// 3. the viewer, not the page, can have moved focus there: either
//    - the pointer's arrival on the frame counted after the viewer's latest
//      input to the shell, and the pointer is on the frame now;
//    - or focus entered after that input, with no shell input since, and at
//      entry either the pointer's arrival had so counted (a click or tap in
//      the frame, or the bridge's `window.focus()` on an area drag's press)
//      or a Tab or Shift+Tab key press in the shell that was the latest
//      shell input moved it there, before focus reached any shell element
//      and within `TAB_MS` (keyboard entry).
//
// A counted arrival. The shell sees the pointer arrive on the frame as a
// `mouseover` on the iframe element (also sent before a touch tap's press).
// Chromium also sends that `mouseover`, with unchanged coordinates, whenever
// the layout changes under a resting pointer: a shell control over the frame
// vanishing (the composer on Cancel or Post, the consent dialog, a banner's
// Dismiss), or an element the page moves (a pin) coming and going. So the
// shell records the pointer's position from every trusted pointer event it
// gets (moves, presses, and the boundary events `mouseover` and a `mouseout`
// leaving the window), and an arrival counts when one of these holds:
// - a `mouseover` of the frame whose coordinates differ from those of the
//   previous boundary event, when that was the latest pointer event (a
//   boundary event at the same spot is a layout change, never a move), and
//   lie more than 2 px from where the pointer was at the viewer's latest
//   shell input (for key presses too). After a key press with no known
//   position, such an arrival counts only after a trusted move since that
//   key. Before any shell input every such arrival counts;
// - a trusted move over one of the shield's bands (below) with real
//   movement, however small: non-zero `movementX`/`movementY`, or screen
//   coordinates changed since the previous pointer event. Chromium's
//   re-hit-tests after a layout change send boundary events, and no move
//   with movement;
// - a wheel over a band;
// - a touch press on a band.
//
// The shield. Moves inside a cross-origin frame never reach the shell, so a
// pointer resting over the frame after shell input (a click on Cancel, Post,
// Allow, a pin or Dismiss over the page, keys or an input method's text with
// the pointer on the page, or the window losing focus) would have to leave
// the frame and come back before a click in the page counted. So at such
// input the shell covers the frame with transparent bands
// (`registerShield`), beneath every shell control, that leave a 9 px hole
// where the pointer was last seen. After a press on a shell control the hole
// stays closed for `DOUBLE_CLICK_MS`, so the second click of a double-click
// on that control within that time lands on a band, not in the page; a
// slower double-click, which a platform may allow, reaches the page. A
// press, drag or wheel in the hole reaches the page. The bands stay up until
// the viewer's own input over them: a move with real movement, a wheel or a
// touch press, each of which counts as the pointer's arrival and lowers them
// (so the page loses at most that one move or wheel event; a touch tap's
// click, hit-tested after the bands come down, reaches the page). A mouse
// press on a band (the second click of a double-click, a re-press before
// the hole opens, or a press after a move the shell could not see) reaches
// neither the page nor any shell control: it is shell input, it never
// counts as the pointer's arrival, the bands stay up, and it shows the hint
// (`onShieldPress`). Touch input raises no bands. A key press while the
// pointer is on a band leaves the bands as they are. A touch tap inside the
// hole reaches the page without counting as an arrival.
//
// A pick start the shell refuses shows the hint only when it can be the
// viewer's own press (`pickHintAllowed`: the pointer over the frame's box,
// focus in the frame, no counted arrival since the latest shell input), so
// a page posting starts can show it only while a press of the viewer's there
// would be refused the same way.
//
// Residual of `frameGesture()` (what a hostile page can still do): within the
// activation window (about five seconds) after the viewer's latest input to
// the shell, once the viewer moves the pointer onto or over the frame, turns
// the wheel or touches it there, or Tabs into it, the page can pull focus
// into itself and have a composer-tier call counted without a click or key
// in the page: it can open the composer, prefilled, or forge a pick. The
// viewer sees that composer, and nothing is posted without them.
//
// `frameGestureStrict()` adds: no shell input of any kind, forwarded keys
// included, for the last `SHELL_QUIET_MS`. The shell's script starting counts
// as such input only when the shell is already active as it starts: input
// before the script was not seen, and its activation, if any is left, lasts
// at most 5 s more. When the shell is inactive at the start, no earlier
// input can make it active later, so nothing is waited for. Chromium keeps a
// transient activation 5 s, and every input to the shell that Chromium lets
// grant it activation is one of `SHELL_INPUT_EVENTS` (an assistive
// technology's press dispatches a `pointerdown` too), input to a foreign
// frame (seen as focus sitting there, the window's blur toward it, or the
// pointer leaving it), or input before the script ran. So once the quiet time has
// passed, an active `navigator.userActivation` comes from input to the
// frame, unless Chromium grants the shell activation through a source none
// of these see; Clax knows of none beyond script the viewer runs on the
// tab themselves (a bookmarklet). Its residual: a page can act within the
// activation window after the viewer's own click or key in the page,
// whatever that input was meant for. The cost: a viewer who clicks a page's
// control within that time after using the shell is refused
// (`shell_input_recent`) and must click again.

/** Chromium's transient user activation lasts 5 s from the input that
 * granted it; half a second more so a shell input's activation has surely
 * expired before a strict call may rely on `isActive`. */
export const SHELL_QUIET_MS = 5_500;
/** How long the gesture hint stays on screen. */
export const HINT_MS = 2_500;
/** How long a Tab press may take to move focus into the frame; a Tab that
 * moves focus to a shell element ends the wait at once. */
const TAB_MS = 500;
/** Farther than this from the input position, in CSS pixels, is a real move. */
const MOVE_PX = 2;
/** The shield's hole reaches this far from the pointer, in CSS pixels. */
const HOLE_PX = 4;
/** After a press on a shell control over the frame, the hole stays closed
 * this long (a double-click's interval), so the second click of a
 * double-click on that control within it never reaches the page. */
const DOUBLE_CLICK_MS = 500;
/** How often the shell checks whether focus sits on something foreign. */
const FOCUS_POLL_MS = 100;

/** Every event type through which Chromium or the HTML spec lets the viewer's
 * input to a document grant it user activation, or that marks their
 * interaction with it (a drag's start and end, a cancelled pointer, text an
 * input method composes or commits): each trusted one reaching the shell
 * window is shell input, for both tiers, except a key the shell forwards to
 * the page (`setForwardedKeys`). The rule is allow-all: an input type left
 * out of this list is the exception that needs a reason, not the default. */
export const SHELL_INPUT_EVENTS = [
  "keydown", "mousedown", "pointerdown", "pointerup", "touchend", "click", "auxclick", "dblclick",
  "contextmenu", "drop", "dragstart", "dragend", "pointercancel", "wheel",
  "beforeinput", "input", "compositionstart", "compositionupdate", "compositionend", "textInput",
] as const;
/** The input types that carry text (an input method's composition or commit,
 * the emoji picker, dictation, or typing): the shell treats them like a key. */
const TEXT_EVENTS: ReadonlySet<string> = new Set(["beforeinput", "input", "compositionstart", "compositionupdate", "compositionend", "textInput"]);

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
/** The same, in screen coordinates, when the event gave them. */
let lastScreen: Pos | null = null;
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
/** Bumped each time the shield is raised, so a pending hole opening knows
 * whether it still applies. */
let raised = 0;
/** Whether the latest `mouseover` targeted one of the shield's bands. */
let overShield = false;
/** Whether the latest `mouseover` targeted something foreign (`isForeign`). */
let overForeign = false;
/** A touch press on a band whose release and click are still to come: they
 * are that tap's, not new input. */
let touchOnBand = false;
let forwarded: (e: KeyboardEvent) => boolean = () => false;
let shieldPressed: (pointerType: string) => void = () => {};

const isFrame = (el: EventTarget | null) => el instanceof HTMLIFrameElement && el.classList.contains("frame");
/** Whether `el` is someone else's: outside the element the shell renders
 * into (`#app`), and not the content frame, `body` or `html`. An extension's
 * inline menu is, often a frame inside a shadow root, open or closed, whose
 * events and focus are retargeted to the shadow host. Input there activates
 * the shell and reaches none of its listeners. */
const isForeign = (el: EventTarget | null) => {
  if (!(el instanceof Element) || isFrame(el)) return false;
  const doc = el.ownerDocument;
  if (el === doc.body || el === doc.documentElement) return false;
  const root = doc.getElementById("app");
  return !!root && !root.contains(el);
};
const activeNow = () => (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation?.isActive === true;
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
 * shell forwards to the page, the shell's script starting): only
 * `frameGestureStrict` waits for it. */
export function noteQuietBreak(): void {
  lastShellInputAt = now();
}

/** Counts the pointer as having arrived on the frame now, by the viewer's own
 * input over it (a real move, a wheel or a touch press on a band), and
 * lowers the shield. */
function arriveOnFrame(): void {
  overFrame = true;
  overShield = false;
  if (!pointerFresh()) pointerOnFrameSince = ++seq;
  lowerShield();
}

/** Records a trusted pointer move or press at (x, y) in the shell (the watcher
 * does this; tests call it). `band`: the move is over one of the shield's
 * bands; `moved`: it shows real movement (non-zero `movementX`/`movementY`,
 * or screen coordinates changed since the previous pointer event). A band
 * move with real movement is the pointer's arrival on the frame, and lowers
 * the shield; one without leaves the shield up. */
export function notePointerAt(x: number, y: number, pointerType = "mouse", band = false, moved = true): void {
  lastPos = { x, y };
  lastWasBoundary = false;
  lastPointerType = pointerType;
  lastMoveAt = ++seq;
  if (band && holeAt && moved) arriveOnFrame();
}

/** Records the pointer moving onto `target` at (x, y) (the watcher does this
 * for trusted `mouseover`s, and for a `mouseout` leaving the window with a
 * null target; tests call it). An arrival on the frame counts only after a
 * real move (see the header). */
export function notePointerOver(target: EventTarget | null, x = NaN, y = NaN): void {
  const known = Number.isFinite(x) && Number.isFinite(y);
  // Within a pixel: a press's pointer coordinates are fractional, a
  // boundary event's whole.
  const layout = known && lastWasBoundary && !!lastPos && Math.abs(lastPos.x - x) < 1 && Math.abs(lastPos.y - y) < 1;
  if (known) { lastPos = { x, y }; lastWasBoundary = true; }
  overFrame = isFrame(target);
  overShield = isShield(target);
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

/** Sets what the shell does when a mouse press lands on the shield (its
 * hint); returns the reset. */
export function onShieldPress(f: (pointerType: string) => void): () => void {
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
 * over the frame's box at shell input (the watcher calls it at that input).
 * `press`: the input was a press on a shell control, so the hole stays
 * closed for `DOUBLE_CLICK_MS`. A key press while the pointer is on a band
 * leaves the shield as it is: the band's own `mouseover` does not move the
 * hole under a pointer the shell never saw move. */
export function raiseShieldIfOverFrame(doc: Document = document, press = false): void {
  if (!press && holeAt && overShield) return;
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
  const token = ++raised;
  layBands(r, press ? null : holeAt);
  if (press) setTimeout(() => { if (raised === token && holeAt) layBands(frame.getBoundingClientRect(), holeAt); }, DOUBLE_CLICK_MS);
}

/** Lays the bands over the frame's box `r`, leaving a hole at `hole` (none:
 * one band covers the whole frame). */
function layBands(r: DOMRect, hole: Pos | null): void {
  if (!shield) return;
  const [top, bottom, left, right] = Array.from(shield.children) as HTMLElement[];
  const band = (el: HTMLElement | undefined, l: string, t: string, w: string, h: string) => { if (el) Object.assign(el.style, { left: l, top: t, width: w, height: h }); };
  if (!hole) {
    band(top, "0", "0", "100%", "100%");
    for (const el of [bottom, left, right]) band(el, "0", "0", "0", "0");
  } else {
    // The shield covers the frame's box exactly (both fill the stage).
    const hx = hole.x - r.left;
    const hy = hole.y - r.top;
    band(top, "0", "0", "100%", `${Math.max(0, hy - HOLE_PX)}px`);
    band(bottom, "0", `${hy + HOLE_PX + 1}px`, "100%", `calc(100% - ${hy + HOLE_PX + 1}px)`);
    band(left, "0", `${hy - HOLE_PX}px`, `${Math.max(0, hx - HOLE_PX)}px`, `${2 * HOLE_PX + 1}px`);
    band(right, `${hx + HOLE_PX + 1}px`, `${hy - HOLE_PX}px`, `calc(100% - ${hx + HOLE_PX + 1}px)`, `${2 * HOLE_PX + 1}px`);
  }
  shield.style.display = "block";
}

/** Whether a pick start the shell refused may show the hint: only when it
 * can be the viewer's own press, and the hint is true advice for it. The
 * pointer is over the frame's box (on the frame or a band, not on a shell
 * control), focus is in the frame (a press in the page puts it there), and
 * the pointer has made no counted arrival since the viewer's latest shell
 * input (so a press there is refused for want of a move). */
export function pickHintAllowed(doc: Document = document): boolean {
  return (overFrame || overShield) && !pointerFresh() && isFrame(doc.activeElement);
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

/** Records a trusted press on one of the shield's bands at (x, y) (the
 * watcher does this; tests call it). The press reaches no shell control, and
 * is shell input like any other (both tiers).
 * - A mouse press never counts as the pointer arriving on the page: it is
 *   recorded as a boundary position (so the frame's `mouseover` at that spot
 *   does not count), the bands stay up, so the viewer's next move lands on a
 *   band and counts, and the viewer is told why (`onShieldPress`).
 * - A touch press is the finger on the page: it counts as the pointer's
 *   arrival and lowers the bands, so the tap's click, hit-tested after it,
 *   reaches the page and counts. */
export function noteShieldPress(x: number, y: number, pointerType = "mouse"): void {
  lastPos = { x, y };
  lastWasBoundary = true;
  lastPointerType = pointerType;
  noteShellInput();
  touchOnBand = pointerType === "touch";
  if (touchOnBand) { arriveOnFrame(); return; }
  shieldPressed(pointerType);
}

function enterFrame(): void {
  frameEnteredAt = ++seq;
  entryByViewer = pointerFresh() || (tabAt >= 0 && tabAt === lastShellInput);
}

/** Watches `doc` (the shell document) for trusted input, for the pointer
 * moving onto and off the content frame, and for focus moving into the frame;
 * returns the unwatcher. Input is captured on the shell window, before any
 * shell handler can stop it. The watch starting (the shell's script running)
 * counts toward the strict tier's quiet time: input before it was not seen. */
export function watchGestures(doc: Document = document): () => void {
  const win = doc.defaultView;
  if (!win) return () => {};
  // Input before the script ran was not seen: if the shell is already active
  // now, that input's activation lasts at most 5 s more, so the strict tier
  // waits it out. If not, no earlier input can make it active later.
  if (activeNow()) noteQuietBreak();
  const onInput = (e: Event) => {
    if (!e.isTrusted) return;
    if (e.type === "keydown") {
      const k = e as KeyboardEvent;
      if (!noteShellKey(k)) return;
      raiseShieldIfOverFrame(doc);
      // A Tab from outside the document (the browser's own controls) can land
      // in the frame with no blur here; the entry shows as focus in the frame.
      const at = lastShellInput;
      if (k.key === "Tab") setTimeout(() => { if (tabAt === at && frameEnteredAt < at && isFrame(doc.activeElement)) enterFrame(); }, 0);
      return;
    }
    if (TEXT_EVENTS.has(e.type)) {
      // Text with no key press of its own (an input method, the emoji
      // picker, dictation) is handled like a key; after a key or a press the
      // shield is already where that input left it.
      noteShellInput();
      if (!holeAt) raiseShieldIfOverFrame(doc);
      return;
    }
    if (isShield(e.target)) {
      // Nor does a press on a band move focus.
      if (e.type === "mousedown") e.preventDefault();
      if (e.type === "pointerdown") { const p = e as PointerEvent; noteShieldPress(p.clientX, p.clientY, p.pointerType); return; }
      // A touch tap's own release on the band it lowered.
      if (touchOnBand) { noteQuietBreak(); return; }
      noteShellInput();
      // A wheel on a band is the viewer's input over the page: it counts as
      // the pointer's arrival, and the wheel's next events reach the page.
      if (e.type === "wheel") arriveOnFrame();
      return;
    }
    if (e.type === "pointerdown") {
      touchOnBand = false;
      // The shield sits beneath every shell control, so raising it at the
      // press leaves the press's click to its control.
      const p = e as PointerEvent;
      notePointerAt(p.clientX, p.clientY, p.pointerType);
      noteScreen(p);
      noteShellInput();
      raiseShieldIfOverFrame(doc, true);
      return;
    }
    noteShellInput();
  };
  const noteScreen = (e: MouseEvent) => { if (Number.isFinite(e.screenX) && Number.isFinite(e.screenY)) lastScreen = { x: e.screenX, y: e.screenY }; };
  const onMove = (e: MouseEvent) => {
    if (!e.isTrusted) return;
    // Real movement: Chromium's layout re-hit-tests send boundary events, and
    // any move they send has no movement and the same screen position.
    const moved = e.movementX !== 0 || e.movementY !== 0 || (!!lastScreen && (e.screenX !== lastScreen.x || e.screenY !== lastScreen.y));
    noteScreen(e);
    notePointerAt(e.clientX, e.clientY, e instanceof PointerEvent ? e.pointerType : lastPointerType || "mouse", isShield(e.target), moved);
  };
  // The pointer leaving something foreign (`isForeign`) while the shell is
  // active: the viewer may have clicked or typed in it, which the shell
  // cannot see.
  const leaveForeign = (to: EventTarget | null) => {
    if (overForeign && activeNow()) noteShellInput();
    overForeign = isForeign(to);
  };
  const onOver = (e: MouseEvent) => { if (!e.isTrusted) return; noteScreen(e); leaveForeign(e.target); notePointerOver(e.target, e.clientX, e.clientY); };
  const onOut = (e: MouseEvent) => { if (!e.isTrusted || e.relatedTarget !== null) return; noteScreen(e); leaveForeign(null); notePointerOver(null, e.clientX, e.clientY); };
  // Focus on something foreign is shell input for as long as it stays there:
  // focus moving there from the content frame fires nothing in the shell.
  const focusPoll = win.setInterval(() => { if (isForeign(doc.activeElement)) noteShellInput(); }, FOCUS_POLL_MS);
  const onFocus = (e: FocusEvent) => { if (isFrame(e.target)) enterFrame(); else tabAt = -1; };
  // Focus moving into a cross-origin frame (a click, Tab, or the page's own
  // window.focus()) fires no focus event on the iframe element in Chromium;
  // the shell window's blur, with the frame focused, marks it. A blur to
  // anything else (a foreign frame, which activates the shell when clicked,
  // or another window) is shell input.
  const onBlur = () => {
    // Read where focus went on the next tick: at blur time `activeElement`
    // can still name the content frame when focus went into a frame inside
    // a shadow root.
    setTimeout(() => {
      if (isFrame(doc.activeElement)) { enterFrame(); return; }
      noteShellInput();
      raiseShieldIfOverFrame(doc);
    }, 0);
  };
  const on: [string, EventListener][] = [
    ...SHELL_INPUT_EVENTS.map(t => [t, onInput] as [string, EventListener]),
    ["pointermove", onMove as EventListener], ["mousemove", onMove as EventListener],
    ["mouseover", onOver as EventListener], ["mouseout", onOut as EventListener],
    ["focusin", onFocus as EventListener],
  ];
  for (const [t, f] of on) win.addEventListener(t, f, true);
  win.addEventListener("blur", onBlur);
  return () => {
    win.clearInterval(focusPoll);
    for (const [t, f] of on) win.removeEventListener(t, f, true);
    win.removeEventListener("blur", onBlur);
  };
}

/** True when the viewer's latest input went to the content frame (see the
 * conditions above). */
export function frameGesture(doc: Document = document): boolean {
  const ua = (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation;
  if (ua?.isActive !== true) return false;
  if (!isFrame(doc.activeElement)) return false;
  if (pointerFresh()) return true;
  return lastShellInput < frameEnteredAt && entryByViewer;
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
  lastScreen = null;
  lastWasBoundary = false;
  lastPointerType = "";
  lastMoveAt = -1;
  inputPos = null;
  forwarded = () => false;
  shieldPressed = () => {};
  overShield = false;
  overForeign = false;
  touchOnBand = false;
  lowerShield();
}

/** Stops the watch this module starts on the shell document as it loads.
 * Tests call it: each fresh import of the module starts its own watch. */
export const unwatchShell: () => void = typeof document !== "undefined" ? watchGestures(document) : () => {};
