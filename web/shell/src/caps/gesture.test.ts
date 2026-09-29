import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { forgetGestures, frameGesture, noteShellInput, watchGestures } from "./gesture";

const active = (on: boolean) => Object.defineProperty(navigator, "userActivation", { value: { isActive: on }, configurable: true });

describe("frame gestures", () => {
  let frame: HTMLIFrameElement;
  let button: HTMLButtonElement;
  let stop: () => void;
  beforeEach(() => {
    forgetGestures();
    stop = watchGestures(document);
    frame = document.createElement("iframe");
    frame.className = "frame";
    button = document.createElement("button");
    document.body.append(frame, button);
  });
  afterEach(() => {
    stop();
    frame.remove();
    button.remove();
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
  });

  it("is a click in the content frame: activation with focus moved into the frame", () => {
    active(true);
    frame.focus();
    expect(document.activeElement).toBe(frame);
    expect(frameGesture()).toBe(true);
  });

  it("is not activation the viewer gave the shell", () => {
    active(true);
    button.focus();
    expect(frameGesture()).toBe(false);
    // Focus in the frame, but the viewer's latest input went to the shell.
    frame.focus();
    noteShellInput();
    expect(frameGesture()).toBe(false);
    // They click into the frame again: its gesture counts.
    button.focus();
    frame.focus();
    expect(frameGesture()).toBe(true);
  });

  it("is nothing without transient activation, or where the browser cannot tell", () => {
    frame.focus();
    expect(frameGesture()).toBe(false);
    active(false);
    expect(frameGesture()).toBe(false);
  });

  it("marks focus entering the frame on the shell window's blur", () => {
    active(true);
    frame.focus();
    noteShellInput();
    expect(frameGesture()).toBe(false);
    // As in Chromium: focus moves into a cross-origin frame without a focus event on the iframe.
    window.dispatchEvent(new Event("blur"));
    expect(frameGesture()).toBe(true);
  });

  it("counts only trusted shell input", () => {
    active(true);
    frame.focus();
    document.dispatchEvent(new Event("pointerdown"));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    expect(frameGesture()).toBe(true);
  });
});
