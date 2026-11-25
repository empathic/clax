import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { queueRequest } from "../q/fixtures";
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
  "GET /api/queues/q1/members": { members: [{ public_id: "p1", name: "Alex", role: "admin", maker: true, owner: false }, { public_id: "p2", name: "Mia", role: "triage", maker: false, owner: false }] },
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
    expect(m.root.querySelector('[data-grant="sg2"] .limit')!.textContent).toBe(` · previews: ${PREVIEW}`);
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

  it("speaks a member row's parts apart and names whom each Remove and Revoke acts on", async () => {
    stubFetch(routes({
      "GET /api/queues/q1/service-grants": { grants: [{ id: "g1", queue_id: "q1", name: "prod backend", public_key: "k", created_at: "2026-11-02T10:00:00Z" }] },
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const who = (pid: string) => m.root.querySelector(`[data-member="${pid}"] .who`)!.textContent;
    expect(who("p1")).toBe("Alex · made this queue");
    expect(who("p2")).toBe("Mia · p2");
    expect(m.root.querySelector('[data-member="p2"] button')!.getAttribute("aria-label")).toBe("Remove Mia");
    expect(m.root.querySelector('[data-member="p2"] select')!.getAttribute("aria-label")).toBe("Role of Mia");
    expect(m.root.querySelector('[data-grant="g1"] .who')!.textContent).toBe("prod backend · made 2026-11-02");
    expect(m.root.querySelector('[data-grant="g1"] button')!.getAttribute("aria-label")).toBe("Revoke prod backend");
  });

  it("says each member change in the status line, replacing the last, and warns of a member who cannot read reports", async () => {
    let members = [
      { public_id: "p1", name: "Alex", role: "admin", maker: true, owner: false, reads_reports: true },
      { public_id: "p2", name: "Mia", role: "triage", maker: false, owner: false, reads_reports: true },
    ];
    stubFetch(routes({
      "GET /api/queues/q1/members": () => ({ members }),
      "GET /api/queues/q1/people?q=le": { people: [{ public_id: "p3", name: "Lee", avatar_url: null, owner: false }] },
      "PUT /api/queues/q1/members/p3": () => { members = [...members, { public_id: "p3", name: "Lee", role: "triage", maker: false, owner: false, reads_reports: false }]; return {}; },
      "PUT /api/queues/q1/members/p2": () => { members = members.map(x => (x.public_id === "p2" ? { ...x, role: "admin" } : x)); return {}; },
      "DELETE /api/queues/q1/members/p2": () => { members = members.filter(x => x.public_id !== "p2"); return {}; },
    }));
    vi.stubGlobal("confirm", () => true);
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const status = () => m.root.querySelector(".part [role=status].hint")!.textContent;
    const find = m.root.querySelector<HTMLInputElement>('input[name="person"]')!;
    type(find, "le");
    await settle();
    click(m.root.querySelector('[data-found="p3"] button'));
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Add member"));
    await settle();
    const cant = "Lee cannot read this queue's reports until the owner lets them read comments, in Settings › People.";
    expect(status()).toBe(`Added Lee. ${cant}`);
    expect(m.root.querySelector('[data-member="p3"] .cant')!.textContent).toBe(cant);
    expect(m.root.querySelector('[data-member="p2"] .cant')).toBeNull();
    // The next search empties it: nothing stale is left.
    type(find, "x");
    expect(status()).toBe("");
    type(find, "");
    await settle();
    await flush(() => { const s = m.root.querySelector<HTMLSelectElement>('[data-member="p2"] select')!; s.value = "admin"; s.dispatchEvent(new Event("change", { bubbles: true })); });
    await settle();
    expect(status()).toBe("Mia is now Admin.");
    // A removal says so, and focus, gone with the row, goes to Add a person.
    const remove = m.root.querySelector<HTMLButtonElement>('[aria-label="Remove Mia"]')!;
    remove.focus();
    click(remove);
    await settle();
    expect(m.root.querySelector('[data-member="p2"]')).toBeNull();
    expect(status()).toBe("Removed Mia.");
    expect(document.activeElement).toBe(find);
  });

  it("says a waiting login's removal and puts focus on Add a person", async () => {
    let pending = [{ id: "i1", github_login: "octo-cat", role: "triage", created_at: "2026-11-25T10:00:00Z" }];
    stubFetch(routes({
      "GET /api/queues/q1/members": () => ({ members: [], pending, github_logins: true }),
      "DELETE /api/queues/q1/members/pending/i1": () => { pending = []; return {}; },
    }));
    vi.stubGlobal("confirm", () => true);
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    const remove = m.root.querySelector<HTMLButtonElement>('[aria-label="Remove @octo-cat"]')!;
    remove.focus();
    click(remove);
    await settle();
    expect(m.root.querySelector(".part [role=status].hint")!.textContent).toBe("Removed @octo-cat's invitation.");
    expect(document.activeElement).toBe(m.root.querySelector('input[name="person"]'));
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
    expect(list()).toEqual(["ClaxBot (quick)", "work-changeling-queue-agent · through changelist-clax-queue · next asked after 30 min"].map((t, i) => (i === 0 ? `First: ${t}` : t)));
    click(m.root.querySelector('[aria-label="Move work-changeling-queue-agent up"]'));
    await settle();
    expect((saved[0] as { name: string }).name).toBe("work-changeling-queue-agent");
    click(m.root.querySelector('[aria-label="Remove ClaxBot (quick)"]'));
    await settle();
    expect(saved).toHaveLength(1);
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
    // A local Clax has no GitHub sign-in: nothing is offered by login.
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent?.startsWith("Add @"))).toBe(false);
    answer({ people: [{ public_id: "p9", name: "Mo" }] });
    await settle();
    expect(m.root.querySelector('[data-found="p9"]')).toBeNull();
  });

  it("offers a login the search finds no one by, where people sign in with GitHub", async () => {
    stubFetch(routes({
      "GET /api/queues/q1/members": { members: [], pending: [], github_logins: true },
      "GET /api/queues/q1/people?q=zz": { people: [] },
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    type(m.root.querySelector('input[name="person"]'), "zz");
    await settle();
    expect(m.root.querySelector(".pick [role=status]")!.textContent).toContain("No one connected to this Clax goes by that name. Add @zz by their GitHub login");
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent === "Add @zz")).toBe(true);
  });

  it("adds a teammate by @github-login, lists one waiting for their GitHub sign-in, and removes it after confirming", async () => {
    let pending: unknown[] = [];
    const members = [{ public_id: "p1", name: "Alex", role: "admin", maker: true, owner: false }];
    const calls = stubFetch(routes({
      "GET /api/queues/q1/members": () => ({ members, pending, github_logins: true }),
      "PUT /api/queues/q1/members": (b: { github_login: string; role: string }) => {
        pending = [{ id: "i1", github_login: b.github_login, role: b.role, created_at: "2026-11-25T10:00:00Z" }];
        return [201, { invite: pending[0] }];
      },
      "DELETE /api/queues/q1/members/pending/i1": () => { pending = []; return {}; },
    }));
    vi.stubGlobal("confirm", () => true);
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    type(m.root.querySelector('input[name="person"]'), "@Octo-Cat");
    await settle();
    // An `@login` searches no one by name.
    expect(calls.some(c => c.url.includes("/people?q="))).toBe(false);
    expect(m.root.querySelector(".pick [role=status]")!.textContent).toContain("Add @octo-cat by their GitHub login");
    await flush(() => { const s = m.root.querySelector<HTMLSelectElement>(".add .field select")!; s.value = "admin"; s.dispatchEvent(new Event("change", { bubbles: true })); });
    const add = [...m.root.querySelectorAll("button")].find(b => b.textContent === "Add @octo-cat") as HTMLButtonElement;
    expect(add.disabled).toBe(false);
    click(add);
    await settle();
    expect(calls.find(c => c.method === "PUT")).toMatchObject({ url: "/api/queues/q1/members", body: { github_login: "octo-cat", role: "admin" } });
    const row = m.root.querySelector('[data-pending="octo-cat"]')!;
    expect(row.querySelector(".who")!.textContent).toBe("@octo-cat · waiting for their GitHub sign-in");
    expect(row.querySelector(".fixed")!.textContent).toBe("Admin");
    expect(m.root.querySelector(".part [role=status].hint")!.textContent).toBe("@octo-cat becomes a member when they first sign in here with GitHub.");
    expect((m.root.querySelector('input[name="person"]') as HTMLInputElement).value).toBe("");
    click(row.querySelector('[aria-label="Remove @octo-cat"]'));
    await settle();
    expect(calls.some(c => c.method === "DELETE" && c.url === "/api/queues/q1/members/pending/i1")).toBe(true);
    expect(m.root.querySelector("[data-pending]")).toBeNull();
  });

  it("offers no add for text that is no GitHub login, and adds one signed in here as a member at once", async () => {
    const calls = stubFetch(routes({
      "GET /api/queues/q1/members": { members: [], pending: [], github_logins: true },
      "PUT /api/queues/q1/members": [201, { member: { public_id: "p7", name: "Sam", role: "triage", maker: false, owner: false } }],
    }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await settle();
    type(m.root.querySelector('input[name="person"]'), "@-nope");
    await settle();
    expect(([...m.root.querySelectorAll("button")].find(b => b.textContent === "Add member") as HTMLButtonElement).disabled).toBe(true);
    type(m.root.querySelector('input[name="person"]'), "@sam-lee");
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Add @sam-lee"));
    await settle();
    expect(calls.find(c => c.method === "PUT")?.body).toEqual({ github_login: "sam-lee", role: "triage" });
    expect(m.root.querySelector(".part [role=status].hint")!.textContent).toBe("Added Sam (@sam-lee).");
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
