import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { queueRequest } from "../q/fixtures";
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
});
