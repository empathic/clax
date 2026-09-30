import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SHELL_QUIET_MS, forgetGestures, frameGesture, frameGestureStrict, noteQuietBreak, noteShellInput, noteShieldPress, noteShellKey, notePointerAt, notePointerOver, onShieldPress, raiseShieldIfOverFrame, registerShield, setForwardedKeys, shielded, watchGestures } from "./gesture";

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
  let stop: () => void;
  /** The page's own `window.focus()`: focus enters the frame, as the shell
   * sees it in Chromium (the shell window's blur with the frame focused). */
  const pull = () => { frame.focus(); window.dispatchEvent(new Event("blur")); };
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
    document.body.append(frame, button);
    active(true);
  });
  afterEach(() => {
    stop();
    frame.remove();
    button.remove();
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
    pull();
    expect(frameGesture()).toBe(true);
    // Shift+Tab into a cross-origin frame lands some tasks later.
    clickShell();
    noteShellInput(true);
    await nextTask();
    pull();
    expect(frameGesture()).toBe(true);
    // A Tab press that moved focus among shell controls, then the page pulls focus.
    clickShell();
    noteShellInput(true);
    const other = document.createElement("button");
    document.body.append(other);
    other.focus();
    other.remove();
    pull();
    expect(frameGesture()).toBe(false);
    // A Tab press whose wait has run out, then the pull.
    vi.useFakeTimers();
    clickShell();
    noteShellInput(true);
    vi.advanceTimersByTime(500);
    vi.useRealTimers();
    pull();
    expect(frameGesture()).toBe(false);
    // Another key after the Tab, then the pull.
    clickShell();
    noteShellInput(true);
    noteShellInput();
    pull();
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
      // The first move out of the hole lands on a band and lowers it; the
      // arrival that follows counts.
      notePointerAt(CANCEL.x + 6, CANCEL.y);
      expect(shielded()).toBe(false);
      expect(el.style.display).toBe("");
      notePointerOver(frame, CANCEL.x + 6, CANCEL.y);
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

    it("does nothing at a press on a band: not shell input, the shield falls, and the viewer is told", () => {
      const hint = vi.fn();
      const off = onShieldPress(hint);
      clickShell();
      arrive(300, 250);
      pull();
      expect(frameGesture()).toBe(true);
      raiseShieldIfOverFrame();
      noteShieldPress(340, 250);
      expect(hint).toHaveBeenCalledTimes(1);
      expect(shielded()).toBe(false);
      expect(frameGesture()).toBe(true);
      expect(frameGestureStrict()).toBe("shell_input_recent");
      off();
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
