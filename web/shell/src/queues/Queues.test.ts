import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { queueRequest } from "../q/fixtures";
import Queues from "./Queues.svelte";

const QUEUE = { id: "q1", name: "Bugs", origins: ["https://app.example"], reporters_see: "own", created_at: "2026-10-27T10:00:00Z", role: "admin" };
const routes = (o: Record<string, unknown> = {}) => ({
  "GET /api/queues": { queues: [QUEUE] },
  "GET /api/queues/q1/reports": { reports: [{ state: "open" }, { state: "open" }, { state: "claimed" }], next: null },
  "GET /api/queues/q1/members": { members: [{ public_id: "p1", name: "Alex", role: "admin", maker: true, owner: false }, { public_id: "p2", name: "Mia", role: "triage", maker: false, owner: false }] },
  "GET /api/queues/q1/service-grants": { grants: [] },
  "GET /api/queues/q1/webhooks": { webhooks: [] },
  ...o,
});
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const HOOK = {
  id: "wh1", url: "https://app.example/hook",
  events: ["report.created", "report.claimed", "report.released", "report.replied", "report.fix_proposed", "report.verified", "report.reopened", "report.resolved"],
  created_at: "2026-11-24T10:00:00Z", state: "verified", verified_at: "2026-11-24T10:00:01Z", verify_by: null, disabled_at: null, given_up: 0,
  failing_since: null, last_error: null, dropped: 0, last_dropped_at: null,
};
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
    expect(card.querySelector(".origins")!.textContent).toContain("https://app.example");
    expect(document.title).toBe("Queues · Clax");
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

  it("offers Verify again to an unverified endpoint and Re-enable to a disabled one, and counts dropped events", async () => {
    const verify = { id: "dv", webhook_id: "wh1", event_id: "evt_v", event: "webhook.verify", report_id: null, state: "failed", attempts: 1, next_at: null, last_at: "2026-11-24T10:00:00Z", last_status: 200, last_error: "the answer is not a JSON object", delivered_at: null, created_at: "2026-11-24T10:00:00Z" };
    const calls = stubFetch(routes({
      "GET /api/queues/q1/webhooks": { webhooks: [
        { ...HOOK, state: "unverified", verified_at: null, verify_by: "2026-11-25T10:00:00Z", last_error: "verification failed: the answer is not a JSON object" },
        { ...HOOK, id: "wh2", state: "disabled", disabled_at: "2026-11-24T11:00:00Z", given_up: 5, dropped: 3, last_dropped_at: "2026-11-24T10:30:00Z" },
        { ...HOOK, id: "wh3", state: "expired", verified_at: null, verify_by: "2026-11-23T10:00:00Z" },
      ] },
      "GET /api/queues/q1/webhooks/wh1/deliveries": { deliveries: [verify] },
      "POST /api/queues/q1/webhooks/wh1/verify": { webhook: HOOK },
      "POST /api/queues/q1/webhooks/wh2/verify": { webhook: HOOK },
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const [unv, dis, exp] = ["wh1", "wh2", "wh3"].map(id => m.root.querySelector(`[data-webhook="${id}"]`)!);
    expect(unv.querySelector(".state")!.textContent).toBe("waiting for its verification answer, until 2026-11-25 10:00");
    expect(dis.querySelector(".state")!.textContent).toBe("disabled 2026-11-24 11:00 after 5 deliveries in a row failed");
    expect(dis.querySelector(".dropped")!.textContent).toBe("3 events dropped over the hourly cap, latest 2026-11-24 10:30");
    expect(exp.querySelector(".state")!.textContent).toContain("remove it and add it again");
    const names = (el: Element) => [...el.querySelectorAll(":scope > button")].map(b => b.textContent);
    expect(names(unv)).toEqual(["Verify again", "Deliveries", "Remove"]);
    expect(names(dis)).toEqual(["Re-enable", "Deliveries", "Remove"]);
    expect(names(exp)).toEqual(["Deliveries", "Remove"]);
    click([...unv.querySelectorAll("button")].find(b => b.textContent === "Deliveries"));
    await settle();
    const d = m.root.querySelector('[data-delivery="dv"]')!;
    expect(d.textContent).toContain("not verified");
    expect(d.querySelector("button"), "a verification is not redelivered").toBeNull();
    click([...unv.querySelectorAll("button")].find(b => b.textContent === "Verify again"));
    await settle();
    click([...dis.querySelectorAll("button")].find(b => b.textContent === "Re-enable"));
    await settle();
    expect(calls.filter(c => c.method === "POST").map(c => c.url)).toEqual(["/api/queues/q1/webhooks/wh1/verify", "/api/queues/q1/webhooks/wh2/verify"]);
  });

  it("adds a webhook for the events chosen and shows its secret once", async () => {
    const made = { webhook: { ...HOOK, events: ["report.created"], state: "unverified", verified_at: null, verify_by: "2026-11-25T10:00:00Z" }, secret: "whsec_abc" };
    const calls = stubFetch(routes({ "POST /api/queues/q1/webhooks": made }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    expect(m.root.textContent).toContain("No webhooks.");
    type(m.root.querySelector('input[name="webhook_url"]'), " https://app.example/hook ");
    // Every event is chosen to start with; keep only "Filed".
    for (const box of [...m.root.querySelectorAll<HTMLInputElement>('input[name="event"]')].filter(b => b.value !== "report.created")) click(box);
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Add webhook"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toMatchObject({ url: "/api/queues/q1/webhooks", body: { url: "https://app.example/hook", events: ["report.created"] } });
    const secret = m.root.querySelector('[aria-label="New webhook"]')!;
    expect(secret.textContent).toContain("not shown again");
    expect(secret.querySelector("code")!.textContent).toBe("whsec_abc");
    // What the endpoint must answer before it is sent any report event.
    expect(secret.textContent).toContain("webhook.verify");
    expect(secret.textContent).toContain('{"answer": "<hex HMAC-SHA256 of webhook.verify.<challenge> under the secret>"}');
    expect(secret.textContent).toContain("res.json(event.reply)");
    click([...secret.querySelectorAll("button")].find(b => b.textContent === "Done"));
    expect(m.root.textContent).not.toContain("whsec_abc");
  });

  it("says an endpoint is failing, shows its deliveries and redelivers one", async () => {
    const hook = { ...HOOK, failing_since: "2026-11-24T11:30:00Z", last_error: "upstream down" };
    const failed = { id: "d1", webhook_id: "wh1", event_id: "evt_1", event: "report.created", report_id: "t1", state: "failed", attempts: 8, next_at: null, last_at: "2026-11-24T11:30:00Z", last_status: 503, last_error: "upstream down", delivered_at: null, created_at: "2026-11-24T10:00:00Z" };
    const calls = stubFetch(routes({
      "GET /api/queues/q1/webhooks": { webhooks: [hook] },
      "GET /api/queues/q1/webhooks/wh1/deliveries": { deliveries: [failed] },
      "POST /api/queues/q1/webhooks/wh1/deliveries/d1/redeliver": { delivery: { ...failed, state: "pending", attempts: 0 } },
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const line = m.root.querySelector('[data-webhook="wh1"]')!;
    expect(line.textContent).toContain("every event");
    expect(line.querySelector(".failing")!.textContent).toBe("failing since 2026-11-24 11:30");
    click([...line.querySelectorAll("button")].find(b => b.textContent === "Deliveries"));
    await settle();
    const d = m.root.querySelector('[data-delivery="d1"]')!;
    expect(d.textContent).toContain("gave up after 8 attempts · 503");
    expect(d.textContent).toContain("upstream down");
    click([...d.querySelectorAll("button")].find(b => b.textContent === "Redeliver"));
    await settle();
    expect(calls.some(c => c.method === "POST" && c.url === "/api/queues/q1/webhooks/wh1/deliveries/d1/redeliver")).toBe(true);
  });

  it("limits a new service grant to the queue's preview patterns, and names a limited grant's origins", async () => {
    const PREVIEW = "https://app-pr-*.fly.dev";
    const queue = { ...QUEUE, origins: ["https://app.example", PREVIEW] };
    const made = { grant: { id: "sg2", name: "previews", origins: [PREVIEW], created_at: "2026-10-27T11:00:00Z" }, token: "cxq_p", private_key: "MC4p" };
    const calls = stubFetch(routes({
      "GET /api/queues": { queues: [queue] },
      "GET /api/queues/q1/service-grants": { grants: [{ id: "sg1", name: "prod", origins: null, created_at: "2026-10-26T11:00:00Z" }, made.grant] },
      "POST /api/queues/q1/service-grants": made,
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    expect(m.root.querySelector('[data-grant="sg1"]')!.textContent).not.toContain("previews:");
    expect(m.root.querySelector('[data-grant="sg2"] .limit')!.textContent).toBe(`previews: ${PREVIEW}`);
    const only = m.root.querySelector<HTMLInputElement>('input[name="previews_only"]')!;
    expect(only.closest("label")!.textContent).toContain(`Previews only: mint only for ${PREVIEW}`);
    type(m.root.querySelector('input[name="grant_name"]'), "previews");
    click(only);
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Create service grant"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toMatchObject({ url: "/api/queues/q1/service-grants", body: { name: "previews", origins: [PREVIEW] } });
  });

  it("offers no previews-only grant on a queue without an origin pattern", async () => {
    stubFetch(routes());
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    expect(m.root.querySelector('input[name="previews_only"]')).toBeNull();
  });

  it("adds a member found by name at a role, and removes one after confirming", async () => {
    const calls = stubFetch(routes({
      "GET /api/queues/q1/people?q=sa": { people: [{ public_id: "p3", name: "Sam", avatar_url: null, owner: false }, { public_id: "p2", name: "Mia Sato", avatar_url: null, owner: false }] },
      "PUT /api/queues/q1/members/p3": {},
      "DELETE /api/queues/q1/members/p2": {},
    }));
    vi.stubGlobal("confirm", () => true);
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const add = () => [...m.root.querySelectorAll("button")].find(b => b.textContent === "Add member") as HTMLButtonElement;
    expect(add().disabled).toBe(true);
    type(m.root.querySelector('input[name="person"]'), "sa");
    await settle();
    // Someone already a member is listed but cannot be chosen again.
    expect((m.root.querySelector('[data-found="p2"] button') as HTMLButtonElement).disabled).toBe(true);
    click(m.root.querySelector('[data-found="p3"] button'));
    expect((m.root.querySelector('input[name="person"]') as HTMLInputElement).value).toBe("Sam");
    click(add());
    await settle();
    expect(calls.find(c => c.method === "PUT")).toMatchObject({ url: "/api/queues/q1/members/p3", body: { role: "triage" } });
    click(m.root.querySelector('[data-member="p2"] button'));
    await settle();
    expect(calls.some(c => c.method === "DELETE" && c.url === "/api/queues/q1/members/p2")).toBe(true);
  });

  it("shows no Requests section without pending requests, or when the daemon has no such route", async () => {
    for (const extra of [{ "GET /api/queue-requests?status=pending": { requests: [] } }, {}]) {
      const calls = stubFetch(routes(extra));
      const m = mount(Queues, {});
      await settle();
      expect(calls.some(c => c.url === "/api/queue-requests?status=pending")).toBe(true);
      expect(m.root.querySelector(".qreqs")).toBeNull();
      expect(m.root.querySelector(".qerr")).toBeNull();
      expect(m.root.querySelector('[data-id="q1"]')).not.toBeNull();
      document.body.replaceChildren();
    }
  });

  it("lists pending requests above the queues, and an approval lists the queues again with the new one", async () => {
    const made = { ...QUEUE, id: "q9", name: "Site feedback" };
    let approvedYet = false;
    const calls = stubFetch(routes({
      "GET /api/queues": () => ({ queues: approvedYet ? [QUEUE, made] : [QUEUE] }),
      "GET /api/queues/q9/reports": { reports: [], next: null },
      "GET /api/queue-requests?status=pending": { requests: [queueRequest()] },
      [`POST /api/queue-requests/${queueRequest().id}/approve`]: () => { approvedYet = true; return { request: queueRequest({ status: "approved", queue_id: "q9" }), queue: made }; },
    }));
    const m = mount(Queues, {});
    await settle();
    const sec = m.root.querySelector(".qreqs")!;
    expect(sec.querySelector("h3")!.textContent).toBe("Requests");
    expect(sec.compareDocumentPosition(m.root.querySelector(".qlist")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(sec.querySelector(".qrcard .chead > span")!.textContent).toBe("Claude Code asks for a report queue “Site feedback”");
    const lists = () => calls.filter(c => c.url === "/api/queues").length;
    const before = lists();
    click([...sec.querySelectorAll("button")].find(b => b.textContent === "Approve"));
    await settle();
    expect(lists()).toBe(before + 1);
    expect(sec.querySelector(".outcome")!.textContent).toContain("Approved: the queue is made");
    expect(m.root.querySelector('[data-id="q9"] h3')!.textContent).toBe("Site feedback");
  });

  it("lists the maker first as the queue's admin, with nothing to change or remove", async () => {
    stubFetch(routes());
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const rows = [...m.root.querySelectorAll("[data-member]")];
    expect(rows.map(r => r.getAttribute("data-member"))).toEqual(["p1", "p2"]);
    expect(rows[0].textContent).toContain("made this queue");
    expect(rows[0].querySelector("select, button")).toBeNull();
    expect(rows[1].querySelector("select")).not.toBeNull();
  });

  it("says when no one connected goes by a name, and drops a search answered after a later one", async () => {
    let answer: (v: unknown) => void = () => {};
    const slow = new Promise(r => { answer = r; });
    stubFetch(routes({ "GET /api/queues/q1/people?q=zz": { people: [] } }));
    const base = globalThis.fetch;
    vi.stubGlobal("fetch", (url: string, init?: RequestInit) =>
      url === "/api/queues/q1/people?q=m" ? slow.then(b => new Response(JSON.stringify(b))) : base(url, init));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    type(m.root.querySelector('input[name="person"]'), "m");
    type(m.root.querySelector('input[name="person"]'), "zz");
    await settle();
    expect(m.root.querySelector(".pick [role=status]")!.textContent).toContain("Only the owner and connected people can be added");
    answer({ people: [{ public_id: "p9", name: "Mo" }] });
    await settle();
    expect(m.root.querySelector('[data-found="p9"]')).toBeNull();
  });

  it("offers New queue only where this browser may make one, and marks a queue it oversees without being a member", async () => {
    stubFetch(routes({
      "GET /api/viewers/me": { viewer: { public_id: "p5" }, queues: { member: 1, may_make: false } },
      "GET /api/queues": { queues: [{ ...QUEUE, member: false }] },
    }));
    const m = mount(Queues, {});
    await settle();
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent === "New queue")).toBe(false);
    expect(m.root.querySelector(".qpolicy")!.textContent).toContain("An administrator makes new queues");
    expect(m.root.querySelector('[data-id="q1"] .qrow')!.textContent).toContain("Not a member");
  });
});
