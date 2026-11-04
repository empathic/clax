import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import Queues from "./Queues.svelte";
import { after } from "../clock";

vi.mock("../clock", async orig => {
  const real = await orig<typeof import("../clock")>();
  return { ...real, after: vi.fn(real.after) };
});
const lapseTimers = () => vi.mocked(after).mock.calls.filter(c => c[2] === "queues.lapse").length;

const QUEUE = { id: "q1", name: "Bugs", origins: ["https://app.example"], reporters_see: "own", created_at: "2026-10-27T10:00:00Z", role: "admin" };
const routes = (o: Record<string, unknown> = {}) => ({
  "GET /api/queues": { queues: [QUEUE] },
  "GET /api/queues/q1/reports": { reports: [{ state: "open" }, { state: "open" }, { state: "claimed" }], next: null },
  "GET /api/queues/q1/members": { members: [{ public_id: "p2", name: "Mia", role: "triage" }] },
  "GET /api/queues/q1/service-grants": { grants: [] },
  "GET /api/queues/q1/responders": { responders: [] },
  ...o,
});
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const type = (el: Element | null, v: string) => flush(() => { const i = el as HTMLInputElement; i.value = v; i.dispatchEvent(new Event("input", { bubbles: true })); });

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Queues", () => {
  it("lists each queue with its origins and its reports counted by state", async () => {
    stubFetch(routes());
    const m = mount(Queues, {});
    await settle();
    const card = m.root.querySelector('[data-id="q1"]')!;
    expect(card.querySelector("h3")!.textContent).toBe("Bugs");
    expect(card.querySelector(".counts")!.textContent).toBe("2 open · 1 claimed · 0 fix proposed · 0 verified · 0 resolved");
    const counts = [...card.querySelectorAll(".counts a")];
    expect(counts.map(a => a.textContent)).toEqual(["2 open", "1 claimed", "0 fix proposed", "0 verified", "0 resolved"]);
    expect(counts.map(a => a.getAttribute("href"))).toEqual(["open", "claimed", "fix_proposed", "verified", "resolved"].map(s => `/queues/q1?state=${s}`));
    expect(counts[0].getAttribute("aria-label")).toBe("2 open reports in Bugs");
    expect(counts[1].getAttribute("aria-label")).toBe("1 claimed report in Bugs");
    expect(card.querySelector(".origins")!.textContent).toContain("https://app.example");
    expect(document.title).toBe("Queues · Clax");
  });

  it("watches the queues topic, updates the counts in place at a ping and says what rose, moving no focus", async () => {
    let reports = [{ state: "open" }, { state: "open" }, { state: "claimed" }];
    const calls = stubFetch(routes({ "GET /api/queues/q1/reports": () => ({ reports, next: null }) }));
    let hear: ((e: { type: string }) => void) | null = null;
    const off = vi.fn();
    const watch = vi.fn((_t: string[], on: (e: { type: string }) => void) => { hear = on; return off; });
    const m = mount(Queues, { stream: { watch } as unknown as import("../stream").EventStream });
    await settle();
    expect(watch.mock.calls[0][0]).toEqual(["queues"]);
    const card = m.root.querySelector('[data-id="q1"]')!;
    const counts = card.querySelector(".counts")!;
    expect(counts.textContent).toBe("2 open · 1 claimed · 0 fix proposed · 0 verified · 0 resolved");
    const button = [...m.root.querySelectorAll("button")].find(b => b.textContent === "New queue")!;
    button.focus();
    // The agent claims one: the same element now counts it, and the status line says so.
    reports = [{ state: "open" }, { state: "claimed" }, { state: "claimed" }];
    hear!({ type: "queues" });
    await settle();
    expect(card.querySelector(".counts")).toBe(counts);
    expect(counts.textContent).toBe("1 open · 2 claimed · 0 fix proposed · 0 verified · 0 resolved");
    const heard = m.root.querySelector(".qheard")!;
    expect(heard.getAttribute("role")).toBe("status");
    expect(heard.textContent).toBe("1 report claimed in Bugs.");
    expect(document.activeElement).toBe(button);
    // A stream that comes back refetches and says nothing.
    reports = [{ state: "open" }, { state: "claimed" }, { state: "resolved" }];
    const before = calls.length;
    hear!({ type: "ready" });
    await settle();
    expect(calls.length).toBeGreaterThan(before);
    expect(counts.textContent).toBe("1 open · 1 claimed · 0 fix proposed · 0 verified · 1 resolved");
    expect(heard.textContent).toBe("1 report claimed in Bugs.");
    m.unmount();
    expect(off).toHaveBeenCalled();
  });

  it("fetches again when the soonest lease shown lapses, and a page gone mid-fetch plants no timer", async () => {
    const claimed = { state: "claimed", claim: { holder_name: "claude", expires_at: new Date(Date.now() + 60_000).toISOString() } };
    stubFetch(routes({ "GET /api/queues/q1/reports": { reports: [claimed], next: null } }));
    const n = lapseTimers();
    const shown = mount(Queues, {});
    await settle();
    expect(lapseTimers()).toBe(n + 1);
    shown.unmount();
    const gone = mount(Queues, {});
    gone.unmount();
    await settle();
    expect(lapseTimers()).toBe(n + 1);
  });

  it("puts Reports first among a card's actions, for triage as for admins, and a count opens its tab in place", async () => {
    history.replaceState(null, "", "/queues");
    stubFetch(routes({
      "GET /api/queues": { queues: [QUEUE, { ...QUEUE, id: "q2", name: "Ideas", role: "triage" }] },
      "GET /api/queues/q2/reports": { reports: [], next: null },
      "GET /api/queues/q1/reports?state=open&order=newest": { reports: [], next: null },
    }));
    const m = mount(Queues, {});
    await settle();
    const acts = (id: string) => [...m.root.querySelectorAll(`[data-id="${id}"] .acts > *`)].map(a => a.textContent);
    expect(acts("q1")).toEqual(["Reports", "Manage", "Edit", "Delete"]);
    expect(acts("q2")).toEqual(["Reports"]);
    const reports = m.root.querySelector<HTMLAnchorElement>('[data-id="q2"] .acts a')!;
    expect([reports.getAttribute("href"), reports.getAttribute("aria-label")]).toEqual(["/queues/q2", "Reports in Ideas"]);
    click(m.root.querySelector('[data-id="q1"] .counts a'));
    await settle();
    expect(`${location.pathname}${location.search}`).toBe("/queues/q1?state=open");
    expect(m.root.querySelector('[role="tab"][aria-selected="true"]')!.getAttribute("data-tab")).toBe("open");
    history.replaceState(null, "", "/queues");
  });

  it("makes a queue from the form, its origins one per line, and refuses a line that is not an origin", async () => {
    const calls = stubFetch(routes({ "POST /api/queues": { queue: { ...QUEUE, id: "q2" } } }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "New queue"));
    type(m.root.querySelector('form.qform input[name="name"]'), "Feedback");
    type(m.root.querySelector("form.qform textarea"), "https://a.example\nnot an origin");
    click(m.root.querySelector('form.qform input[value="all"]'));
    click(m.root.querySelector('form.qform button[type="submit"]'));
    await settle();
    expect(m.root.querySelector("form.qform .err")!.textContent).toContain("not an origin");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    type(m.root.querySelector("form.qform textarea"), "https://a.example\nhttps://b.example/");
    click(m.root.querySelector('form.qform button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({ name: "Feedback", origins: ["https://a.example", "https://b.example"], reporters_see: "all" });
    expect(m.root.querySelector(".qstat")!.textContent).toBe("Made Feedback.");
  });

  it("shows a new service grant's token and key once, with the warning, and forgets them on Done", async () => {
    const made = { grant: { id: "sg1", name: "backend", created_at: "2026-10-27T11:00:00Z" }, token: "cxq_secret", private_key: "MC4CAQAw" };
    const calls = stubFetch(routes({ "POST /api/queues/q1/service-grants": made }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    expect(m.root.querySelector('[data-member="p2"]')!.textContent).toContain("Mia");
    type(m.root.querySelector('input[name="grant_name"]'), "backend");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Create service grant"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toMatchObject({ url: "/api/queues/q1/service-grants", body: { name: "backend" } });
    const secret = m.root.querySelector(".secret")!;
    expect(secret.textContent).toContain("not shown again");
    expect([...secret.querySelectorAll("code")].map(c => c.textContent)).toEqual(["cxq_secret", "MC4CAQAw"]);
    click([...secret.querySelectorAll("button")].find(b => b.textContent === "Done"));
    expect(m.root.querySelector(".secret")).toBeNull();
    expect(m.root.textContent).not.toContain("cxq_secret");
  });

  it("adds a member by public ID at a role, and removes one after confirming", async () => {
    const calls = stubFetch(routes({ "PUT /api/queues/q1/members/p3": {}, "DELETE /api/queues/q1/members/p2": {} }));
    vi.stubGlobal("confirm", () => true);
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    type(m.root.querySelector('input[name="public_id"]'), " p3 ");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Add member"));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toMatchObject({ url: "/api/queues/q1/members/p3", body: { role: "triage" } });
    click(m.root.querySelector('[data-member="p2"] button'));
    await settle();
    expect(calls.some(c => c.method === "DELETE" && c.url === "/api/queues/q1/members/p2")).toBe(true);
  });

  it("names who answers the queue, in order: adds an agent through a grant, moves one up, removes one", async () => {
    let saved: unknown[] = [];
    const calls = stubFetch(routes({
      "GET /api/queues/q1/service-grants": { grants: [{ id: "g1", queue_id: "q1", name: "changelist-clax-queue", public_key: "k", created_at: "2026-11-02T10:00:00Z" }] },
      "GET /api/queues/q1/responders": { responders: [{ kind: "bot", profile: { instructions: "", tier: "quick", tools: [] } }] },
      "PUT /api/queues/q1/responders": (b: { responders: unknown[] }) => { saved = b.responders; return { responders: saved }; },
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const list = () => [...m.root.querySelectorAll("[data-responder] .who")].map(e => e.textContent);
    expect(list()).toEqual(["First: ClaxBot (quick)"]);
    type(m.root.querySelector('input[name="agent_name"]'), "work-changeling-queue-agent");
    await flush(() => { const s = m.root.querySelector<HTMLSelectElement>('select[name="agent_grant"]')!; s.value = "g1"; s.dispatchEvent(new Event("change", { bubbles: true })); });
    type(m.root.querySelector('input[name="agent_escalate"]'), "30");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Add agent"));
    await settle();
    expect(calls.find(c => c.method === "PUT")?.body).toEqual({ responders: [
      { kind: "bot", profile: { instructions: "", tier: "quick", tools: [] } },
      { kind: "agent", name: "work-changeling-queue-agent", service_grant_id: "g1", escalate_after_secs: 1800 },
    ] });
    expect(list()).toEqual(["ClaxBot (quick)", "work-changeling-queue-agentthrough changelist-clax-queue · next asked after 30 min"].map((t, i) => (i === 0 ? `First: ${t}` : t)));
    click(m.root.querySelector('[aria-label="Move work-changeling-queue-agent up"]'));
    await settle();
    expect((saved[0] as { name: string }).name).toBe("work-changeling-queue-agent");
    click(m.root.querySelector('[aria-label="Remove ClaxBot (quick)"]'));
    await settle();
    expect(saved).toHaveLength(1);
  });
});
