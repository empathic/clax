import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import AdminPage from "./AdminPage.svelte";
import AdminPeople from "./AdminPeople.svelte";
import AdminPricing from "./AdminPricing.svelte";
import AdminProvider from "./AdminProvider.svelte";
import AdminUsage from "./AdminUsage.svelte";
import { adminPerson, meBot, pricing, provider, usage } from "./fixtures";
import { forget } from "./me";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const choose = (el: HTMLSelectElement, value: string) => flush(() => { el.value = value; el.dispatchEvent(new Event("change", { bubbles: true })); });
const button = (root: Element, text: string | RegExp) => [...root.querySelectorAll("button")].find(b => (typeof text === "string" ? b.textContent?.trim() === text : text.test(b.textContent ?? "")))!;

const ALEX = adminPerson({ person: "p_alex", name: "Alex", role: "owner", key_source: "instance", week: { weekly_milli: 2_000_000, lift_milli: 0, spent_milli: 3_280_000 }, tokens_week: 9_400_000 });
const SAM = adminPerson();
const MIA = adminPerson({ person: "p_mia", name: "Mia", role: "admin", flags: { claxbot: false, claxbot_unenforced: true }, key_source: "instance" });

beforeEach(() => { forget(); history.replaceState(null, "", "/admin"); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Admin", () => {
  it("is for admins: anyone else is told so and nothing more is asked", async () => {
    const calls = stubFetch({ "GET /api/me/bot": meBot({ role: "member" }) });
    const m = mount(AdminPage, {});
    await settle();
    expect(m.root.textContent).toContain("Admin is for the owner and the admins they name");
    expect(calls.some(c => c.url.startsWith("/api/admin"))).toBe(false);
  });

  it("opens the tab the address names, among links to the audit log and, locally, Settings", async () => {
    history.replaceState(null, "", "/admin?tab=usage");
    stubFetch({ "GET /api/me/bot": meBot({ role: "admin" }), "GET /api/admin/usage": usage() });
    const m = mount(AdminPage, {});
    await vi.waitFor(() => expect(m.root.textContent).toContain("Credits spent (paid)"));
    const tabs = [...m.root.querySelectorAll(".bpg-tabs a")].map(a => [a.textContent, a.getAttribute("aria-current")]);
    expect(tabs).toEqual([["People & flags", null], ["Provider", null], ["Pricing", null], ["Usage", "page"], ["Audit", null], ["Settings", null]]);
  });
});

describe("Admin › People & flags", () => {
  it("lists people with their role, flags, key, credits and week, and the owner changes roles", async () => {
    const calls = stubFetch({ "GET /api/admin/people": [ALEX, SAM, MIA], "PUT /api/admin/people/p_sam/role": SAM, "PUT /api/admin/people/p_sam/flags": SAM });
    const m = mount(AdminPeople, { role: "owner" });
    await settle();
    const rows = [...m.root.querySelectorAll("tbody tr")];
    expect(rows.map(r => r.querySelector("td")!.textContent)).toEqual(["Alex", "Sam", "Mia"]);
    // The owner's own role is not a choice.
    expect(rows[0].querySelector("select")).toBeNull();
    expect(rows[0].textContent).toContain("Owner · admin");
    expect(rows[1].textContent).toContain("Own key");
    expect(rows[1].querySelector('[data-l="Plan"]')!.textContent).toBe("Lifted +500");
    expect(rows[1].querySelector('[data-l="Weekly credits"]')!.textContent).toContain("2,500");
    expect(rows[2].textContent).toContain("Instance");
    const stats = m.root.querySelector(".stats")!.textContent!;
    expect(stats).toContain("2 of 3");
    expect(stats).toContain("12Runs this week");
    expect(stats).toContain("3Refused at the limit");
    choose(rows[1].querySelector("select")!, "admin");
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({ url: "/api/admin/people/p_sam/role", method: "PUT", body: { role: "admin" } });
    click(rows[1].querySelector('[role="switch"][aria-label="ClaxBot for Sam"]')!);
    await settle();
    expect(calls.filter(c => c.method === "PUT").at(-1)).toEqual({ url: "/api/admin/people/p_sam/flags", method: "PUT", body: { claxbot: false } });
    click(m.root.querySelector('[role="switch"][aria-label="Unenforced limit for Mia"]')!);
    await settle();
    expect(calls.filter(c => c.method === "PUT").at(-1)).toEqual({ url: "/api/admin/people/p_mia/flags", method: "PUT", body: { claxbot_unenforced: false } });
  });

  it("puts a refused role change back in its select", async () => {
    const calls = stubFetch({ "GET /api/admin/people": [ALEX, SAM], "PUT /api/admin/people/p_sam/role": [403, { error: { code: "owner_only", message: "only the owner grants or removes admin" } }] });
    const m = mount(AdminPeople, { role: "owner" });
    await settle();
    const sel = m.root.querySelectorAll("tbody tr")[1].querySelector("select")!;
    choose(sel, "admin");
    await settle();
    expect(calls.filter(c => c.method === "PUT")).toHaveLength(1);
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Only the owner grants or removes admin.");
    expect(sel.value).toBe("member");
    // The list was read again.
    expect(calls.filter(c => c.url === "/api/admin/people")).toHaveLength(2);
  });

  it("shows roles read-only to an admin who is not the owner", async () => {
    stubFetch({ "GET /api/admin/people": [ALEX, SAM] });
    const m = mount(AdminPeople, { role: "admin" });
    await settle();
    expect(m.root.querySelector("tbody select")).toBeNull();
    expect(m.root.querySelectorAll("tbody tr")[1].textContent).toContain("Member");
  });

  it("grants a lift with a reason, and refuses one without", async () => {
    const calls = stubFetch({ "GET /api/admin/people": [SAM], "PUT /api/admin/people/p_sam/credits": SAM });
    const m = mount(AdminPeople, { role: "admin" });
    await settle();
    click(m.root.querySelector('[aria-label="Change the lift of Sam"]')!);
    const [lift, reason] = [...m.root.querySelectorAll<HTMLInputElement>(".lift input")];
    expect(lift.value).toBe("500");
    expect(document.activeElement).toBe(lift);
    type(lift, "1000");
    flush(() => m.root.querySelector(".lift")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("Say why");
    type(reason, "beta tester");
    flush(() => m.root.querySelector(".lift")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({ url: "/api/admin/people/p_sam/credits", method: "PUT", body: { lift_milli: 1_000_000, reason: "beta tester" } });
    expect(m.root.querySelector(".lift")).toBeNull();
  });
});

describe("Admin › Provider", () => {
  it("lists the provider's models, saves what changed, and leaves fields from the file read-only", async () => {
    const calls = stubFetch({
      "GET /api/admin/provider": provider(),
      "GET /api/admin/provider/models": { models: [{ id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5" }, { id: "claude-haiku-5-5", display_name: "Claude Haiku 5.5" }, { id: "claude-opus-5-5", display_name: "Claude Opus 5.5" }], fetched_at: "2026-10-21T12:00:00Z" },
      "PUT /api/admin/provider": (b: unknown) => provider({ base_url: (b as { base_url?: string }).base_url ?? provider().base_url }),
      "POST /api/admin/provider/test": { ok: true, latency_ms: 212 },
    });
    const m = mount(AdminProvider, { now: () => new Date("2026-10-21T12:00:00Z") });
    await settle();
    const sel = m.root.querySelector<HTMLSelectElement>("#pv-model-default")!;
    expect(sel.disabled).toBe(false);
    expect([...sel.options].map(o => o.value)).toContain("claude-opus-5-5");
    expect(sel.value).toBe("claude-sonnet-5-5");
    // The key: never shown, its last four and when it was set.
    expect(m.root.querySelector(".bkey-s")!.textContent).toContain("ending 77d1");
    // Who set it, by name (the daemon sends {id, name}), never "[object Object]".
    expect(m.root.querySelector(".bkey-s")!.textContent).toContain("by Alex");
    expect(m.root.querySelector(".bkey-s")!.textContent).not.toContain("object");
    expect(m.root.textContent).not.toContain("sk-ant");
    // max_steps is set in config.toml: read-only, badged.
    const cap = m.root.querySelector<HTMLInputElement>("#pv-cap-max_steps")!;
    expect(cap.readOnly).toBe(true);
    expect(cap.closest(".field")!.querySelector(".bsrc")!.textContent).toBe("config.toml (read-only)");
    choose(sel, "claude-opus-5-5");
    type(m.root.querySelector<HTMLInputElement>("#pv-base")!, "https://llm.example");
    click(button(m.root, "Save"));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({ url: "/api/admin/provider", method: "PUT", body: { base_url: "https://llm.example", models: { default: "claude-opus-5-5" } } });
    click(button(m.root, "Test connection"));
    await settle();
    expect(m.root.querySelector('[role="status"] .ok')!.textContent).toBe("Connected in 212 ms.");
  });

  it("disables the model selectors with the reason until the models can be listed, and replaces the key write-only", async () => {
    const calls = stubFetch({
      "GET /api/admin/provider": provider({ key: { set: false, last4: null, set_at: null, last_ok_at: null } }),
      "GET /api/admin/provider/models": [409, { error: { code: "no_key", message: "no key is set" } }],
      "PUT /api/admin/provider/key": provider(),
    });
    const m = mount(AdminProvider, {});
    await settle();
    const sel = m.root.querySelector<HTMLSelectElement>("#pv-model-default")!;
    expect(sel.disabled).toBe(true);
    expect(m.root.querySelector("#pv-model-default-why")!.textContent).toContain("No key is set.");
    click(button(m.root, /^Set/));
    const input = m.root.querySelector<HTMLInputElement>('.bkey input[type="password"]')!;
    expect(document.activeElement).toBe(input);
    type(input, "sk-ant-secret-77d1");
    flush(() => input.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    await settle();
    expect(calls.find(c => c.url === "/api/admin/provider/key")).toEqual({ url: "/api/admin/provider/key", method: "PUT", body: { key: "sk-ant-secret-77d1" } });
    // The key is gone from the page once sent; the models are listed again.
    expect(m.root.querySelector('.bkey input[type="password"]')).toBeNull();
    expect(calls.filter(c => c.url === "/api/admin/provider/models")).toHaveLength(2);
  });

  it("takes a custom model ID the provider does not list", async () => {
    const calls = stubFetch({ "GET /api/admin/provider": provider(), "GET /api/admin/provider/models": { models: [{ id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5" }], fetched_at: "x" }, "PUT /api/admin/provider": provider() });
    const m = mount(AdminProvider, {});
    await settle();
    const sel = m.root.querySelector<HTMLSelectElement>("#pv-model-quick")!;
    choose(sel, sel.options[sel.options.length - 1].value);
    const input = m.root.querySelector<HTMLInputElement>("#pv-model-quick")!;
    expect(input.tagName).toBe("INPUT");
    type(input, "claude-haiku-9");
    flush(() => input.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    click(button(m.root, "Save"));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ models: { quick: "claude-haiku-9" } });
  });
});

describe("Admin › Pricing", () => {
  it("derives credits per unit from cost, margin and price, and saves the margins edited", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing(), "PUT /api/admin/pricing": (b: unknown) => b });
    const m = mount(AdminPricing, {});
    await settle();
    const rows = [...m.root.querySelectorAll("tbody tr")];
    expect(rows.map(r => r.querySelectorAll("td")[4].textContent)).toEqual(["420", "0.76", "1"]);
    const margin = rows[0].querySelector<HTMLInputElement>('input[aria-label="Margin on Sonnet 5.5 input, percent"]')!;
    type(margin, "50");
    expect(rows[0].querySelectorAll("td")[4].textContent).toBe("450");
    type(m.root.querySelector<HTMLInputElement>("#pr-free")!, "2500");
    click(button(m.root, "Save"));
    await settle();
    const put = calls.find(c => c.method === "PUT")!.body as { free_weekly_milli: number; lines: { id: string; margin_bp: number }[] };
    expect(put.free_weekly_milli).toBe(2_500_000);
    expect(put.lines.map(l => [l.id, l.margin_bp])).toEqual([["sonnet-in", 5000], ["compute", 9000], ["harness", 0]]);
  });
});

describe("Admin › Usage", () => {
  it("shows revenue, cost, the free tier's cost and the margin before and after it, by person and by line", async () => {
    stubFetch({ "GET /api/admin/usage": usage() });
    const m = mount(AdminUsage, {});
    await settle();
    const stats = m.root.querySelector(".stats")!.textContent!;
    expect(stats).toContain("3,410");
    expect(stats).toContain("$34.10 revenue");
    expect(stats).toContain("$4.60 our cost");
    expect(stats).toContain("32.8%");
    expect(stats).toContain("after the free tier: 19.4%");
    expect(m.root.textContent).toContain("Week of Oct 19 – Oct 25");
    expect([...m.root.querySelectorAll("tbody tr")].map(r => r.querySelector("td")!.textContent)).toEqual(["Sam", "Script compute"]);
  });
});
