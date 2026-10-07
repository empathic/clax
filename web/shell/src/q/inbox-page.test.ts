import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import type { InboxItem } from "../api";
import { flush, mount } from "../test/svelte";
import { InboxFeed, QuestionFeed } from "./feed.svelte";
import { item, view } from "./fixtures";
import InboxPage from "./InboxPage.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
const now = new Date("2026-10-07T09:17:03.120Z");

const Q = item("question", { id: "IQ", seq: 9 });
const R = item("reply", { id: "IR", seq: 8 });
const READ = [1, 2, 3].map(n => item("published", { id: `D${n}`, seq: 4 - n, read: true }));

type Call = { url: string; method: string; body: unknown };
/** The owner routes: `unread` and `read` items by search text; every request is kept. */
function stubRoutes(unread: Record<string, InboxItem[]> = { "": [Q, R] }) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
    const ok = (b: unknown) => new Response(JSON.stringify(b));
    if (url === "/api/artifacts") return ok({ artifacts: [] });
    if (url === "/api/inbox/read") return ok({ marked: 2, unread: 0 });
    const one = /^\/api\/inbox\/(\w+)\/read$/.exec(url);
    if (one) return ok({ item: { ...[Q, R].find(i => i.id === one[1])!, read: true }, unread: 1 });
    const u = new URL(url, "http://x");
    const q = u.searchParams.get("q") ?? "";
    if (u.searchParams.get("read") === "unread") return ok({ items: unread[q] ?? [], next_cursor: null, unread: 2, total: (unread[q] ?? []).length });
    if (u.searchParams.get("limit") === "1") return ok({ items: READ.slice(0, 1), next_cursor: "3", unread: 2, total: 1240 });
    if (u.searchParams.get("before") === "2") return ok({ items: READ.slice(2), next_cursor: null, unread: 2, total: 1240 });
    return ok({ items: READ.slice(0, 2), next_cursor: "2", unread: 2, total: 1240 });
  }));
  return calls;
}

function page(go = vi.fn()) {
  const inbox = new InboxFeed();
  const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go, now });
  return { m, go, inbox };
}

beforeEach(() => { history.replaceState(null, "", "/inbox"); });
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("InboxPage", () => {
  it("lists unread items with open questions first as cards, and folds the read ones behind their count", async () => {
    const calls = stubRoutes();
    const { m } = page();
    await settle();
    const unread = m.root.querySelector('section[aria-labelledby="inbox-unread-h"]')!;
    expect(unread.querySelector("h3")!.textContent).toBe("Unread2");
    const card = unread.querySelector(".qcard")!;
    const rows = [...unread.querySelectorAll(".irow")];
    expect(card.getAttribute("data-question")).toBe(view().id);
    expect(rows.map(r => r.getAttribute("data-item"))).toEqual(["IR"]);
    // The card comes before the rows.
    expect(card.compareDocumentPosition(rows[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(m.root.querySelector(".ihead .icnt")!.textContent).toBe("2 unread");
    const fold = m.root.querySelector<HTMLButtonElement>(".fold")!;
    expect(fold.textContent!.trim()).toBe("Show 1,240 read items");
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(m.root.querySelector("#inbox-read")).toBeNull();
    expect(calls.map(c => c.url)).toEqual(expect.arrayContaining(["/api/inbox?read=unread", "/api/inbox?read=read&limit=1"]));
    click(fold);
    await settle();
    expect([...m.root.querySelectorAll("#inbox-read .irow")].map(r => r.getAttribute("data-item"))).toEqual(["D1", "D2"]);
    click(m.root.querySelector("#inbox-read .more")!);
    await settle();
    expect([...m.root.querySelectorAll("#inbox-read .irow")].map(r => r.getAttribute("data-item"))).toEqual(["D1", "D2", "D3"]);
    expect(calls.at(-1)!.url).toBe("/api/inbox?read=read&before=2");
    expect(m.root.querySelector("#inbox-read .more")).toBeNull();
    expect(fold.textContent!.trim()).toBe("Hide read items");
  });

  it("keeps the search in the URL 250 ms after typing stops, and fetches both sections for it", async () => {
    vi.useFakeTimers();
    const calls = stubRoutes({ "": [Q, R], dash: [R] });
    const { m } = page();
    await vi.advanceTimersByTimeAsync(0);
    flush();
    const box = m.root.querySelector<HTMLInputElement>(".isearch")!;
    const n = calls.length;
    flush(() => { box.value = "da"; box.dispatchEvent(new Event("input", { bubbles: true })); });
    await vi.advanceTimersByTimeAsync(100);
    flush(() => { box.value = "dash"; box.dispatchEvent(new Event("input", { bubbles: true })); });
    await vi.advanceTimersByTimeAsync(249);
    expect(location.search).toBe("");
    expect(calls).toHaveLength(n);
    await vi.advanceTimersByTimeAsync(1);
    flush();
    expect(location.search).toBe("?search=dash");
    expect(calls.slice(n).map(c => c.url)).toEqual(["/api/inbox?q=dash&read=unread", "/api/inbox?q=dash&read=read&limit=1"]);
    await vi.advanceTimersByTimeAsync(0);
    flush();
    expect([...m.root.querySelectorAll(".irow")].map(r => r.getAttribute("data-item"))).toEqual(["IR"]);
    expect(m.root.querySelector(".qcard")).toBeNull();
  });

  it("reads its search from the URL, and marks all read with that search and the newest item shown", async () => {
    history.replaceState(null, "", "/inbox?search=dash&kind=reply");
    const calls = stubRoutes({ dash: [R] });
    const { m } = page();
    await settle();
    expect(m.root.querySelector<HTMLInputElement>(".isearch")!.value).toBe("dash");
    expect(m.root.querySelector('.kind[aria-pressed="true"]')!.textContent).toBe("Replies");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Mark all read")!);
    await settle();
    const post = calls.find(c => c.method === "POST")!;
    expect(post).toEqual({ url: "/api/inbox/read", method: "POST", body: { all: true, filter: { q: "dash", kind: ["reply"] }, upto: 8 } });
    // The sections are fetched again after.
    expect(calls.filter(c => c.url.startsWith("/api/inbox?q=dash")).length).toBe(4);
  });

  it("opens a row's item and marks it read, and marks one read by its dot without opening it", async () => {
    const calls = stubRoutes();
    const { m, go } = page();
    await settle();
    click(m.root.querySelector('.irow[data-item="IR"] .dot')!);
    await settle();
    expect(calls.filter(c => c.method === "POST").map(c => c.url)).toEqual(["/api/inbox/IR/read"]);
    expect(go).not.toHaveBeenCalled();
    click(m.root.querySelector('.irow[data-item="IR"] .open')!);
    await settle();
    expect(go).toHaveBeenCalledWith(R.url);
  });

  it("keeps a question's card that closed here when the sections are fetched again, and drops it for a new search", async () => {
    const lists: Record<string, InboxItem[]> = { "": [Q, R], dash: [R] };
    stubRoutes(lists);
    const inbox = new InboxFeed();
    const questions = new QuestionFeed();
    const m = mount(InboxPage, { inbox, questions, go: vi.fn(), now });
    await settle();
    flush(() => { questions.upsert(view()); });
    flush(() => { questions.upsert({ ...view(), status: "answered", answers: [{ selected: ["Two"], text: null }, { selected: ["Left"], text: null }, { selected: [], text: "ok" }] }); });
    // Answered: read now, so the daemon no longer lists it as unread.
    lists[""] = [R];
    (inbox as unknown as { tell(c: unknown): void }).tell({ refetch: true });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(m.root.querySelector(".qcard")!.hasAttribute("data-closed")).toBe(true);
    const box = m.root.querySelector<HTMLInputElement>(".isearch")!;
    flush(() => { box.value = "dash"; box.dispatchEvent(new Event("input", { bubbles: true })); });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(m.root.querySelector(".qcard")).toBeNull();
  });

  it("goes to the question's card for `?q=`, focused", async () => {
    history.replaceState(null, "", `/inbox?q=${view().id}`);
    stubRoutes();
    const { m } = page();
    await settle();
    expect(document.activeElement).toBe(m.root.querySelector(".qcard"));
  });

  it("toggles its filters behind Filters, which phone width shows alone", async () => {
    stubRoutes();
    const { m } = page();
    await settle();
    const toggle = m.root.querySelector<HTMLButtonElement>(".filters-toggle")!;
    const filters = m.root.querySelector("#inbox-filters")!;
    expect([toggle.getAttribute("aria-expanded"), filters.hasAttribute("data-open")]).toEqual(["false", false]);
    click(toggle);
    expect([toggle.getAttribute("aria-expanded"), filters.hasAttribute("data-open")]).toEqual(["true", true]);
    // The phone's rule: filters not opened are hidden there.
    expect(document.head.textContent).toMatch(/@media \(max-width: 700px\)[^@]*\.inbox \.filters:not\(\[data-open\]\)\s*\{\s*display:\s*none;?\s*\}/);
  });

  it("asks for notification permission only from Notify me, and says when they are blocked", async () => {
    stubRoutes();
    const requestPermission = vi.fn(async () => "denied" as const);
    vi.stubGlobal("Notification", { permission: "default", requestPermission });
    const { m } = page();
    await settle();
    expect(requestPermission).not.toHaveBeenCalled();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Notify me")!);
    await settle();
    expect(requestPermission).toHaveBeenCalledTimes(1);
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent === "Notify me")).toBe(false);
    expect(m.root.querySelector(".quiet")!.textContent).toContain("Notifications are blocked");
  });
});
