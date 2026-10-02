import { describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import KeysSheet from "./ui/KeysSheet.svelte";
import Mark from "./ui/Mark.svelte";
import { KEY_ROWS } from "./view/keys";
import { MARK_SVG } from "./view/mark";
import ThemeSwitch from "./ui/ThemeSwitch.svelte";

describe("Echo chrome", () => {
  it("a playful mark's halves meet on one click and part on the next", () => {
    const m = mount(Mark, { playful: true });
    const b = m.root.querySelector("button.mark") as HTMLButtonElement;
    expect(b.getAttribute("aria-pressed")).toBe("false");
    flush(() => b.click());
    expect(b.getAttribute("aria-pressed")).toBe("true");
    flush(() => b.click());
    expect(b.getAttribute("aria-pressed")).toBe("false");
    m.unmount();
  });

  it("draws the same symbol as MARK_SVG, as decoration inside a named element", () => {
    const want = document.createElement("div");
    want.innerHTML = MARK_SVG;
    // The image says the product's name; the button, beside the "Clax" heading, names itself.
    for (const [props, name] of [[{}, "Clax"], [{ playful: true }, "Clax mark"]] as const) {
      const m = mount(Mark, props);
      const svg = m.root.querySelector("svg.mk")!;
      expect(svg.outerHTML).toBe(want.querySelector("svg")!.outerHTML);
      expect(svg.getAttribute("aria-hidden")).toBe("true");
      expect(m.root.firstElementChild!.getAttribute("aria-label")).toBe(name);
      m.unmount();
    }
    const hero = mount(Mark, { size: "hero", apart: true });
    expect(hero.root.querySelector(".mark")!.getAttribute("data-size")).toBe("hero");
    expect(hero.root.querySelector(".mark")!.hasAttribute("data-apart")).toBe(true);
    hero.unmount();
  });

  it("the switch flips the shown scheme and names what it will do", () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: !q.includes("dark"), addEventListener() {}, removeEventListener() {} }));
    try {
      const m = mount(ThemeSwitch, {});
      const b = m.root.querySelector("button") as HTMLButtonElement;
      expect(b.getAttribute("aria-label")).toBe("Dark theme");
      expect(b.getAttribute("aria-pressed")).toBe("false");
      expect(b.title).toBe("Switch to dark");
      flush(() => b.click());
      expect(document.documentElement.dataset.theme).toBe("dark");
      expect(b.getAttribute("aria-pressed")).toBe("true");
      expect(b.title).toBe("Switch to light");
      flush(() => b.click());
      expect(document.documentElement.dataset.theme).toBeUndefined();
      expect(b.getAttribute("aria-pressed")).toBe("false");
      m.unmount();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("the keys sheet is a labelled dialog of the rows, closed by its button and by Escape", () => {
    const onClose = vi.fn();
    const m = mount(KeysSheet, { onClose });
    const d = m.root.querySelector("[role=dialog]")!;
    expect(document.getElementById(d.getAttribute("aria-labelledby")!)!.textContent).toBe("Keyboard shortcuts");
    expect(d.querySelectorAll("dt")).toHaveLength(KEY_ROWS.length);
    flush(() => d.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    flush(() => (d.querySelector(".foot button") as HTMLButtonElement).click());
    expect(onClose).toHaveBeenCalledTimes(2);
    m.unmount();
  });

  it("the keys sheet takes focus, keeps Tab inside, makes the rest inert, and gives all back when it closes", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    const m = mount(KeysSheet, { onClose: () => {} });
    const close = m.root.querySelector(".foot button") as HTMLButtonElement;
    expect(document.activeElement).toBe(close);
    const d = m.root.querySelector("[role=dialog]")!;
    for (const shiftKey of [false, true]) {
      const tab = new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true });
      close.dispatchEvent(tab);
      expect(tab.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(close);
    }
    expect(d.getAttribute("aria-modal")).toBe("true");
    expect(trigger.hasAttribute("inert")).toBe(true);
    expect(d.closest("[inert]")).toBeNull();
    m.unmount();
    expect(trigger.hasAttribute("inert")).toBe(false);
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });
});
