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
  const stream = fakeStream();
  feed.start(stream);
  stream.on.get("questions")!({ type: "ready" });
  await settle();
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
    document.body.innerHTML = `<div class="page"><header class="topbar"><a class="home"></a><div class="ttl"></div><div class="island"></div></header><aside class="sidebar"><div class="questions-slot"></div></aside><div class="phone-tabs"><button>Page</button><button>Threads</button></div></div>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    // Subscribed first; the topics go live, and the feeds fetch.
    expect([...stream.on.keys()].sort()).toEqual(["inbox", "questions"]);
    for (const f of stream.on.values()) f({ type: "ready" });
    await settle();
    const bar = document.querySelector(".topbar")!;
    const link = bar.querySelector(".inbox-link")!;
    expect(link.nextElementSibling).toBe(bar.querySelector(".island"));
    expect(link.querySelector(".count")!.textContent).toBe("4");
    // And at the end of the phone's tab bar, which shows it at phone width in place of the top bar's.
    expect(document.querySelector(".phone-tabs")!.lastElementChild!.matches("a.inbox-link")).toBe(true);
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
    expect(document.querySelector(".inbox-link")).toBeNull();
    expect(slot.querySelector(".qcard")).toBeNull();
    expect(document.title).toBe("Quarterly Review");
    expect(stream.on.size).toBe(0);
  });

  it("shows a thread's question inside its card, and counts and links it from the sidebar's block", async () => {
    vi.resetModules();
    const inT = view({ id: "QT", thread: { id: "t1", open: true } });
    const loose = view({ id: "QA", created_at: "2026-10-07T09:05:00.000Z" });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? { unread: 2, questions: [inT, loose], latest: [] } : { questions: [inT, loose], open: 2 }))));
    document.body.innerHTML = `<div class="page"><header class="topbar"><div class="island"></div></header><aside class="sidebar"><div class="questions-slot"></div><article class="thread"><div class="thread-q-slot" data-slot-thread="t1"></div></article></aside></div>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    for (const f of stream.on.values()) f({ type: "ready" });
    await settle();
    // The thread cards' questions load with the first question in an open thread.
    await vi.waitFor(() => { flush(); expect([...document.querySelectorAll(".thread-q-slot .qcard")].map(c => (c as HTMLElement).dataset.question)).toEqual(["QT"]); });
    expect([...document.querySelectorAll(".questions-slot .qcard")].map(c => (c as HTMLElement).dataset.question)).toEqual(["QA"]);
    expect(document.querySelector(".questions-slot h2")!.textContent).toBe("Questions for you 2");
    // The links to threads load with the first question in an open thread.
    await vi.waitFor(() => { flush(); expect(document.querySelector<HTMLAnchorElement>(".questions-slot a.q-thread")?.getAttribute("href")).toBe("?thread=t1"); });
    // A card mounted later gets its questions too.
    const late = document.createElement("div");
    late.className = "thread-q-slot";
    late.dataset.slotThread = "t1";
    document.querySelector(".sidebar")!.append(late);
    await settle();
    expect(late.querySelectorAll(".qcard")).toHaveLength(1);
    // A card removed takes its questions with it, and stopping empties the rest.
    late.remove();
    await settle();
    stop();
    flush();
    expect(document.querySelector(".thread-q-slot .qcard")).toBeNull();
  });

  it("in app mode mounts the questions into the app's sheet, not the hidden sidebar, and tells the page each change", async () => {
    vi.resetModules();
    const q = view();
    const elsewhere = view({ id: "Q3", artifact: { id: "other", title: "Other", kind: "html" } });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? { unread: 0, questions: [], latest: [] } : { questions: [q, elsewhere], open: 2 }))));
    document.documentElement.classList.add("app");
    document.body.innerHTML = `<aside class="sidebar"><div class="questions-slot"></div></aside><section class="app-questions"><div class="questions-slot"></div></section>`;
    const counts: unknown[] = [];
    const hear = (e: Event) => counts.push((e as CustomEvent).detail);
    document.addEventListener("clax-questions", hear);
    try {
      const qmod = await import("./index");
      const stream = fakeStream();
      const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
      for (const f of stream.on.values()) f({ type: "ready" });
      await settle();
      expect(document.querySelectorAll(".app-questions .qcard")).toHaveLength(1);
      expect(document.querySelector(".sidebar .side-q")).toBeNull();
      expect(counts.at(-1)).toEqual({ open: [q.id], closed: [] });
      // The sidebar mounting again does not take the list from the sheet.
      const side = document.querySelector(".sidebar > .questions-slot")!;
      flush(() => side.dispatchEvent(new CustomEvent("clax-questions-slot", { bubbles: true })));
      expect(document.querySelectorAll(".app-questions .qcard")).toHaveLength(1);
      expect(side.querySelector(".side-q")).toBeNull();
      // Answered: none open, its card still shown saying so.
      flush(() => stream.on.get("questions")!({ type: "question", question: { ...q, status: "answered", answered_via: "shell" } }));
      expect(counts.at(-1)).toEqual({ open: [], closed: [q.id] });
      stop();
      flush();
      expect(counts.at(-1)).toEqual({ open: [], closed: [] });
      expect(document.querySelector(".app-questions .qcard")).toBeNull();
    } finally {
      document.removeEventListener("clax-questions", hear);
      document.documentElement.classList.remove("app");
    }
  });

  it("in app mode shows a question asked in a thread as a card in the app's sheet, since the thread cards are hidden", async () => {
    vi.resetModules();
    const inT = view({ id: "QT", thread: { id: "t1", open: true } });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? { unread: 1, questions: [inT], latest: [] } : { questions: [inT], open: 1 }))));
    document.documentElement.classList.add("app");
    document.body.innerHTML = `<aside class="sidebar"><div class="questions-slot"></div><article class="thread"><div class="thread-q-slot" data-slot-thread="t1"></div></article></aside><section class="app-questions"><div class="questions-slot"></div></section>`;
    try {
      const qmod = await import("./index");
      const stream = fakeStream();
      const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
      for (const f of stream.on.values()) f({ type: "ready" });
      await settle();
      expect([...document.querySelectorAll<HTMLElement>(".app-questions .qcard")].map(c => c.dataset.question)).toEqual(["QT"]);
      expect(document.querySelector(".app-questions a.q-thread")).toBeNull();
      expect(document.querySelector(".thread-q-slot .qcard")).toBeNull();
      stop();
    } finally {
      document.documentElement.classList.remove("app");
    }
  });

  it("outside app mode fills the sidebar even when a sheet's slot is in the document", async () => {
    vi.resetModules();
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? { unread: 0, questions: [], latest: [] } : { questions: [view()], open: 1 }))));
    document.body.innerHTML = `<aside class="sidebar"><div class="questions-slot"></div></aside><section class="app-questions"><div class="questions-slot"></div></section>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    for (const f of stream.on.values()) f({ type: "ready" });
    await settle();
    expect(document.querySelectorAll(".sidebar .qcard")).toHaveLength(1);
    expect(document.querySelector(".app-questions .qcard")).toBeNull();
    stop();
  });

  it("closes the notifications shown once nothing is left unread, as after a bulk mark", async () => {
    vi.resetModules();
    const made: { closed: boolean }[] = [];
    vi.stubGlobal("Notification", class { static permission = "granted"; closed = false; tag: string; onclick = null; constructor(_t: string, o: NotificationOptions) { this.tag = o.tag ?? ""; made.push(this); } close() { this.closed = true; } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ unread: 2, questions: [], latest: [] }))));
    document.body.innerHTML = `<header class="topbar"><div class="island"></div></header>`;
    const qmod = await import("./index");
    const notify: ((d: Record<string, unknown>) => void)[] = [];
    const stream = { ...fakeStream(), onNotify: (f: (d: Record<string, unknown>) => void) => { notify.push(f); return () => {}; } };
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    for (const f of stream.on.values()) f({ type: "ready" });
    await settle();
    const { item } = await import("./fixtures");
    notify[0]({ item: item("reply", { id: "N1" }), unread: 2 });
    notify[0]({ item: item("published", { id: "N2" }), unread: 2 });
    await vi.waitFor(() => expect(made).toHaveLength(2));
    stream.on.get("inbox")!({ type: "inbox_read", ids: null, read: true, unread: 0 });
    expect(made.map(n => n.closed)).toEqual([true, true]);
    stop();
  });

  it("shows nothing when the owner routes refuse the caller", async () => {
    vi.resetModules();
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 403 })));
    document.body.innerHTML = `<header class="topbar"><div class="island"></div></header><aside class="sidebar"><div class="questions-slot"></div></aside>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    for (const f of stream.on.values()) f({ type: "ready" });
    await settle();
    expect(document.querySelector(".inbox-link")).toBeNull();
    expect(document.querySelector(".questions-slot .qcard")).toBeNull();
    expect(document.querySelector<HTMLElement>(".questions-slot .side-q")!.hidden).toBe(true);
    expect(document.title).not.toMatch(/^\(/);
    stop();
    expect(stream.on.size).toBe(0);
  });
});
