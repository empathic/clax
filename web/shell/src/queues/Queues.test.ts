import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import Queues from "./Queues.svelte";

// The shell's clock, with each timer it is asked for kept here, run only by a test.
const timers: { ms: number; fn: () => void }[] = [];
vi.mock("../clock", async orig => ({
  ...(await orig<typeof import("../clock")>()),
  after: (ms: number, fn: () => void) => { const t = { ms, fn }; timers.push(t); return () => { const i = timers.indexOf(t); if (i >= 0) timers.splice(i, 1); }; },
}));
const { VIEW_WAIT_MS } = await import("../view/view-wait");

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

  it("is ready once the queues are counted, and keeps the list shown until a queue's view has its reports", async () => {
    history.replaceState(null, "", "/queues");
    let answer: (() => void) | undefined;
    const held = new Promise<void>(r => { answer = r; });
    let views = 0;
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url === "/api/queues") return new Response(JSON.stringify({ queues: [QUEUE] }));
      // The list's count, then the queue's view's first page, held.
      if (url.startsWith("/api/queues/q1/reports")) { if (url.includes("order=newest") && views++ === 0) await held; return new Response(JSON.stringify({ reports: [], next: null })); }
      return new Response("{}", { status: 404 });
    }));
    const ready = vi.fn();
    const m = mount(Queues, { onready: ready });
    await settle();
    expect(ready).toHaveBeenCalledTimes(1);
    expect(m.root.querySelector(".qcard .counts")!.textContent).not.toContain("Counting");
    click(m.root.querySelector('.qcard h3 a'));
    await settle();
    expect(location.pathname).toBe("/queues/q1");
    // The list stays in view; the queue's view is laid out unseen until its reports are in.
    expect(m.root.querySelector(".qpane:not(.held) .queues")).not.toBeNull();
    expect(m.root.querySelector(".qpane.held .qview")).not.toBeNull();
    answer!();
    await vi.waitFor(() => expect(m.root.querySelector(".qpane:not(.held) .qview .qnone")).not.toBeNull());
    expect(m.root.querySelector(".queues")).toBeNull();
    expect(ready).toHaveBeenCalledTimes(2);
    history.replaceState(null, "", "/");
  });

  it("shows a queue's view whose reports do not come VIEW_WAIT_MS after the move, saying it reads them, and a list whose counts do not come", async () => {
    history.replaceState(null, "", "/queues");
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url === "/api/queues") return new Response(JSON.stringify({ queues: [QUEUE] }));
      // Neither the list's count nor the queue's view's reports ever answer.
      if (url.startsWith("/api/queues/q1/reports")) return new Promise<Response>(() => {});
      return new Response("{}", { status: 404 });
    }));
    const ready = vi.fn();
    const m = mount(Queues, { onready: ready });
    await settle();
    expect(ready).not.toHaveBeenCalled();
    flush(() => timers.find(t => t.ms === VIEW_WAIT_MS)!.fn());
    await settle();
    expect(ready).toHaveBeenCalledTimes(1);
    expect(m.root.querySelector(".qpane:not(.held) .qcard .counts")!.textContent).toBe("Counting reports…");
    click(m.root.querySelector(".qcard h3 a"));
    await settle();
    expect(m.root.querySelector(".qpane.held .qview")).not.toBeNull();
    flush(() => timers.find(t => t.ms === VIEW_WAIT_MS)!.fn());
    await settle();
    expect(m.root.querySelector(".qpane:not(.held) .qview")).not.toBeNull();
    expect(m.root.querySelector(".queues")).toBeNull();
    expect(ready).toHaveBeenCalledTimes(2);
    history.replaceState(null, "", "/");
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
