import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { forgetGestures, frameGesture, noteShellInput, notePointerOver, shellKey, watchGestures } from "./gesture";

const active = (on: boolean) => Object.defineProperty(navigator, "userActivation", { value: { isActive: on }, configurable: true });
const nextTask = () => new Promise(r => setTimeout(r, 0));

describe("frame gestures", () => {
  let frame: HTMLIFrameElement;
  let button: HTMLButtonElement;
  let stop: () => void;
  /** The page's own `window.focus()`: focus enters the frame, as the shell
   * sees it in Chromium (the shell window's blur with the frame focused). */
  const pull = () => { frame.focus(); window.dispatchEvent(new Event("blur")); };
  /** The viewer clicks a shell control (the pointer is on it). */
  const clickShell = () => { notePointerOver(button); button.focus(); noteShellInput(); };
  beforeEach(() => {
    forgetGestures();
    stop = watchGestures(document);
    frame = document.createElement("iframe");
    frame.className = "frame";
    button = document.createElement("button");
    document.body.append(frame, button);
    active(true);
  });
  afterEach(() => {
    stop();
    frame.remove();
    button.remove();
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
  });

  it("is a click in the content frame: the pointer moved onto the frame after the shell input, then focus entered", () => {
    clickShell();
    notePointerOver(frame);
    frame.focus();
    expect(document.activeElement).toBe(frame);
    expect(frameGesture()).toBe(true);
  });

  it("is an area drag's press: the bridge's window.focus() with the pointer on the frame", () => {
    clickShell();
    notePointerOver(frame);
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

  it("is not the page pulling focus while the viewer types in the shell with the pointer resting on the frame", () => {
    clickShell();
    notePointerOver(frame);
    button.focus();
    noteShellInput(); // a key typed in the shell field
    pull();
    expect(frameGesture()).toBe(false);
    // Nor a click in the frame without the pointer leaving it first; moving
    // it off and back on counts, as it would for any pointer arriving.
    button.focus();
    notePointerOver(frame);
    frame.focus();
    expect(frameGesture()).toBe(false);
    notePointerOver(button);
    notePointerOver(frame);
    expect(frameGesture()).toBe(true);
  });

  it("is the residual: a pointer moved onto the frame within the activation window of shell input, then a focus pull", () => {
    clickShell();
    pull();
    expect(frameGesture()).toBe(false);
    notePointerOver(frame);
    expect(frameGesture()).toBe(true);
  });

  it("keeps a click's entry for keys in the frame after the pointer has left it", () => {
    clickShell();
    notePointerOver(frame);
    frame.focus();
    notePointerOver(button);
    expect(frameGesture()).toBe(true);
    // The pointer left the window.
    notePointerOver(null);
    expect(frameGesture()).toBe(true);
  });

  it("is not a gesture once input reaches the shell after focus entered the frame", () => {
    clickShell();
    notePointerOver(frame);
    frame.focus();
    noteShellInput();
    expect(frameGesture()).toBe(false);
    // They click into the frame again: its gesture counts.
    clickShell();
    notePointerOver(frame);
    frame.focus();
    expect(frameGesture()).toBe(true);
  });

  it("is a Tab from the shell whose default action moves focus into the frame", async () => {
    clickShell();
    button.focus();
    noteShellInput(true);
    pull();
    expect(frameGesture()).toBe(true);
    // A Tab press among shell controls, then the page pulls focus in a later task.
    clickShell();
    noteShellInput(true);
    await nextTask();
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
    notePointerOver(frame);
    frame.focus();
    active(false);
    expect(frameGesture()).toBe(false);
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
    expect(frameGesture()).toBe(false);
  });

  it("is nothing while focus is outside the frame", () => {
    notePointerOver(frame);
    button.focus();
    expect(frameGesture()).toBe(false);
  });

  it("counts only trusted events", () => {
    clickShell();
    frame.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    pull();
    expect(frameGesture()).toBe(false);
    notePointerOver(frame);
    expect(frameGesture()).toBe(true);
    document.dispatchEvent(new Event("pointerdown"));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    document.dispatchEvent(new MouseEvent("mouseout", { relatedTarget: null }));
    expect(frameGesture()).toBe(true);
  });

  it("does not count keys that act on the page as input to the shell", () => {
    for (const key of ["Alt", "AltGraph", "Control", "Meta", "Shift", "Escape"]) expect(shellKey({ key, altKey: key === "Alt" }), key).toBe(false);
    expect(shellKey({ key: "ArrowUp", altKey: true })).toBe(false);
    expect(shellKey({ key: "ArrowDown", altKey: true })).toBe(false);
    for (const key of ["a", "Enter", "Tab", " ", "ArrowUp", "Backspace"]) expect(shellKey({ key, altKey: false }), key).toBe(true);
    expect(shellKey({ key: "ArrowLeft", altKey: true })).toBe(true);
  });
});
