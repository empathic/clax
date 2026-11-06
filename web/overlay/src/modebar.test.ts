// The picking hint never covers what can be picked (BRIEF
// clax-feedback-plugins Addendum 4): pointing at the page's header nav while
// picking moves the chip to the bottom edge, and back as the pointer goes down.
import { render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { KEEP_CLEAR_DWELL_MS, KEEP_CLEAR_PX, edgeFor } from "./keep-clear";
import ModeBar from "./ModeBar.svelte";

describe("edgeFor", () => {
  it("stays put until the pointer comes near, then takes the other edge", () => {
    expect(edgeFor("top", 600, 800, 40)).toBe("top");
    expect(edgeFor("top", 40 + KEEP_CLEAR_PX, 800, 40)).toBe("bottom");
    expect(edgeFor("bottom", 400, 800, 40)).toBe("bottom");
    expect(edgeFor("bottom", 800 - 40 - KEEP_CLEAR_PX, 800, 40)).toBe("top");
  });
});

describe("the mode bar while picking", () => {
  afterEach(() => { document.body.innerHTML = ""; });

  it("moves off the header nav the pointer is on, keeps the full hint for screen readers, and shows a short one", async () => {
    document.body.innerHTML = `<header style="height:48px"><nav><a href="/a">Pull requests</a><a href="/b">Dashboard</a></nav></header>`;
    const { container } = render(ModeBar, { props: { mode: "element", onMode: () => {}, onCancel: () => {} } });
    const bar = container.querySelector<HTMLElement>(".modebar")!;
    expect(bar.dataset.edge).toBe("top");
    vi.useFakeTimers();
    const nav = document.querySelector("nav a")!;
    nav.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 30, clientY: 20 }));
    flushSync();
    // Not at once: a pointer on its way to the chip gets there first.
    expect(bar.dataset.edge).toBe("top");
    vi.advanceTimersByTime(KEEP_CLEAR_DWELL_MS);
    flushSync();
    expect(bar.dataset.edge).toBe("bottom");
    expect(bar.classList.contains("bottom")).toBe(true);
    // Back down the page: the chip goes back to the top, clear of the pointer.
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 30, clientY: window.innerHeight - 10 }));
    vi.advanceTimersByTime(KEEP_CLEAR_DWELL_MS);
    flushSync();
    expect(bar.dataset.edge).toBe("top");
    vi.useRealTimers();
    await tick();
    flushSync();
    expect(bar.querySelector('[role="status"]')?.textContent).toMatch(/Click what your report is about/);
    expect(bar.querySelector(".hint")?.getAttribute("aria-hidden")).toBe("true");
    expect(bar.querySelector(".hint")?.textContent).toBe("Click or select it · Esc");
  });

  it("stays put for a pointer that reaches it, so its buttons can be clicked", () => {
    vi.useFakeTimers();
    const { container } = render(ModeBar, { props: { mode: "element", onMode: () => {}, onCancel: () => {}, onReports: () => {} } });
    const bar = container.querySelector<HTMLElement>(".modebar")!;
    bar.getBoundingClientRect = () => ({ left: 400, right: 800, top: 8, bottom: 44, width: 400, height: 36, x: 400, y: 8, toJSON() {} }) as DOMRect;
    // Coming up toward it: near, then on it before the dwell runs out.
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 600, clientY: 90 }));
    vi.advanceTimersByTime(KEEP_CLEAR_DWELL_MS - 100);
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 600, clientY: 26 }));
    vi.advanceTimersByTime(KEEP_CLEAR_DWELL_MS * 4);
    flushSync();
    expect(bar.dataset.edge).toBe("top");
    vi.useRealTimers();
  });

  it("stays put for a slow approach from straight above, however long it lingers near, and moves for a pointer off to the side", () => {
    vi.useFakeTimers();
    const { container } = render(ModeBar, { props: { mode: "element", onMode: () => {}, onCancel: () => {}, onReports: () => {} } });
    const bar = container.querySelector<HTMLElement>(".modebar")!;
    bar.getBoundingClientRect = () => ({ left: 400, right: 800, top: 8, bottom: 44, width: 400, height: 36, x: 400, y: 8, toJSON() {} }) as DOMRect;
    for (const y of [120, 100, 80, 60]) {
      window.dispatchEvent(new MouseEvent("mousemove", { clientX: 790, clientY: y }));
      vi.advanceTimersByTime(KEEP_CLEAR_DWELL_MS);
    }
    flushSync();
    expect(bar.dataset.edge).toBe("top");
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 900, clientY: 60 }));
    vi.advanceTimersByTime(KEEP_CLEAR_DWELL_MS);
    flushSync();
    expect(bar.dataset.edge).toBe("bottom");
    vi.useRealTimers();
  });

  it("shows R on Your reports, kept out of its name", () => {
    const { container } = render(ModeBar, { props: { mode: "element", onMode: () => {}, onCancel: () => {}, onReports: () => {}, waiting: 2 } });
    const b = container.querySelector<HTMLButtonElement>(".reports")!;
    expect(b.getAttribute("aria-keyshortcuts")).toBe("R");
    expect(b.querySelector("kbd")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("offers only the modes it is given", () => {
    const { container } = render(ModeBar, { props: { mode: null, modes: [{ mode: "screen", label: "This page", key: "S" }], onMode: () => {}, onCancel: () => {} } });
    expect([...container.querySelectorAll(".modes button")].map(b => b.textContent)).toEqual(["This pageS"]);
  });
});
