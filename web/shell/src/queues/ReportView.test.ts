import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import type { DiagLine, Report } from "../../../sdk/src/types";
import { QUEUE, report } from "./fixtures";
import ReportView from "./ReportView.svelte";

const R = "/api/queues/q1/reports/t1";
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const type = (el: Element | null, v: string) => flush(() => { const i = el as HTMLInputElement; i.value = v; i.dispatchEvent(new Event("input", { bubbles: true })); });
const choose = (el: Element | null, v: string) => flush(() => { const s = el as HTMLSelectElement; s.value = v; s.dispatchEvent(new Event("change", { bubbles: true })); });
const button = (root: Element, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent?.trim() === text);
const submit = (root: Element) => click(root.querySelector('form.af button[type="submit"]'));
const view = () => mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US" });

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("ReportView", () => {
  it("shows the report, its reporter and conversation, and no claim or release", async () => {
    const r = report(1, {
      reporter_profile: { name: "Ana", github: "ana-k" },
      comments: [...report(1).comments, { id: "c2", author_kind: "viewer", author_name: "Mia", body: "Looking", created_at: "2026-10-29T08:00:00Z" }],
    });
    stubFetch({ [`GET ${R}`]: { report: r } });
    const m = view();
    await settle();
    expect(m.root.querySelector("h2")!.textContent).toBe("Report 1: the total is wrong");
    expect(m.root.querySelector(".by")!.textContent).toContain("Ana");
    expect(m.root.querySelector<HTMLAnchorElement>('.by a[href="https://github.com/ana-k"]')).not.toBeNull();
    expect(m.root.querySelector(".anchor")!.textContent).toBe("On Element: Total");
    expect([...m.root.querySelectorAll(".convo .cm")].map(c => c.getAttribute("data-id"))).toEqual(["c2"]);
    const actions = [...m.root.querySelectorAll(".btns button")].map(b => b.textContent);
    expect(actions).toEqual(["Propose fix", "Resolve"]);
    expect([...m.root.querySelectorAll("button")].filter(b => /claim|release/i.test(b.textContent ?? ""))).toEqual([]);
  });

  it("outlines what the render left out, in the clip's own pixels", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1) } });
    const m = view();
    await settle();
    const img = m.root.querySelector<HTMLImageElement>(".clip img")!;
    expect(img.getAttribute("src")).toBe(`${R}/clip`);
    Object.defineProperty(img, "naturalWidth", { value: 1000 });
    Object.defineProperty(img, "naturalHeight", { value: 500 });
    flush(() => img.dispatchEvent(new Event("load")));
    const o = m.root.querySelector<HTMLElement>(".omit")!;
    expect([o.style.left, o.style.top, o.style.width, o.style.height]).toEqual(["10%", "10%", "20%", "20%"]);
    expect(o.textContent).toBe("iframe");
    // At 1× the clip is its size on the reporter's screen (dpr 2), at 2× twice it.
    click(button(m.root, "1×"));
    expect(m.root.querySelector<HTMLElement>(".clipframe")!.style.width).toBe("500px");
    click(button(m.root, "2×"));
    expect(m.root.querySelector<HTMLElement>(".clipframe")!.style.width).toBe("1000px");
  });

  it("propose fix posts summary and url", async () => {
    const proposed: Report = report(1, { state: "fix_proposed", fixes: [{ n: 1, summary: "Rounds the total", url: "https://github.com/o/r/pull/9", proposer_name: "Mia", created_at: "2026-10-29T09:00:00Z", outcome: "pending" }] });
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1) }, [`POST ${R}/fix`]: { report: proposed } });
    const m = view();
    await settle();
    click(button(m.root, "Propose fix"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".acts .err")!.textContent).toBe("Say what the fix changes.");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    type(m.root.querySelector('textarea[name="summary"]'), " Rounds the total ");
    type(m.root.querySelector('input[name="url"]'), "https://github.com/o/r/pull/9");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/fix`, method: "POST", body: { summary: "Rounds the total", url: "https://github.com/o/r/pull/9" } });
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Fix proposed");
    expect(m.root.querySelector('.fix[data-n="1"]')!.textContent).toContain("Waiting for the reporter");
    expect([...m.root.querySelectorAll(".btns button")].map(b => b.textContent)).toEqual(["Propose fix", "Resolve", "Verify for the reporter", "Reopen for the reporter"]);
  });

  it("resolve requires a reason", async () => {
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1) }, [`POST ${R}/resolve`]: { report: report(1, { state: "resolved", status: "resolved", resolve_reason: "duplicate" }) } });
    const m = view();
    await settle();
    click(button(m.root, "Resolve"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".acts .err")!.textContent).toBe("Choose why the report is resolved.");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    choose(m.root.querySelector('select[name="reason"]'), "duplicate");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/resolve`, method: "POST", body: { reason: "duplicate" } });
    expect(m.root.querySelector(".rh")!.textContent).toContain("Duplicate");
    expect([...m.root.querySelectorAll(".btns button")].map(b => b.textContent)).toEqual(["Reopen for the reporter"]);
  });

  it("verify on behalf posts verify", async () => {
    const waiting = report(1, { state: "fix_proposed" });
    const calls = stubFetch({ [`GET ${R}`]: { report: waiting }, [`POST ${R}/verify`]: { report: report(1, { state: "verified", status: "resolved" }) } });
    const m = view();
    await settle();
    click(button(m.root, "Verify for the reporter"));
    type(m.root.querySelector('form.af textarea[name="note"]'), "Checked on 0.6.1");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/verify`, method: "POST", body: { note: "Checked on 0.6.1" } });
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Verified");
    expect(m.root.querySelector(".acts .stat")!.textContent).toBe("Marked verified for the reporter.");
  });

  it("shows a refusal from the daemon", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed" }) }, [`POST ${R}/verify`]: [409, { error: { code: "not_fix_proposed", message: "no fix waits for a verdict" } }] });
    const m = view();
    await settle();
    click(button(m.root, "Verify for the reporter"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".acts .err")!.textContent).toBe("No fix waits for a verdict.");
  });

  it("diagnostics filter by level", async () => {
    const lines: DiagLine[] = [
      { at: "2026-10-29T07:12:01.000Z", level: "info", kind: "console", text: "booted" },
      { at: "2026-10-29T07:12:02.000Z", level: "error", kind: "error", text: "TypeError: total is undefined" },
      { at: "2026-10-29T07:12:02.500Z", level: "warn", kind: "request", text: "GET /api/total 500" },
    ];
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1, { diagnostics_lines: 3 }) }, [`GET ${R}/diagnostics`]: { lines } });
    const m = view();
    await settle();
    const d = m.root.querySelector<HTMLDetailsElement>("details.diag")!;
    expect(d.open).toBe(false);
    expect(d.querySelector("summary")!.textContent).toContain("3 lines");
    expect(calls.some(c => c.url.includes("diagnostics"))).toBe(false);
    flush(() => { d.open = true; d.dispatchEvent(new Event("toggle")); });
    await settle();
    expect([...d.querySelectorAll(".line")].map(l => l.getAttribute("data-level"))).toEqual(["info", "error", "warn"]);
    click(d.querySelector('input[type="radio"][value="error"]'));
    expect([...d.querySelectorAll(".line .text")].map(l => l.textContent)).toEqual(["TypeError: total is undefined"]);
    click(d.querySelector('input[type="radio"][value="debug"]'));
    expect(d.querySelector(".line")).toBeNull();
    expect(d.textContent).toContain("No debug lines.");
  });
});
