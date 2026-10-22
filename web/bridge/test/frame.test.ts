import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FRAME_FALLBACK_MS, nextFrame } from "../src/frame";

describe("nextFrame", () => {
  let frames: FrameRequestCallback[] = [];
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    frames = [];
    vi.stubGlobal("requestAnimationFrame", (f: FrameRequestCallback) => frames.push(f));
    vi.stubGlobal("cancelAnimationFrame", (n: number) => { frames[n - 1] = () => {}; });
  });
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("runs at the next frame, once", () => {
    const fn = vi.fn();
    nextFrame(fn);
    frames[0](0);
    vi.advanceTimersByTime(FRAME_FALLBACK_MS);
    expect(fn).toHaveBeenCalledTimes(1);
  });
  it("runs after FRAME_FALLBACK_MS when no frame comes, once", () => {
    const fn = vi.fn();
    nextFrame(fn);
    vi.advanceTimersByTime(FRAME_FALLBACK_MS - 1);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(fn).toHaveBeenCalledTimes(1);
    frames[0](0);
    expect(fn).toHaveBeenCalledTimes(1);
  });
  it("runs neither way once cancelled", () => {
    const fn = vi.fn();
    nextFrame(fn)();
    vi.advanceTimersByTime(FRAME_FALLBACK_MS);
    frames[0](0);
    expect(fn).not.toHaveBeenCalled();
  });
});
