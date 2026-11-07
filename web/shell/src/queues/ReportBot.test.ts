import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { meBot, run } from "../bot/fixtures";
import { forget } from "../bot/me";
import { botView } from "../bot/send.svelte";
import { report } from "./fixtures";
import ReportBot from "./ReportBot.svelte";

const LIST = "/api/bot/runs?artifact=a1&thread=t1";
const ME = "GET /api/me/bot";
const { steps: _steps, ...SUMMARY } = run({ artifact_id: "a1" });
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const button = (root: ParentNode, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent?.trim() === text);
const open = async (routes: Record<string, unknown>, o: Parameters<typeof report>[1] = {}) => {
  const calls = stubFetch({ [ME]: meBot(), [`GET ${LIST}`]: { runs: [], may_send: true }, ...routes });
  const m = mount(ReportBot, { report: report(1, o) });
  await settle();
  return { calls, m };
};

beforeEach(() => { forget(); botView.run = null; botView.started = []; });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("ReportBot", () => {
  it("sends the report to ClaxBot, saying whose credits it spends, and shows its run", async () => {
    const started = { ...SUMMARY, id: "r9" };
    const { calls, m } = await open({
      [ME]: meBot({ estimate_milli: { low: 2000, high: 9000 } }),
      "POST /api/bot/runs": { run: started },
      "GET /api/bot/runs/r9": run({ id: "r9", artifact_id: "a1", status: "running" }),
    });
    expect(m.root.textContent).toContain("using your credits, usually 2–9 credits");
    click(button(m.root, "Send to ClaxBot"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: "/api/bot/runs", method: "POST", body: { thread_ids: ["t1"], artifact_id: "a1" } });
    expect(botView.run).toBe("r9");
    const panel = m.root.querySelector("#bot-run-panel")!;
    expect(panel.querySelector(".st")!.textContent).toBe("Working");
    expect(panel.querySelector(".answers")!.textContent).toContain("Report 1: the total is wrong");
    expect(button(panel, "Stop")).toBeDefined();
  });

  it("offers nothing to someone ClaxBot is not on for, or who may not send here", async () => {
    const off = await open({ [ME]: meBot({ enabled: false, available: false, unavailable_reason: "no_flag" }) });
    expect(off.m.root.textContent).toBe("");
    document.body.replaceChildren();
    forget();
    const refused = await open({ [`GET ${LIST}`]: { runs: [], may_send: false } });
    expect(button(refused.m.root, "Send to ClaxBot")).toBeUndefined();
    document.body.replaceChildren();
    forget();
    const unread = await open({ [`GET ${LIST}`]: [404, { error: { code: "not_found", message: "not found" } }] });
    expect(button(unread.m.root, "Send to ClaxBot")).toBeUndefined();
  });

  it("shows no bare heading to a viewer with no runs and no right to send", async () => {
    const { m } = await open({ [`GET ${LIST}`]: { runs: [], may_send: false } });
    expect(m.root.textContent).not.toContain("ClaxBot");
    expect(m.root.querySelector("section.rbot")).toBeNull();
  });

  it("keeps its section for a viewer who may not send but has a run to follow", async () => {
    const { m } = await open({ [`GET ${LIST}`]: { runs: [{ ...SUMMARY, status: "finished" }], may_send: false } });
    expect(m.root.querySelector("section.rbot h3")!.textContent).toBe("ClaxBot");
    expect(button(m.root, "See the run")).toBeDefined();
  });

  it("says why ClaxBot cannot take the report now", async () => {
    const { m } = await open({ [ME]: meBot({ available: false, unavailable_reason: "not_configured" }) });
    expect(button(m.root, "Send to ClaxBot")).toBeUndefined();
    expect(m.root.textContent).toContain("Not set up on this Clax yet");
  });

  it("offers the send on a report with a fix waiting, and not on one that is closed or claimed", async () => {
    const waiting = await open({}, { state: "fix_proposed" });
    expect(button(waiting.m.root, "Send to ClaxBot")).toBeDefined();
    document.body.replaceChildren();
    const closed = await open({}, { state: "resolved", status: "resolved" });
    expect(button(closed.m.root, "Send to ClaxBot")).toBeUndefined();
    document.body.replaceChildren();
    const claimed = await open({}, { state: "claimed", claim: { holder_name: "Mia", expires_at: "2026-10-29T08:30:00Z" } as never });
    expect(button(claimed.m.root, "Send to ClaxBot")).toBeUndefined();
    expect(claimed.m.root.textContent).toContain("Mia holds this report: release the claim to send it to ClaxBot.");
  });

  it("shows a run at work in place of the send, and its panel stops it for who may", async () => {
    const { calls, m } = await open({
      [`GET ${LIST}`]: { runs: [{ ...SUMMARY, may_stop: true }], may_send: true },
      "GET /api/bot/runs/r1": run({ artifact_id: "a1" }),
      "POST /api/bot/runs/r1/cancel": { run: { ...SUMMARY, status: "stopping" } },
    });
    expect(button(m.root, "Send to ClaxBot")).toBeUndefined();
    expect(m.root.textContent).toContain("ClaxBot is working on this report.");
    click(button(m.root, "See the run"));
    await settle();
    click(button(m.root.querySelector("#bot-run-panel")!, "Stop"));
    await settle();
    expect(calls.some(c => c.method === "POST" && c.url === "/api/bot/runs/r1/cancel")).toBe(true);
  });

  it("lets someone else's run be followed and not stopped", async () => {
    const { m } = await open({
      [`GET ${LIST}`]: { runs: [{ ...SUMMARY, may_stop: false }], may_send: true },
      "GET /api/bot/runs/r1": run({ artifact_id: "a1" }),
    });
    click(button(m.root, "See the run"));
    await settle();
    const panel = m.root.querySelector("#bot-run-panel")!;
    expect(button(panel, "Stop")).toBeUndefined();
    expect(panel.textContent).toContain("Sam's run: you can follow it, and only they can stop it.");
  });

  it("offers the send again beside a run that ended, and keeps that run a click away", async () => {
    const { m } = await open({ [`GET ${LIST}`]: { runs: [{ ...SUMMARY, status: "gave_up" }], may_send: true } });
    expect(m.root.textContent).toContain("Last run: Gave up");
    expect(button(m.root, "See the run")).toBeDefined();
    expect(button(m.root, "Send to ClaxBot")).toBeDefined();
  });

  it("opens the run that already works on the report, and says so", async () => {
    const { m } = await open({
      "POST /api/bot/runs": [409, { error: { code: "run_active", message: "a run is at work", run: { id: "r1" } } }],
      "GET /api/bot/runs/r1": run({ artifact_id: "a1" }),
    });
    click(button(m.root, "Send to ClaxBot"));
    await settle();
    expect(botView.run).toBe("r1");
    expect(m.root.textContent).toContain("ClaxBot is already working on this thread: its run is open.");
  });

  it("shows a refusal as the daemon words it", async () => {
    const { m } = await open({ "POST /api/bot/runs": [400, { error: { code: "thread_resolved", message: "a resolved thread cannot be sent" } }] });
    click(button(m.root, "Send to ClaxBot"));
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("ClaxBot did not start: A resolved thread cannot be sent.");
  });

  it("closes the panel as the report's view goes", async () => {
    const { m } = await open({
      [`GET ${LIST}`]: { runs: [SUMMARY], may_send: true },
      "GET /api/bot/runs/r1": run({ artifact_id: "a1" }),
    });
    click(button(m.root, "See the run"));
    await settle();
    expect(botView.run).toBe("r1");
    m.unmount();
    expect(botView.run).toBeNull();
  });

  it("reads the runs again as the report changes, and an older answer does not undo a newer one", async () => {
    let runs: unknown[] = [];
    const { m } = await open({ [`GET ${LIST}`]: () => ({ runs, may_send: true }) });
    expect(button(m.root, "See the run")).toBeUndefined();
    runs = [SUMMARY];
    m.update({ report: report(1, { state: "fix_proposed" }) });
    await settle();
    expect(button(m.root, "See the run")).toBeDefined();
  });
});
