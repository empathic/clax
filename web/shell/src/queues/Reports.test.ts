import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { QUEUE, report } from "./fixtures";
import { newestAcross } from "./model";
import Reports from "./Reports.svelte";

const Q2 = { ...QUEUE, id: "q2", name: "Ideas" };
const at = (h: number) => `2026-10-29T${String(h).padStart(2, "0")}:00:00Z`;
const reply = (n: number) => ({ id: `r${n}`, author_kind: "viewer" as const, author_name: "Ana", body: "On it", created_at: at(20) });
const ids = (root: Element) => [...root.querySelectorAll(".rrow")].map(r => r.getAttribute("data-id"));
const pick = (el: Element | null, v: string) => flush(() => { const s = el as HTMLSelectElement; s.value = v; s.dispatchEvent(new Event("change", { bubbles: true })); });

beforeEach(() => { history.replaceState(null, "", "/reports"); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("newestAcross", () => {
  it("merges the queues' listings latest first, holding back what a queue with more could still outrank", () => {
    const a = { reports: [report(1, { created_at: at(9) }), report(2, { created_at: at(5) })], next: "c" };
    const b = { reports: [report(3, { created_at: at(8) }), report(4, { created_at: at(2) })], next: null };
    expect(newestAcross([a, b]).map(r => r.id)).toEqual(["t1", "t3", "t2"]);
    expect(newestAcross([{ ...a, next: null }, b]).map(r => r.id)).toEqual(["t1", "t3", "t2", "t4"]);
  });
});

describe("Reports", () => {
  it("lists every queue's reports latest first, each with its queue, state, first line and replies, linking to its view", async () => {
    stubFetch({
      "GET /api/queues": { queues: [QUEUE, Q2] },
      "GET /api/queues/q1/reports?order=newest": { reports: [report(1, { created_at: at(10), comments: [{ ...report(1).comments[0], body: "\n  Totals wrong\nsecond line" }, reply(1), reply(2)] }), report(2, { created_at: at(6) })], next: null },
      "GET /api/queues/q2/reports?order=newest": { reports: [report(3, { queue_id: "q2", state: "claimed", created_at: at(8) })], next: null },
    });
    const m = mount(Reports, {});
    await settle();
    expect(document.title).toBe("Reports · Clax");
    expect(ids(m.root)).toEqual(["t1", "t3", "t2"]);
    const row = m.root.querySelector('[data-id="t1"]')!;
    expect(row.querySelector(".line")!.textContent).toBe("Totals wrong");
    expect(row.querySelector(".qname")!.textContent).toBe("Bugs");
    expect(row.querySelector(".nrep")!.textContent).toBe("2 replies");
    expect(row.querySelector(".state")!.textContent).toBe("Open");
    const link = row.querySelector("a")!;
    expect(link.getAttribute("href")).toBe("/queues/q1/t1");
    expect(link.getAttribute("aria-label")).toMatch(/^Totals wrong\. Bugs, Open, 2 replies, filed /);
    const t3 = m.root.querySelector('[data-id="t3"]')!;
    expect([t3.querySelector(".qname")!.textContent, t3.querySelector(".state")!.textContent, t3.querySelector(".nrep")!.textContent]).toEqual(["Ideas", "Claimed", "0 replies"]);
    expect(t3.querySelector("a")!.getAttribute("href")).toBe("/queues/q2/t3");
    expect(m.root.querySelector('[role="status"]')!.textContent).toBe("3 reports.");
  });

  it("filters by state and by queue, keeping both in the address", async () => {
    const calls = stubFetch({
      "GET /api/queues": { queues: [QUEUE, Q2] },
      "GET /api/queues/q1/reports?order=newest": { reports: [report(1)], next: null },
      "GET /api/queues/q2/reports?order=newest": { reports: [report(2, { queue_id: "q2" })], next: null },
      "GET /api/queues/q1/reports?state=resolved&order=newest": { reports: [], next: null },
      "GET /api/queues/q2/reports?state=resolved&order=newest": { reports: [report(3, { queue_id: "q2", state: "resolved" })], next: null },
    });
    const m = mount(Reports, {});
    await settle();
    const [state, queue] = m.root.querySelectorAll("form.filters select");
    pick(state, "resolved");
    await settle();
    expect(ids(m.root)).toEqual(["t3"]);
    expect(location.search).toBe("?state=resolved");
    pick(queue, "q1");
    await settle();
    expect(ids(m.root)).toEqual([]);
    expect(m.root.querySelector(".rnone")!.textContent).toBe("No reports match.");
    expect(location.search).toBe("?state=resolved&queue=q1");
    expect(calls.map(c => c.url).slice(-1)).toEqual(["/api/queues/q1/reports?state=resolved&order=newest"]);
  });

  it("opens on the filters the address names, and loads more from every queue that has more", async () => {
    history.replaceState(null, "", "/reports?state=open&queue=q1");
    const calls = stubFetch({
      "GET /api/queues": { queues: [QUEUE, Q2] },
      "GET /api/queues/q1/reports?state=open&order=newest": { reports: [report(1, { created_at: at(9) })], next: "k" },
      "GET /api/queues/q1/reports?state=open&order=newest&cursor=k": { reports: [report(2, { created_at: at(3) })], next: null },
    });
    const m = mount(Reports, {});
    await settle();
    const [state, queue] = m.root.querySelectorAll<HTMLSelectElement>("form.filters select");
    expect([state.value, queue.value]).toEqual(["open", "q1"]);
    expect(ids(m.root)).toEqual(["t1"]);
    flush(() => [...m.root.querySelectorAll("button")].find(b => b.textContent === "Load more")!.click());
    await settle();
    expect(ids(m.root)).toEqual(["t1", "t2"]);
    expect(calls.some(c => c.url.includes("/q2/"))).toBe(false);
  });
});
