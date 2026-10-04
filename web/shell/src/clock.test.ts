import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { after, now, wall } from "./clock";
import { PUBLISH_GAP_MS, PUBLISH_PER_MINUTE } from "./caps/artifact";
import { forgetBudgets, takeSlot } from "./caps/budget";
import { DOUBLE_CLICK_MS, HINT_MS, SHELL_QUIET_MS } from "./caps/gesture";
import { HIDDEN_MS } from "./stream";
import { LOOK_EVERY_MS } from "./view/artifact-controller";
import { ALLOW_DELAY_MS } from "./view/prompt-queue";
import { SEEN_AFTER_MS } from "./view/sidebar-model";
import { RETRY_MS } from "../../bridge/src/part-loader";

type TestClock = { advance(ms: number): number; readonly skew: number; readonly pending: number };
const clock = () => (globalThis as unknown as { claxTestClock: TestClock }).claxTestClock;

describe("the shell's clock", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("runs a timer when its time has passed in real time", () => {
    const fn = vi.fn();
    after(1000, fn);
    vi.advanceTimersByTime(999);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(fn).toHaveBeenCalledOnce();
  });

  it("an advance moves both readings and runs what fell due, in order, once", () => {
    const ran: string[] = [];
    const [n, w] = [now(), wall()];
    after(300, () => ran.push("b"));
    after(100, () => ran.push("a"));
    after(900, () => ran.push("late"));
    clock().advance(500);
    expect(ran).toEqual(["a", "b"]);
    expect(now() - n).toBeGreaterThanOrEqual(500);
    expect(wall() - w).toBeGreaterThanOrEqual(500);
    vi.advanceTimersByTime(1000);
    expect(ran).toEqual(["a", "b", "late"]);
    expect(clock().pending).toBe(0);
  });

  it("a timer started by a timer during an advance falls due from its parent's time", () => {
    const ran: string[] = [];
    after(1000, () => { ran.push("seen"); after(1000, () => ran.push("looked")); });
    clock().advance(1500);
    expect(ran).toEqual(["seen"]);
    clock().advance(500);
    expect(ran).toEqual(["seen", "looked"]);
  });

  it("a cancelled timer never runs", () => {
    const fn = vi.fn();
    after(100, fn)();
    clock().advance(200);
    vi.advanceTimersByTime(200);
    expect(fn).not.toHaveBeenCalled();
  });

  it("keeps its skew in sessionStorage, so a reload of the tab keeps it", () => {
    const before = clock().skew;
    clock().advance(250);
    expect(Number(sessionStorage.getItem("clax.test-clock.skew"))).toBe(before + 250);
  });
});

// The browser tests advance the clock past these rules; their real durations
// are pinned here, and each is judged against the clock.
describe("the real durations the browser tests advance past", () => {
  it("are the shell's values", () => {
    expect(SHELL_QUIET_MS).toBe(5_500);
    expect(ALLOW_DELAY_MS).toBe(500);
    expect(PUBLISH_GAP_MS).toBe(2_000);
    expect(PUBLISH_PER_MINUTE).toBe(10);
    expect(HINT_MS).toBe(2_500);
    expect(DOUBLE_CLICK_MS).toBe(500);
    expect(SEEN_AFTER_MS).toBe(1_000);
    expect(LOOK_EVERY_MS).toBe(1_000);
    expect(HIDDEN_MS).toBe(30_000);
    expect(RETRY_MS).toBe(2_000);
  });

  it("the publish gap is judged on the clock", () => {
    vi.useFakeTimers();
    forgetBudgets();
    sessionStorage.clear();
    const limits = { gapMs: PUBLISH_GAP_MS, perWindow: { n: PUBLISH_PER_MINUTE, ms: 60_000 } };
    expect(takeSlot("gap-test", limits)).toBe(0);
    clock().advance(PUBLISH_GAP_MS - 50);
    expect(takeSlot("gap-test", limits)).toBeGreaterThan(0);
    clock().advance(50);
    expect(takeSlot("gap-test", limits)).toBe(0);
    vi.useRealTimers();
  });
});
