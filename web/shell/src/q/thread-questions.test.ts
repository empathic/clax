// The questions asked in threads and in the terminal, on a feed the test
// holds: the sidebar's block, the links to thread cards, and the terminal
// card through the list. No test here resets modules, so the parts that
// load on demand share the statically imported list's Svelte runtime.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { QuestionView } from "../api";
import type { StreamEvent } from "../stream";
import { flush, mount } from "../test/svelte";
import { QuestionFeed } from "./feed.svelte";
import { view } from "./fixtures";
import SidebarQuestions from "./SidebarQuestions.svelte";
import ThreadLinks from "./ThreadLinks.svelte";

const AID = "7q3k9mzx2b4t";
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flush(); };

/** A feed holding `qs` open, filled as its topic goes live. */
async function feedOf(qs: QuestionView[]) {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ questions: qs, open: qs.length }))));
  const feed = new QuestionFeed({ after: () => () => {} });
  const on = new Map<string, (e: StreamEvent) => void>();
  feed.start({ watch: (topics: readonly string[], f: (e: StreamEvent) => void) => { for (const t of topics) on.set(t, f); return () => {}; } });
  on.get("questions")!({ type: "ready" });
  await settle();
  return feed;
}

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("questions in threads and in the terminal", () => {
  it("a question whose thread is gone or resolved is a card in the sidebar block", async () => {
    const feed = await feedOf([view({ id: "QR", thread: { id: "t1", open: false } })]);
    const m = mount(SidebarQuestions, { aid: AID, feed });
    expect([...m.root.querySelectorAll<HTMLElement>(".qcard")].map(c => c.dataset.question)).toEqual(["QR"]);
    expect(m.root.querySelector("a.q-thread")).toBeNull();
    expect(m.root.querySelector(".qcard a.about")).toBeNull();
    m.unmount();
  });

  it("keeps a thread question's link while its card says what closed it, so the block keeps its height", async () => {
    const q = view({ id: "QT", thread: { id: "t1", open: true } });
    const feed = await feedOf([q]);
    const m = mount(SidebarQuestions, { aid: AID, feed });
    await vi.waitFor(() => { flush(); expect(m.root.querySelector("a.q-thread")).not.toBeNull(); });
    feed.upsert({ ...q, status: "answered", closed_at: "2026-10-07T09:10:00.000Z" });
    flush();
    expect(m.root.querySelector("a.q-thread")).not.toBeNull();
    expect(m.root.querySelector<HTMLElement>(".side-q")!.hidden).toBe(false);
    expect(m.root.querySelector(".side-q .c")!.textContent).toBe("0");
    m.unmount();
  });

  it("shows an in-terminal question as a terminal card, whose Ask me here posts the pull", async () => {
    const t = view({ id: "QN", in_terminal: true, source: "hook" });
    const feed = await feedOf([t]);
    const m = mount(SidebarQuestions, { aid: AID, feed });
    // The terminal card loads with the first in-terminal question shown.
    await vi.waitFor(() => { flush(); expect(m.root.querySelector<HTMLElement>(".qcard.in-terminal")?.dataset.question).toBe("QN"); });
    const fetched = vi.fn(async (_url: string, _init?: RequestInit) => new Response(JSON.stringify({ question: { ...t, status: "withdrawn", pulled: true, closed_at: "2026-10-07T09:10:00.000Z" } })));
    vi.stubGlobal("fetch", fetched);
    await feed.pull("QN");
    flush();
    expect(fetched.mock.calls[0][0]).toBe("/api/questions/QN/pull");
    expect(fetched.mock.calls[0][1]!.method).toBe("POST");
    expect(m.root.querySelector(".qcard .closed")!.textContent).toBe("Sent to claude: it asks here when it next checks in");
    m.unmount();
  });

  it("opens a folded thread card from its link, then scrolls to its question and focuses it", async () => {
    const q = view({ id: "QF", thread: { id: "t1", open: true } });
    // A folded card (a thread of another page): its head opens it, and its slot fills a moment later.
    document.body.innerHTML = `<article class="thread-card far folded" data-thread="t1"><header><button type="button" class="card-head"></button></header></article>`;
    const card = document.querySelector<HTMLElement>(".thread-card")!;
    const head = card.querySelector("button")!;
    head.addEventListener("click", () => {
      card.classList.remove("folded");
      const slot = document.createElement("div");
      slot.className = "thread-q-slot";
      slot.dataset.thread = "t1";
      slot.scrollIntoView = vi.fn();
      card.append(slot);
      queueMicrotask(() => { slot.innerHTML = `<article class="qcard" data-question="QF" tabindex="-1"></article>`; });
    });
    const m = mount(ThreadLinks, { list: [q] });
    const a = m.root.querySelector<HTMLAnchorElement>("a.q-thread")!;
    expect(a.getAttribute("href")).toBe("?thread=t1");
    const e = new MouseEvent("click", { bubbles: true, cancelable: true });
    a.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect((document.activeElement as HTMLElement | null)?.dataset.question).toBe("QF"));
    expect(card.querySelector<HTMLElement>(".thread-q-slot")!.scrollIntoView).toHaveBeenCalled();
    m.unmount();
  });

  it("leaves a link to a thread with no card on the page to navigate to it", () => {
    const m = mount(ThreadLinks, { list: [view({ thread: { id: "t9", open: true } })] });
    let prevented: boolean | null = null;
    // Heard after the link's own handler; jsdom does not navigate.
    const hear = (ev: Event) => { prevented = ev.defaultPrevented; ev.preventDefault(); };
    document.addEventListener("click", hear);
    m.root.querySelector("a.q-thread")!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    document.removeEventListener("click", hear);
    expect(prevented).toBe(false);
    m.unmount();
  });
});
