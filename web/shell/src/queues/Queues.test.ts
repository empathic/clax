import { afterEach, describe, expect, it, vi } from "vitest";
import * as me from "../bot/me";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import Queues from "./Queues.svelte";

const QUEUE = { id: "q1", name: "Bugs", origins: ["https://app.example"], reporters_see: "own", public_notice: false, created_at: "2026-10-27T10:00:00Z", role: "admin" };
const routes = (o: Record<string, unknown> = {}) => ({
  "GET /api/queues": { queues: [QUEUE] },
  "GET /api/queues/q1/reports": { reports: [{ state: "open" }, { state: "open" }, { state: "claimed" }], next: null },
  "GET /api/queues/q1/members": { members: [{ public_id: "p2", name: "Mia", role: "triage" }] },
  "GET /api/queues/q1/service-grants": { grants: [] },
  "GET /api/queues/q1/repo": { repo: null, app: { configured: false, install_url: null, forge_api_url: null }, runtime: { configured: false, third_party: false } },
  "GET /api/queues/q1/notifiers/zulip": { notifier: null },
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

  it("sets a queue's public notice from its edit form, and offers it on no new queue", async () => {
    const calls = stubFetch(routes({ "PATCH /api/queues/q1": { queue: { ...QUEUE, public_notice: true } } }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "New queue"));
    await settle();
    expect(m.root.querySelector('form.qform input[name="public_notice"]')).toBeNull();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Cancel"));
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Edit"));
    await settle();
    const box = m.root.querySelector<HTMLInputElement>('form.qform input[name="public_notice"]')!;
    expect(box.checked).toBe(false);
    expect(box.closest("label")!.textContent).toContain("public pull request");
    click(box);
    click(m.root.querySelector('form.qform button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PATCH")!.body).toMatchObject({ public_notice: true });
  });

  it("shows a new service grant's token and key once, with the warning, and forgets them on Done", async () => {
    const made = { grant: { id: "sg1", name: "backend", created_at: "2026-10-27T11:00:00Z" }, token: "cxq_secret", private_key: "MC4CAQAw" };
    const calls = stubFetch(routes({ "POST /api/queues/q1/service-grants": made }));
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await vi.dynamicImportSettled();
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

  it("opens Manage only once its panel loads, and tries again on the first click after a failure", async () => {
    stubFetch(routes());
    const load = vi.spyOn(me, "queueAdmin").mockRejectedValueOnce(new Error("chunk gone"));
    const m = mount(Queues, {});
    await settle();
    const manage = () => [...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage")!;
    click(manage());
    await settle();
    expect(manage().getAttribute("aria-expanded")).toBe("false");
    expect(m.root.querySelector('[data-id="q1"] [role="alert"]')!.textContent).toContain("Could not load this queue's settings.");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Try again"));
    await vi.dynamicImportSettled();
    await settle();
    expect(manage().getAttribute("aria-expanded")).toBe("true");
    expect(m.root.querySelector('[data-id="q1"] [role="alert"]')).toBeNull();
    expect(m.root.querySelector('[data-member="p2"]')).not.toBeNull();
    load.mockRestore();
  });

  it("adds a member by public ID at a role, and removes one after confirming", async () => {
    const calls = stubFetch(routes({ "PUT /api/queues/q1/members/p3": {}, "DELETE /api/queues/q1/members/p2": {} }));
    vi.stubGlobal("confirm", () => true);
    const m = mount(Queues, {});
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Manage"));
    await vi.dynamicImportSettled();
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
