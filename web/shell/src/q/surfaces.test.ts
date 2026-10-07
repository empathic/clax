import { afterEach, describe, expect, it, vi } from "vitest";
import type { QuestionView } from "../api";
import type { StreamEvent } from "../stream";
import { flush, mount } from "../test/svelte";
import { QuestionFeed } from "./feed.svelte";
import { view } from "./fixtures";
import SidebarQuestions from "./SidebarQuestions.svelte";

const AID = "7q3k9mzx2b4t";
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flush(); };

/** A stream whose topics' handlers the test calls. */
function fakeStream() {
  const on = new Map<string, (e: StreamEvent) => void>();
  return {
    on,
    watch: (topics: readonly string[], f: (e: StreamEvent) => void) => { for (const t of topics) on.set(t, f); return () => { for (const t of topics) on.delete(t); }; },
    onNotify: () => () => {},
  };
}

/** A feed holding `qs` open, with a clock the test moves. */
async function feedOf(qs: QuestionView[]) {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ questions: qs, open: qs.length }))));
  const due: { at: number; fn: () => void }[] = [];
  let t = 0;
  const feed = new QuestionFeed({ after: (ms, fn) => { due.push({ at: t + ms, fn }); return () => {}; } });
  await feed.start(fakeStream());
  const advance = (ms: number) => flush(() => { t += ms; for (const d of due.filter(x => x.at <= t)) { due.splice(due.indexOf(d), 1); d.fn(); } });
  return { feed, advance };
}

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); document.documentElement.removeAttribute("data-questions"); });

describe("the questions in the artifact view", () => {
  it("shows the open questions about the artifact, and hands focus on when a closed card leaves", async () => {
    const q1 = view({ id: "Q1", created_at: "2026-10-07T09:00:00.000Z" });
    const q2 = view({ id: "Q2", created_at: "2026-10-07T09:01:00.000Z" });
    const elsewhere = view({ id: "Q3", artifact: { id: "other", title: "Other", kind: "html" } });
    const { feed, advance } = await feedOf([q1, q2, elsewhere]);
    const aside = document.createElement("aside");
    aside.className = "sidebar";
    document.body.append(aside);
    const m = mount(SidebarQuestions, { aid: AID, feed }, aside);
    const cards = () => [...m.root.querySelectorAll<HTMLElement>(".qcard")];
    expect(cards().map(c => c.dataset.question)).toEqual(["Q1", "Q2"]);
    expect(m.root.querySelector("h2")!.textContent).toBe("Questions for you 2");
    // The page link is left out: it is this page.
    expect(m.root.querySelector(".qcard .about")).toBeNull();
    cards()[0].focus();
    flush(() => feed.upsert({ ...q1, status: "declined" }));
    // Closed, it stays, with focus, for 4 s.
    expect(cards().map(c => c.dataset.question)).toEqual(["Q1", "Q2"]);
    expect(cards()[0].hasAttribute("data-closed")).toBe(true);
    expect(document.activeElement).toBe(cards()[0]);
    advance(4000);
    // It left: the card in its place has focus.
    expect(cards().map(c => c.dataset.question)).toEqual(["Q2"]);
    expect(document.activeElement).toBe(cards()[0]);
    flush(() => feed.upsert({ ...q2, status: "answered", answered_via: "terminal" }));
    advance(4000);
    // The last left: the block hides, and the sidebar takes focus.
    expect(cards()).toEqual([]);
    expect(m.root.querySelector<HTMLElement>(".side-q")!.hidden).toBe(true);
    expect(document.activeElement).toBe(aside);
    m.unmount();
  });

  it("leaves focus the person moved elsewhere when a closed card leaves", async () => {
    const q1 = view({ id: "Q1" });
    const { feed, advance } = await feedOf([q1]);
    const m = mount(SidebarQuestions, { aid: AID, feed });
    const input = document.createElement("input");
    document.body.append(input);
    m.root.querySelector<HTMLElement>(".qcard")!.focus();
    input.focus();
    flush(() => feed.upsert({ ...q1, status: "declined" }));
    advance(4000);
    expect(document.activeElement).toBe(input);
    m.unmount();
  });

  it("puts Inbox in the top bar, fills the sidebar's slot each time it mounts, and marks the threads controls while a question is open", async () => {
    vi.resetModules();
    const q = view();
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? { unread: 4, questions: [q], latest: [] } : { questions: [q], open: 1 }))));
    document.head.innerHTML = `<title>Quarterly Review</title><link rel="icon" href="/_clax/mark.svg">`;
    document.body.innerHTML = `<div class="page"><header class="topbar"><a class="home"></a><div class="ttl"></div><div class="island"></div></header><aside class="sidebar"><div class="questions-slot"></div></aside></div>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    await settle();
    const bar = document.querySelector(".topbar")!;
    const link = bar.querySelector(".inbox-link")!;
    expect(link.nextElementSibling).toBe(bar.querySelector(".island"));
    expect(link.querySelector(".count")!.textContent).toBe("4");
    expect(document.title).toBe("(4) Quarterly Review");
    expect(document.querySelectorAll(".sidebar .questions-slot .qcard")).toHaveLength(1);
    expect(document.documentElement.hasAttribute("data-questions")).toBe(true);
    expect([...stream.on.keys()].sort()).toEqual(["inbox", "questions"]);
    // The sidebar mounts again (the panel was closed and opened): its new slot is filled.
    document.querySelector(".sidebar")!.innerHTML = `<div class="questions-slot"></div>`;
    const slot = document.querySelector(".questions-slot")!;
    flush(() => slot.dispatchEvent(new CustomEvent("clax-questions-slot", { bubbles: true })));
    expect(slot.querySelectorAll(".qcard")).toHaveLength(1);
    stream.on.get("questions")!({ type: "question", question: { ...q, status: "answered" } });
    expect(document.documentElement.hasAttribute("data-questions")).toBe(false);
    stop();
    flush();
    expect(bar.querySelector(".inbox-link")).toBeNull();
    expect(slot.querySelector(".qcard")).toBeNull();
    expect(document.title).toBe("Quarterly Review");
    expect(stream.on.size).toBe(0);
  });

  it("shows nothing and subscribes nothing for anyone but the owner", async () => {
    vi.resetModules();
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 403 })));
    document.body.innerHTML = `<header class="topbar"><div class="island"></div></header><aside class="sidebar"><div class="questions-slot"></div></aside>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    await settle();
    expect(document.querySelector(".inbox-link")).toBeNull();
    expect(document.querySelector(".questions-slot")!.children).toHaveLength(0);
    expect(stream.on.size).toBe(0);
    stop();
  });
});
