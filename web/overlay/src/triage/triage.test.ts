import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import QueueView from "./QueueView.svelte";
import ReportView from "./ReportView.svelte";
import { type Queue, type Report, type TriageApi, aboutOf, arrange, changes, claimText, metaOf, pageOf, readAll, routeOf, selectorOf, serial, titleOf } from "./triage";

const QID = "01M66XHDKTMTCVB95FE9BQN04V";
const queue: Queue = { id: QID, name: "Changeling feedback", origins: ["http://127.0.0.1:7700"], reporters_see: "own", created_at: "2026-11-01T00:00:00Z", role: "admin" };
let n = 0;
function report(over: Partial<Report> & { note?: string; url?: string } = {}): Report {
  const id = `01J9${String(++n).padStart(22, "A")}`;
  const url = over.url ?? "http://127.0.0.1:7700/";
  return {
    id, queue_id: QID, status: "open", state: "open", reporter_id: "r1", metadata: null, metadata_version: 0, has_clip: false, claim: null,
    artifact: { id: "a1", external_id: new URL(url).pathname, title: "Dashboard", url, metadata: null, metadata_version: 0, revision: null },
    anchor: { kind: "element", selector: "#save", quote: "Save", label: "Save button" },
    comments: [{ id: `${id}c`, author_kind: "reporter", author_name: "Alex", body: over.note ?? "Save does nothing", created_at: "2026-11-02T10:00:00Z" }],
    created_at: `2026-11-02T10:00:${String(n).padStart(2, "0")}Z`,
    ...over,
  };
}
/** A queue of `reports` in memory, whose `ping` tells every watcher that something changed. */
function fakeApi(reports: Report[]) {
  const watchers = new Set<() => void>();
  const api: TriageApi & { data: Report[]; ping(): void } = {
    data: reports,
    ping: () => watchers.forEach(f => f()),
    queues: async () => [queue],
    reports: async () => ({ reports: api.data, next: null }),
    report: async (_q, t) => { const r = api.data.find(x => x.id === t); if (!r) throw new Error("No such report."); return r; },
    clip: async () => "data:image/png;base64,AAAA",
    reply: async (_q, t, body) => {
      const r = api.data.find(x => x.id === t)!;
      const next = { ...r, comments: [...r.comments, { id: `${t}r${r.comments.length}`, author_kind: "viewer" as const, author_name: "Bobby", body, created_at: "2026-11-02T11:00:00Z" }] };
      api.data = api.data.map(x => (x.id === t ? next : x));
      return next;
    },
    resolve: async (_q, t) => { const r = { ...api.data.find(x => x.id === t)!, status: "resolved" as const, state: "resolved" as const }; api.data = api.data.map(x => (x.id === t ? r : x)); return r; },
    watch: f => { watchers.add(f); return () => watchers.delete(f); },
  };
  return api;
}
afterEach(() => cleanup());

describe("the triage model", () => {
  it("reads a queue's and a report's own paths", () => {
    expect(routeOf(`/queues/${QID}`)).toEqual({ qid: QID, tid: null });
    expect(routeOf(`/queues/${QID}/01J9X/`)).toEqual({ qid: QID, tid: "01J9X" });
    expect(routeOf("/queues")).toBeNull();
    expect(routeOf(`/queues/${QID}/a/b`)).toBeNull();
  });

  it("names a report by its note's first line, and says what it is about", () => {
    expect(titleOf(report({ note: "\n  First line\nsecond" }))).toBe("First line");
    expect(titleOf(report({ note: "x".repeat(100) }))).toHaveLength(80);
    expect(titleOf(report({ comments: [] }))).toBe("Report without a note");
    expect(aboutOf(report())).toBe("“Save button”");
    expect(aboutOf(report({ anchor: { kind: "artifact" } }))).toBe("The whole page");
    expect(aboutOf(report({ anchor: { kind: "area" } }))).toBe("An area of the page");
    expect(selectorOf(report())).toBe("#save");
    expect(selectorOf(report({ anchor: { kind: "artifact", selector: "x" } }))).toBeNull();
  });

  it("opens a report's page with its route, and only an http(s) one", () => {
    expect(pageOf(report({ url: "http://127.0.0.1:7700/pr?x=1", anchor: { kind: "element", route: "#/tab" } }))).toBe("http://127.0.0.1:7700/pr#/tab");
    expect(pageOf(report({ url: "javascript:alert(1)" as never }))).toBeNull();
  });

  it("puts the page's reports first, newest first, filtered by state", () => {
    const a = report({ url: "http://127.0.0.1:7700/a" }), b = report({ url: "http://127.0.0.1:7700/b", state: "resolved", status: "resolved" }), c = report({ url: "http://127.0.0.1:7700/a?q=1" });
    expect(arrange([a, b, c], "all", "http://127.0.0.1:7700/a#x")).toEqual({ here: [c, a], rest: [b] });
    expect(arrange([a, b, c], "resolved", null)).toEqual({ here: [], rest: [b] });
  });

  it("says what changed between two reads", () => {
    const a = report(), b = report();
    expect(changes([a], [a, b])).toBe("1 new report.");
    expect(changes([a, b], [{ ...a, state: "claimed" }, { ...b, comments: [...b.comments, b.comments[0]] }])).toBe("1 report claimed. New reply on 1 report.");
    expect(changes([a], [a])).toBe("");
  });

  it("reads a report's metadata as flat text pairs only", () => {
    expect(metaOf(report({ metadata: { build: "1", n: 2, ok: true, nested: { x: 1 }, list: [1] } }))).toEqual([["build", "1"], ["n", "2"], ["ok", "true"]]);
    expect(metaOf(report({ metadata: ["x"] }))).toEqual([]);
    expect(metaOf(report())).toEqual([]);
  });

  it("says who holds a claim and for how long", () => {
    expect(claimText(report({ claim: { holder_name: "claude", expires_at: "2026-11-02T10:05:00Z" } }), Date.parse("2026-11-02T10:00:00Z"))).toBe("Claimed by claude, 5 min left");
    expect(claimText(report())).toBeNull();
  });

  it("reads every page, at most MAX_PAGES", async () => {
    const pages = vi.fn(async (_q: string, c: string | null) => ({ reports: [report()], next: c === null ? "2" : c === "2" ? null : null }));
    expect((await readAll({ reports: pages }, QID)).reports).toHaveLength(2);
    const endless = { reports: async () => ({ reports: [report()], next: "more" }) };
    expect(await readAll(endless, QID)).toMatchObject({ more: true });
  });

  it("runs one read at a time, and once more for calls made meanwhile", async () => {
    let runs = 0;
    let release!: () => void;
    const run = serial(() => { runs++; return new Promise<void>(r => { release = r; }); });
    const first = run();
    void run(); void run();
    release();
    await Promise.resolve(); await Promise.resolve();
    release();
    await first;
    expect(runs).toBe(2);
  });
});

describe("QueueView", () => {
  it("lists the queue's reports as links to their own pages, this page's first, and filters by state", async () => {
    const here = report({ note: "Here", url: "http://127.0.0.1:7700/x" });
    const there = report({ note: "There", url: "http://127.0.0.1:7700/y", state: "claimed", claim: { holder_name: "claude", expires_at: new Date(Date.now() + 600_000).toISOString() } });
    render(QueueView, { props: { api: fakeApi([here, there]), queue, page: "http://127.0.0.1:7700/x", href: (r: Report) => `/queues/${QID}/${r.id}` } });
    expect(screen.getByRole("heading", { name: "Changeling feedback" })).toBeTruthy();
    const link = await screen.findByRole("link", { name: /^Here/ });
    expect(link.getAttribute("href")).toBe(`/queues/${QID}/${here.id}`);
    const headings = screen.getAllByRole("heading", { level: 3 }).map(h => h.textContent);
    expect(headings).toEqual(["On this page", "Elsewhere"]);
    expect(screen.getByRole("link", { name: /^There.*Claimed by claude/ })).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: /^Claimed/ }));
    expect(screen.queryByRole("link", { name: /^Here/ })).toBeNull();
  });

  it("follows the queue live: a new report appears in place and is said politely", async () => {
    const api = fakeApi([report({ note: "First" })]);
    render(QueueView, { props: { api, queue, href: () => "#" } });
    await screen.findByRole("link", { name: /^First/ });
    const status = screen.getByRole("status");
    expect(status.textContent).toBe("");
    api.data = [...api.data, report({ note: "Second" })];
    api.ping();
    await screen.findByRole("link", { name: /^Second/ });
    expect(status.textContent).toBe("1 new report.");
  });

  it("opens a report in place when the host asks, a modified click still following the link", async () => {
    const r = report({ note: "Open me" });
    const onOpen = vi.fn();
    render(QueueView, { props: { api: fakeApi([r]), queue, href: () => "/x", onOpen } });
    const link = await screen.findByRole("link", { name: /^Open me/ });
    await fireEvent.click(link);
    expect(onOpen).toHaveBeenCalledWith(r);
    await fireEvent.click(link, { metaKey: true });
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("keeps what it shows when a read fails, and says so", async () => {
    const api = fakeApi([report({ note: "Kept" })]);
    render(QueueView, { props: { api, queue, href: () => "#" } });
    await screen.findByRole("link", { name: /^Kept/ });
    api.reports = async () => { throw new Error("Clax is down."); };
    api.ping();
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Could not read the reports: Clax is down."));
    expect(screen.getByRole("link", { name: /^Kept/ })).toBeTruthy();
  });
});

describe("ReportView", () => {
  it("shows the report's zones: state and claim, what it is about, the screenshot, the thread with each author, and details", async () => {
    const r = report({ has_clip: true, claim: { holder_name: "claude", expires_at: new Date(Date.now() + 300_000).toISOString() }, state: "claimed" });
    render(ReportView, { props: { api: fakeApi([r]), qid: QID, tid: r.id } });
    expect(await screen.findByRole("heading", { name: "Save does nothing" })).toBeTruthy();
    expect(screen.getByText(/Claimed by claude/)).toBeTruthy();
    expect(within(screen.getByRole("region", { name: "What it is about" })).getByRole("link", { name: "Open on the page" }).getAttribute("href")).toBe("http://127.0.0.1:7700/");
    expect((await screen.findByRole("img")).getAttribute("alt")).toBe("Screenshot filed with the report: “Save button”");
    expect(within(screen.getByRole("region", { name: "Thread" })).getByText("(reporter)")).toBeTruthy();
    expect(document.title).toBe("Report: Save does nothing · Clax");
  });

  it("replies and resolves, saying each in the status line; offers Show on the page where the host can highlight", async () => {
    const r = report();
    const onShow = vi.fn();
    render(ReportView, { props: { api: fakeApi([r]), qid: QID, tid: r.id, onShow } });
    await screen.findByRole("heading", { name: "Save does nothing" });
    await fireEvent.click(screen.getByRole("button", { name: "Show on the page" }));
    expect(onShow).toHaveBeenCalledOnce();
    await fireEvent.input(screen.getByRole("textbox", { name: "Reply" }), { target: { value: "On it" } });
    await fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Reply sent."));
    expect(screen.getByText("On it")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Resolved."));
    expect(screen.queryByRole("button", { name: "Resolve" })).toBeNull();
  });

  it("hears replies live, and says a failed action without losing the draft", async () => {
    const r = report();
    const api = fakeApi([r]);
    render(ReportView, { props: { api, qid: QID, tid: r.id } });
    await screen.findByRole("heading", { name: "Save does nothing" });
    await api.reply(QID, r.id, "From the agent");
    api.ping();
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("1 new reply."));
    api.reply = async () => { throw new Error("Not allowed."); };
    await fireEvent.input(screen.getByRole("textbox", { name: "Reply" }), { target: { value: "Draft" } });
    await fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Not done: Not allowed."));
    expect((screen.getByRole("textbox", { name: "Reply" }) as HTMLTextAreaElement).value).toBe("Draft");
  });

  it("shows the page's text as text", async () => {
    const r = report({ note: "<img src=x onerror=alert(1)>" });
    render(ReportView, { props: { api: fakeApi([r]), qid: QID, tid: r.id } });
    await screen.findByRole("heading", { name: "<img src=x onerror=alert(1)>" });
    expect(document.querySelector("img")).toBeNull();
  });
});
