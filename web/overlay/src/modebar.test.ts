// The picking hint never covers what can be picked (BRIEF
// clax-feedback-plugins Addendum 4): pointing at the page's header nav while
// picking moves the chip to the bottom edge, and back as the pointer goes down.
import { render } from "@testing-library/svelte";
import { flushSync } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import ModeBar, { KEEP_CLEAR_PX, edgeFor } from "./ModeBar.svelte";

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

  it("moves off the header nav the pointer is on, keeps the full hint for screen readers, and shows a short one", () => {
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
    expect(bar.querySelector('[role="status"]')?.textContent).toMatch(/Click what your report is about/);
    expect(bar.querySelector(".hint")?.getAttribute("aria-hidden")).toBe("true");
    expect(bar.querySelector(".hint")?.textContent).toBe("Click or select it · Esc");
  });

  it("offers only the modes it is given", () => {
    const { container } = render(ModeBar, { props: { mode: null, modes: [{ mode: "screen", label: "This page", key: "S" }], onMode: () => {}, onCancel: () => {} } });
    expect([...container.querySelectorAll(".modes button")].map(b => b.textContent)).toEqual(["This pageS"]);
  });
});
