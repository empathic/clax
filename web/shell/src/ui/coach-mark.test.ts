import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { COACH_MS, COACH_PAD, type FrameScheduler, showCoachMark } from "./coach-mark";

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

/** A frame scheduler a test steps: `flush()` runs the callbacks waiting now, once. */
function fakeFrames() {
  let waiting: Array<{ fn: () => void; live: boolean }> = [];
  const schedule: FrameScheduler = fn => {
    const w = { fn, live: true };
    waiting.push(w);
    return () => { w.live = false; };
  };
  return { schedule, flush() { const run = waiting; waiting = []; for (const w of run) if (w.live) w.fn(); }, get waiting() { return waiting.filter(w => w.live).length; } };
}
let frames = fakeFrames();

const dismissers: Array<() => void> = [];
const show = (el: HTMLElement, shown: (e: Element) => boolean = always) => {
  const d = showCoachMark(el, shown, frames.schedule);
  if (d) dismissers.push(d);
  return d!;
};

// Fake timers pin performance.now(), which the clock reads, so a test's advance is exact however loaded the machine.
beforeEach(() => { vi.useFakeTimers(); frames = fakeFrames(); });

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

  it("follows_its_control_when_any_scrolling_ancestor_scrolls", () => {
    const box = document.createElement("div");
    box.style.overflowX = "auto";
    box.style.overflowY = "auto";
    document.body.appendChild(box);
    box.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, width: 400, height: 300, right: 400, bottom: 300, toJSON() {} }) as DOMRect;
    const el = document.createElement("button");
    box.appendChild(el);
    let top = 100;
    el.getBoundingClientRect = () => ({ x: 20, y: top, left: 20, top, width: 80, height: 32, right: 100, bottom: top + 32, toJSON() {} }) as DOMRect;
    show(el);
    expect(marks()[0].style.top).toBe("96px");
    top = 60;
    box.dispatchEvent(new Event("scroll"));
    expect(marks()).toHaveLength(1);
    expect(marks()[0].style.top).toBe("56px");
  });

  it("a_mark_goes_when_its_control_scrolls_out_of_view", () => {
    const box = document.createElement("div");
    box.style.overflowX = "auto";
    box.style.overflowY = "auto";
    document.body.appendChild(box);
    box.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, width: 400, height: 300, right: 400, bottom: 300, toJSON() {} }) as DOMRect;
    const el = document.createElement("button");
    box.appendChild(el);
    let top = 100;
    el.getBoundingClientRect = () => ({ x: 20, y: top, left: 20, top, width: 80, height: 32, right: 100, bottom: top + 32, toJSON() {} }) as DOMRect;
    show(el);
    top = 500;
    box.dispatchEvent(new Event("scroll"));
    expect(marks()).toHaveLength(0);
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

  it("a_control_disabled_while_ringed_loses_its_ring_on_the_next_frame", () => {
    const el = control();
    // The real test of "shown": a disabled control is not shown.
    const d = showCoachMark(el, e => !(e as HTMLButtonElement).disabled, frames.schedule)!;
    dismissers.push(d);
    expect(marks()).toHaveLength(1);
    el.disabled = true;
    expect(marks()).toHaveLength(1);
    frames.flush();
    expect(marks()).toHaveLength(0);
    expect(frames.waiting).toBe(0);
  });

  it("the_ring_follows_a_control_that_a_sibling_pushed_sideways", () => {
    const el = control();
    // The control's box is where it sits among its siblings.
    el.getBoundingClientRect = () => {
      const x = 100 + [...document.body.children].filter(c => c.classList.contains("pushed")).length * 60;
      return { x, y: 10, left: x, top: 10, width: 80, height: 32, right: x + 80, bottom: 42, toJSON() {} } as DOMRect;
    };
    show(el);
    expect(marks()[0].style.left).toBe("96px");
    const sibling = document.createElement("span");
    sibling.className = "pushed";
    document.body.insertBefore(sibling, el);
    expect(marks()[0].style.left).toBe("96px");
    frames.flush();
    expect(marks()[0].style.left).toBe("156px");
    frames.flush();
    expect(marks()).toHaveLength(1);
  });

  it("a_mark_not_shown_at_once_draws_nothing_and_returns_null", () => {
    expect(showCoachMark(control(), () => false, frames.schedule)).toBeNull();
    expect(marks()).toHaveLength(0);
    expect(frames.waiting).toBe(0);
    expect(document.querySelectorAll(".coach-mark")).toHaveLength(0);
  });

  describe("once dismissed, nothing listens or polls", () => {
    const ways: Array<[string, () => void]> = [
      ["the timer", () => advance(COACH_MS)],
      ["a press", () => { document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })); }],
      ["Escape", () => { document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); }],
      ["a replacing highlight", () => { show(control()); }],
    ];
    for (const [name, dismissIt] of ways) {
      it(`dismissed_by_${name.replace(/\s/g, "_")}`, () => {
        const shown = vi.fn(() => true);
        const inner = document.createElement("div");
        document.body.appendChild(inner);
        showCoachMark(control(), shown, frames.schedule);
        dismissIt();
        // A replacement's own mark is dismissed too, so only the first's listeners could call `shown`.
        for (const d of dismissers.splice(0)) d();
        shown.mockClear();
        inner.dispatchEvent(new Event("scroll"));
        window.dispatchEvent(new Event("resize"));
        frames.flush();
        expect(shown).not.toHaveBeenCalled();
        expect(marks()).toHaveLength(0);
      });
    }
  });
});
