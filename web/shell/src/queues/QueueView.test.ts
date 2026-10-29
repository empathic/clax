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

  it("opens on the tab the address names", async () => {
    history.replaceState(null, "", "/queues/q1?state=resolved");
    stubFetch({ [`GET ${LIST}?state=resolved&order=newest`]: { reports: [], next: null } });
    const m = mount(QueueView, { qid: "q1", queue: QUEUE, nav: () => {} });
    await settle();
    expect(tab(m.root, "resolved")!.getAttribute("aria-selected")).toBe("true");
    expect(m.root.querySelector(".qnone")!.textContent).toBe("No resolved reports.");
  });
});
