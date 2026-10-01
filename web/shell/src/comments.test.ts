import type { Component } from "svelte";
import { describe, expect, it, vi } from "vitest";
import { isSubmitKey, submitKeysLabel } from "./comments";
import { flush, mount } from "./test/svelte";
import { type Thread, areaLabel } from "./threads";
import Composer from "./ui/Composer.svelte";
import Pins from "./ui/Pins.svelte";
import Sidebar from "./ui/Sidebar.svelte";
import { type Draft, nextDraft, withClip } from "./view/composer-model";
import { PIN_RIGHT_ROOM } from "./view/pins-model";

const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const thread = (id: string, status: "open" | "resolved" = "open"): Thread => ({
  id, artifact_id: "7q3k9mzx2b4t", version_n: 1, anchor, status, sent_to_agent: false, has_clip: false, clip_url: null,
  created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "2026-09-29T10:00:00.000Z" }],
});
const at = (id: string, y: number) => ({ id, found: true, method: "exact" as const, rect: { x: 10, y, w: 100, h: 20 } });

function mountIt<P extends Record<string, unknown>>(C: Component<P>, props: P) {
  const view = mount(C, props);
  const { root } = view;
  return { root, update: view.update, done: () => { view.unmount(); root.remove(); } };
}

describe("Pins", () => {
  it("numbers pins like the sidebar's Open section and skips detached, resolved, unmeasured, and scrolled-away threads", () => {
    const threads = [thread("a"), thread("b"), thread("c"), thread("d"), thread("e", "resolved"), thread("f")];
    const resolved = { a: at("a", 40), b: { id: "b", found: false, method: null, rect: null }, c: at("c", -50), e: at("e", 40), f: at("f", 200) };
    const { root, done } = mountIt(Pins, { threads, resolved, onSelect: vi.fn() });
    // Open and not detached: a (1), c (2), d (3, not measured yet), f (4).
    expect(Array.from(root.querySelectorAll("button.thread-pin")).map(b => b.textContent)).toEqual(["1", "4"]);
    const { root: side, done: doneSide } = mountIt(Sidebar, { threads, resolved, now: new Date(), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
    expect(Array.from(side.querySelectorAll(".section-open .thread-num")).map(n => n.textContent)).toEqual(["1", "2", "3", "4"]);
    // A viewer comment must not carry the page-level `.viewer` layout class.
    expect(side.querySelector(".comment.viewer")).toBeNull();
    expect(side.querySelector(".comment.from-viewer .author")!.textContent).toBe("Viewer");
    done();
    doneSide();
  });

  it("pins only the threads on the page the frame shows, numbered like the sidebar", () => {
    const onAbout = (id: string): Thread => ({ ...thread(id), anchor: { ...anchor, file: "about.html" } });
    const threads = [thread("a"), onAbout("b"), onAbout("c")];
    const resolved = { a: at("a", 40), b: at("b", 80), c: at("c", 120) };
    const pins = (file: string) => {
      const { root, done } = mountIt(Pins, { threads, resolved, file, onSelect: vi.fn(), width: 800 });
      const out = Array.from(root.querySelectorAll<HTMLElement>("button.thread-pin")).map(b => `${b.textContent}@${b.style.top}`);
      done();
      return out;
    };
    expect(pins("index.html")).toEqual(["1@28px"]);
    expect(pins("about.html")).toEqual(["1@68px", "2@108px"]);
    const { root: side, done } = mountIt(Sidebar, { threads, resolved, file: "about.html", now: new Date(), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
    expect(Array.from(side.querySelectorAll(".section-open .thread-num")).map(n => n.textContent)).toEqual(["1", "2"]);
    done();
  });

  it("pins an area thread at its drawn area's top right, labels it by its share, and reports hovers on pins and cards", () => {
    const area: Thread = { ...thread("a"), anchor: { ...anchor, kind: "area", selector: "main > section", quote: null, area: { x: 0.1, y: 0.2, w: 0.4213, h: 0.18 } } };
    const resolved = { a: { id: "a", found: true, method: "selector" as const, rect: { x: 100, y: 300, w: 200, h: 80 } } };
    const onHover = vi.fn();
    const { root, done } = mountIt(Pins, { threads: [area], resolved, onSelect: vi.fn(), onHover, width: 800 });
    const pin = root.querySelector<HTMLElement>("button.thread-pin")!;
    expect([pin.style.left, pin.style.top]).toEqual(["288px", "288px"]);
    pin.dispatchEvent(new MouseEvent("mouseenter"));
    expect(onHover).toHaveBeenLastCalledWith(area);
    pin.dispatchEvent(new MouseEvent("mouseleave"));
    expect(onHover).toHaveBeenLastCalledWith(null);
    done();
    const { root: side, done: doneSide } = mountIt(Sidebar, { threads: [area], resolved, now: new Date(), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn(), onHover });
    expect(side.querySelector(".anchor-label")!.textContent).toBe("Area in main > section (42% × 18%)");
    expect(areaLabel({ ...area.anchor, area: { x: 0, y: 0, w: 1, h: 0.004 } })).toBe("Area in main > section (100% × <1%)");
    const card = side.querySelector(".thread-card")!;
    onHover.mockClear();
    card.dispatchEvent(new MouseEvent("mouseenter"));
    expect(onHover).toHaveBeenLastCalledWith(area);
    card.dispatchEvent(new MouseEvent("mouseleave"));
    expect(onHover).toHaveBeenLastCalledWith(null);
    doneSide();
  });

  it("keeps a full-width region's pin inside the stage and clear of the frame's scrollbar", () => {
    const wide = { a: { id: "a", found: true, method: "exact" as const, rect: { x: 0, y: 40, w: 400, h: 20 } } };
    const { root, done } = mountIt(Pins, { threads: [thread("a")], resolved: wide, onSelect: vi.fn(), width: 400 });
    expect(root.querySelector<HTMLElement>("button.thread-pin")!.style.left).toBe(`${400 - PIN_RIGHT_ROOM}px`);
    done();
  });
});

describe("page-opened composers", () => {
  const d = (quote: string): Omit<Draft, "pickId"> => ({ anchor: { ...anchor, quote }, version: 1, clip: null });
  it("open fresh, refuse over typed text, and move with the text for a drawn area", () => {
    const open: Draft = { pickId: "p1", ...d("first") };
    expect(nextDraft(null, "", d("a"), undefined, () => "n")).toMatchObject({ pickId: "n", anchor: { quote: "a" } });
    expect(nextDraft(open, "  ", d("a"), undefined, () => "n")).toMatchObject({ pickId: "n" });
    expect(nextDraft(open, "typed", d("a"))).toBeNull();
    // Same pickId: the composer (and its text) is kept, on the new anchor.
    expect(nextDraft(open, "typed", d("b"), { area: true })).toMatchObject({ pickId: "p1", anchor: { quote: "b" } });
  });
  it("take a late clip only while the draft still waits for that token", () => {
    const waiting: Draft = { pickId: "p1", ...d("a"), capturing: true, clipToken: "t1" };
    const png = new Blob([new Uint8Array([1])]);
    expect(withClip(waiting, "t1", png)).toMatchObject({ clip: png, capturing: false, clipToken: undefined });
    expect(withClip(waiting, "t2", png)).toBe(waiting);
    expect(withClip(null, "t1", png)).toBeNull();
    const moved = { ...waiting, clipToken: "t3" };
    expect(withClip(moved, "t1", png)).toBe(moved);
  });
  it("say a screenshot is being taken", () => {
    const { root, done } = mountIt(Composer, { draft: { pickId: "p", ...d("a"), capturing: true }, onCancel: vi.fn(), onSubmit: vi.fn(async () => {}) });
    expect(root.textContent).toContain("Taking the screenshot…");
    done();
  });
});

describe("the submit shortcut", () => {
  const draft = (extra: Partial<Draft> = {}): Draft => ({ pickId: "p", anchor, version: 1, clip: null, ...extra });
  const key = (el: Element, init: KeyboardEventInit) => {
    const e = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...init });
    flush(() => { el.dispatchEvent(e); });
    return e;
  };
  function composer(extra: Partial<Draft> = {}, onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {})) {
    const m = mountIt(Composer, { draft: draft(extra), onCancel: vi.fn(), onSubmit });
    const ta = m.root.querySelector("textarea")!;
    const typeText = (v: string) => flush(() => { ta.value = v; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    return { ...m, ta, typeText, onSubmit };
  }

  it("is Enter with Cmd or Ctrl, outside an IME composition", () => {
    const k = { key: "Enter", metaKey: false, ctrlKey: false, isComposing: false };
    expect(isSubmitKey({ ...k, metaKey: true })).toBe(true);
    expect(isSubmitKey({ ...k, ctrlKey: true })).toBe(true);
    expect(isSubmitKey(k)).toBe(false);
    expect(isSubmitKey({ ...k, metaKey: true, isComposing: true })).toBe(false);
    expect(isSubmitKey({ ...k, key: "a", metaKey: true })).toBe(false);
    expect(submitKeysLabel("MacIntel")).toBe("⌘↵");
    expect(submitKeysLabel("Win32")).toBe("Ctrl+Enter");
    expect(submitKeysLabel("Linux x86_64")).toBe("Ctrl+Enter");
  });

  for (const mod of ["metaKey", "ctrlKey"] as const) {
    it(`posts the comment on ${mod === "metaKey" ? "Cmd" : "Ctrl"}+Enter, once`, () => {
      const { ta, typeText, onSubmit, done } = composer();
      typeText("Looks good.");
      const e = key(ta, { [mod]: true });
      expect(e.defaultPrevented).toBe(true);
      key(ta, { [mod]: true });
      expect(onSubmit).toHaveBeenCalledTimes(1);
      expect(onSubmit).toHaveBeenCalledWith("Looks good.");
      done();
    });
  }

  it("leaves plain Enter to the textarea", () => {
    const { ta, typeText, onSubmit, done } = composer();
    typeText("Line one");
    const e = key(ta, {});
    expect(e.defaultPrevented).toBe(false);
    expect(onSubmit).not.toHaveBeenCalled();
    done();
  });

  it("does nothing while Post is disabled: empty text, a screenshot being taken, or a post in flight", async () => {
    const empty = composer();
    empty.typeText("   ");
    key(empty.ta, { metaKey: true });
    expect(empty.onSubmit).not.toHaveBeenCalled();
    empty.done();

    const capturing = composer({ capturing: true });
    capturing.typeText("   ");
    key(capturing.ta, { ctrlKey: true });
    expect(capturing.root.textContent).toContain("Taking the screenshot…");
    expect(capturing.onSubmit).not.toHaveBeenCalled();
    capturing.done();

    let finish!: () => void;
    const slow = composer({}, vi.fn(() => new Promise<void>(r => { finish = r; })));
    slow.typeText("Slow.");
    key(slow.ta, { metaKey: true });
    expect(slow.root.querySelector<HTMLButtonElement>("button[type=submit]")!.disabled).toBe(true);
    key(slow.ta, { metaKey: true });
    flush(() => { slow.root.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    expect(slow.onSubmit).toHaveBeenCalledTimes(1);
    finish();
    slow.done();
  });

  it("posts on the shortcut pressed while the screenshot is taken once it is in, exactly the text shown then", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mountIt(Composer, { draft: d, onCancel: vi.fn(), onSubmit });
    const ta = m.root.querySelector("textarea")!;
    flush(() => { ta.value = "Why flat?"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    expect(key(ta, { metaKey: true }).defaultPrevented).toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();
    expect(m.root.querySelector<HTMLButtonElement>("button[type=submit]")!.getAttribute("aria-disabled")).toBe("true");
    expect(m.root.textContent).toContain("Posting once the screenshot is taken…");
    m.update({ draft: { ...d, capturing: false, clipToken: undefined, clipError: "blank" }, onCancel: vi.fn(), onSubmit });
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith("Why flat?");
    expect(ta.value).toBe("Why flat?");
    m.done();
  });

  it("drops a queued shortcut when the text is edited after it, so a half-finished edit is never posted", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mountIt(Composer, { draft: d, onCancel: vi.fn(), onSubmit });
    const ta = m.root.querySelector("textarea")!;
    flush(() => { ta.value = "Why flat"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    key(ta, { ctrlKey: true });
    expect(m.root.textContent).toContain("Posting once the screenshot is taken…");
    flush(() => { ta.value = "Why flat, and why"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    expect(m.root.textContent).toContain("Taking the screenshot…");
    m.update({ draft: { ...d, capturing: false, clipToken: undefined, clipError: "blank" }, onCancel: vi.fn(), onSubmit });
    expect(onSubmit).not.toHaveBeenCalled();
    expect(m.root.querySelector<HTMLButtonElement>("button[type=submit]")!.disabled).toBe(false);
    m.done();
  });

  it("drops a queued shortcut when the composer moves to another anchor", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mountIt(Composer, { draft: d, onCancel: vi.fn(), onSubmit });
    const ta = m.root.querySelector("textarea")!;
    flush(() => { ta.value = "Here"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    key(ta, { metaKey: true });
    expect(m.root.textContent).toContain("Posting once the screenshot is taken…");
    const moved = { ...d, anchor: { ...d.anchor, selector: "body > p" }, clipToken: "t2" };
    m.update({ draft: moved, onCancel: vi.fn(), onSubmit });
    expect(m.root.textContent).toContain("Taking the screenshot…");
    m.update({ draft: { ...moved, capturing: false, clipToken: undefined, clipError: "blank" }, onCancel: vi.fn(), onSubmit });
    expect(onSubmit).not.toHaveBeenCalled();
    expect(ta.value).toBe("Here");
    m.done();
  });

  it("drops a queued shortcut when the composer moves to another anchor with its screenshot already settled", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mountIt(Composer, { draft: d, onCancel: vi.fn(), onSubmit });
    const ta = m.root.querySelector("textarea")!;
    flush(() => { ta.value = "Here"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    key(ta, { metaKey: true });
    const moved = { ...d, anchor: { ...d.anchor, selector: "body > p" }, capturing: undefined, clipToken: undefined, clipError: "anchored by the page" };
    m.update({ draft: moved, onCancel: vi.fn(), onSubmit });
    m.update({ draft: moved, onCancel: vi.fn(), onSubmit });
    expect(onSubmit).not.toHaveBeenCalled();
    expect(ta.value).toBe("Here");
    m.done();
  });

  it("keeps Post focusable while the screenshot is taken, and a click or Enter on it queues the post like the shortcut", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mountIt(Composer, { draft: d, onCancel: vi.fn(), onSubmit });
    const ta = m.root.querySelector("textarea")!;
    const postButton = m.root.querySelector<HTMLButtonElement>("button[type=submit]")!;
    flush(() => { ta.value = "Ship it"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    expect(postButton.disabled).toBe(false);
    expect(postButton.getAttribute("aria-disabled")).toBe("true");
    postButton.focus();
    expect(document.activeElement).toBe(postButton);
    // A click (Enter on a focused button clicks it) submits the form.
    flush(() => { postButton.click(); });
    expect(onSubmit).not.toHaveBeenCalled();
    expect(m.root.textContent).toContain("Posting once the screenshot is taken…");
    m.update({ draft: { ...d, capturing: false, clipToken: undefined, clipError: "blank" }, onCancel: vi.fn(), onSubmit });
    expect(onSubmit).toHaveBeenCalledWith("Ship it");
    expect(postButton.getAttribute("aria-disabled")).toBeNull();
    m.done();
  });

  it("describes the waiting Post by the status line, which says when a post is queued", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mountIt(Composer, { draft: d, onCancel: vi.fn(), onSubmit });
    const postButton = m.root.querySelector<HTMLButtonElement>("button[type=submit]")!;
    const status = m.root.querySelector<HTMLElement>("[role=status]")!;
    expect(status.textContent).toBe("Taking the screenshot…");
    expect(status.id).not.toBe("");
    expect(postButton.getAttribute("aria-describedby")).toBe(status.id);
    const ta = m.root.querySelector("textarea")!;
    flush(() => { ta.value = "Ship it"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => { postButton.click(); });
    expect(m.root.querySelector("[role=status]")).toBe(status);
    expect(status.textContent).toBe("Posting once the screenshot is taken…");
    m.update({ draft: { ...d, capturing: false, clipToken: undefined, clipError: "blank" }, onCancel: vi.fn(), onSubmit });
    expect(m.root.querySelector("[role=status]")!.textContent).toBe("No screenshot: blank");
    expect(postButton.hasAttribute("aria-describedby")).toBe(false);
    m.done();
  });

  it("ignores the shortcut during an IME composition", () => {
    const { ta, typeText, onSubmit, done } = composer();
    typeText("変換中");
    key(ta, { metaKey: true, isComposing: true });
    expect(onSubmit).not.toHaveBeenCalled();
    done();
  });

  it("names the shortcut in Post's tooltip", () => {
    const { root, done } = composer();
    expect(root.querySelector("button[type=submit]")!.getAttribute("title")).toBe(`Post comment (${submitKeysLabel()})`);
    done();
  });

  it("sends a sidebar reply on Cmd+Enter or Ctrl+Enter, never an empty one or mid-composition", () => {
    const onReply = vi.fn();
    const { root, done } = mountIt(Sidebar, { threads: [thread("a")], resolved: {}, now: new Date(), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply });
    const input = root.querySelector<HTMLInputElement>("input[aria-label=Reply]")!;
    const typeText = (v: string) => flush(() => { input.value = v; input.dispatchEvent(new Event("input", { bubbles: true })); });
    key(input, { metaKey: true });
    typeText("First reply.");
    key(input, { metaKey: true, isComposing: true });
    expect(onReply).not.toHaveBeenCalled();
    expect(key(input, { metaKey: true }).defaultPrevented).toBe(true);
    expect(onReply).toHaveBeenLastCalledWith(expect.objectContaining({ id: "a" }), "First reply.");
    expect(input.value).toBe("");
    typeText("Second reply.");
    key(input, { ctrlKey: true });
    expect(onReply).toHaveBeenLastCalledWith(expect.objectContaining({ id: "a" }), "Second reply.");
    expect(onReply).toHaveBeenCalledTimes(2);
    done();
  });
});
