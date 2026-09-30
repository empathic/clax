import { render } from "preact";
import { act } from "preact/test-utils";
import { describe, expect, it, vi } from "vitest";
import { Composer, type Draft, MAX_EARLY_KEYS, PIN_RIGHT_ROOM, Pins, earlyKeys, isSubmitKey, nextDraft, submitKeysLabel, typeKeys, withClip, withEarly } from "./comments";
import { Sidebar } from "./sidebar";
import { type Thread, areaLabel } from "./threads";

const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const thread = (id: string, status: "open" | "resolved" = "open"): Thread => ({
  id, artifact_id: "7q3k9mzx2b4t", version_n: 1, anchor, status, sent_to_agent: false, has_clip: false, clip_url: null,
  created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "2026-09-29T10:00:00.000Z" }],
});
const at = (id: string, y: number) => ({ id, found: true, method: "exact" as const, rect: { x: 10, y, w: 100, h: 20 } });

function mount(node: preact.ComponentChild) {
  const root = document.createElement("div");
  document.body.appendChild(root);
  render(node, root);
  return { root, done: () => { render(null, root); root.remove(); } };
}

describe("Pins", () => {
  it("numbers pins like the sidebar's Open section and skips detached, resolved, unmeasured, and scrolled-away threads", () => {
    const threads = [thread("a"), thread("b"), thread("c"), thread("d"), thread("e", "resolved"), thread("f")];
    const resolved = { a: at("a", 40), b: { id: "b", found: false, method: null, rect: null }, c: at("c", -50), e: at("e", 40), f: at("f", 200) };
    const { root, done } = mount(<Pins threads={threads} resolved={resolved} onSelect={vi.fn()} />);
    // Open and not detached: a (1), c (2), d (3, not measured yet), f (4).
    expect(Array.from(root.querySelectorAll("button.thread-pin")).map(b => b.textContent)).toEqual(["1", "4"]);
    const { root: side, done: doneSide } = mount(<Sidebar threads={threads} resolved={resolved} now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />);
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
      const { root, done } = mount(<Pins threads={threads} resolved={resolved} file={file} onSelect={vi.fn()} width={800} />);
      const out = Array.from(root.querySelectorAll<HTMLElement>("button.thread-pin")).map(b => `${b.textContent}@${b.style.top}`);
      done();
      return out;
    };
    expect(pins("index.html")).toEqual(["1@28px"]);
    expect(pins("about.html")).toEqual(["1@68px", "2@108px"]);
    const { root: side, done } = mount(<Sidebar threads={threads} resolved={resolved} file="about.html" now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />);
    expect(Array.from(side.querySelectorAll(".section-open .thread-num")).map(n => n.textContent)).toEqual(["1", "2"]);
    done();
  });

  it("pins an area thread at its drawn area's top right, labels it by its share, and reports hovers on pins and cards", () => {
    const area: Thread = { ...thread("a"), anchor: { ...anchor, kind: "area", selector: "main > section", quote: null, area: { x: 0.1, y: 0.2, w: 0.4213, h: 0.18 } } };
    const resolved = { a: { id: "a", found: true, method: "selector" as const, rect: { x: 100, y: 300, w: 200, h: 80 } } };
    const onHover = vi.fn();
    const { root, done } = mount(<Pins threads={[area]} resolved={resolved} onSelect={vi.fn()} onHover={onHover} width={800} />);
    const pin = root.querySelector<HTMLElement>("button.thread-pin")!;
    expect([pin.style.left, pin.style.top]).toEqual(["288px", "288px"]);
    pin.dispatchEvent(new MouseEvent("mouseenter"));
    expect(onHover).toHaveBeenLastCalledWith(area);
    pin.dispatchEvent(new MouseEvent("mouseleave"));
    expect(onHover).toHaveBeenLastCalledWith(null);
    done();
    const { root: side, done: doneSide } = mount(<Sidebar threads={[area]} resolved={resolved} now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} onHover={onHover} />);
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
    const { root, done } = mount(<Pins threads={[thread("a")]} resolved={wide} onSelect={vi.fn()} width={400} />);
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
    const { root, done } = mount(<Composer draft={{ pickId: "p", ...d("a"), capturing: true }} onCancel={vi.fn()} onSubmit={vi.fn(async () => {})} />);
    expect(root.textContent).toContain("Taking the screenshot…");
    done();
  });
});

describe("keys typed in the page before the composer had focus", () => {
  const d = (quote: string): Omit<Draft, "pickId"> => ({ anchor: { ...anchor, quote }, version: 1, clip: null });
  it("are taken only as characters, newlines and Backspace, at most the room left", () => {
    expect(earlyKeys(["V", "i", "a", " ", "é", "😀", "\n", "Backspace"], 100)).toEqual(["V", "i", "a", " ", "é", "😀", "\n", "Backspace"]);
    expect(earlyKeys(["a", "b", "c"], 2)).toEqual(["a", "b"]);
    expect(earlyKeys(["a"], 0)).toEqual([]);
    for (const bad of [["ab"], ["\u0007"], ["\r"], ["Enter"], ["Tab"], [5], [""], "abc", null, { 0: "a" }, Array(MAX_EARLY_KEYS + 1).fill("a")]) expect(earlyKeys(bad, 100)).toBeNull();
  });
  it("are typed at the end of the text, Backspace removing a whole character", () => {
    expect(typeKeys("", ["V", "i", "a"])).toBe("Via");
    expect(typeKeys("Vi", ["x", "Backspace", "a", "\n"])).toBe("Via\n");
    expect(typeKeys("a😀", ["Backspace"])).toBe("a");
    expect(typeKeys("", ["Backspace"])).toBe("");
  });
  it("go only to the composer for their pick, until it has all of them", () => {
    const open: Draft = { pickId: "p1", ...d("a"), early: { keys: ["V"], done: false } };
    expect(withEarly(open, "p1", ["i"], false)!.early).toEqual({ keys: ["V", "i"], done: false });
    expect(withEarly(open, "p1", [], true)!.early).toEqual({ keys: ["V"], done: true });
    expect(withEarly(open, "p2", ["i"], false)).toBe(open);
    expect(withEarly(null, "p1", ["i"], false)).toBeNull();
    const done = { ...open, early: { keys: ["V"], done: true } };
    expect(withEarly(done, "p1", ["i"], false)).toBe(done);
    const pageOpened: Draft = { pickId: "p1", ...d("a") };
    expect(withEarly(pageOpened, "p1", ["i"], false)).toBe(pageOpened);
  });
  it("come before keys typed in the composer while they arrive, and none are lost", async () => {
    const draft: Draft = { pickId: "p1", ...d("a"), capturing: true, clipToken: "p1", early: { keys: [], done: false } };
    const onText = vi.fn();
    const root = document.createElement("div");
    document.body.appendChild(root);
    const show = (dr: Draft) => act(() => { render(<Composer draft={dr} onCancel={vi.fn()} onSubmit={vi.fn(async () => {})} onText={onText} />, root); });
    show(draft);
    const textarea = root.querySelector("textarea")!;
    expect(document.activeElement).toBe(textarea);
    show({ ...draft, early: { keys: ["V", "i"], done: false } });
    expect(textarea.value).toBe("Vi");
    // Typed in the composer before the page's last keys are in: held.
    for (const key of ["t", "h"]) {
      const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      textarea.dispatchEvent(e);
      expect(e.defaultPrevented).toBe(true);
    }
    // A shortcut is not text: it is not held.
    const cut = new KeyboardEvent("keydown", { key: "x", metaKey: true, bubbles: true, cancelable: true });
    textarea.dispatchEvent(cut);
    expect(cut.defaultPrevented).toBe(false);
    show({ ...draft, early: { keys: ["V", "i", "a", " "], done: false } });
    expect(textarea.value).toBe("Via ");
    show({ ...draft, early: { keys: ["V", "i", "a", " "], done: true } });
    expect(textarea.value).toBe("Via th");
    expect(onText).toHaveBeenLastCalledWith("Via th");
    // Once all are in, keys typed here go straight in.
    const e = new KeyboardEvent("keydown", { key: "e", bubbles: true, cancelable: true });
    textarea.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
    act(() => { render(null, root); });
    root.remove();
  });
  it("post on the shortcut pressed while keys are held, once every key is in", () => {
    const draft: Draft = { pickId: "p1", ...d("a"), early: { keys: ["O"], done: false } };
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const root = document.createElement("div");
    document.body.appendChild(root);
    const show = (dr: Draft) => act(() => { render(<Composer draft={dr} onCancel={vi.fn()} onSubmit={onSubmit} />, root); });
    show(draft);
    const textarea = root.querySelector("textarea")!;
    act(() => { textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "k", bubbles: true, cancelable: true })); });
    const submit = new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true });
    act(() => { textarea.dispatchEvent(submit); });
    expect(submit.defaultPrevented).toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();
    show({ ...draft, early: { keys: ["O"], done: true } });
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith("Ok");
    act(() => { render(null, root); });
    root.remove();
  });
  it("hold nothing in a composer the page opened", () => {
    const root = document.createElement("div");
    document.body.appendChild(root);
    act(() => { render(<Composer draft={{ pickId: "p", ...d("a") }} onCancel={vi.fn()} onSubmit={vi.fn(async () => {})} />, root); });
    const e = new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true });
    root.querySelector("textarea")!.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
    act(() => { render(null, root); });
    root.remove();
  });
});

describe("the submit shortcut", () => {
  const draft = (extra: Partial<Draft> = {}): Draft => ({ pickId: "p", anchor, version: 1, clip: null, ...extra });
  const key = (el: Element, init: KeyboardEventInit) => {
    const e = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...init });
    act(() => { el.dispatchEvent(e); });
    return e;
  };
  function composer(extra: Partial<Draft> = {}, onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {})) {
    const m = mount(<Composer draft={draft(extra)} onCancel={vi.fn()} onSubmit={onSubmit} />);
    const ta = m.root.querySelector("textarea")!;
    const typeText = (v: string) => act(() => { ta.value = v; ta.dispatchEvent(new Event("input", { bubbles: true })); });
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
    act(() => { slow.root.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    expect(slow.onSubmit).toHaveBeenCalledTimes(1);
    finish();
    slow.done();
  });

  it("posts on the shortcut pressed while the screenshot is taken once it is in, with the text as it is then", () => {
    const onSubmit = vi.fn<(body: string) => Promise<void>>(async () => {});
    const d = draft({ capturing: true, clipToken: "t" });
    const m = mount(<Composer draft={d} onCancel={vi.fn()} onSubmit={onSubmit} />);
    const ta = m.root.querySelector("textarea")!;
    act(() => { ta.value = "Why flat"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    expect(key(ta, { metaKey: true }).defaultPrevented).toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();
    expect(m.root.querySelector<HTMLButtonElement>("button[type=submit]")!.disabled).toBe(true);
    expect(m.root.textContent).toContain("Posting once the screenshot is taken…");
    act(() => { ta.value = "Why flat?"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    act(() => { render(<Composer draft={{ ...d, capturing: false, clipToken: undefined, clipError: "blank" }} onCancel={vi.fn()} onSubmit={onSubmit} />, m.root); });
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith("Why flat?");
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
    const { root, done } = mount(<Sidebar threads={[thread("a")]} resolved={{}} now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={onReply} />);
    const input = root.querySelector<HTMLInputElement>("input[aria-label=Reply]")!;
    const typeText = (v: string) => act(() => { input.value = v; input.dispatchEvent(new Event("input", { bubbles: true })); });
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
