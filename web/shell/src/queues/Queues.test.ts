import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import Queues from "./Queues.svelte";

const QUEUE = { id: "q1", name: "Bugs", origins: ["https://app.example"], reporters_see: "own", created_at: "2026-10-27T10:00:00Z", role: "admin" };
const routes = (o: Record<string, unknown> = {}) => ({
  "GET /api/queues": { queues: [QUEUE] },
  "GET /api/queues/q1/reports": { reports: [{ state: "open" }, { state: "open" }, { state: "claimed" }], next: null },
  "GET /api/queues/q1/members": { members: [{ public_id: "p2", name: "Mia", role: "triage" }] },
  "GET /api/queues/q1/service-grants": { grants: [] },
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
});
