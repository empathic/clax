import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { COACH_MS, COACH_PAD, showCoachMark } from "./coach-mark";

type TestClock = { advance(ms: number): number };
const advance = (ms: number) => (globalThis as unknown as { claxTestClock: TestClock }).claxTestClock.advance(ms);

const control = (rect = { x: 100, y: 10, width: 80, height: 32 }) => {
  const el = document.createElement("button");
  document.body.appendChild(el);
  el.getBoundingClientRect = () => ({ ...rect, left: rect.x, top: rect.y, right: rect.x + rect.width, bottom: rect.y + rect.height, toJSON() {} }) as DOMRect;
  return el;
};
const marks = () => [...document.querySelectorAll<HTMLElement>(".coach-mark")];
const always = () => true;

const dismissers: Array<() => void> = [];
const show = (el: HTMLElement, shown: (e: Element) => boolean = always) => { const d = showCoachMark(el, shown); dismissers.push(d); return d; };

// Fake timers pin performance.now(), which the clock reads, so a test's advance is exact however loaded the machine.
beforeEach(() => { vi.useFakeTimers(); });

afterEach(() => {
  for (const d of dismissers.splice(0)) d();
  advance(COACH_MS);
  document.body.replaceChildren();
  document.head.querySelector("style[data-coach-mark]")?.remove();
  vi.useRealTimers();
});

describe("a coach mark", () => {
  it("a_coach_mark_rings_its_control_and_goes_after_six_seconds", () => {
    expect(COACH_MS).toBe(6_000);
    expect(COACH_PAD).toBe(4);
    show(control());
    const [ring] = marks();
    expect(marks()).toHaveLength(1);
    expect(ring.style.left).toBe("96px");
    expect(ring.style.top).toBe("6px");
    expect(ring.style.width).toBe("88px");
    expect(ring.style.height).toBe("40px");
    expect(ring.getAttribute("aria-hidden")).toBe("true");
    expect(ring.parentElement).toBe(document.body);
    advance(5_999);
    expect(marks()).toHaveLength(1);
    advance(1);
    expect(marks()).toHaveLength(0);
  });

  it("injects its style once", () => {
    show(control());
    show(control());
    expect(document.querySelectorAll("style[data-coach-mark]")).toHaveLength(1);
  });

  it("a_press_anywhere_in_the_window_dismisses_it", () => {
    show(control());
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(marks()).toHaveLength(0);
    show(control());
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(marks()).toHaveLength(0);
    show(control());
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    expect(marks()).toHaveLength(1);
  });

  it("a_second_mark_replaces_the_first", () => {
    show(control());
    show(control({ x: 300, y: 50, width: 20, height: 20 }));
    expect(marks()).toHaveLength(1);
    expect(marks()[0].style.left).toBe("296px");
  });

  it("a_mark_goes_when_its_control_stops_being_shown", () => {
    let on = true;
    const el = control();
    show(el, () => on);
    window.dispatchEvent(new Event("resize"));
    expect(marks()).toHaveLength(1);
    on = false;
    window.dispatchEvent(new Event("resize"));
    expect(marks()).toHaveLength(0);
  });

  it("follows its control to where it moved", () => {
    const el = control();
    show(el);
    el.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, width: 10, height: 10, right: 10, bottom: 10, toJSON() {} }) as DOMRect;
    window.dispatchEvent(new Event("resize"));
    expect(marks()[0].style.left).toBe("-4px");
    expect(marks()[0].style.width).toBe("18px");
  });

  it("dismiss_is_idempotent", () => {
    const d = show(control());
    d();
    expect(() => d()).not.toThrow();
    expect(marks()).toHaveLength(0);
    const e = show(control());
    advance(COACH_MS);
    expect(() => { e(); e(); }).not.toThrow();
    expect(marks()).toHaveLength(0);
    // A stale dismiss does not remove a newer mark.
    const old = show(control());
    show(control());
    old();
    expect(marks()).toHaveLength(1);
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(marks()).toHaveLength(0);
  });
});
