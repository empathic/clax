import { describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { keyboardTrail } from "../view/trail";
import QuestionCard from "./QuestionCard.svelte";
import { view } from "./fixtures";

const buttons = (root: Element, text: string) => [...root.querySelectorAll("button")].filter(b => b.textContent === text);
/** A trusted pointer's click (`detail` 1), or Enter or Space on a button (`detail` 0). */
const click = (el: Element, detail = 1) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail })); });
const key = (el: Element, k: string, init: KeyboardEventInit = {}) =>
  flush(() => { dispatchTrusted(el, new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init })); });
const type = (el: HTMLInputElement | HTMLTextAreaElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input")); });

describe("QuestionCard", () => {
  it("answers every kind and enables Answer only when complete", async () => {
    const onAnswer = vi.fn(async () => {});
    const m = mount(QuestionCard, { q: view(), onAnswer, onDecline: vi.fn(async () => {}) });
    const answer = () => buttons(m.root, "Answer claude")[0] as HTMLButtonElement;
    expect(answer().disabled).toBe(true);
    click(m.root.querySelector<HTMLInputElement>('input[value="Two"]')!);
    click(m.root.querySelectorAll<HTMLElement>(".chip")[1]);
    click(m.root.querySelector<HTMLInputElement>('input[value="Right"]')!);
    click(m.root.querySelectorAll<HTMLElement>(".chip")[2]);
    const ta = m.root.querySelector("textarea")!; ta.value = "ok"; ta.dispatchEvent(new Event("input"));
    flush();
    expect(answer().disabled).toBe(false);
    expect([...m.root.querySelectorAll(".chip.done")]).toHaveLength(3);
    click(answer());
    await flush();
    expect(onAnswer).toHaveBeenCalledWith({ answers: [{ selected: ["Two"], text: null }, { selected: ["Right"], text: null }, { selected: [], text: "ok" }] });
    m.unmount();
  });

  it("renders hostile strings as text", () => {
    const evil = "<img src=x onerror=alert(1)>\u202eevil";
    const q = view({ questions: [{ question: evil, header: evil, options: [{ label: evil, description: evil, preview: evil + "x".repeat(20000) }, { label: "b" }], multi_select: false, other: true }] });
    const m = mount(QuestionCard, { q, onAnswer: vi.fn(), onDecline: vi.fn() });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.querySelector("pre")!.textContent!.startsWith(evil)).toBe(true);
    expect(m.root.textContent).toContain("<img src=x onerror=alert(1)>");
    m.unmount();
    const a = view({ status: "answered", answers: [{ selected: [], text: evil }, { selected: ["Left"], text: null }, { selected: [], text: evil }] });
    const m2 = mount(QuestionCard, { q: a, onAnswer: vi.fn(), onDecline: vi.fn() });
    expect(m2.root.querySelector("img")).toBeNull();
    expect(m2.root.querySelector(".answers")!.textContent).toContain(`Other: ${evil}`);
    m2.unmount();
  });

  it("offers Answer in the terminal only on a mirrored question, and shows a closed state", async () => {
    const onRelease = vi.fn(async () => {});
    const m = mount(QuestionCard, { q: view({ source: "hook" }), onAnswer: vi.fn(), onDecline: vi.fn(), onRelease });
    expect(buttons(m.root, "Answer in the terminal")).toHaveLength(1);
    expect(m.root.querySelectorAll(".rec")).toHaveLength(1);
    click(buttons(m.root, "Answer in the terminal")[0]);
    await flush();
    expect(onRelease).toHaveBeenCalledOnce();
    m.unmount();
    const m1 = mount(QuestionCard, { q: view(), onAnswer: vi.fn(), onDecline: vi.fn(), onRelease });
    expect(buttons(m1.root, "Answer in the terminal")).toHaveLength(0);
    m1.unmount();
    const mirrored = view({ source: "hook", questions: [{ ...view().questions[0], options: [{ label: "Vitest (Recommended)", recommended: true }, { label: "Jest" }] }] });
    const onAnswer = vi.fn(async () => {});
    const m3 = mount(QuestionCard, { q: mirrored, onAnswer, onDecline: vi.fn() });
    // The chip says it once; the stored label still keys the answer.
    expect(m3.root.querySelectorAll(".rec")).toHaveLength(1);
    expect(m3.root.querySelector(".lbl")!.textContent).toBe("Vitest");
    click(m3.root.querySelector('input[value="Vitest (Recommended)"]')!);
    click(buttons(m3.root, "Answer claude")[0]);
    await flush();
    expect(onAnswer).toHaveBeenCalledWith({ answers: [{ selected: ["Vitest (Recommended)"], text: null }] });
    m3.update({ q: { ...mirrored, status: "answered", answered_via: "shell", answers: [{ selected: ["Vitest (Recommended)"], text: null }] }, onAnswer, onDecline: vi.fn() });
    expect(m3.root.querySelector(".answers .a")!.textContent).toBe("Vitest");
    m3.unmount();
    const m2 = mount(QuestionCard, { q: view({ status: "released", source: "hook" }), onAnswer: vi.fn(), onDecline: vi.fn() });
    expect(m2.root.querySelector(".closed")!.textContent).toBe("Moved to the terminal");
    expect(buttons(m2.root, "Answer claude")).toHaveLength(0);
    m2.unmount();
  });

  it("shows the answers of an answered question, and leaves out the link to the page it is on", () => {
    const q = view({ status: "answered", answered_via: "shell", answers: [{ selected: ["Two"], text: null }, { selected: ["Left", "Right"], text: null }, { selected: [], text: "ship it" }] });
    const m = mount(QuestionCard, { q, here: "7q3k9mzx2b4t", onAnswer: vi.fn(), onDecline: vi.fn() });
    expect(m.root.querySelector(".closed")!.textContent).toBe("Answered");
    expect([...m.root.querySelectorAll(".answers .a")].map(d => d.textContent)).toEqual(["Two", "Left, Right", "ship it"]);
    expect(m.root.querySelector("a.about")).toBeNull();
    m.unmount();
    const m2 = mount(QuestionCard, { q: view(), onAnswer: vi.fn(), onDecline: vi.fn() });
    expect(m2.root.querySelector("a.about")!.getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    m2.unmount();
  });

  it("follows the keys: digits pick, arrows move between options, Enter answers once complete", async () => {
    const onAnswer = vi.fn(async () => {});
    const q = view({ questions: [view().questions[0]] });
    const m = mount(QuestionCard, { q, onAnswer, onDecline: vi.fn() });
    const radio = (l: string) => m.root.querySelector<HTMLInputElement>(`input[value="${l}"]`)!;
    radio("Two").focus();
    key(radio("Two"), "Enter");
    expect(onAnswer).not.toHaveBeenCalled();
    key(radio("Two"), "2");
    expect(radio("One").checked).toBe(true);
    expect(document.activeElement).toBe(radio("One"));
    expect(m.root.querySelector("pre")!.textContent).toBe("|ab|");
    key(radio("One"), "ArrowDown");
    expect(radio("Two").checked).toBe(true);
    expect(document.activeElement).toBe(radio("Two"));
    // Digits typed into Other are text.
    const other = m.root.querySelector<HTMLInputElement>('.other input')!;
    key(other, "1");
    expect(radio("Two").checked).toBe(true);
    key(radio("Two"), "Enter");
    await flush();
    expect(onAnswer).toHaveBeenCalledWith({ answers: [{ selected: ["Two"], text: null }] });
    m.unmount();
  });

  it("asks for a click when the keys may have been the page's", async () => {
    const onAnswer = vi.fn(async () => {});
    const onDecline = vi.fn(async () => {});
    const q = view({ questions: [view().questions[2]] });
    const m = mount(QuestionCard, { q, onAnswer, onDecline });
    const hint = m.root.querySelector(".act-hint")!;
    expect(hint.getAttribute("role")).toBe("status");
    keyboardTrail.taint();
    try {
      const ta = m.root.querySelector("textarea")!;
      type(ta, "pwd hunter2");
      key(ta, "Enter");
      expect(onAnswer).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to answer");
      // Enter or Space on a button clicks with detail 0.
      click(buttons(m.root, "Answer claude")[0], 0);
      click(buttons(m.root, "Skip")[0], 0);
      expect(onAnswer).not.toHaveBeenCalled();
      expect(onDecline).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to skip");
      // The hint goes once the trail clears; a pointer's click acts.
      flush(() => keyboardTrail.clear());
      expect(hint.textContent).toBe("");
      keyboardTrail.taint();
      click(buttons(m.root, "Answer claude")[0]);
      await flush();
      expect(onAnswer).toHaveBeenCalledWith({ answers: [{ selected: [], text: "pwd hunter2" }] });
    } finally {
      keyboardTrail.clear();
      m.unmount();
    }
  });

  it("says when an action failed and lets it be tried again", async () => {
    let fail = true;
    const onDecline = vi.fn(async () => { if (fail) throw new Error("down"); });
    const m = mount(QuestionCard, { q: view(), onAnswer: vi.fn(), onDecline });
    click(buttons(m.root, "Skip")[0]);
    await vi.waitFor(() => expect(m.root.querySelector(".act-hint")!.textContent).toBe("Not sent: Clax did not answer. Try again."));
    flush();
    expect((buttons(m.root, "Skip")[0] as HTMLButtonElement).disabled).toBe(false);
    fail = false;
    click(buttons(m.root, "Skip")[0]);
    await flush();
    expect(onDecline).toHaveBeenCalledTimes(2);
    m.unmount();
  });

  it("keeps focus on a card that closes while it holds focus, even when the closing control loses it first", async () => {
    let release!: () => void;
    const onRelease = vi.fn(() => new Promise<void>(r => { release = r; }));
    const props = { q: view({ source: "hook" }), onAnswer: vi.fn(), onDecline: vi.fn(), onRelease };
    const m = mount(QuestionCard, props);
    const article = m.root.querySelector("article")!;
    const b = buttons(m.root, "Answer in the terminal")[0];
    b.focus();
    click(b);
    // The pending action disables the button, and the browser drops its focus to nowhere.
    b.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: null }));
    (document.activeElement as HTMLElement).blur();
    await Promise.resolve();
    release();
    await flush();
    m.update({ ...props, q: view({ source: "hook", status: "released" }) });
    expect(document.activeElement).toBe(article);
    m.unmount();
  });

  it("leaves focus alone when it had left the card for the page", async () => {
    const props = { q: view(), onAnswer: vi.fn(), onDecline: vi.fn() };
    const m = mount(QuestionCard, props);
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const r = m.root.querySelector<HTMLInputElement>('input[value="Two"]')!;
    r.focus();
    outside.focus();
    await Promise.resolve();
    m.update({ ...props, q: view({ status: "withdrawn" }) });
    expect(document.activeElement).toBe(outside);
    outside.remove();
    m.unmount();
  });

  it("announces a close while mounted, says when the person's action was not taken, and stays quiet when mounted closed", async () => {
    const status = (m: { root: HTMLElement }) => m.root.querySelector('[role="status"]')!;
    const quiet = mount(QuestionCard, { q: view({ status: "answered", answered_via: "shell" }), onAnswer: vi.fn(), onDecline: vi.fn() });
    expect(status(quiet).textContent).toBe("");
    quiet.unmount();
    // Closed by the person's own Skip.
    const props = { q: view(), onAnswer: vi.fn(), onDecline: vi.fn(async () => {}) };
    const m = mount(QuestionCard, props);
    const region = status(m);
    click(buttons(m.root, "Skip")[0]);
    await flush();
    m.update({ ...props, q: view({ status: "declined" }) });
    expect(status(m)).toBe(region);
    expect(region.textContent).toBe("Skipped");
    expect(region.classList.contains("sr")).toBe(true);
    m.unmount();
    // Closed by something else first (a 409): the answer was not taken, and the card says so where it is seen.
    const q = view({ agent: { handle: "a_de2252", harness: "codex", project: "sales" }, questions: [view().questions[2]] });
    const p2 = { q, onAnswer: vi.fn(async () => {}), onDecline: vi.fn() };
    const m2 = mount(QuestionCard, p2);
    type(m2.root.querySelector("textarea")!, "Nothing here yet.");
    click(buttons(m2.root, "Answer codex")[0]);
    await flush();
    m2.update({ ...p2, q: { ...q, status: "withdrawn" } });
    expect(status(m2).textContent).toBe("Not answered: codex stopped waiting");
    expect(status(m2).classList.contains("sr")).toBe(false);
    // Closed without any action of the person's: just what happened.
    const p3 = { q: view(), onAnswer: vi.fn(), onDecline: vi.fn() };
    const m3 = mount(QuestionCard, p3);
    m3.update({ ...p3, q: view({ status: "answered", answered_via: "terminal" }) });
    expect(status(m3).textContent).toBe("Answered in the terminal");
    m2.unmount(); m3.unmount();
  });

  it("says the daemon's reason for a refused action", async () => {
    const { ApiError } = await import("../api");
    const onAnswer = vi.fn(async () => { throw new ApiError(400, "\"Notes\": a free-text question takes text only", "invalid_answer"); });
    const q = view({ questions: [view().questions[2]] });
    const m = mount(QuestionCard, { q, onAnswer, onDecline: vi.fn() });
    type(m.root.querySelector("textarea")!, "x");
    click(buttons(m.root, "Answer claude")[0]);
    await vi.waitFor(() => expect(m.root.querySelector(".act-hint")!.textContent).toBe("Not sent: \"Notes\": a free-text question takes text only"));
    m.unmount();
  });

  it("names each tab by its whole header and whether it is answered, and shows the selected preview once focus leaves the options", () => {
    const long = "A very long header";
    const q = view({ questions: [{ ...view().questions[0], header: long }, { ...view().questions[1], header: `${long} 2` }] });
    const m = mount(QuestionCard, { q, onAnswer: vi.fn(), onDecline: vi.fn() });
    const tabs = [...m.root.querySelectorAll('[role="tab"]')];
    expect(tabs.map(t => t.getAttribute("aria-label"))).toEqual([long, `${long} 2`]);
    expect(tabs[0].getAttribute("title")).toBe(long);
    click(m.root.querySelector('input[value="One"]')!);
    expect(m.root.querySelector('[role="tab"]')!.getAttribute("aria-label")).toBe(`${long}, answered`);
    const two = m.root.querySelector<HTMLInputElement>('input[value="Two"]')!;
    flush(() => { two.focus(); });
    expect(m.root.querySelector("pre")!.textContent).toBe("|a|b|");
    const answer = buttons(m.root, "Answer claude")[0];
    // As the browser does when Tab moves on (jsdom's focus() fires no focusout).
    flush(() => { answer.focus(); two.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: answer })); });
    expect(m.root.querySelector("pre")!.textContent).toBe("|ab|");
    m.unmount();
  });
});
