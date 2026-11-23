import { describe, expect, it } from "vitest";
import { late, markShown } from "./topbar-gate.svelte";

const settle = () => new Promise(r => setTimeout(r));

describe("the top bar's slots", () => {
  it("keeps a control in its place when its slot holds it before the bar shows, and puts one that comes after at the free end, each later one before the last", async () => {
    const bar = document.createElement("div");
    const slot = (filled: boolean) => {
      const s = document.createElement("span");
      if (filled) s.append(document.createElement("button"));
      bar.append(s);
      late(s);
      return s;
    };
    const early = slot(true);
    const empty = slot(false);
    const before = slot(false);
    before.append(document.createElement("button"));
    await settle();
    // Filled before the bar showed: in place.
    expect(early.style.getPropertyValue("--late")).toBe("");
    expect(before.style.getPropertyValue("--late")).toBe("");
    markShown();
    const later = slot(false);
    empty.append(document.createElement("a"));
    await settle();
    later.append(document.createElement("a"));
    await settle();
    const a = Number(empty.style.getPropertyValue("--late"));
    const b = Number(later.style.getPropertyValue("--late"));
    expect(a).toBeLessThan(0);
    expect(b).toBeLessThan(a);
    // Decided once: a control that goes and comes back keeps its place.
    empty.replaceChildren();
    empty.append(document.createElement("a"));
    await settle();
    expect(Number(empty.style.getPropertyValue("--late"))).toBe(a);
  });
});
