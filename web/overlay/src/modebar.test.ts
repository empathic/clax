// The picking hint never covers what can be picked (BRIEF
// clax-feedback-plugins Addendum 4): pointing at the page's header nav while
// picking moves the chip to the bottom edge, and back as the pointer goes down.
import { render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { KEEP_CLEAR_PX, edgeFor } from "./keep-clear";
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
    const nav = document.querySelector("nav a")!;
    nav.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 30, clientY: 20 }));
    flushSync();
    expect(bar.dataset.edge).toBe("bottom");
    expect(bar.classList.contains("bottom")).toBe(true);
    // Back down the page: the chip goes back to the top, clear of the pointer.
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 30, clientY: window.innerHeight - 10 }));
    flushSync();
    expect(bar.dataset.edge).toBe("top");
    await tick();
    flushSync();
    expect(bar.querySelector('[role="status"]')?.textContent).toMatch(/Click what your report is about/);
    expect(bar.querySelector(".hint")?.getAttribute("aria-hidden")).toBe("true");
    expect(bar.querySelector(".hint")?.textContent).toBe("Click or select it · Esc");
  });

  it("holds still for a pointer coming to press it, and moves off what a pointer rests near", () => {
    vi.useFakeTimers();
    try {
      const { container } = render(ModeBar, { props: { mode: "element", onMode: () => {}, onCancel: () => {} } });
      const bar = container.querySelector<HTMLElement>(".modebar")!;
      bar.getBoundingClientRect = () => ({ left: 400, right: 900, top: 8, bottom: 52, width: 500, height: 44, x: 400, y: 8, toJSON() {} });
      const move = (x: number, y: number) => { window.dispatchEvent(new MouseEvent("mousemove", { clientX: x, clientY: y })); flushSync(); };
      // In its column, near it: it waits.
      move(600, 90);
      vi.advanceTimersByTime(300);
      expect(bar.dataset.edge).toBe("top");
      // On it: it stays, however long the pointer is there.
      move(600, 30);
      vi.advanceTimersByTime(5000);
      flushSync();
      expect(bar.dataset.edge).toBe("top");
      // Resting near it, off it, it moves so what is under it can be reached.
      move(600, 90);
      vi.advanceTimersByTime(800);
      flushSync();
      expect(bar.dataset.edge).toBe("bottom");
    } finally {
      vi.useRealTimers();
    }
  });

  it("moves at once when the pointer is near its edge but outside its column", () => {
    const { container } = render(ModeBar, { props: { mode: "element", onMode: () => {}, onCancel: () => {} } });
    const bar = container.querySelector<HTMLElement>(".modebar")!;
    bar.getBoundingClientRect = () => ({ left: 400, right: 900, top: 8, bottom: 52, width: 500, height: 44, x: 400, y: 8, toJSON() {} });
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 100, clientY: 20 }));
    flushSync();
    expect(bar.dataset.edge).toBe("bottom");
  });

  it("offers only the modes it is given", () => {
    const { container } = render(ModeBar, { props: { mode: null, modes: [{ mode: "screen", label: "This page", key: "S" }], onMode: () => {}, onCancel: () => {} } });
    expect([...container.querySelectorAll(".modes button")].map(b => b.textContent)).toEqual(["This pageS"]);
  });
});
