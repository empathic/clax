import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import type { InboxItem } from "../api";
import { flush, mount } from "../test/svelte";
import { InboxFeed, QuestionFeed } from "./feed.svelte";
import { approvalCard, item, view } from "./fixtures";
import InboxPage from "./InboxPage.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
const now = new Date("2026-10-07T09:17:03.120Z");

const Q = item("question", { id: "IQ", seq: 9 });
const R = item("reply", { id: "IR", seq: 8 });
const READ = [1, 2, 3, 4, 5].map(n => item("published", { id: `D${n}`, seq: 6 - n, read: true }));
/** How many read items the daemon counts. */
const READ_TOTAL = 1240;

/** The agent strip the daemon answers: one agent waiting on you since 09:05:03. */
let STRIP: unknown[] = [];
let stripFetches = 0;

type Call = { url: string; method: string; body: unknown };
const ok = (b: unknown) => new Response(JSON.stringify(b));

/** `items` (newest first) as `GET /api/inbox?group=artifact` groups them, each listing its `rows` newest. */
function groupsOf(items: InboxItem[], rows: number) {
  const by = new Map<string, InboxItem[]>();
  for (const i of items) by.set(i.artifact?.id ?? "none", [...(by.get(i.artifact?.id ?? "none") ?? []), i]);
  return [...by.values()].map(g => ({
    artifact: g[0].artifact, total: g.length, unread: g.filter(i => !i.read).length, newest_at: g[0].created_at,
    items: g.slice(0, rows), next_cursor: g.length > rows ? String(g[rows - 1].seq) : null,
  }));
}

/** A list or grouping of `items` as the daemon answers `url`: its kinds, artifact, cursor and limit applied. */
function answer(url: string, items: InboxItem[], total?: number) {
  const u = new URL(url, "http://x");
  const kinds = u.searchParams.get("kind")?.split(",");
  const art = u.searchParams.get("artifact");
  const before = Number(u.searchParams.get("before") ?? Infinity);
  const match = items.filter(i => (!kinds || kinds.includes(i.kind)) && (!art || (i.artifact?.id ?? "none") === art) && i.seq < before);
  if (u.searchParams.get("group") === "artifact") return ok({ groups: groupsOf(match, Number(u.searchParams.get("rows") ?? 3)), next_cursor: null, unread: 2, total: total ?? match.length });
  const limit = Number(u.searchParams.get("limit") ?? 50);
  return ok({ items: match.slice(0, limit), next_cursor: match.length > limit ? String(match[limit - 1].seq) : null, unread: 2, total: total ?? match.length });
}

/** The owner routes: `unread` items by search text and the `read` ones; every request is kept. */
function stubRoutes(unread: Record<string, InboxItem[]> = { "": [Q, R] }, read: InboxItem[] = READ) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    // The strip's fetches are counted apart: the sections' are what most tests follow.
    if (url === "/api/inbox?group=agent") { stripFetches++; return ok({ agents: STRIP, unread: 2 }); }
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
    if (url === "/api/artifacts") return ok({ artifacts: [] });
    if (url === "/api/inbox/read") return ok({ marked: 2, unread: 0 });
    const one = /^\/api\/inbox\/(\w+)\/read$/.exec(url);
    if (one) return ok({ item: { ...Object.values(unread).flat().find(i => i.id === one[1])!, read: true }, unread: 1 });
    const u = new URL(url, "http://x");
    if (u.searchParams.get("read") === "unread") return answer(url, unread[u.searchParams.get("q") ?? ""] ?? []);
    return answer(url, read, READ_TOTAL);
  }));
  return calls;
}

function page(go = vi.fn()) {
  const inbox = new InboxFeed();
  const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go, now });
  return { m, go, inbox };
}

beforeEach(() => { history.replaceState(null, "", "/inbox"); });
afterEach(() => { STRIP = []; stripFetches = 0; vi.useRealTimers(); vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("InboxPage", () => {
  it("shows open questions as cards, then the other unread items grouped by page, and folds the read ones behind their count", async () => {
    const S = item("reply", { id: "IS", seq: 7, artifact: { id: "9x8w7v6u5t4s", title: "Sales dashboard", kind: "html", page_url: null }, created_at: "2026-10-07T07:17:03.120Z" });
    const calls = stubRoutes({ "": [Q, R, S] });
    const { m } = page();
    await settle();
    const unread = m.root.querySelector('section[aria-labelledby="inbox-unread-h"]')!;
    expect(unread.querySelector("h3")!.textContent).toBe("Unread3");
    const card = unread.querySelector(".qcard")!;
    expect(card.getAttribute("data-question")).toBe(view().id);
    // The cards under their heading and count.
    expect(unread.querySelector(".pinned-h")!.textContent).toBe("Questions1");
    // One group per page, newest first; the question shows as its card alone.
    const groups = [...unread.querySelectorAll(".igroup")];
    expect(groups.map(g => [g.querySelector(".gtitle")!.textContent, g.querySelector(".gmeta")!.textContent])).toEqual([["Quarterly Review", "1 new · 5m"], ["Sales dashboard", "1 new · 2h"]]);
    expect(groups[0].querySelector("a.gtitle")!.getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect([...groups[0].querySelectorAll(".irow")].map(r => r.getAttribute("data-item"))).toEqual(["IR"]);
    // A grouped row leaves its page out of its title.
    expect(groups[0].querySelector(".irow .title")!.textContent).toBe("claude replied");
    expect(card.compareDocumentPosition(groups[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // A group with something new starts open; its toggle folds it.
    const toggle = groups[1].querySelector<HTMLButtonElement>(".gtoggle")!;
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(groups[1].querySelector(".irow")).toBeNull();
    expect(m.root.querySelector(".ihead .icnt")!.textContent).toBe("2 unread");
    const fold = m.root.querySelector<HTMLButtonElement>(".fold")!;
    expect(fold.textContent!.trim()).toBe("Show 1,240 read items");
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(m.root.querySelector("#inbox-read")).toBeNull();
    expect(calls.map(c => c.url)).toEqual(expect.arrayContaining([
      "/api/inbox?kind=question%2Capproval%2Cconnection&read=unread&limit=200", "/api/inbox?read=unread&group=artifact&rows=3", "/api/inbox?read=read&limit=1",
    ]));
    click(fold);
    await settle();
    // The read section's one group starts open, three rows, then more of it.
    const readGroup = m.root.querySelector("#inbox-read .igroup")!;
    expect(readGroup.querySelector(".gmeta")!.textContent).toBe("5m");
    expect(readGroup.querySelector(".gread")).toBeNull();
    expect([...readGroup.querySelectorAll(".irow")].map(r => r.getAttribute("data-item"))).toEqual(["D1", "D2", "D3"]);
    click(readGroup.querySelector(".gmore")!);
    await settle();
    expect([...readGroup.querySelectorAll(".irow")].map(r => r.getAttribute("data-item"))).toEqual(["D1", "D2", "D3", "D4", "D5"]);
    expect(calls.at(-1)!.url).toBe("/api/inbox?artifact=9x8w7v6u5t4s&read=read&before=3&limit=20");
    expect(readGroup.querySelector(".gmore")).toBeNull();
    expect(fold.textContent!.trim()).toBe("Hide read items");
  });

  it("marks one group read up to its newest item shown, and puts items about no page in Other", async () => {
    const S = item("reply", { id: "IS", seq: 7, artifact: { id: "9x8w7v6u5t4s", title: "Sales dashboard", kind: "html", page_url: null } });
    const calls = stubRoutes({ "": [R, S] }, [item("published", { id: "D9", seq: 2, read: true, artifact: null })]);
    const { m } = page();
    await settle();
    const group = m.root.querySelector('.igroup[data-group="7q3k9mzx2b4t"]')!;
    click(group.querySelector(".gread")!);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: "/api/inbox/read", method: "POST", body: { all: true, filter: { artifact: "7q3k9mzx2b4t" }, upto: 8 } });
    // No card, no heading over the cards.
    expect(m.root.querySelector(".pinned-h")).toBeNull();
    click(m.root.querySelector(".fold")!);
    await settle();
    // Read items about no page: one group, "Other", with no link.
    const other = m.root.querySelector('#inbox-read .igroup[data-group="none"]')!;
    expect(other.querySelector(".gtitle")!.textContent).toBe("Other");
    expect(other.querySelector("a.gtitle")).toBeNull();
  });

  it("moves a group's new count as its items are read elsewhere, keeping the row where it is", async () => {
    const S = item("reply", { id: "IS", seq: 7 });
    stubRoutes({ "": [R, S] });
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    const meta = () => m.root.querySelector(".igroup .gmeta")!.textContent;
    expect(meta()).toBe("2 new · 5m");
    (inbox as unknown as { tell(c: unknown): void }).tell({ item: { ...S, read: true } });
    flush();
    expect(meta()).toBe("1 new · 5m");
    expect(m.root.querySelector("#inbox-unread-h")!.textContent).toBe("Unread1");
    expect(m.root.querySelector('.irow[data-item="IS"]')!.hasAttribute("data-unread")).toBe(false);
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
    expect(calls.slice(n).map(c => c.url)).toEqual([`/api/inbox?q=dash&kind=question%2Capproval%2Cconnection&read=unread&limit=200`, "/api/inbox?q=dash&read=unread&group=artifact&rows=3", "/api/inbox?q=dash&read=read&limit=1"]);
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

  it("fetches its first page once: when the inbox topic first goes live", async () => {
    const calls = stubRoutes();
    const inbox = new InboxFeed();
    const handlers: ((e: { type: string }) => void)[] = [];
    inbox.start({ watch: (_t: readonly string[], on: (e: { type: string }) => void) => { handlers.push(on); return () => {}; } } as never);
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    const pages = () => calls.filter(c => c.url.startsWith("/api/inbox?")).map(c => c.url);
    expect(pages()).toEqual([]);
    handlers[0]({ type: "ready" });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(pages()).toEqual(["/api/inbox?kind=question%2Capproval%2Cconnection&read=unread&limit=200", "/api/inbox?read=unread&group=artifact&rows=3", "/api/inbox?read=read&limit=1"]);
    expect(m.root.querySelectorAll(".irow").length).toBeGreaterThan(0);
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
  it("shows approval and connection requests as cards after the questions and before the groups, and ended agents and invited machines as rows in Other", async () => {
    const A = item("approval", { id: "IA", seq: 5 });
    const C = item("connection", { id: "IC", seq: 6 });
    const J = item("connection", { id: "IJ", seq: 4, connection: { id: "g_j", status: "approved", via: "invite", client: { name: "kim-pc", key: "SHA256:kk", peer_ip: "192.168.1.9" }, person: { public_id: "p9", display_name: "Kim" } } });
    const E = item("ended", { id: "IE", seq: 3, artifact: null });
    stubRoutes({ "": [Q, C, R, A, J, E].sort((a, b) => b.seq - a.seq) });
    const { m } = page();
    await settle();
    const unread = m.root.querySelector('section[aria-labelledby="inbox-unread-h"]')!;
    const order = [...unread.querySelectorAll(".qcard, .acard, .ccard, .irow")].map(e => e.className.split(" ")[0] + (e.getAttribute("data-item") ? `:${e.getAttribute("data-item")}` : ""));
    expect(order).toEqual(["qcard", "acard", "ccard", "irow:IR", "irow:IJ", "irow:IE"]);
    // Questions and requests together: what waits on the person.
    expect(unread.querySelector(".pinned-h")!.textContent).toBe("Waiting on you3");
    expect(unread.querySelector('.irow[data-item="IJ"]')!.textContent).toContain("Kim connected from kim-pc");
    expect(unread.querySelector('.irow[data-item="IE"] .released a')!.textContent).toBe("Quarterly Review");
    // The connection request is a card: Other counts the two rows new.
    const other = unread.querySelector('.igroup[data-group="none"]')!;
    expect([other.querySelector(".gtitle")!.textContent, other.querySelector(".gmeta")!.textContent]).toEqual(["Other", "2 new · 5m"]);
  });

  it("offers Mark all read only while something unread could be read: an open approval request stays unread until decided", async () => {
    stubRoutes({ "": [item("approval", { id: "IA", seq: 5 })] });
    const { m } = page();
    await settle();
    const markAll = [...m.root.querySelectorAll("button")].find(b => b.textContent === "Mark all read") as HTMLButtonElement;
    expect(markAll.disabled).toBe(true);
  });

  it("keeps an approval's card where it is once decided, through a refetch, once, with the counts moved", async () => {
    const A = item("approval", { id: "IA", seq: 6 });
    const lists: Record<string, InboxItem[]> = { "": [A, R] };
    stubRoutes(lists);
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    expect(m.root.querySelector("#inbox-unread-h")!.textContent).toBe("Unread2");
    const decided = { ...A, read: true, approval: approvalCard({ status: "approved", scope: "thread" }) };
    (inbox as unknown as { tell(c: unknown): void }).tell({ item: decided });
    await settle();
    expect(m.root.querySelector(".acard .outcome")!.textContent).toContain("Allowed in this thread");
    // Read now: Unread counts one fewer.
    expect(m.root.querySelector("#inbox-unread-h")!.textContent).toBe("Unread1");
    // Fetched again, the daemon no longer lists it as unread: the card stays.
    lists[""] = [R];
    (inbox as unknown as { tell(c: unknown): void }).tell({ refetch: true });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(m.root.querySelectorAll(".acard")).toHaveLength(1);
    expect(m.root.querySelector(".acard .outcome")!.textContent).toContain("Allowed in this thread");
  });

  it("shows a decided card once when the read section, which lists it too, is open", async () => {
    const A = item("approval", { id: "IA", seq: 6 });
    const decided = { ...A, read: true, approval: approvalCard({ status: "declined" }) };
    const calls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      calls.push(url);
      if (url === "/api/artifacts") return ok({ artifacts: [] });
      const u = new URL(url, "http://x");
      return answer(url, u.searchParams.get("read") === "unread" ? [A] : [decided]);
    }));
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    (inbox as unknown as { tell(c: unknown): void }).tell({ item: decided });
    await settle();
    click(m.root.querySelector(".fold")!);
    await settle();
    expect(m.root.querySelectorAll('.acard[data-approval="01J9R0000000000000000000AA"]')).toHaveLength(1);
  });

  it("offers the Connections filter to the owner alone", async () => {
    stubRoutes();
    const { m } = page();
    await settle();
    const kinds = () => [...m.root.querySelectorAll(".kind")].map(k => k.textContent);
    expect(kinds()).not.toContain("Connections");
    expect(kinds()).toEqual(expect.arrayContaining(["Approvals", "Ended"]));
  });

  it("goes to the approval's card for `?approval=`, focused", async () => {
    history.replaceState(null, "", "/inbox?approval=01J9R0000000000000000000AA");
    stubRoutes({ "": [item("approval", { id: "IA", seq: 6 }), R] });
    const { m } = page();
    await settle();
    expect(document.activeElement).toBe(m.root.querySelector(".acard"));
  });

  it("filters by the approval, connection and ended kinds", async () => {
    history.replaceState(null, "", "/inbox?kind=connection");
    const calls = stubRoutes();
    const { m } = page();
    await settle();
    expect(m.root.querySelector('.kind[aria-pressed="true"]')!.textContent).toBe("Connections");
    expect(calls.map(c => c.url)).toEqual(expect.arrayContaining(["/api/inbox?kind=connection&read=unread&limit=200", "/api/inbox?kind=connection&read=unread&group=artifact&rows=3"]));
    expect([...m.root.querySelectorAll(".kind")].map(k => k.textContent)).toEqual(expect.arrayContaining(["Approvals", "Connections", "Ended"]));
  });

  it("opens with the agent strip, whose chip filters the inbox by its agent", async () => {
    STRIP = [
      { handle: "a_web", name: "web", harness: "codex", live: true, unread: 2, blocked: null },
      { handle: "a_cl", name: "clax-claude", harness: "claude", live: true, unread: 1, blocked: { since: "2026-10-07T09:05:03.120Z", questions: 1, approvals: 0 } },
    ];
    const calls = stubRoutes();
    const { m } = page();
    await settle();
    expect(stripFetches).toBeGreaterThan(0);
    const strip = m.root.querySelector(".astrip")!;
    // Above the sections.
    expect(strip.compareDocumentPosition(m.root.querySelector(".sec")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const chips = [...strip.querySelectorAll<HTMLButtonElement>(".achip")];
    expect(chips.map(c => c.textContent!.replace(/\s+/g, " ").trim())).toEqual(["clax-claude · waiting on you 12 min (1 question)", "web · 2 unread"]);
    click(chips[0]);
    await settle();
    expect(location.search).toBe("?agent=a_cl");
    expect(calls.map(c => c.url)).toContain("/api/inbox?agent=a_cl&read=unread&group=artifact&rows=3");
    expect(m.root.querySelector('.achip[aria-pressed="true"]')!.textContent).toContain("clax-claude");
    click(m.root.querySelector('.achip[aria-pressed="true"]')!);
    await settle();
    expect(location.search).toBe("");
  });
});
