import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { KeyTrap, keyText } from "../src/key-trap";

vi.setConfig({ testTimeout: 20_000 });

const key = (k: string, mods: Partial<Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "altKey" | "isComposing">> = {}) => ({ key: k, ctrlKey: false, metaKey: false, altKey: false, isComposing: false, ...mods });

describe("keyText", () => {
  it("gives the text a key types: a character, a newline for Enter, Backspace", () => {
    expect(keyText(key("V"))).toBe("V");
    expect(keyText(key(" "))).toBe(" ");
    expect(keyText(key("😀"))).toBe("😀");
    expect(keyText(key("å", { altKey: true }))).toBe("å");
    // AltGr is Ctrl+Alt on Windows.
    expect(keyText(key("@", { ctrlKey: true, altKey: true }))).toBe("@");
    expect(keyText(key("Enter"))).toBe("\n");
    expect(keyText(key("Backspace"))).toBe("Backspace");
  });
  it("gives nothing for other keys, shortcuts, or a press inside a composition", () => {
    for (const k of ["Shift", "Alt", "Escape", "Tab", "ArrowUp", "Delete", "F5", "Dead", "Unidentified"]) expect(keyText(key(k))).toBeNull();
    expect(keyText(key("v", { metaKey: true }))).toBeNull();
    expect(keyText(key("v", { ctrlKey: true }))).toBeNull();
    expect(keyText(key("Enter", { metaKey: true }))).toBeNull();
    expect(keyText(key("a", { isComposing: true }))).toBeNull();
  });
});

describe("KeyTrap", () => {
  let sent: [string, string[], boolean][];
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
    sent = [];
    heard = [];
    trap = new KeyTrap(window, (id, keys, done) => sent.push([id, keys, done]), { trustedOnly: false, ms: 200 });
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
    expect(sent).toEqual([]);
  });

  it("after a pick, hands every text key to the shell in order and keeps all key events from the page, until the page loses focus", () => {
    const field = document.createElement("input");
    document.body.appendChild(field);
    field.focus();
    trap.start("p1");
    expect(trap.active).toBe(true);
    for (const k of ["V", "i", "Shift", "Backspace", "a", "Enter"]) expect(press(k, field).defaultPrevented).toBe(true);
    field.dispatchEvent(new InputEvent("input", { bubbles: true, data: "x" }));
    expect(heard).toEqual([]);
    expect(sent).toEqual([["p1", ["V"], false], ["p1", ["i"], false], ["p1", ["Backspace"], false], ["p1", ["a"], false], ["p1", ["\n"], false]]);
    // The composer took focus.
    dispatchEvent(new Event("blur"));
    expect(trap.active).toBe(false);
    expect(sent.at(-1)).toEqual(["p1", [], true]);
    expect(press("b", field).defaultPrevented).toBe(false);
    expect(heard).toContain("keydown:b");
    expect(sent).toHaveLength(6);
    field.remove();
  });

  it("ends after its time when focus never leaves the page, and at once when the page has no focus", async () => {
    trap.start("p1");
    await new Promise(r => setTimeout(r, 250));
    expect(trap.active).toBe(false);
    expect(sent).toEqual([["p1", [], true]]);
    expect(press("a").defaultPrevented).toBe(false);
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    trap.start("p2");
    expect(trap.active).toBe(false);
    expect(sent.at(-1)).toEqual(["p2", [], true]);
  });

  it("ends an earlier pick's trap when a new one starts", () => {
    trap.start("p1");
    press("a");
    trap.start("p2");
    press("b");
    expect(sent).toEqual([["p1", ["a"], false], ["p1", [], true], ["p2", ["b"], false]]);
  });

  it("neither keeps from the page nor hands on keys the page dispatched itself", () => {
    const real = new KeyTrap(window, (id, keys, done) => sent.push([id, keys, done]));
    real.start("p1");
    const e = press("a");
    expect(e.defaultPrevented).toBe(false);
    expect(heard).toContain("keydown:a");
    expect(sent).toEqual([]);
    dispatchEvent(new Event("blur"));
  });
});
