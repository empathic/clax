import { afterEach, describe, expect, it, vi } from "vitest";
import { KEY_ROWS, holdKeysAcrossLoad, keyAction, keysHeldAtLoad } from "./keys";

const k = (key: string, over: Partial<KeyboardEvent> = {}, target: Element = document.body) =>
  ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false, repeat: false, target, ...over }) as unknown as KeyboardEvent;

describe("keys", () => {
  it("maps the shell's keys: C (either case) and ?", () => {
    expect(["?", "c", "C"].map(x => keyAction(k(x)))).toEqual(["help", "comment", "comment"]);
    expect(keyAction(k("C", { shiftKey: true }))).toBe("comment");
    expect(keyAction(k("?", { shiftKey: true }))).toBe("help");
  });
  it("has no other keys", () => {
    for (const x of ["t", "j", "k", "s", "r", "x", "v", "p", "S", "Enter", " "]) expect(keyAction(k(x)), x).toBeNull();
  });
  it("never acts while typing, composing, repeating, or with a modifier", () => {
    const input = document.createElement("input");
    const area = document.createElement("textarea");
    const edit = document.createElement("div");
    edit.contentEditable = "true";
    for (const t of [input, area, edit]) expect(keyAction(k("c", {}, t))).toBeNull();
    expect(keyAction(k("c", { metaKey: true }))).toBeNull();
    expect(keyAction(k("c", { ctrlKey: true }))).toBeNull();
    expect(keyAction(k("c", { altKey: true }))).toBeNull();
    expect(keyAction(k("c", { isComposing: true }))).toBeNull();
    expect(keyAction(k("c", { repeat: true }))).toBeNull();
  });
  it("never acts inside a dialog, whose keys are its own", () => {
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    const modal = document.createElement("div");
    modal.setAttribute("aria-modal", "true");
    for (const box of [dialog, modal]) {
      const b = document.createElement("button");
      box.append(b);
      for (const x of ["c", "?"]) expect(keyAction(k(x, {}, b))).toBeNull();
    }
  });
  it("lists every row the sheet shows, in order", () => {
    expect(KEY_ROWS.map(r => r.keys.join("+"))).toEqual(["C", "?", "Esc"]);
  });
});

describe("keys held across a load the page caused", () => {
  afterEach(() => { vi.restoreAllMocks(); sessionStorage.clear(); });
  it("holds the next load once", () => {
    expect(keysHeldAtLoad()).toBe(false);
    holdKeysAcrossLoad();
    expect(keysHeldAtLoad()).toBe(true);
    expect(keysHeldAtLoad()).toBe(false);
  });
  it("holds when storage cannot say, and marking never throws", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(() => holdKeysAcrossLoad()).not.toThrow();
    expect(keysHeldAtLoad()).toBe(true);
  });
});
