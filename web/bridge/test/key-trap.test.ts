import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { KeyTrap } from "../src/key-trap";

vi.setConfig({ testTimeout: 20_000 });

describe("KeyTrap", () => {
  let trap: KeyTrap;
  // A page listener added after the trap, as every page script's is.
  let heard: string[];
  const onPage = (e: Event) => { heard.push(`${e.type}:${(e as KeyboardEvent).key ?? ""}`); };
  const press = (k: string, target: EventTarget = document.body) => {
    const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true });
    target.dispatchEvent(e);
    target.dispatchEvent(new KeyboardEvent("keyup", { key: k, bubbles: true, cancelable: true }));
    return e;
  };

  beforeEach(() => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    heard = [];
    trap = new KeyTrap(window, { trustedOnly: false, ms: 200 });
    for (const t of ["keydown", "keyup", "input"]) { addEventListener(t, onPage, true); document.addEventListener(t, onPage); }
  });
  afterEach(() => {
    dispatchEvent(new Event("blur"));
    for (const t of ["keydown", "keyup", "input"]) { removeEventListener(t, onPage, true); document.removeEventListener(t, onPage); }
    vi.restoreAllMocks();
  });

  it("leaves the page's keys alone until a pick", () => {
    const e = press("a");
    expect(e.defaultPrevented).toBe(false);
    expect(heard).toEqual(["keydown:a", "keydown:a", "keyup:a", "keyup:a"]);
  });

  it("after a pick, keeps every key and text event from the page (dropped, with no default action) until the page loses focus", () => {
    const field = document.createElement("input");
    document.body.appendChild(field);
    field.focus();
    trap.start("p1");
    expect(trap.active).toBe(true);
    for (const k of ["V", "i", "Shift", "Backspace", "Enter", "Escape", "Alt"]) expect(press(k, field).defaultPrevented).toBe(true);
    field.dispatchEvent(new InputEvent("input", { bubbles: true, data: "x" }));
    expect(heard).toEqual([]);
    // The composer took focus.
    dispatchEvent(new Event("blur"));
    expect(trap.active).toBe(false);
    expect(press("b", field).defaultPrevented).toBe(false);
    expect(heard).toContain("keydown:b");
    field.remove();
  });

  it("ends when the shell refuses that pick, not another", () => {
    trap.start("p1");
    trap.end("p0");
    expect(trap.active).toBe(true);
    trap.end("p1");
    expect(trap.active).toBe(false);
    expect(press("a").defaultPrevented).toBe(false);
  });

  it("ends after its time when focus never leaves the page, and never starts when the page has no focus", async () => {
    trap.start("p1");
    await new Promise(r => setTimeout(r, 250));
    expect(trap.active).toBe(false);
    expect(press("a").defaultPrevented).toBe(false);
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    trap.start("p2");
    expect(trap.active).toBe(false);
  });

  it("keeps the timer functions in place when it was made, whatever the page puts there later", async () => {
    const own = new KeyTrap(window, { trustedOnly: false, ms: 100 });
    const saved = { set: window.setTimeout, clear: window.clearTimeout };
    try {
      window.setTimeout = (() => { throw new Error("page"); }) as unknown as typeof setTimeout;
      window.clearTimeout = (() => { throw new Error("page"); }) as unknown as typeof clearTimeout;
      own.start("p1");
      own.end("p1");
      expect(own.active).toBe(false);
      own.start("p2");
      expect(own.active).toBe(true);
    } finally {
      window.setTimeout = saved.set;
      window.clearTimeout = saved.clear;
    }
    await new Promise(r => setTimeout(r, 150));
    expect(own.active).toBe(false);
  });

  it("neither keeps from the page nor drops keys the page dispatched itself", () => {
    const real = new KeyTrap(window);
    real.start("p1");
    const e = press("a");
    expect(e.defaultPrevented).toBe(false);
    expect(heard).toContain("keydown:a");
    real.end("p1");
  });
});
