import { describe, expect, it } from "vitest";
import { KEY_ROWS, keyAction } from "./keys";

const k = (key: string, over: Partial<KeyboardEvent> = {}, target: Element = document.body) =>
  ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false, repeat: false, target, ...over }) as unknown as KeyboardEvent;

describe("keys", () => {
  it("maps the shell's keys", () => {
    expect(["?", "c", "C", "t", "j", "k", "Enter", "s", "r"].map(x => keyAction(k(x)))).toEqual(["help", "comment", "comment", "threads", "next", "prev", "reply", "send", "resolve"]);
    expect(keyAction(k("S", { shiftKey: true }))).toBe("sendTicked");
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
    expect(keyAction(k("j", { repeat: true }))).toBeNull();
  });
  it("acts on Enter only outside buttons and links, which Enter already presses", () => {
    expect(keyAction(k("Enter", {}, document.createElement("button")))).toBeNull();
    expect(keyAction(k("Enter", {}, document.createElement("a")))).toBeNull();
  });
  it("lists every row the sheet shows, in order", () => {
    expect(KEY_ROWS.map(r => r.keys.join("+"))).toEqual(["C", "Esc", "T", "J+K", "↵", "S", "R"]);
  });
});
