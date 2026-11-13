import { describe, expect, it, vi } from "vitest";
import type { Version } from "./api";
import Sidebar from "./ui/Sidebar.svelte";
import { flush, mount } from "./test/svelte";
import type { Comment, Thread, Viewer } from "./threads";
import { SEEN_AFTER_MS } from "./view/sidebar-model";

// Local times, so the days fall where a viewer's clock puts them.
const at = (day: number, h: number, m = 0) => new Date(2026, 10, day, h, m).toISOString();
const NOW = new Date(2026, 10, 13, 12, 0);
const me: Viewer = { public_id: "p_me", display_name: "Alex", created_at: at(1, 9) };
const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const comment = (id: string, kind: "viewer" | "agent", when: string, body: string, who = kind === "agent" ? "claude" : "Mia"): Comment => ({
  id, thread_id: id.split(".")[0], author_kind: kind, author_name: who, author_public_id: kind === "viewer" ? (who === "Alex" ? "p_me" : `p_${who}`) : null,
  via_harness: kind === "agent" ? "claude" : null, body, created_at: when,
});
const version = (n: number, when: string, addresses: string[] = []): Version =>
  ({ artifact_id: "7q3k9mzx2b4t", n, label: null, created_at: when, files: {}, agent_harness: "claude", addresses });

// Which rows are open and which days unfolded outlive a sidebar (for the
// page's life): each test has threads, and days, of its own.
let made = 0;
const thread = (name: string, when: string, body: string, o: Partial<Thread> = {}): Thread => {
  const id = `${name}${made}`;
  return { id, artifact_id: "7q3k9mzx2b4t", version_n: 1, anchor, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: when,
    resolved_at: null, resolved_by: null, feedback_state: null, comments: [comment(`${id}.1`, "viewer", when, body)], ...o };
};
const resolvedBy = (t: Thread, when: string): Thread => ({ ...t, status: "resolved", resolved_at: when, resolved_by: "viewer:p_me" });

// Today: v1, Mia's unsent comment (a), the viewer's comment (b) sent in a
// batch, and v2, which addressed b.
function today() {
  made++;
  const a = thread("a", at(13, 9, 10), "Shorter headline");
  const b = thread("b", at(13, 9, 20), "Starter should be $9", { sent_to_agent: true, addressed_in: [2],
    comments: [comment(`b${made}.1`, "viewer", at(13, 9, 20), "Starter should be $9", "Alex"), comment(`b${made}.2`, "agent", at(13, 9, 50), "Done")],
    sends: [{ batch_id: `k${made}`, size: 1, note: null, sent_by: "Alex", sent_at: at(13, 9, 30) }] });
  return { a, b, versions: [version(1, at(13, 9)), version(2, at(13, 10), [b.id])] };
}

function setup(more: Record<string, unknown> = {}) {
  const { a, b, versions } = today();
  const calls = { select: vi.fn(), change: vi.fn(), seen: vi.fn() };
  const props = { versions, shown: 2, agent: "claude", threads: [a, b], resolved: {}, now: NOW, selected: null as string | null, me,
    onSelect: calls.select, onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn(), onSeen: calls.seen,
    view: { now: "timeline" as "timeline" | "threads", onChange: calls.change }, ...more };
  const view = mount(Sidebar, props);
  const root = view.root;
  const row = (t: Thread) => root.querySelector<HTMLButtonElement>(`button.tl-row[data-row="${t.id}"]`);
  const card = (t: Thread) => root.querySelector(`.thread-card[data-thread="${t.id}"]`);
  return { view, root, props, calls, row, card, a, b };
}
const loaded = (root: HTMLElement) => vi.waitFor(() => expect(root.querySelector('[role="tablist"]')).not.toBeNull());
const texts = (root: HTMLElement) => Array.from(root.querySelectorAll(".tl-line .tl-text"), e => e.textContent);

describe("Sidebar views", () => {
  it("without a view, is the thread list alone, with no tabs", () => {
    const { view, root, a, b } = setup({ view: undefined });
    expect(root.querySelector('[role="tablist"]')).toBeNull();
    expect(root.querySelector(".timeline")).toBeNull();
    expect(Array.from(root.querySelectorAll("h2"), h => h.querySelector(".t")!.textContent)).toEqual(["Open", "Detached", "Resolved"]);
    expect(Array.from(root.querySelectorAll(".section-open .thread-card"), c => c.getAttribute("data-thread"))).toEqual([b.id, a.id]);
    view.unmount();
  });

  it("in the Threads view, leads the list with the tabs and shows today's sections", async () => {
    const { view, root, a, b } = setup({ view: { now: "threads", onChange: vi.fn() }, onSendUnsent: vi.fn() });
    await loaded(root);
    const tabs = Array.from(root.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
    expect(tabs.map(t => [t.textContent, t.getAttribute("aria-selected")])).toEqual([["Timeline", "false"], ["Threads", "true"]]);
    expect(root.querySelector(".timeline")).toBeNull();
    expect(Array.from(root.querySelectorAll(".section-open .thread-card"), c => c.getAttribute("data-thread"))).toEqual([b.id, a.id]);
    // The tabs sit below what stays in both views, directly above the list.
    const panel = root.querySelector('[role="tabpanel"]')!;
    expect(panel.getAttribute("aria-labelledby")).toBe(tabs[1].id);
    expect(root.querySelector('[role="tablist"]')!.parentElement!.nextElementSibling).toBe(panel);
    expect(root.querySelector(".send-unsent")!.compareDocumentPosition(root.querySelector(".sb-tabs")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(panel.querySelector(".section-open")).not.toBeNull();
    view.unmount();
  });

  it("in the Timeline view, lists a line per event newest first, each thread under its newest line only", async () => {
    const { view, root, row, a, b } = setup();
    await loaded(root);
    expect(root.querySelector(".section-open")).toBeNull();
    expect(root.querySelector(".section-resolved")).toBeNull();
    expect(root.querySelector(".tl-day")!.textContent).toBe("Today");
    expect(texts(root)).toEqual(["v2 · published by claude", "You sent a comment", "Mia commented, sent to no one", "v1 · published by claude"]);
    const lines = Array.from(root.querySelectorAll(".tl-line"));
    // A line's rows follow its head, up to the next line or day.
    const rowsAfter = (l: Element) => { const out: (string | null)[] = []; for (let e = l.nextElementSibling; e?.classList.contains("tl-thread"); e = e.nextElementSibling) out.push(e.querySelector("button.tl-row")!.getAttribute("data-row")); return out; };
    expect(lines.map(rowsAfter)).toEqual([[b.id], [], [a.id], []]);
    expect(row(b)!.querySelector(".tl-q")!.textContent).toBe("Starter should be $9");
    expect(row(b)!.querySelector(".tl-st")!.textContent).toBe("✓ v2");
    expect(row(a)!.querySelector(".tl-st")!.textContent).toBe("Send");
    expect(row(a)!.querySelector(".tl-st")!.classList.contains("act")).toBe(true);
    // A line whose rows wait on a person carries the mark; the band is drawn beside the list.
    expect(lines.map(l => l.classList.contains("wants"))).toEqual([false, false, true, false]);
    expect(root.querySelector(".timeline > svg.tl-band")!.getAttribute("aria-hidden")).toBe("true");
    expect(root.querySelector(".thread-card")).toBeNull();
    view.unmount();
  });

  it("heads each day at the level of the thread list's sections, so the outline skips none", async () => {
    const { view, root, props } = setup();
    view.update({ ...props, versions: [version(1, at(12, 9)), ...props.versions.slice(1)] });
    await loaded(root);
    const days = Array.from(root.querySelectorAll(".tl-day"));
    expect(days.map(d => [d.tagName, d.textContent])).toEqual([["H2", "Today"], ["H2", "Yesterday · 1 version"]]);
    view.unmount();
  });

  it("opens a row into the thread's card, and folds it again", async () => {
    const { view, root, row, card, b } = setup();
    await loaded(root);
    expect(row(b)!.getAttribute("aria-expanded")).toBe("false");
    flush(() => row(b)!.click());
    expect(row(b)!.getAttribute("aria-expanded")).toBe("true");
    expect(card(b)!.textContent).toContain("Done");
    expect(row(b)!.parentElement!.contains(card(b))).toBe(true);
    flush(() => row(b)!.click());
    expect(card(b)).toBeNull();
    view.unmount();
  });

  it("keeps an open card, its draft and its focus when a new comment moves its thread to a newer line", async () => {
    const { view, root, row, card, props, a, b } = setup();
    await loaded(root);
    flush(() => row(a)!.click());
    const before = card(a)!;
    const input = before.querySelector<HTMLInputElement>('input[aria-label="Reply"]')!;
    input.focus();
    flush(() => { input.value = "half a thought"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    // Someone else replies: the thread's newest line is now a newer one.
    const later = { ...a, comments: [...a.comments, comment(`${a.id}.2`, "viewer", at(13, 11), "Agreed")] };
    const scrolled: Element[] = [];
    const own = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) { scrolled.push(this); };
    try {
      view.update({ ...props, threads: [later, b] });
    } finally {
      Element.prototype.scrollIntoView = own;
    }
    expect(texts(root)[0]).toBe("Mia replied");
    // It moved to another line: its row and card are scrolled into view.
    expect(scrolled).toEqual([before.closest(".tl-thread")]);
    expect(card(a)).toBe(before);
    expect(card(a)!.querySelector('input[aria-label="Reply"]')).toBe(input);
    expect(input.value).toBe("half a thought");
    expect(document.activeElement).toBe(input);
    view.unmount();
  });

  it("gives focus back to a row's own button when its thread moves to a newer line", async () => {
    const { view, root, row, props, a, b } = setup();
    await loaded(root);
    const button = row(a)!;
    // The browser takes focus from a moved element; the move here does too.
    const move = Node.prototype.insertBefore;
    Node.prototype.insertBefore = function <T extends Node>(this: Node, n: T, ref: Node | null): T {
      if (n instanceof Element && n.contains(document.activeElement)) (document.activeElement as HTMLElement).blur();
      return move.call(this, n, ref) as T;
    };
    try {
      button.focus();
      const later = { ...a, comments: [...a.comments, comment(`${a.id}.2`, "viewer", at(13, 11), "Agreed")] };
      view.update({ ...props, threads: [later, b] });
    } finally {
      Node.prototype.insertBefore = move;
    }
    expect(texts(root)[0]).toBe("Mia replied");
    expect(row(a)).toBe(button);
    expect(document.activeElement).toBe(button);
    view.unmount();
  });

  it("scrolls a moved open row into view when it was in view, and leaves one that was not", async () => {
    const watched = new Map<Element, (e: { isIntersecting: boolean; intersectionRatio: number }[]) => void>();
    vi.stubGlobal("IntersectionObserver", class { constructor(private cb: (e: { isIntersecting: boolean; intersectionRatio: number }[]) => void) {} observe(el: Element) { watched.set(el, this.cb); } disconnect() {} });
    const scrolled: Element[] = [];
    const own = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) { scrolled.push(this); };
    try {
      const { view, root, row, props, a, b } = setup();
      await loaded(root);
      flush(() => row(a)!.click());
      flush(() => row(b)!.click());
      const box = (t: Thread) => row(t)!.parentElement!;
      // a is on screen, read with the mouse; b is not. Nothing holds focus.
      watched.get(box(a))!([{ isIntersecting: true, intersectionRatio: 1 }]);
      watched.get(box(b))!([{ isIntersecting: false, intersectionRatio: 0 }]);
      (document.activeElement as HTMLElement | null)?.blur();
      scrolled.length = 0;
      const a2 = { ...a, comments: [...a.comments, comment(`${a.id}.2`, "viewer", at(13, 11), "Agreed")] };
      view.update({ ...props, threads: [a2, b] });
      expect(texts(root)[0]).toBe("Mia replied");
      expect(scrolled).toEqual([box(a)]);
      scrolled.length = 0;
      view.update({ ...props, threads: [a2, { ...b, comments: [...b.comments, comment(`${b.id}.3`, "viewer", at(13, 11, 30), "Thanks", "Alex")] }] });
      expect(texts(root)[0]).toBe("You replied");
      expect(scrolled).toEqual([]);
      view.unmount();
    } finally {
      Element.prototype.scrollIntoView = own;
      vi.unstubAllGlobals();
    }
  });

  it("keeps the card open, with its reply box, when the viewer's own reply on an addressed row writes a newer line", async () => {
    const onReply = vi.fn();
    const { view, root, row, card, props, a, b } = setup({ onReply });
    await loaded(root);
    flush(() => row(b)!.click());
    const before = card(b)!;
    const input = before.querySelector<HTMLInputElement>('input[aria-label="Reply"]')!;
    input.focus();
    flush(() => { input.value = "Not quite"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })));
    expect(onReply).toHaveBeenCalledWith(b, "Not quite");
    const replied = { ...b, comments: [...b.comments, comment(`${b.id}.3`, "viewer", at(13, 11), "Not quite", "Alex")] };
    view.update({ ...props, onReply, threads: [a, replied] });
    expect(texts(root)[0]).toBe("You replied");
    expect(row(b)!.getAttribute("aria-expanded")).toBe("true");
    expect(card(b)).toBe(before);
    expect(card(b)!.querySelector('input[aria-label="Reply"]')).toBe(input);
    expect(document.activeElement).toBe(input);
    view.unmount();
  });

  it("opens the selected thread's row", async () => {
    const { view, root, row, card, props, a } = setup();
    await loaded(root);
    view.update({ ...props, selected: a.id });
    await vi.waitFor(() => expect(row(a)!.getAttribute("aria-expanded")).toBe("true"));
    expect(card(a)).not.toBeNull();
    view.unmount();
  });

  it("closes the row a selection opened when the selection moves on", async () => {
    const { view, root, row, props, a, b } = setup();
    await loaded(root);
    view.update({ ...props, selected: a.id });
    expect(row(a)!.getAttribute("aria-expanded")).toBe("true");
    view.update({ ...props, selected: b.id });
    expect(row(b)!.getAttribute("aria-expanded")).toBe("true");
    expect(row(a)!.getAttribute("aria-expanded")).toBe("false");
    view.unmount();
  });

  it("still closes a row the selection opened when it is selected again after a clear, then the selection moves on", async () => {
    const { view, root, row, props, a, b } = setup();
    await loaded(root);
    view.update({ ...props, selected: a.id });
    view.update({ ...props, selected: null });
    view.update({ ...props, selected: a.id });
    expect(row(a)!.getAttribute("aria-expanded")).toBe("true");
    view.update({ ...props, selected: b.id });
    expect(row(a)!.getAttribute("aria-expanded")).toBe("false");
    view.unmount();
  });

  it("keeps a row the viewer opened or toggled by hand when the selection moves on", async () => {
    const { view, root, row, props, a, b } = setup();
    await loaded(root);
    flush(() => row(a)!.click());
    view.update({ ...props, selected: a.id });
    view.update({ ...props, selected: b.id });
    expect(row(a)!.getAttribute("aria-expanded")).toBe("true");
    // Opened by the selection, then closed and opened again by hand.
    view.update({ ...props, selected: null });
    flush(() => row(b)!.click());
    view.update({ ...props, selected: b.id });
    flush(() => row(b)!.click());
    flush(() => row(b)!.click());
    view.update({ ...props, selected: a.id });
    expect(row(b)!.getAttribute("aria-expanded")).toBe("true");
    view.unmount();
  });

  it("keeps a row the selection opened while its card holds focus or a reply draft", async () => {
    const c = thread("c", at(13, 11), "Say how many seats");
    const { view, root, row, card, props, a, b } = setup();
    const all = { ...props, threads: [a, b, c] };
    view.update(all);
    await loaded(root);
    view.update({ ...all, selected: a.id });
    card(a)!.querySelector<HTMLInputElement>('input[aria-label="Reply"]')!.focus();
    view.update({ ...all, selected: b.id });
    expect(row(a)!.getAttribute("aria-expanded")).toBe("true");
    const input = card(b)!.querySelector<HTMLInputElement>('input[aria-label="Reply"]')!;
    flush(() => { input.value = "half a thought"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    view.update({ ...all, selected: c.id });
    expect(row(b)!.getAttribute("aria-expanded")).toBe("true");
    view.unmount();
  });

  it("opens another page's thread unfolded under its row, and folding the card closes the row", async () => {
    const { view, root, row, card, a, b } = setup({ file: "about.html" });
    await loaded(root);
    flush(() => row(a)!.click());
    const head = card(a)!.querySelector<HTMLButtonElement>(".card-head")!;
    expect(head.getAttribute("aria-expanded")).toBe("true");
    expect(card(a)!.querySelector('input[aria-label="Reply"]')).not.toBeNull();
    flush(() => head.click());
    expect(card(a)).toBeNull();
    expect(row(a)!.getAttribute("aria-expanded")).toBe("false");
    expect(row(b)).not.toBeNull();
    view.unmount();
  });

  it("folds an earlier day to its summary and its pending rows, and unfolds it on click", async () => {
    const c = thread("c", at(12, 9, 10), "Mention cancelling");
    const d = resolvedBy(thread("d", at(12, 9, 20), "Old typo"), at(12, 9, 40));
    const { view, root, row } = setup({ threads: [c, d], versions: [version(1, at(12, 9)), version(2, at(13, 10))] });
    await loaded(root);
    const fold = root.querySelector<HTMLButtonElement>("button.tl-fold")!;
    expect(fold.textContent).toBe("Yesterday · 1 version · 2 comments · 1 resolved");
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(texts(root)).toEqual(["v2 · published by claude"]);
    expect(row(c)).not.toBeNull();
    expect(row(d)).toBeNull();
    flush(() => fold.click());
    expect(texts(root)).toEqual(["v2 · published by claude", "You resolved a comment", "Mia made 2 comments, sent to no one", "v1 · published by claude"]);
    expect(row(d)).not.toBeNull();
    // Its heading folds it again.
    flush(() => root.querySelector<HTMLButtonElement>('button.tl-fold[aria-expanded="true"]')!.click());
    expect(row(d)).toBeNull();
    view.unmount();
  });

  it("moves focus to a folded day's summary when closing a card takes away the row that held it", async () => {
    const c = thread("c", at(12, 9, 10), "Mention cancelling");
    const { view, root, row, card, props } = setup({ threads: [c], versions: [version(1, at(12, 9)), version(2, at(13, 10))] });
    await loaded(root);
    const fold = root.querySelector<HTMLButtonElement>('button.tl-fold[aria-expanded="false"]')!;
    flush(() => row(c)!.click());
    expect(card(c)).not.toBeNull();
    // Resolved (here, at a time of that day): no longer pending, the row
    // stays in the folded day while it is open.
    view.update({ ...props, threads: [resolvedBy(c, at(12, 9, 30))] });
    expect(root.querySelector('button.tl-fold[aria-expanded="false"]')).toBe(fold);
    expect(row(c)).not.toBeNull();
    row(c)!.focus();
    flush(() => row(c)!.click());
    expect(row(c)).toBeNull();
    expect(document.activeElement).toBe(fold);
    view.unmount();
  });

  it("keeps an open row in a folded day: a thread reopened whose newest line is now an earlier day's version", async () => {
    const c = thread("c", at(12, 9, 10), "Mention cancelling", { sent_to_agent: true, addressed_in: [2], status: "resolved", resolved_at: at(13, 9), resolved_by: "viewer:p_me" });
    const versions = [version(1, at(12, 9)), version(2, at(12, 10), [c.id]), version(3, at(13, 10))];
    const { view, root, row, card, props } = setup({ threads: [c], versions });
    await loaded(root);
    flush(() => row(c)!.click());
    expect(card(c)).not.toBeNull();
    view.update({ ...props, threads: [{ ...c, status: "open", resolved_at: null, resolved_by: null }] });
    expect(root.querySelector("button.tl-fold")!.getAttribute("aria-expanded")).toBe("false");
    expect(row(c)!.getAttribute("aria-expanded")).toBe("true");
    expect(row(c)!.querySelector(".tl-st")!.textContent).toBe("✓ v2");
    expect(card(c)).not.toBeNull();
    view.unmount();
  });

  it("keeps an open row when a new day's first event folds the day it is in", async () => {
    const d = thread("d", at(12, 9, 10), "Mention cancelling", { sent_to_agent: true, addressed_in: [2] });
    const versions = [version(1, at(12, 9)), version(2, at(12, 10), [d.id])];
    const { view, root, row, card, props } = setup({ threads: [d], versions });
    await loaded(root);
    expect(root.querySelector("button.tl-fold")).toBeNull();
    flush(() => row(d)!.click());
    view.update({ ...props, threads: [d, thread("e", at(13, 11), "New today")] });
    expect(root.querySelector("button.tl-fold")!.getAttribute("aria-expanded")).toBe("false");
    expect(row(d)!.getAttribute("aria-expanded")).toBe("true");
    expect(card(d)).not.toBeNull();
    view.unmount();
  });

  it("unfolds the day of a thread that is selected", async () => {
    const e = resolvedBy(thread("e", at(11, 9, 20), "Old typo"), at(11, 9, 40));
    const { view, root, row, props } = setup({ threads: [e], versions: [version(1, at(11, 9)), version(2, at(13, 10))] });
    await loaded(root);
    expect(row(e)).toBeNull();
    view.update({ ...props, selected: e.id });
    await vi.waitFor(() => expect(row(e)?.getAttribute("aria-expanded")).toBe("true"));
    view.unmount();
  });

  it("marks a collapsed row's thread looked once it has been half visible for SEEN_AFTER_MS", async () => {
    const seen = new Map<Element, (e: { intersectionRatio: number }[]) => void>();
    vi.stubGlobal("IntersectionObserver", class { constructor(private cb: (e: { intersectionRatio: number }[]) => void) {} observe(el: Element) { seen.set(el, this.cb); } disconnect() {} });
    try {
      const { view, root, row, calls, a } = setup();
      await loaded(root);
      vi.useFakeTimers();
      seen.get(row(a)!)!([{ intersectionRatio: 1 }]);
      vi.advanceTimersByTime(SEEN_AFTER_MS - 1);
      expect(calls.seen).not.toHaveBeenCalled();
      vi.advanceTimersByTime(1);
      expect(calls.seen).toHaveBeenCalledWith(a);
      view.unmount();
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it("keeps a collapsed row's half-visible timer when its thread is updated, and reports the thread as it is then", async () => {
    const seen = new Map<Element, (e: { intersectionRatio: number }[]) => void>();
    vi.stubGlobal("IntersectionObserver", class { constructor(private cb: (e: { intersectionRatio: number }[]) => void) {} observe(el: Element) { seen.set(el, this.cb); } disconnect() {} });
    try {
      const { view, root, row, calls, props, a, b } = setup();
      await loaded(root);
      vi.useFakeTimers();
      seen.get(row(a)!)!([{ intersectionRatio: 1 }]);
      vi.advanceTimersByTime(SEEN_AFTER_MS - 1);
      // The same thread as a new object, as each refetch gives it; its lines are unchanged.
      const later = { ...a, comments: [...a.comments] };
      view.update({ ...props, threads: [later, b] });
      vi.advanceTimersByTime(1);
      expect(calls.seen).toHaveBeenCalledTimes(1);
      expect(calls.seen).toHaveBeenCalledWith(later);
      view.unmount();
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it("read-only, opens a card with no reply box", async () => {
    const { view, root, row, card, a } = setup({ readOnly: true });
    await loaded(root);
    flush(() => row(a)!.click());
    expect(card(a)).not.toBeNull();
    expect(root.querySelector('input[aria-label="Reply"]')).toBeNull();
    view.unmount();
  });

  it("with comments off, says above the timeline why its cards have no reply box", async () => {
    const { view, root, row, a } = setup({ noReply: "Comments are off here." });
    await loaded(root);
    const note = root.querySelector('[role="tabpanel"] > .no-reply');
    expect(note?.textContent).toBe("Comments are off here.");
    expect(note!.nextElementSibling!.classList.contains("timeline")).toBe(true);
    flush(() => row(a)!.click());
    expect(root.querySelector('input[aria-label="Reply"]')).toBeNull();
    view.unmount();
  });

  it("ticks with Shift over the open cards in the timeline's order, newest line first", async () => {
    const onToggle = vi.fn();
    const c = thread("c", at(13, 11), "Say how many seats");
    const { view, root, row, props, a, b } = setup({ selection: { ids: [], anchor: null }, onToggle });
    view.update({ ...props, threads: [a, b, c] });
    await loaded(root);
    for (const t of [a, c]) flush(() => row(t)!.click());
    const box = (t: Thread) => root.querySelector<HTMLInputElement>(`.thread-card[data-thread="${t.id}"] .thread-check`)!;
    flush(() => box(a).dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true })));
    expect(onToggle).toHaveBeenLastCalledWith(a, true, [c.id, a.id]);
    flush(() => row(b)!.click());
    flush(() => box(c).click());
    expect(onToggle).toHaveBeenLastCalledWith(c, false, [c.id, b.id, a.id]);
    view.unmount();
  });

  it("switches views from the tabs, by click and by arrow key", async () => {
    const { view, root, calls } = setup();
    await loaded(root);
    const tab = (name: string) => Array.from(root.querySelectorAll<HTMLButtonElement>('[role="tab"]')).find(t => t.textContent === name)!;
    expect(tab("Timeline").getAttribute("aria-selected")).toBe("true");
    expect([tab("Timeline").tabIndex, tab("Threads").tabIndex]).toEqual([0, -1]);
    flush(() => tab("Threads").click());
    expect(calls.change).toHaveBeenLastCalledWith("threads");
    tab("Timeline").focus();
    flush(() => tab("Timeline").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true })));
    expect(calls.change).toHaveBeenLastCalledWith("threads");
    expect(document.activeElement).toBe(tab("Threads"));
    view.unmount();
  });

  it("selects the newest thread that waits on a person from \"N need you\"", async () => {
    const c = thread("c", at(13, 11), "Say how many seats");
    const first = setup();
    first.view.update({ ...first.props, threads: [first.a, first.b, c] });
    await loaded(first.root);
    const needs = first.root.querySelector<HTMLButtonElement>("button.tl-needs")!;
    expect(needs.textContent).toBe("2 need you");
    flush(() => needs.click());
    expect(first.calls.select).toHaveBeenCalledWith(c);
    first.view.unmount();
    const one = setup();
    one.view.update({ ...one.props, threads: [one.a] });
    await loaded(one.root);
    expect(one.root.querySelector("button.tl-needs")!.textContent).toBe("1 needs you");
    one.view.unmount();
    const none = setup();
    none.view.update({ ...none.props, threads: [none.b] });
    await loaded(none.root);
    expect(none.root.querySelector("button.tl-needs")).toBeNull();
    none.view.unmount();
  });

  it("with no threads, follows the v1 line with the Open section's hint", async () => {
    const { view, root } = setup({ threads: [], versions: [version(1, at(13, 9))] });
    await loaded(root);
    expect(texts(root)).toEqual(["v1 · published by claude"]);
    expect(root.querySelector(".timeline .empty-open")!.textContent).toMatch(/^Nothing open\. Press C/);
    view.unmount();
  });
});

describe("sidebarView", () => {
  const fresh = async () => { vi.resetModules(); return (await import("./ui/sidebar-view.svelte")).sidebarView; };

  it("starts on the timeline, and remembers a choice", async () => {
    localStorage.removeItem("clax.sidebar");
    const v = await fresh();
    expect(v.now).toBe("timeline");
    v.set("threads");
    expect(v.now).toBe("threads");
    expect(JSON.parse(localStorage.getItem("clax.sidebar")!)).toEqual({ view: "threads" });
    expect((await fresh()).now).toBe("threads");
    localStorage.setItem("clax.sidebar", '{"view":"nonsense"}');
    expect((await fresh()).now).toBe("timeline");
  });

  it("keeps the default when storage throws", async () => {
    vi.stubGlobal("localStorage", { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); } });
    try {
      const v = await fresh();
      expect(v.now).toBe("timeline");
      v.set("threads");
      expect(v.now).toBe("threads");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
