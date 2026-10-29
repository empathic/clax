import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { QUEUE, report } from "./fixtures";
import QueueView from "./QueueView.svelte";

const LIST = "/api/queues/q1/reports";
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const tab = (root: Element, key: string) => root.querySelector(`[role="tab"][data-tab="${key}"]`);
const ids = (root: Element) => [...root.querySelectorAll(".rrow")].map(r => r.getAttribute("data-id"));

beforeEach(() => { history.replaceState(null, "", "/queues/q1"); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("QueueView", () => {
  it("filters by state tab", async () => {
    const calls = stubFetch({
      [`GET ${LIST}?state=active&order=newest`]: { reports: [report(2), report(1, { state: "claimed" })], next: null },
      [`GET ${LIST}?state=fix_proposed&order=newest`]: { reports: [report(3, { state: "fix_proposed", fixes: [{ n: 1, summary: "s", proposer_name: "Ana", created_at: "2026-10-29T08:00:00Z", outcome: "pending" }] })], next: null },
      [`GET ${LIST}?order=newest`]: { reports: [report(3), report(2), report(1)], next: null },
    });
    const m = mount(QueueView, { qid: "q1", queue: QUEUE, nav: () => {} });
    await settle();
    expect(m.root.querySelector("h2")!.textContent).toBe("Bugs");
    expect(tab(m.root, "active")!.getAttribute("aria-selected")).toBe("true");
    expect(ids(m.root)).toEqual(["t2", "t1"]);
    const row = m.root.querySelector('[data-id="t2"]')!;
    expect(row.querySelector("a")!.getAttribute("href")).toBe("/queues/q1/t2");
    expect(row.querySelector(".note")!.textContent).toBe("Report 2: the total is wrong");
    expect(row.querySelector(".who")!.textContent).toBe("emp-1");
    expect(row.querySelector(".rel")!.textContent).toBe("0.6.0");
    expect(row.querySelector<HTMLImageElement>("img.thumb")!.getAttribute("src")).toBe("/api/queues/q1/reports/t2/clip");
    expect(m.root.querySelector('[data-id="t1"] .state')!.textContent).toBe("Claimed");

    click(tab(m.root, "fix_proposed"));
    await settle();
    expect(ids(m.root)).toEqual(["t3"]);
    expect(m.root.querySelector('[data-id="t3"] .nfix')!.textContent).toBe("1 fix");
    expect(location.search).toBe("?state=fix_proposed");

    click(tab(m.root, "all"));
    await settle();
    expect(ids(m.root)).toEqual(["t3", "t2", "t1"]);
    expect(calls.map(c => c.url)).toEqual([`${LIST}?state=active&order=newest`, `${LIST}?state=fix_proposed&order=newest`, `${LIST}?order=newest`]);
  });

  it("loads more with the cursor", async () => {
    const calls = stubFetch({
      [`GET ${LIST}?state=active&order=newest`]: { reports: [report(3), report(2)], next: "t2" },
      [`GET ${LIST}?state=active&order=newest&cursor=t2`]: { reports: [report(1)], next: null },
    });
    const m = mount(QueueView, { qid: "q1", queue: QUEUE, nav: () => {} });
    await settle();
    expect(ids(m.root)).toEqual(["t3", "t2"]);
    const more = [...m.root.querySelectorAll("button")].find(b => b.textContent === "Load more");
    click(more);
    await settle();
    expect(ids(m.root)).toEqual(["t3", "t2", "t1"]);
    expect(calls.at(-1)!.url).toBe(`${LIST}?state=active&order=newest&cursor=t2`);
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent === "Load more")).toBe(false);
  });

  it("moves between tabs with the arrow, Home and End keys, and rows keep the tab", async () => {
    const page = (n: number) => ({ reports: [report(n)], next: null });
    stubFetch({
      [`GET ${LIST}?state=active&order=newest`]: page(1),
      [`GET ${LIST}?state=fix_proposed&order=newest`]: page(2),
      [`GET ${LIST}?order=newest`]: page(3),
    });
    const m = mount(QueueView, { qid: "q1", queue: QUEUE, nav: () => {} });
    await settle();
    const list = m.root.querySelector('[role="tablist"]')!;
    const key = (k: string) => flush(() => list.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true })));
    const selected = () => m.root.querySelector('[role="tab"][aria-selected="true"]')!.getAttribute("data-tab");
    key("ArrowRight");
    await settle();
    expect([selected(), document.activeElement?.getAttribute("data-tab")]).toEqual(["fix_proposed", "fix_proposed"]);
    expect(m.root.querySelector(".rrow a")!.getAttribute("href")).toBe("/queues/q1/t2?state=fix_proposed");
    key("End");
    await settle();
    expect(selected()).toBe("all");
    key("ArrowRight");
    await settle();
    expect(selected()).toBe("active");
    expect(m.root.querySelector(".rrow a")!.getAttribute("href")).toBe("/queues/q1/t1");
    key("ArrowLeft");
    await settle();
    expect(selected()).toBe("all");
    key("Home");
    await settle();
    expect(selected()).toBe("active");
  });

  it("refetches the tab in place when its queue's topic says a report changed", async () => {
    let first = { reports: [report(3), report(2)], next: "t2" as string | null };
    let rest = { reports: [report(1)], next: null as string | null };
    const calls = stubFetch({
      [`GET ${LIST}?state=active&order=newest`]: () => first,
      [`GET ${LIST}?state=active&order=newest&cursor=t2`]: () => rest,
    });
    const handlers: ((e: { type: string; topic?: string; [k: string]: unknown }) => void)[] = [];
    const topics: string[][] = [];
    const stream = { watch: (t: string[], f: (typeof handlers)[number]) => { topics.push(t); handlers.push(f); return () => {}; } };
    const m = mount(QueueView, { qid: "q1", queue: QUEUE, nav: () => {}, stream: stream as never });
    await settle();
    expect(topics).toEqual([["queue:q1"]]);
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Load more"));
    await settle();
    expect(ids(m.root)).toEqual(["t3", "t2", "t1"]);
    const row = m.root.querySelector('[data-id="t2"]');
    // A new report is filed and t1 is claimed: both pages are fetched
    // again, the rows never leave meanwhile, and the change is said.
    first = { reports: [report(4), report(3)], next: "t3" };
    stubFetch({
      [`GET ${LIST}?state=active&order=newest`]: () => first,
      [`GET ${LIST}?state=active&order=newest&cursor=t3`]: () => ({ reports: [report(2), report(1, { state: "claimed" })], next: null }),
    });
    const sent = (e: Parameters<(typeof handlers)[number]>[0]) => flush(() => { for (const h of handlers) h(e); });
    sent({ type: "report", topic: "queue:q1", report_id: "t4", state: "open" });
    expect(ids(m.root)).toEqual(["t3", "t2", "t1"]);
    await settle();
    expect(ids(m.root)).toEqual(["t4", "t3", "t2", "t1"]);
    expect(m.root.querySelector('[data-id="t2"]')).toBe(row);
    expect(m.root.querySelector('[data-id="t1"] .state')!.textContent).toBe("Claimed");
    expect(m.root.querySelector('[aria-live="polite"].sr')!.textContent).toBe("1 new report. 1 report changed.");
    expect(calls.length).toBe(2);
  });

  it("opens on the tab the address names", async () => {
    history.replaceState(null, "", "/queues/q1?state=resolved");
    stubFetch({ [`GET ${LIST}?state=resolved&order=newest`]: { reports: [], next: null } });
    const m = mount(QueueView, { qid: "q1", queue: QUEUE, nav: () => {} });
    await settle();
    expect(tab(m.root, "resolved")!.getAttribute("aria-selected")).toBe("true");
    expect(m.root.querySelector(".qnone")!.textContent).toBe("No resolved reports.");
  });
});
