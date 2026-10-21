// The tag picker (ui/TagPicker.svelte) by keyboard and pointer.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TagRef } from "./api";
import { flush, mount } from "./test/svelte";
import TagPicker from "./ui/TagPicker.svelte";

const TAGS: TagRef[] = [{ id: "1", name: "design", colour: "purple" }, { id: "2", name: "draft", colour: "gray" }, { id: "3", name: "Q4", colour: "blue" }];
const key = (el: Element, k: string) => flush(() => { el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true })); });
const type = (el: HTMLInputElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });

afterEach(() => { document.body.replaceChildren(); });

function picker(chosen: Set<string>) {
  const onToggle = vi.fn((t: TagRef) => { if (!chosen.delete(t.id)) chosen.add(t.id); });
  const onClose = vi.fn();
  const m = mount(TagPicker, { tags: TAGS, chosen: (t: TagRef) => chosen.has(t.id), onToggle, label: "Tags", onClose });
  const field = m.root.querySelector<HTMLInputElement>("input[type=search]")!;
  const boxes = () => [...m.root.querySelectorAll<HTMLInputElement>("input[type=checkbox]")];
  return { m, field, boxes, onToggle, onClose };
}

describe("TagPicker", () => {
  it("opens on its search, checks the chosen tags, and moves by the arrows", () => {
    const { field, boxes, m } = picker(new Set(["3"]));
    expect(document.activeElement).toBe(field);
    expect(boxes().map(b => b.checked)).toEqual([false, false, true]);
    expect(m.root.querySelector("ul")!.getAttribute("aria-label")).toBe("Tags");
    key(field, "ArrowDown");
    expect(document.activeElement).toBe(boxes()[0]);
    key(boxes()[0], "End");
    expect(document.activeElement).toBe(boxes()[2]);
    key(boxes()[2], "ArrowUp");
    expect(document.activeElement).toBe(boxes()[1]);
    key(boxes()[1], "Home");
    key(boxes()[0], "ArrowUp");
    expect(document.activeElement).toBe(field);
  });

  it("asks for a change and shows the box as chosen says until then", () => {
    const chosen = new Set<string>();
    const { boxes, onToggle, onClose, m } = picker(chosen);
    // The box keeps chosen's state; the parent changes it.
    flush(() => { boxes()[1].click(); });
    expect(onToggle).toHaveBeenCalledWith(TAGS[1]);
    expect(chosen.has("2")).toBe(true);
    m.update({ tags: [...TAGS], chosen: (t: TagRef) => chosen.has(t.id), onToggle, label: "Tags", onClose });
    expect(boxes()[1].checked).toBe(true);
  });

  it("searches names, toggles the only match with Enter, says when nothing matches, and closes on Escape", () => {
    const { field, boxes, onToggle, onClose, m } = picker(new Set());
    type(field, "DR");
    expect(boxes()).toHaveLength(1);
    key(field, "Enter");
    expect(onToggle).toHaveBeenCalledWith(TAGS[1]);
    type(field, "zz");
    expect(m.root.textContent).toContain("No tag matches “zz”.");
    // A printable key stays in the picker, away from the shell's keys.
    const heard = vi.fn();
    document.addEventListener("keydown", heard);
    key(field, "c");
    expect(heard).not.toHaveBeenCalled();
    key(field, "Escape");
    expect(onClose).toHaveBeenCalled();
    expect(heard).not.toHaveBeenCalled();
    document.removeEventListener("keydown", heard);
  });
});
