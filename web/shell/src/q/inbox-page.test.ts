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
/** Read items on Sales dashboard. */
const READ = [1, 2, 3, 4, 5].map(n => item("published", { id: `D${n}`, seq: 6 - n, read: true }));

/** The agent strip the daemon answers: one agent waiting on you since 09:05:03. */
let STRIP: unknown[] = [];
let stripFetches = 0;

type Call = { url: string; method: string; body: unknown };
const ok = (b: unknown) => new Response(JSON.stringify(b));
const bySeq = (a: InboxItem, b: InboxItem) => b.seq - a.seq;

/** `items` as `GET /api/inbox?group=artifact` groups them: newest group
 * first, each listing its unread items newest first, then its read ones,
 * `rows` in all, with where the rest begins (`u<seq>`, `r<seq>`, `r`). */
function groupsOf(items: InboxItem[], rows: number) {
  const by = new Map<string, InboxItem[]>();
  for (const i of [...items].sort(bySeq)) by.set(i.artifact?.id ?? "none", [...(by.get(i.artifact?.id ?? "none") ?? []), i]);
  return [...by.values()].map(g => {
    const unread = g.filter(i => !i.read), read = g.filter(i => i.read);
    let shown = unread.slice(0, rows), next: string | null = unread.length > rows ? `u${unread[rows - 1].seq}` : null;
    if (!next && read.length) {
      const left = rows - shown.length;
      if (!left) next = "r";
      else { shown = [...shown, ...read.slice(0, left)]; next = read.length > left ? `r${read[left - 1].seq}` : null; }
    }
    return { artifact_id: g[0].artifact?.id ?? null, artifact: g[0].artifact, total: g.length, unread: unread.length, newest_at: g[0].created_at, items: shown, next_cursor: next, newest: g[0].seq };
  });
}

/** A list or grouping of `items` as the daemon answers `url`: its read state, kinds, artifact, cursor and limit applied. */
function answer(url: string, items: InboxItem[]) {
  const u = new URL(url, "http://x");
  const read = u.searchParams.get("read");
  const kinds = u.searchParams.get("kind")?.split(",");
  const art = u.searchParams.get("artifact");
  const before = Number(u.searchParams.get("before") ?? Infinity);
  const grouped = u.searchParams.get("group") === "artifact";
  const match = items.filter(i => (read === "unread" ? !i.read : read === "read" ? i.read : true) && (!kinds || kinds.includes(i.kind)) && (!art || (i.artifact?.id ?? "none") === art) && (grouped || i.seq < before)).sort(bySeq);
  if (grouped) {
    // Groups page by their newest item, 20 a page unless `limit` says.
    const all = groupsOf(match, Number(u.searchParams.get("rows") ?? 3)).filter(g => g.newest < before);
    const size = Number(u.searchParams.get("limit") ?? 20);
    return ok({ groups: all.slice(0, size), next_cursor: all.length > size ? String(all[size - 1].newest) : null, unread: 2 });
  }
  const limit = Number(u.searchParams.get("limit") ?? 50);
  return ok({ items: match.slice(0, limit), next_cursor: match.length > limit ? String(match[limit - 1].seq) : null, unread: 2, total: match.length });
}

/** The owner routes: the unread items by search text, and the read ones; every request is kept. */
function stubRoutes(unread: Record<string, InboxItem[]> = { "": [Q, R] }, read: InboxItem[] = READ) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    // The strip's fetches are counted apart: the list's are what most tests follow.
    if (url === "/api/inbox?group=agent") { stripFetches++; return ok({ agents: STRIP, unread: 2 }); }
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
    if (url === "/api/artifacts") return ok({ artifacts: [] });
    if (url === "/api/inbox/read") return ok({ marked: 2, unread: 0 });
    const one = /^\/api\/inbox\/(\w+)\/read$/.exec(url);
    if (one) return ok({ item: { ...Object.values(unread).flat().find(i => i.id === one[1])!, read: true }, unread: 1 });
    const q = new URL(url, "http://x").searchParams.get("q") ?? "";
    return answer(url, [...(unread[q] ?? []), ...(q ? [] : read)]);
  }));
  return calls;
}

function page(go = vi.fn()) {
  const inbox = new InboxFeed();
  const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go, now });
  return { m, go, inbox };
}
const groupEl = (root: Element, key: string) => root.querySelector(`.igroup[data-group="${key}"]`)!;
const rowIds = (el: Element) => [...el.querySelectorAll(".irow")].map(r => r.getAttribute("data-item"));
const tell = (inbox: InboxFeed, c: unknown) => (inbox as unknown as { tell(c: unknown): void }).tell(c);

beforeEach(() => { history.replaceState(null, "", "/inbox"); });
afterEach(() => { STRIP = []; stripFetches = 0; vi.useRealTimers(); vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("InboxPage", () => {
  it("lists each page once: its group bold and open with something new, a quiet folded line without", async () => {
    const calls = stubRoutes();
    const { m } = page();
    await settle();
    const card = m.root.querySelector(".qcard")!;
    expect(card.getAttribute("data-question")).toBe(view().id);
    // The cards under their heading and count, on top.
    expect(m.root.querySelector(".pinned-h")!.textContent).toBe("Questions1");
    // One group per page, newest first; no Read section.
    const groups = [...m.root.querySelectorAll(".igroup")];
    expect(groups.map(g => [g.querySelector(".gtitle")!.textContent, g.querySelector(".gmeta")!.textContent, g.hasAttribute("data-fresh")]))
      .toEqual([["Quarterly Review, 1 new", "1 new · 5m", true], ["Sales dashboard", "5m", false]]);
    expect(m.root.querySelector(".fold, #inbox-read")).toBeNull();
    expect(card.compareDocumentPosition(groups[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(groups[0].querySelector("h3.gtitle a")!.getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    // The toggle is named by the group's heading, its state in aria-expanded, and controls the body.
    const [first, second] = groups.map(g => g.querySelector<HTMLButtonElement>(".gtoggle")!);
    // Its name says how many are new; the visible count is not read twice.
    expect(document.getElementById(first.getAttribute("aria-labelledby")!)!.textContent).toBe("Quarterly Review, 1 new");
    expect(groups[0].querySelector(".gnew")!.getAttribute("aria-hidden")).toBe("true");
    expect(document.getElementById(second.getAttribute("aria-labelledby")!)!.textContent).toBe("Sales dashboard");
    expect(document.getElementById(first.getAttribute("aria-controls")!)!.classList.contains("gbody")).toBe(true);
    // Something new: open, its new row first (the question is its card alone). Nothing new: folded, no Mark read.
    expect([first.getAttribute("aria-expanded"), second.getAttribute("aria-expanded")]).toEqual(["true", "false"]);
    expect(rowIds(groups[0])).toEqual(["IR"]);
    expect(groups[0].querySelector(".irow .title")!.textContent).toBe("claude replied");
    expect(groups[1].querySelector<HTMLElement>(".gbody")!.hidden).toBe(true);
    expect(groups[1].querySelector(".gread")).toBeNull();
    expect(calls.map(c => c.url)).toEqual(expect.arrayContaining(["/api/inbox?kind=question%2Capproval%2Cconnection&read=unread&limit=200", "/api/inbox?group=artifact&rows=3"]));
    // The quiet line opens in place: its newest read rows, then more of it.
    click(second);
    expect(second.getAttribute("aria-expanded")).toBe("true");
    expect(rowIds(groups[1])).toEqual(["D1", "D2", "D3"]);
    click(groups[1].querySelector(".gmore")!);
    await settle();
    expect(rowIds(groups[1])).toEqual(["D1", "D2", "D3", "D4", "D5"]);
    expect(calls.at(-1)!.url).toBe("/api/inbox?artifact=9x8w7v6u5t4s&read=read&before=3&limit=20");
    expect(groups[1].querySelector(".gmore")).toBeNull();
  });

  it("pages a group's new items first, then its read ones", async () => {
    const unread = [12, 11, 10, 9].map(n => item("reply", { id: `U${n}`, seq: n }));
    const read = [7, 6].map(n => item("reply", { id: `R${n}`, seq: n, read: true }));
    const calls = stubRoutes({ "": unread }, read);
    const { m } = page();
    await settle();
    const g = groupEl(m.root, "7q3k9mzx2b4t");
    expect([rowIds(g), g.querySelector(".gmeta")!.textContent]).toEqual([["U12", "U11", "U10"], "4 new · 5m"]);
    click(g.querySelector(".gmore")!);
    await settle();
    expect(calls.at(-1)!.url).toBe("/api/inbox?artifact=7q3k9mzx2b4t&read=unread&before=10&limit=20");
    expect(rowIds(g)).toEqual(["U12", "U11", "U10", "U9"]);
    click(g.querySelector(".gmore")!);
    await settle();
    expect(calls.at(-1)!.url).toBe("/api/inbox?artifact=7q3k9mzx2b4t&read=read&limit=20");
    expect(rowIds(g)).toEqual(["U12", "U11", "U10", "U9", "R7", "R6"]);
    expect(g.querySelector(".gmore")).toBeNull();
  });

  it("narrows the list to groups with something new with the Unread chip, kept in the URL", async () => {
    const calls = stubRoutes();
    const { m } = page();
    await settle();
    const chip = [...m.root.querySelectorAll<HTMLButtonElement>(".kind")].find(b => b.textContent === "Unread")!;
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    click(chip);
    await settle();
    expect(location.search).toBe("?unread=1");
    expect(calls.at(-1)!.url).toBe("/api/inbox?read=unread&group=artifact&rows=3");
    expect([...m.root.querySelectorAll(".igroup .gtitle")].map(t => t.textContent)).toEqual(["Quarterly Review, 1 new"]);
    expect(chip.getAttribute("aria-pressed")).toBe("true");
    m.unmount();
    history.replaceState(null, "", "/inbox?unread=1");
    const again = page();
    await settle();
    expect([...again.m.root.querySelectorAll<HTMLButtonElement>(".kind")].find(b => b.textContent === "Unread")!.getAttribute("aria-pressed")).toBe("true");
  });

  it("marks one group read, but its page's pinned question, and keeps it open where it is", async () => {
    const S = item("reply", { id: "IS", seq: 7, artifact: { id: "9x8w7v6u5t4s", title: "Sales dashboard", kind: "html", page_url: null } });
    const lists: Record<string, InboxItem[]> = { "": [Q, R, S] };
    const calls = stubRoutes(lists, []);
    const { m } = page();
    await settle();
    const g = groupEl(m.root, "7q3k9mzx2b4t");
    expect(g.querySelector(".gmeta")!.textContent).toBe("1 new · 5m");
    // Read now: the daemon lists the reply as read.
    lists[""] = [Q, { ...R, read: true }, S];
    click(g.querySelector(".gread")!);
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({ all: true, filter: { artifact: "7q3k9mzx2b4t" }, upto: 9, except: ["IQ"] });
    // In place, still open, nothing new; the card stays pinned.
    const after = [...m.root.querySelectorAll(".igroup")];
    expect(after.map(x => x.getAttribute("data-group"))).toEqual(["7q3k9mzx2b4t", "9x8w7v6u5t4s"]);
    expect([after[0].querySelector(".gtoggle")!.getAttribute("aria-expanded"), after[0].hasAttribute("data-fresh"), rowIds(after[0])]).toEqual(["true", false, ["IR"]]);
    expect(m.root.querySelector(".qcard")).not.toBeNull();
  });

  it("keeps the groups paged in by More pages and the rows by Show more through a group's Mark read and a live refetch", async () => {
    // A page with five new items, and 21 pages with one each: two pages of groups.
    const big = [1005, 1004, 1003, 1002, 1001].map(n => item("reply", { id: `B${n}`, seq: n }));
    const small = Array.from({ length: 21 }, (_, n) => item("reply", { id: `S${n}`, seq: 100 + n, artifact: { id: `pg${n}`, title: `Page ${n}`, kind: "html", page_url: null } }));
    const lists: Record<string, InboxItem[]> = { "": [...big, ...small] };
    const calls = stubRoutes(lists, []);
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    const keys = () => [...m.root.querySelectorAll(".igroup")].map(g => g.getAttribute("data-group"));
    expect(keys()).toHaveLength(20);
    click(m.root.querySelector(".inbox > .sec > .more")!);
    await settle();
    expect(keys()).toHaveLength(22);
    const bigGroup = groupEl(m.root, "7q3k9mzx2b4t");
    click(bigGroup.querySelector(".gmore")!);
    await settle();
    expect(rowIds(bigGroup)).toEqual(["B1005", "B1004", "B1003", "B1002", "B1001"]);
    const before = keys();
    // Mark read on a group of the second page: it and every group stay, and the big group keeps its five rows.
    lists[""] = lists[""].map(i => (i.id === "S0" ? { ...i, read: true } : i));
    click(groupEl(m.root, "pg0").querySelector(".gread")!);
    await settle();
    expect(calls.at(-1)!.url).toBe("/api/inbox?limit=22&group=artifact&rows=3");
    expect(keys()).toEqual(before);
    expect(rowIds(groupEl(m.root, "7q3k9mzx2b4t"))).toEqual(["B1005", "B1004", "B1003", "B1002", "B1001"]);
    expect(groupEl(m.root, "pg0").hasAttribute("data-fresh")).toBe(false);
    // A live refetch (the stream resynced) keeps them too.
    tell(inbox, { refetch: true });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(keys()).toEqual(before);
    expect(rowIds(groupEl(m.root, "7q3k9mzx2b4t"))).toHaveLength(5);
  });

  it("keeps a group's rows where they are after its Mark read, the row it read shown read", async () => {
    const U = item("reply", { id: "U5", seq: 5 });
    const read = [9, 8, 7].map(n => item("reply", { id: `R${n}`, seq: n, read: true }));
    const lists: Record<string, InboxItem[]> = { "": [U] };
    stubRoutes(lists, read);
    const { m } = page();
    await settle();
    const g = groupEl(m.root, "7q3k9mzx2b4t");
    expect(rowIds(g)).toEqual(["U5", "R9", "R8"]);
    // Read now: the daemon's first rows would be R9, R8, R7.
    lists[""] = [{ ...U, read: true }];
    click(g.querySelector(".gread")!);
    await settle();
    expect(rowIds(g)).toEqual(["U5", "R9", "R8"]);
    expect(g.querySelector('.irow[data-item="U5"]')!.hasAttribute("data-unread")).toBe(false);
    expect(g.hasAttribute("data-fresh")).toBe(false);
  });

  it("keeps a group read under the Unread chip where it is, shown read, until the next load", async () => {
    history.replaceState(null, "", "/inbox?unread=1");
    const S = item("reply", { id: "IS", seq: 7, artifact: { id: "9x8w7v6u5t4s", title: "Sales dashboard", kind: "html", page_url: null } });
    const lists: Record<string, InboxItem[]> = { "": [R, S] };
    stubRoutes(lists, []);
    const { m } = page();
    await settle();
    const keys = () => [...m.root.querySelectorAll(".igroup")].map(g => g.getAttribute("data-group"));
    expect(keys()).toEqual(["7q3k9mzx2b4t", "9x8w7v6u5t4s"]);
    lists[""] = [{ ...R, read: true }, S];
    click(groupEl(m.root, "7q3k9mzx2b4t").querySelector(".gread")!);
    await settle();
    // The daemon no longer lists it under Unread; it stays, read.
    expect(keys()).toEqual(["7q3k9mzx2b4t", "9x8w7v6u5t4s"]);
    const g = groupEl(m.root, "7q3k9mzx2b4t");
    expect([g.hasAttribute("data-fresh"), g.querySelector(".irow")!.hasAttribute("data-unread")]).toEqual([false, false]);
    // The next load (the filter changes and back) drops it.
    const chip = [...m.root.querySelectorAll<HTMLButtonElement>(".kind")].find(b => b.textContent === "Unread")!;
    click(chip);
    await settle();
    click(chip);
    await settle();
    expect(keys()).toEqual(["9x8w7v6u5t4s"]);
  });

  it("puts items about no page in Other, with no link", async () => {
    stubRoutes({ "": [R] }, [item("published", { id: "D9", seq: 2, read: true, artifact: null })]);
    const { m } = page();
    await settle();
    const other = groupEl(m.root, "none");
    expect(other.querySelector(".gtitle")!.textContent).toBe("Other");
    expect(other.querySelector(".gtitle a")).toBeNull();
  });

  it("moves a group's new count as its items are read elsewhere, keeping the row and the group where they are", async () => {
    const S = item("reply", { id: "IS", seq: 7 });
    stubRoutes({ "": [R, S] }, []);
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    const meta = () => m.root.querySelector(".igroup .gmeta")!.textContent;
    expect(meta()).toBe("2 new · 5m");
    tell(inbox, { item: { ...S, read: true } });
    flush();
    expect(meta()).toBe("1 new · 5m");
    expect(m.root.querySelector('.irow[data-item="IS"]')!.hasAttribute("data-unread")).toBe(false);
    tell(inbox, { item: { ...R, read: true } });
    flush();
    // Nothing new now, and still open.
    expect([meta(), m.root.querySelector(".igroup .gtoggle")!.getAttribute("aria-expanded")]).toEqual(["5m", "true"]);
  });

  it("fetches the list again when an item of a shown group, past its listed rows, is read elsewhere", async () => {
    const more = [7, 6, 5, 4].map(n => item("reply", { id: `I${n}`, seq: n }));
    const lists: Record<string, InboxItem[]> = { "": [R, ...more] };
    stubRoutes(lists, []);
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    const meta = () => m.root.querySelector(".igroup .gmeta")!.textContent;
    expect(meta()).toBe("5 new · 5m");
    // The fourth, not listed, is read in another tab.
    lists[""] = [R, ...more.map(i => (i.id === "I5" ? { ...i, read: true } : i))];
    tell(inbox, { item: { ...more[2], read: true } });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(meta()).toBe("4 new · 5m");
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
    expect(calls.slice(n).map(c => c.url)).toEqual([`/api/inbox?q=dash&kind=question%2Capproval%2Cconnection&read=unread&limit=200`, "/api/inbox?q=dash&group=artifact&rows=3"]);
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
    // The list is fetched again after.
    expect(calls.filter(c => c.url.startsWith("/api/inbox?q=dash")).length).toBe(2);
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
    expect(pages()).toEqual(["/api/inbox?kind=question%2Capproval%2Cconnection&read=unread&limit=200", "/api/inbox?group=artifact&rows=3"]);
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
    stubRoutes({ "": [Q, C, R, A, J, E] }, []);
    const { m } = page();
    await settle();
    const unread = m.root;
    const order = [...unread.querySelectorAll(".qcard, .acard, .ccard, .irow")].map(e => e.className.split(" ")[0] + (e.getAttribute("data-item") ? `:${e.getAttribute("data-item")}` : ""));
    expect(order).toEqual(["qcard", "acard", "ccard", "irow:IR", "irow:IJ", "irow:IE"]);
    // Questions and requests together: what waits on the person.
    expect(unread.querySelector(".pinned-h")!.textContent).toBe("Waiting on you3");
    expect(unread.querySelector('.irow[data-item="IJ"]')!.textContent).toContain("Kim connected from kim-pc");
    expect(unread.querySelector('.irow[data-item="IE"] .released a')!.textContent).toBe("Quarterly Review");
    // The connection request is a card: Other counts the two rows new.
    const other = unread.querySelector('.igroup[data-group="none"]')!;
    expect([other.querySelector(".gtitle")!.textContent, other.querySelector(".gmeta")!.textContent]).toEqual(["Other, 2 new", "2 new · 5m"]);
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
    stubRoutes(lists, []);
    const inbox = new InboxFeed();
    const m = mount(InboxPage, { inbox, questions: new QuestionFeed(), go: vi.fn(), now });
    await settle();
    const decided = { ...A, read: true, approval: approvalCard({ status: "approved", scope: "thread" }) };
    (inbox as unknown as { tell(c: unknown): void }).tell({ item: decided });
    await settle();
    expect(m.root.querySelector(".acard .outcome")!.textContent).toContain("Allowed in this thread");
    // Fetched again, the daemon no longer lists it as unread: the card stays.
    lists[""] = [R];
    (inbox as unknown as { tell(c: unknown): void }).tell({ refetch: true });
    await new Promise(r => setTimeout(r, 300));
    await settle();
    expect(m.root.querySelectorAll(".acard")).toHaveLength(1);
    expect(m.root.querySelector(".acard .outcome")!.textContent).toContain("Allowed in this thread");
  });

  it("shows a decided card once, pinned, though its group lists it too once fetched again", async () => {
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
    (inbox as unknown as { tell(c: unknown): void }).tell({ refetch: true });
    await new Promise(r => setTimeout(r, 300));
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
    expect(calls.map(c => c.url)).toEqual(expect.arrayContaining(["/api/inbox?kind=connection&read=unread&limit=200", "/api/inbox?kind=connection&group=artifact&rows=3"]));
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
    expect(calls.map(c => c.url)).toContain("/api/inbox?agent=a_cl&group=artifact&rows=3");
    expect(m.root.querySelector('.achip[aria-pressed="true"]')!.textContent).toContain("clax-claude");
    click(m.root.querySelector('.achip[aria-pressed="true"]')!);
    await settle();
    expect(location.search).toBe("");
  });
});
