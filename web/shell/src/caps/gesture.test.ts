import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SHELL_INPUT_EVENTS, SHELL_QUIET_MS, forgetGestures, pickHintAllowed, frameGesture, frameGestureStrict, noteQuietBreak, noteShellInput, noteShieldPress, noteShellKey, notePointerAt, notePointerOver, onShieldPress, raiseShieldIfOverFrame, registerShield, setForwardedKeys, shielded, watchGestures } from "./gesture";

const active = (on: boolean) => Object.defineProperty(navigator, "userActivation", { value: { isActive: on }, configurable: true });
const nextTask = () => new Promise(r => setTimeout(r, 0));
const key = (k: string, altKey = false) => new KeyboardEvent("keydown", { key: k, altKey });

// The frame's box is (0, 0)–(500, 400); the shell button sits at (600, 20),
// outside it, and the composer's Cancel at (450, 380), over it.
const BUTTON = { x: 600, y: 20 };
const CANCEL = { x: 450, y: 380 };

describe("frame gestures", () => {
  let frame: HTMLIFrameElement;
  let button: HTMLButtonElement;
  let app: HTMLDivElement;
  let stop: () => void;
  /** The page's own `window.focus()`: focus enters the frame, as the shell
   * sees it in Chromium (the shell window's blur with the frame focused). */
  const pull = () => { frame.focus(); window.dispatchEvent(new Event("blur")); };
  /** The same, once the watcher has read where focus went (the next tick). */
  const pulled = async () => { pull(); await nextTask(); };
  /** The viewer clicks a shell control at `at` (the pointer is on it). */
  const clickShell = (at = BUTTON) => { notePointerOver(button, at.x, at.y); notePointerAt(at.x, at.y); button.focus(); noteShellInput(); };
  /** The pointer really moves onto the frame at (x, y), from the shell. */
  const arrive = (x = 250, y = 200) => { notePointerAt(x + 300, y); notePointerOver(frame, x, y); };
  beforeEach(() => {
    forgetGestures();
    stop = watchGestures(document);
    frame = document.createElement("iframe");
    frame.className = "frame";
    frame.getBoundingClientRect = () => new DOMRect(0, 0, 500, 400);
    button = document.createElement("button");
    // The shell renders into #app; anything else in the document is foreign.
    app = document.createElement("div");
    app.id = "app";
    app.append(frame, button);
    document.body.append(app);
    active(true);
  });
  afterEach(async () => {
    // The watcher reads a blur on the next tick: let it, in this test's state.
    await nextTask();
    stop();
    app.remove();
    registerShield(null);
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
  });

  it("is a click in the content frame: the pointer moved onto the frame after the shell input, then focus entered", () => {
    clickShell();
    arrive();
    frame.focus();
    expect(document.activeElement).toBe(frame);
    expect(frameGesture()).toBe(true);
  });

  it("is an area drag's press: the bridge's window.focus() with the pointer on the frame", () => {
    clickShell();
    arrive();
    pull();
    expect(frameGesture()).toBe(true);
  });

  it("is not the page pulling focus after a click on a shell control, the pointer still on it", () => {
    clickShell();
    pull();
    expect(frameGesture()).toBe(false);
    // Nor on the frame's own focus event (a same-process frame).
    button.focus();
    frame.focus();
    expect(frameGesture()).toBe(false);
  });

  it("does not count an arrival at the position of the latest shell input: a control over the frame vanished from under the pointer", () => {
    clickShell(CANCEL);
    // The composer closes; Chromium sends a mouseover on the iframe at the
    // unchanged position.
    notePointerOver(frame, CANCEL.x, CANCEL.y);
    pull();
    expect(frameGesture()).toBe(false);
    // Within 2 px is no move either.
    notePointerOver(button, CANCEL.x, CANCEL.y);
    notePointerOver(frame, CANCEL.x + 2, CANCEL.y - 2);
    expect(frameGesture()).toBe(false);
    // An arrival elsewhere is a real move.
    notePointerOver(button, CANCEL.x, CANCEL.y);
    notePointerOver(frame, CANCEL.x - 3, CANCEL.y);
    expect(frameGesture()).toBe(true);
  });

  it("takes the pointer's position for a key press too: Enter on the consent dialog's Allow", () => {
    // The pointer rests on the dialog's backdrop, over the frame.
    notePointerAt(300, 250);
    noteShellInput();
    notePointerOver(frame, 300, 250);
    pull();
    expect(frameGesture()).toBe(false);
  });

  it("counts an arrival after a key press with no known position only after a trusted move", () => {
    noteShellInput();
    notePointerOver(frame, 300, 250);
    pull();
    expect(frameGesture()).toBe(false);
    notePointerOver(button, 520, 250);
    notePointerAt(520, 250);
    notePointerOver(frame, 490, 250);
    expect(frameGesture()).toBe(true);
  });

  it("counts every arrival before any shell input", () => {
    notePointerOver(frame, 10, 10);
    pull();
    expect(frameGesture()).toBe(true);
  });

  it("is not the page pulling focus while the viewer types in the shell with the pointer resting on the frame", () => {
    clickShell();
    arrive();
    button.focus();
    noteShellInput(); // a key typed in the shell field
    pull();
    expect(frameGesture()).toBe(false);
    // Nor a click in the frame without the pointer moving; moving it off and
    // back on counts, as it would for any pointer arriving.
    button.focus();
    frame.focus();
    expect(frameGesture()).toBe(false);
    notePointerOver(button, 520, 200);
    notePointerAt(520, 200);
    notePointerOver(frame, 480, 200);
    expect(frameGesture()).toBe(true);
  });

  it("is the residual: a pointer moved onto the frame within the activation window of shell input, then a focus pull", () => {
    clickShell();
    pull();
    expect(frameGesture()).toBe(false);
    arrive();
    expect(frameGesture()).toBe(true);
  });

  it("keeps a click's entry for keys in the frame after the pointer has left it", () => {
    clickShell();
    arrive();
    frame.focus();
    notePointerOver(button, BUTTON.x, BUTTON.y);
    expect(frameGesture()).toBe(true);
    // The pointer left the window.
    notePointerOver(null);
    expect(frameGesture()).toBe(true);
  });

  it("is not a gesture once input reaches the shell after focus entered the frame", () => {
    clickShell();
    arrive();
    frame.focus();
    noteShellInput();
    expect(frameGesture()).toBe(false);
    // They click into the frame again: its gesture counts.
    clickShell();
    arrive();
    frame.focus();
    expect(frameGesture()).toBe(true);
  });

  it("is a Tab from the shell whose default action moves focus into the frame", async () => {
    clickShell();
    button.focus();
    noteShellInput(true);
    await pulled();
    expect(frameGesture()).toBe(true);
    // Shift+Tab into a cross-origin frame lands some tasks later.
    clickShell();
    noteShellInput(true);
    await nextTask();
    await pulled();
    expect(frameGesture()).toBe(true);
    // A Tab press that moved focus among shell controls, then the page pulls focus.
    clickShell();
    noteShellInput(true);
    const other = document.createElement("button");
    document.body.append(other);
    other.focus();
    other.remove();
    await pulled();
    expect(frameGesture()).toBe(false);
    // A Tab press whose wait has run out, then the pull.
    vi.useFakeTimers();
    clickShell();
    noteShellInput(true);
    vi.advanceTimersByTime(500);
    vi.useRealTimers();
    await pulled();
    expect(frameGesture()).toBe(false);
    // Another key after the Tab, then the pull.
    clickShell();
    noteShellInput(true);
    noteShellInput();
    await pulled();
    expect(frameGesture()).toBe(false);
  });

  it("is nothing without transient activation, or where the browser cannot tell", () => {
    clickShell();
    arrive();
    frame.focus();
    active(false);
    expect(frameGesture()).toBe(false);
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
    expect(frameGesture()).toBe(false);
  });

  it("is nothing while focus is outside the frame", () => {
    arrive();
    button.focus();
    expect(frameGesture()).toBe(false);
  });

  it("counts only trusted events", () => {
    clickShell();
    frame.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, clientX: 10, clientY: 10 }));
    document.dispatchEvent(new MouseEvent("mousemove", { clientX: 10, clientY: 10 }));
    pull();
    expect(frameGesture()).toBe(false);
    arrive();
    expect(frameGesture()).toBe(true);
    document.dispatchEvent(new Event("pointerdown"));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    document.dispatchEvent(new MouseEvent("mouseout", { relatedTarget: null }));
    expect(frameGesture()).toBe(true);
  });

  it("records every activating or interaction event type as shell input", () => {
    expect([...SHELL_INPUT_EVENTS].sort()).toEqual(["auxclick", "beforeinput", "click", "compositionend", "compositionstart", "compositionupdate", "contextmenu", "dblclick", "dragend", "dragstart", "drop", "input", "keydown", "mousedown", "pointercancel", "pointerdown", "pointerup", "textInput", "touchend", "wheel"]);
  });

  it("takes a blur of the shell window to anything but the content frame as shell input, read on the next tick (N14)", async () => {
    clickShell();
    arrive();
    pull();
    expect(frameGesture()).toBe(true);
    // A frame in the shell document (a password manager's menu) takes focus.
    const other = document.createElement("iframe");
    document.body.append(other);
    other.focus();
    window.dispatchEvent(new Event("blur"));
    await nextTask();
    frame.focus();
    expect(frameGesture()).toBe(false);
    other.remove();
    // Another window: focus leaves the document.
    clickShell();
    arrive();
    pull();
    expect(frameGesture()).toBe(true);
    button.focus();
    button.blur();
    window.dispatchEvent(new Event("blur"));
    await nextTask();
    frame.focus();
    expect(frameGesture()).toBe(false);
    // Where focus went is read after the blur: at blur time the content frame
    // may still be named when focus went into a frame in a shadow root.
    clickShell();
    arrive();
    pull();
    await nextTask();
    expect(frameGesture()).toBe(true);
    const host = document.createElement("x-menu");
    document.body.append(host);
    const inner = document.createElement("iframe");
    host.attachShadow({ mode: "closed" }).append(inner);
    frame.focus();
    window.dispatchEvent(new Event("blur"));
    inner.focus();
    await nextTask();
    expect(document.activeElement).toBe(host);
    frame.focus();
    expect(frameGesture()).toBe(false);
    host.remove();
  });

  for (const mode of ["none", "open", "closed"] as const) {
    it(`takes focus sitting in a foreign frame (${mode === "none" ? "plain" : `in a ${mode} shadow root`}) as shell input, however it got there (N14)`, async () => {
      clickShell();
      arrive();
      frame.focus();
      expect(frameGesture()).toBe(true);
      // Focus moves from the content frame to another frame: no event here.
      const other = document.createElement("iframe");
      const host = document.createElement(mode === "none" ? "div" : "x-menu");
      if (mode === "none") host.append(other); else host.attachShadow({ mode }).append(other);
      document.body.append(host);
      other.focus();
      await new Promise(r => setTimeout(r, 150));
      frame.focus();
      expect(frameGesture()).toBe(false);
      host.remove();
    });
  }

  it("takes nothing inside the shell's own root, body or html as foreign", async () => {
    clickShell();
    arrive();
    frame.focus();
    button.focus();
    await new Promise(r => setTimeout(r, 150));
    frame.focus();
    // Only the one click: the focus check saw nothing foreign.
    expect(frameGesture()).toBe(true);
  });

  it("keeps counting a pointer that arrived after shell input that left focus in the frame: a wheel over the shell", () => {
    clickShell();
    arrive();
    frame.focus();
    expect(frameGesture()).toBe(true);
    // The pointer leaves for the shell, a wheel there, and it comes back:
    // focus never left the frame, so no new entry is seen.
    notePointerOver(button, BUTTON.x, BUTTON.y);
    notePointerAt(BUTTON.x, BUTTON.y);
    noteShellInput();
    expect(frameGesture()).toBe(false);
    notePointerAt(520, 200);
    notePointerOver(frame, 480, 200);
    expect(frameGesture()).toBe(true);
  });

  it("allows a refused pick's hint only when it can be the viewer's own press: pointer over the frame's box, focus in the frame, no counted arrival since shell input (N10)", () => {
    // The pointer on a shell control: never.
    clickShell();
    pull();
    expect(pickHintAllowed()).toBe(false);
    // Resting on the frame where the shell input left it, focus in the frame:
    // a press there is refused, and the hint says why, each time.
    clickShell(CANCEL);
    notePointerOver(frame, CANCEL.x, CANCEL.y);
    pull();
    expect(pickHintAllowed()).toBe(true);
    expect(pickHintAllowed()).toBe(true);
    // Focus in the shell (the viewer typing there): never.
    button.focus();
    expect(pickHintAllowed()).toBe(false);
    // A counted arrival: the viewer's press would count, so the advice would be wrong.
    notePointerAt(CANCEL.x - 20, CANCEL.y);
    notePointerOver(frame, CANCEL.x - 20, CANCEL.y);
    pull();
    expect(pickHintAllowed()).toBe(false);
  });

  it("does not count the keys the shell forwards to the page, and counts them otherwise", () => {
    clickShell();
    arrive();
    pull();
    expect(frameGesture()).toBe(true);
    // Option in comment mode with the pointer over the frame: the page's key.
    const unset = setForwardedKeys(e => e.key === "Alt" || (e.altKey && e.key === "ArrowUp"));
    expect(noteShellKey(key("Alt"))).toBe(false);
    expect(noteShellKey(key("ArrowUp", true))).toBe(false);
    expect(frameGesture()).toBe(true);
    // Outside comment mode Alt is input to the shell (it grants activation):
    // a pull after it is refused.
    unset();
    expect(noteShellKey(key("Alt"))).toBe(true);
    pull();
    expect(frameGesture()).toBe(false);
    for (const k of ["Shift", "Control", "Meta", "Escape", "a"]) expect(noteShellKey(key(k)), k).toBe(true);
  });

  it("does not count a boundary event at the spot of the previous one: a layout change under a resting pointer (N1)", () => {
    // The pointer rests on the page; the page raises its consent dialog under
    // it, the viewer answers with a key, and the dialog goes.
    clickShell();
    arrive(300, 250);
    notePointerOver(button, 300, 250);
    noteShellInput();
    notePointerOver(frame, 300, 250);
    pull();
    expect(frameGesture()).toBe(false);
  });

  it("does not count a pin brought under a pointer that moved unseen inside the page, then taken away (N1, typing first)", () => {
    clickShell();
    arrive(300, 250);
    noteShellInput(); // typing in the shell; the pointer then moves inside the page, unseen
    notePointerOver(button, 320, 150); // a pin the page scrolled under it
    notePointerOver(frame, 320, 150); // and away
    pull();
    expect(frameGesture()).toBe(false);
  });

  it("counts an arrival at the spot of a move just before it (the shield lowered by that move)", () => {
    clickShell(CANCEL);
    notePointerAt(CANCEL.x - 20, CANCEL.y);
    notePointerOver(frame, CANCEL.x - 20, CANCEL.y);
    pull();
    expect(frameGesture()).toBe(true);
  });

  describe("the strict tier", () => {
    let t = 0;
    beforeEach(() => { t = 100_000; vi.spyOn(performance, "now").mockImplementation(() => t); });
    afterEach(() => { vi.restoreAllMocks(); });

    it("needs frameGesture and no trusted shell input for 5.5 s", () => {
      expect(SHELL_QUIET_MS).toBe(5_500);
      clickShell();
      arrive();
      pull();
      expect(frameGesture()).toBe(true);
      expect(frameGestureStrict()).toBe("shell_input_recent");
      t += 5_499;
      expect(frameGestureStrict()).toBe("shell_input_recent");
      t += 1;
      expect(frameGestureStrict()).toBe("ok");
      // A key the shell forwards, a release or a wheel restarts the wait.
      noteQuietBreak();
      expect(frameGestureStrict()).toBe("shell_input_recent");
      setForwardedKeys(e => e.key === "Alt");
      t += 6_000;
      expect(noteShellKey(key("Alt"))).toBe(false);
      expect(frameGesture()).toBe(true);
      expect(frameGestureStrict()).toBe("shell_input_recent");
      t += 6_000;
      expect(frameGestureStrict()).toBe("ok");
    });

    it("waits out input before the shell's script only when the shell is already active as it starts (N13)", () => {
      // Active at the start: input came before the script, unseen.
      forgetGestures();
      stop();
      stop = watchGestures(document);
      arrive();
      pull();
      expect(frameGesture()).toBe(true);
      expect(frameGestureStrict()).toBe("shell_input_recent");
      t += 5_500;
      expect(frameGestureStrict()).toBe("ok");
      // Inactive at the start: no earlier input can make it active later, so
      // there is nothing to wait for.
      forgetGestures();
      stop();
      active(false);
      stop = watchGestures(document);
      active(true);
      arrive();
      pull();
      expect(frameGestureStrict()).toBe("ok");
    });

    it("is no_gesture without frameGesture", () => {
      t += 10_000;
      expect(frameGestureStrict()).toBe("no_gesture");
    });
  });

  describe("the shield", () => {
    let stage: HTMLDivElement;
    let el: HTMLDivElement;
    let bands: HTMLDivElement[];
    beforeEach(() => {
      stage = document.createElement("div");
      el = document.createElement("div");
      bands = [0, 1, 2, 3].map(() => document.createElement("div"));
      el.append(...bands);
      stage.append(el);
      document.body.append(stage);
      registerShield(el);
    });
    afterEach(() => stage.remove());

    it("rises at shell input over the frame's box with a 9 px hole under the pointer, and falls on the next move", () => {
      clickShell(CANCEL);
      raiseShieldIfOverFrame();
      expect(shielded()).toBe(true);
      expect(el.style.display).toBe("block");
      const [top, bottom, left, right] = bands;
      expect(top.style).toMatchObject({ top: "0px", height: `${CANCEL.y - 4}px` });
      expect(bottom.style.top).toBe(`${CANCEL.y + 5}px`);
      expect(left.style).toMatchObject({ top: `${CANCEL.y - 4}px`, width: `${CANCEL.x - 4}px`, height: "9px" });
      expect(right.style.left).toBe(`${CANCEL.x + 5}px`);
      // The composer closes: the pointer, in the hole, is over the frame.
      notePointerOver(frame, CANCEL.x, CANCEL.y);
      pull();
      expect(frameGesture()).toBe(false);
      // A move over a shell control leaves the bands up.
      notePointerAt(CANCEL.x, CANCEL.y);
      expect(shielded()).toBe(true);
      // A band move without real movement (a layout re-hit-test) leaves
      // them up too, and counts nothing.
      notePointerAt(CANCEL.x + 6, CANCEL.y, "mouse", true, false);
      expect(shielded()).toBe(true);
      expect(frameGesture()).toBe(false);
      // The first real move out of the hole lands on a band: it counts as the
      // arrival and lowers them, however small (N12).
      notePointerAt(CANCEL.x + 5, CANCEL.y, "mouse", true, true);
      expect(shielded()).toBe(false);
      expect(el.style.display).toBe("");
      expect(frameGesture()).toBe(true);
      // The frame's own mouseover after it keeps the arrival, even within 2 px.
      notePointerOver(frame, CANCEL.x + 1, CANCEL.y);
      expect(frameGesture()).toBe(true);
    });

    it("counts a 1 px move on a band after a press on a shell control as the arrival (N12)", () => {
      clickShell(CANCEL);
      raiseShieldIfOverFrame(document, true);
      // Full cover: the pointer is on a band.
      notePointerOver(bands[0], CANCEL.x, CANCEL.y);
      pull();
      expect(frameGesture()).toBe(false);
      notePointerAt(CANCEL.x - 1, CANCEL.y, "mouse", true, true);
      expect(shielded()).toBe(false);
      notePointerOver(frame, CANCEL.x - 2, CANCEL.y);
      expect(frameGesture()).toBe(true);
    });

    it("rises for keys typed with the pointer resting on the page, under the pointer", () => {
      clickShell();
      arrive(300, 250);
      noteShellInput();
      raiseShieldIfOverFrame();
      expect(shielded()).toBe(true);
      expect(bands[2].style.width).toBe("296px");
    });

    it("takes a press on a band as shell input that never counts as the pointer's arrival, keeps the bands up for a mouse, and tells a mouse viewer", () => {
      const hint = vi.fn();
      const off = onShieldPress(hint);
      clickShell();
      arrive(300, 250);
      pull();
      expect(frameGesture()).toBe(true);
      raiseShieldIfOverFrame();
      noteShieldPress(340.4, 250.2);
      expect(hint).toHaveBeenCalledTimes(1);
      // The bands stay until the pointer moves; no arrival counts.
      expect(shielded()).toBe(true);
      expect(frameGesture()).toBe(false);
      notePointerOver(frame, 340, 250);
      pull();
      expect(frameGesture()).toBe(false);
      // A real move lands on a band, lowers them, and counts.
      notePointerAt(341, 250, "mouse", true, true);
      expect(shielded()).toBe(false);
      expect(frameGesture()).toBe(true);
      // A touch press on a band is the finger on the page: it lowers them and
      // counts as the arrival, with no hint (the tap's click reaches the page).
      noteShellInput();
      raiseShieldIfOverFrame();
      expect(frameGesture()).toBe(false);
      noteShieldPress(300, 250, "touch");
      expect(shielded()).toBe(false);
      expect(hint).toHaveBeenCalledTimes(1);
      pull();
      expect(frameGesture()).toBe(true);
      off();
    });

    it("keeps the hole closed for a double-click's interval after a press on a shell control, then opens it", () => {
      vi.useFakeTimers();
      clickShell(CANCEL);
      raiseShieldIfOverFrame(document, true);
      expect(shielded()).toBe(true);
      expect(bands[0].style).toMatchObject({ top: "0px", height: "100%" });
      expect(bands[1].style.height).toBe("0px");
      vi.advanceTimersByTime(500);
      expect(bands[0].style.height).toBe(`${CANCEL.y - 4}px`);
      expect(bands[2].style.width).toBe(`${CANCEL.x - 4}px`);
      vi.useRealTimers();
    });

    it("leaves the bands as they are at a key press while the pointer is on a band (keys typed at a person's speed)", () => {
      clickShell();
      arrive(300, 250);
      noteShellInput();
      raiseShieldIfOverFrame();
      const width = bands[2].style.width;
      // The band appears under the pointer, which moved unseen inside the page.
      notePointerOver(bands[1], 340, 320);
      noteShellInput();
      raiseShieldIfOverFrame();
      expect(shielded()).toBe(true);
      expect(bands[2].style.width).toBe(width);
    });

    it("does not rise for input outside the frame's box, or for touch", () => {
      clickShell();
      raiseShieldIfOverFrame();
      expect(shielded()).toBe(false);
      notePointerAt(CANCEL.x, CANCEL.y, "touch");
      noteShellInput();
      raiseShieldIfOverFrame();
      expect(shielded()).toBe(false);
    });
  });
});
