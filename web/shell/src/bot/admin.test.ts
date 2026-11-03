import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import AdminPage from "./AdminPage.svelte";
import AdminPeople from "./AdminPeople.svelte";
import AdminPricing from "./AdminPricing.svelte";
import AdminProvider from "./AdminProvider.svelte";
import AdminUsage from "./AdminUsage.svelte";
import AdminWeb from "./AdminWeb.svelte";
import type { ModelPricing } from "./api";
import { adminPerson, defaultPriced, listing, meBot, modelPricing, pricing, provider, unitPricing, usage, webSettings, WEB_LINES } from "./fixtures";
import { forget } from "./me";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const choose = (el: HTMLSelectElement, value: string) => flush(() => { el.value = value; el.dispatchEvent(new Event("change", { bubbles: true })); });
const pick = (el: HTMLInputElement) => flush(() => { el.checked = true; el.dispatchEvent(new Event("change", { bubbles: true })); });
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
    expect(tabs).toEqual([["People & flags", null], ["Provider", null], ["Web", null], ["Pricing", null], ["Usage", "page"], ["Audit", null], ["Settings", null]]);
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

  it("picks another API with a base URL known for it, and keeps a base URL of its own", async () => {
    const calls = stubFetch({
      "GET /api/admin/provider": provider(),
      "GET /api/admin/provider/models": { models: [], fetched_at: "x" },
      "PUT /api/admin/provider": (b: unknown) => provider(b as Partial<ReturnType<typeof provider>>),
    });
    const m = mount(AdminProvider, {});
    await settle();
    const api = m.root.querySelector<HTMLSelectElement>("#pv-api")!;
    const base = m.root.querySelector<HTMLInputElement>("#pv-base")!;
    expect(api.value).toBe("anthropic");
    choose(api, "gemini");
    expect(base.value).toBe("https://generativelanguage.googleapis.com/v1beta");
    choose(api, "openai");
    expect(base.value).toBe("https://api.mistral.ai/v1");
    expect([...m.root.querySelectorAll("#pv-base-urls option")].map(o => (o as HTMLOptionElement).value)).toContain("https://api.subconscious.dev/v1");
    type(base, "https://api.subconscious.dev/v1");
    choose(api, "openai");
    expect(base.value).toBe("https://api.subconscious.dev/v1");
    click(button(m.root, "Save"));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ api: "openai", base_url: "https://api.subconscious.dev/v1" });
    expect(m.root.querySelector('[role="status"] .ok')!.textContent).toBe("Saved. Set the key again for the new base URL.");
  });

  it("shows each listed model's price, and says when the selected one's is estimated or missing", async () => {
    const priced = (source: "line" | "catalog" | "default" | "mixed" | null, i: number | null, o: number | null) => ({ source, estimated: source !== "line" && source !== null, input_credits_milli_per_mtok: i, output_credits_milli_per_mtok: o });
    stubFetch({
      "GET /api/admin/provider": provider({ models: { default: "acme-small-2", quick: "gemini-3.5-flash", complex: "claude-opus-5-5" } }),
      "GET /api/admin/provider/models": { models: [
        { id: "gemini-3.5-flash", display_name: "Gemini 3.5 Flash", pricing: priced("catalog", 105_000, 630_000) },
        { id: "acme-small-2", display_name: "acme-small-2", pricing: priced("default", 560_000, 2_800_000) },
        { id: "claude-opus-5-5", display_name: "Claude Opus 5.5", pricing: priced(null, null, null) },
        { id: "claude-haiku-5-5", display_name: "Claude Haiku 5.5", pricing: priced("mixed", 140_000, 700_000) },
      ], fetched_at: "x" },
    });
    const m = mount(AdminProvider, {});
    await settle();
    const sel = m.root.querySelector<HTMLSelectElement>("#pv-model-default")!;
    expect([...sel.options].slice(0, 4).map(o => o.textContent)).toEqual([
      "Gemini 3.5 Flash (gemini-3.5-flash) — 105 / 630 credits per Mtok, tokenulator.dev, estimated",
      "acme-small-2 — 560 / 2,800 credits per Mtok, default rate, estimated",
      "Claude Opus 5.5 (claude-opus-5-5) — no price",
      "Claude Haiku 5.5 (claude-haiku-5-5) — 140 / 700 credits per Mtok, set in part, estimated",
    ]);
    const note = m.root.querySelector("#pv-model-default-price")!;
    expect(note.getAttribute("role")).toBe("status");
    expect(note.textContent).toContain("Estimated: no price set.");
    expect(sel.getAttribute("aria-describedby")).toBe("pv-model-default-price");
    // A model priced from the catalog is estimated too; one with no price says runs on it are refused.
    expect(m.root.querySelector("#pv-model-quick-price")!.textContent).toContain("Estimated: no price set. Runs are charged at tokenulator.dev's price");
    expect(m.root.querySelector("#pv-model-complex-price")!.textContent).toContain("refused");
    // A price set in part is estimated in part.
    choose(sel, "claude-haiku-5-5");
    expect(m.root.querySelector("#pv-model-default-price")!.textContent).toBe("Estimated: only part of its price is set. The rest is charged at tokenulator.dev's price or the default rates until it is priced under Pricing.");
  });

  it("leaves the API read-only when the configuration sets it", async () => {
    stubFetch({ "GET /api/admin/provider": provider({ api: "gemini", sources: { ...provider().sources, api: "file" } }), "GET /api/admin/provider/models": { models: [], fetched_at: "x" } });
    const m = mount(AdminProvider, {});
    await settle();
    const api = m.root.querySelector<HTMLSelectElement>("#pv-api")!;
    expect(api.value).toBe("gemini");
    expect(api.disabled).toBe(true);
  });
});

describe("Admin › Web", () => {
  const WEB_PRICING = pricing({ lines: [...pricing().lines, ...WEB_LINES] });

  it("the_web_tab_saves_the_rule", async () => {
    const calls = stubFetch({
      "GET /api/admin/web": webSettings({ sources: { "fetch.max_per_run": "file" } }),
      "GET /api/admin/pricing": WEB_PRICING,
      "PUT /api/admin/web": (b: unknown) => webSettings({ configured: true, search: { ...webSettings().search, enabled: true, provider: "exa" }, fetch: { ...webSettings().fetch, enabled: true, url_rule: "allowlist", allowlist: (b as { fetch: { allowlist: string[] } }).fetch.allowlist } }),
    });
    const m = mount(AdminWeb, {});
    await settle();
    // The risk, beside the switches.
    expect(m.root.textContent).toContain("Web content can try to steer ClaxBot. It fetches only URLs from the sender's words, searches and pages it read, but which links it follows and what it searches for can still carry data out.");
    const search = m.root.querySelector<HTMLElement>('[role="switch"][aria-label="Web search"]')!;
    const fetchSw = m.root.querySelector<HTMLElement>('[role="switch"][aria-label="Web fetch"]')!;
    expect([search.getAttribute("aria-checked"), fetchSw.getAttribute("aria-checked")]).toEqual(["false", "false"]);
    // Each provider with its price from the Pricing lines.
    const prov = m.root.querySelector<HTMLSelectElement>("#aw-provider")!;
    expect([...prov.options].map(o => o.textContent)).toEqual(["Ceramic · 25 credits per thousand requests", "Exa · 700 credits per thousand requests", "Linkup · 500 credits per thousand requests"]);
    expect(m.root.querySelector<HTMLInputElement>("#aw-max-fetch")!.readOnly).toBe(true);
    const rule = m.root.querySelector<HTMLSelectElement>("#aw-rule")!;
    expect([...rule.options].map(o => [o.value, o.textContent])).toEqual([["sourced", "Only URLs from the sender, searches and fetched pages"], ["any_public", "Any public URL"], ["allowlist", "Allowlist"]]);
    expect(m.root.querySelector("#aw-allow")).toBeNull();
    expect(m.root.textContent).not.toContain("Any URL ClaxBot writes may be fetched");
    choose(rule, "any_public");
    expect(m.root.textContent).toContain("Any URL ClaxBot writes may be fetched, so a page can make it send data out.");
    choose(rule, "allowlist");
    expect(m.root.textContent).not.toContain("Any URL ClaxBot writes may be fetched");
    type(m.root.querySelector<HTMLTextAreaElement>("#aw-allow") as unknown as HTMLInputElement, "docs.example.com\n*.rust-lang.org\n");
    click(search);
    click(fetchSw);
    choose(prov, "exa");
    click(button(m.root, "Save"));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({ url: "/api/admin/web", method: "PUT", body: { search: { enabled: true, provider: "exa" }, fetch: { enabled: true, url_rule: "allowlist", allowlist: ["docs.example.com", "*.rust-lang.org"] } } });
    expect(m.root.querySelector('[role="status"]')!.textContent).toContain("Saved.");
  });

  it("shows why web is unavailable, in place of the credentials hint", async () => {
    const why = 'the site domain "https://sites.example/x" is not a bare host name';
    stubFetch({ "GET /api/admin/web": webSettings({ unavailable: why }), "GET /api/admin/pricing": WEB_PRICING });
    const m = mount(AdminWeb, {});
    await settle();
    expect(m.root.textContent).toContain(`Web access is off: ${why}`);
    expect(m.root.textContent).not.toContain("Not configured: web access needs");
  });

  it("the_token_is_write_only", async () => {
    const calls = stubFetch({
      "GET /api/admin/web": webSettings(),
      "GET /api/admin/pricing": WEB_PRICING,
      "PUT /api/admin/web/token": webSettings({ configured: true, cloudflare: { account_id: "", token: { set: true, last4: "9f3a", set_at: "2026-10-21T09:00:00Z", set_by: { id: "*owner", name: "Alex" }, last_ok_at: null } } }),
    });
    const m = mount(AdminWeb, { now: () => new Date("2026-10-21T12:00:00Z") });
    await settle();
    expect(m.root.textContent).toContain("Not configured");
    click(button(m.root, /^Set/));
    const input = m.root.querySelector<HTMLInputElement>('.bkey input[type="password"]')!;
    type(input, "cf-secret-9f3a");
    flush(() => input.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    await settle();
    expect(calls.find(c => c.url === "/api/admin/web/token")).toEqual({ url: "/api/admin/web/token", method: "PUT", body: { token: "cf-secret-9f3a" } });
    expect(m.root.querySelector('.bkey input[type="password"]')).toBeNull();
    expect(m.root.querySelector(".bkey-s")!.textContent).toContain("ending 9f3a");
    expect(m.root.textContent).not.toContain("cf-secret");
  });

  it("saves the gateway, the provider key alias and the results per search, and refuses a limit outside 1 to 10", async () => {
    const calls = stubFetch({
      "GET /api/admin/web": webSettings({ sources: { "search.limit": "default" } }),
      "PUT /api/admin/web": (b: unknown) => { const x = (b as { search: { gateway: string; byok_alias: string; limit: number } }).search; return webSettings({ search: { ...webSettings().search, ...x } }); },
    });
    const m = mount(AdminWeb, {});
    await settle();
    expect(m.root.textContent).toContain("With an alias, searches bill the provider key stored on the gateway instead of Cloudflare credits.");
    type(m.root.querySelector("#aw-limit")!, "11");
    click(button(m.root, "Save"));
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Results per search is a whole number from 1 to 10.");
    expect(calls.some(c => c.method === "PUT")).toBe(false);
    type(m.root.querySelector("#aw-limit")!, "7");
    type(m.root.querySelector("#aw-gateway")!, " edge ");
    type(m.root.querySelector("#aw-byok")!, "exa-key");
    click(button(m.root, "Save"));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ search: { gateway: "edge", byok_alias: "exa-key", limit: 7 } });
    expect(m.root.querySelector('[role="alert"]')).toBeNull();
  });

  it("never sends a field the configuration sets, says Nothing changed, and refuses a cap that is not a whole number", async () => {
    const calls = stubFetch({
      "GET /api/admin/web": webSettings({ sources: { "search.provider": "file", "fetch.max_per_run": "env:CLAX_BOT__WEB", "search.limit": "file", "search.gateway": "file", "search.byok_alias": "file" } }),
      "PUT /api/admin/web": webSettings(),
    });
    const m = mount(AdminWeb, {});
    await settle();
    click(button(m.root, "Save"));
    expect(m.root.querySelector('[role="status"]')!.textContent).toBe("Nothing changed.");
    expect(calls.some(c => c.method === "PUT")).toBe(false);
    choose(m.root.querySelector("#aw-provider")!, "exa");
    type(m.root.querySelector("#aw-max-fetch")!, "99");
    type(m.root.querySelector("#aw-limit")!, "9");
    type(m.root.querySelector("#aw-gateway")!, "other");
    type(m.root.querySelector("#aw-byok")!, "other");
    type(m.root.querySelector("#aw-max-search")!, "0");
    click(button(m.root, "Save"));
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Searches per run is a whole number, 1 or more.");
    type(m.root.querySelector("#aw-max-search")!, "12");
    click(button(m.root, "Save"));
    await settle();
    // Only the one field the configuration leaves editable is sent.
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ search: { max_per_run: 12 } });
  });

  it("keeps unsaved edits when the token is set", async () => {
    stubFetch({
      "GET /api/admin/web": webSettings(),
      "PUT /api/admin/web/token": webSettings({ search: { ...webSettings().search, enabled: true }, cloudflare: { account_id: "", token: { set: true, last4: "9f3a", set_at: "2026-10-21T09:00:00Z", last_ok_at: null } } }),
    });
    const m = mount(AdminWeb, {});
    await settle();
    type(m.root.querySelector("#aw-max-search")!, "4");
    choose(m.root.querySelector("#aw-rule")!, "any_public");
    click(button(m.root, /^Set/));
    const input = m.root.querySelector<HTMLInputElement>('.bkey input[type="password"]')!;
    type(input, "cf-secret-9f3a");
    flush(() => input.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    await settle();
    expect(m.root.querySelector(".bkey-s")!.textContent).toContain("ending 9f3a");
    expect(m.root.querySelector<HTMLInputElement>("#aw-max-search")!.value).toBe("4");
    expect(m.root.querySelector<HTMLSelectElement>("#aw-rule")!.value).toBe("any_public");
    expect(m.root.querySelector('[role="switch"][aria-label="Web search"]')!.getAttribute("aria-checked")).toBe("false");
  });

  it("is not asked for the daemon's account on a hosted Clax", async () => {
    stubFetch({ "GET /api/admin/web": webSettings(), "GET /api/hosted/me": { name: "x" } });
    const m = mount(AdminWeb, {});
    await settle();
    expect(m.root.querySelector("#aw-account")).toBeNull();
    expect(m.root.querySelector(".bkey")).toBeNull();
  });
});

describe("Admin › Pricing", () => {
  const SONNET = "claude-sonnet-5-5";
  const row = (root: Element, model: string) => root.querySelector<HTMLTableRowElement>(`tr[data-model="${model}"]`)!;
  const cell = (root: Element, model: string, unit: string) => row(root, model).querySelector(`[data-unit="${unit}"]`)!;
  const status = (root: Element, section: string) => root.querySelector(`section[aria-labelledby=${section}] [role="status"]`)!.textContent!.trim();
  const editOf = (root: Element, model: string) => root.querySelector<HTMLButtonElement>(`[aria-label="Edit the rates of ${model}"]`)!;
  const removeOf = (root: Element, model: string) => root.querySelector<HTMLButtonElement>(`[aria-label="Remove the rates set for ${model}"]`)!;
  /** Sonnet with its four units at tokenulator.dev's price, its lines gone. */
  const fromCatalog = () => modelPricing({ units: Object.fromEntries(Object.entries(modelPricing().units).map(([k, u]) => [k, { ...u, source: "catalog", line_source: null }])) as ModelPricing["units"] });

  it("derives the other lines' credits per unit, and saves their margins, the default rates and the explicit-price switch, never a token line", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing(), "PUT /api/admin/pricing": pricing() });
    const m = mount(AdminPricing, {});
    await settle();
    const rows = [...m.root.querySelectorAll("section[aria-labelledby=pr-l] tbody tr")];
    expect(rows.map(r => r.querySelectorAll("td")[4].textContent)).toEqual(["0.76", "1"]);
    type(rows[0].querySelector<HTMLInputElement>('input[aria-label="Margin on Script compute, percent"]')!, "100");
    expect(rows[0].querySelectorAll("td")[4].textContent).toBe("0.80");
    type(m.root.querySelector<HTMLInputElement>("#pr-free")!, "2500");
    const input = m.root.querySelector<HTMLInputElement>("#pr-d-input")!;
    expect(input.value).toBe("560");
    type(input, "600");
    // The cache-write rate is set in config.toml: read-only, badged, and not sent.
    const cw = m.root.querySelector<HTMLInputElement>("#pr-d-cache_write")!;
    expect(cw.readOnly).toBe(true);
    expect(cw.closest(".field")!.querySelector(".bsrc")!.textContent).toBe("config.toml (read-only)");
    click(m.root.querySelector<HTMLInputElement>("#pr-strict")!);
    click(button(m.root, "Save"));
    await settle();
    const put = calls.find(c => c.method === "PUT")!.body as { free_weekly_milli: number; lines: { id: string; margin_bp: number }[]; default_rates: object; require_explicit_price: boolean };
    expect(put.free_weekly_milli).toBe(2_500_000);
    expect(put.lines.map(l => [l.id, l.margin_bp])).toEqual([["compute", 10000], ["harness", 0]]);
    expect(put.default_rates).toEqual({ input: 600_000, cached_input: 28_000, output: 2_800_000 });
    expect(put.require_explicit_price).toBe(true);
    expect(m.root.querySelector(".pr-save .ok")!.textContent).toBe("Saved. New usage is charged at these rates.");
  });

  it("says what is wrong with the form beside Save, as an alert", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing(), "PUT /api/admin/pricing": [409, { error: { code: "setting_overridden", message: "the free tier is set in config.toml" } }] });
    const m = mount(AdminPricing, {});
    await settle();
    type(m.root.querySelector<HTMLInputElement>("#pr-free")!, "-1");
    click(button(m.root, "Save"));
    expect(m.root.querySelector(".pr-save [role=alert]")!.textContent).toBe("The free tier is a number of credits, 0 or more.");
    expect(calls.some(c => c.method === "PUT")).toBe(false);
    type(m.root.querySelector<HTMLInputElement>("#pr-free")!, "100");
    click(button(m.root, "Save"));
    await settle();
    expect(m.root.querySelector(".pr-save [role=alert]")!.textContent).toBe("The free tier is set in config.toml.");
  });

  it("leaves the explicit-price switch alone when the configuration sets it", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing({ require_explicit_price: true, sources: { ...pricing().sources, require_explicit_price: "file" } }), "PUT /api/admin/pricing": pricing() });
    const m = mount(AdminPricing, {});
    await settle();
    const box = m.root.querySelector<HTMLInputElement>("#pr-strict")!;
    expect([box.checked, box.disabled]).toEqual([true, true]);
    expect(box.parentElement!.querySelector(".bsrc")!.textContent).toBe("config.toml (read-only)");
    click(button(m.root, "Save"));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).not.toHaveProperty("require_explicit_price");
    // The lines go back exactly as GET listed them: no token line among them.
    expect((calls.find(c => c.method === "PUT")!.body as { lines: unknown }).lines).toEqual(pricing().lines);
  });

  it("lists each model's rates as charged with its name, tiers and sources, and refreshes the catalog", async () => {
    const mixed = modelPricing({ model: "claude-haiku-5-5", name: "Claude Haiku 5.5", tiers: [], units: { ...modelPricing().units, input: unitPricing({ source: "catalog" }) } });
    const calls = stubFetch({
      "GET /api/admin/pricing": pricing({ models: [modelPricing(), defaultPriced("acme-small-2", ["quick"]), mixed] }),
      "POST /api/admin/pricing/catalog/refresh": pricing({ catalog: { fetched_at: "2026-11-01T09:30:00Z", provider: "anthropic" } }),
    });
    const m = mount(AdminPricing, {});
    await settle();
    expect([...m.root.querySelectorAll(".pr-m tbody tr")].map(r => r.querySelector("b")!.textContent)).toEqual(["Claude Sonnet 5.5", "acme-small-2", "Claude Haiku 5.5"]);
    expect(row(m.root, SONNET).querySelector(".src")!.textContent).toBe(SONNET);
    expect(row(m.root, SONNET).querySelector(".pr-tier")!.textContent).toBe("default");
    expect(cell(m.root, SONNET, "input").textContent).toContain("420");
    expect(cell(m.root, SONNET, "input").textContent).toContain("$3.00 + 40%");
    // Four units from one source: one badge for the row, the lines' source as its title.
    const one = row(m.root, SONNET).querySelectorAll(".bsrc");
    expect([...one].map(b => b.textContent)).toEqual(["set"]);
    expect(one[0].getAttribute("title")).toBe("Anthropic list price as of 2026-10-06 (check)");
    expect(cell(m.root, "acme-small-2", "output").textContent).toContain("2,800");
    expect(row(m.root, "acme-small-2").querySelector(".bsrc")!.textContent).toBe("default rate, estimated");
    expect(row(m.root, "acme-small-2").querySelector(".pr-tier")!.textContent).toBe("quick");
    // Mixed sources: a badge per unit.
    expect(cell(m.root, "claude-haiku-5-5", "input").querySelector(".bsrc")!.textContent).toBe("tokenulator.dev, estimated");
    expect(cell(m.root, "claude-haiku-5-5", "output").querySelector(".bsrc")!.textContent).toBe("set");
    expect(cell(m.root, "claude-haiku-5-5", "output").querySelector(".bsrc")!.getAttribute("title")).toBe("Anthropic list price as of 2026-10-06 (check)");
    expect(cell(m.root, "claude-haiku-5-5", "input").querySelector(".bsrc")!.getAttribute("title")).toBeNull();
    // A model with no rate card set has nothing to remove.
    expect(removeOf(m.root, "acme-small-2")).toBeNull();
    const cat = m.root.querySelector(".pr-cat")!;
    expect(cat.textContent).toContain("tokenulator.dev prices as of Oct 31, 2026, 06:00 UTC (provider anthropic)");
    click(button(cat, "Refresh"));
    expect(button(cat, "Refreshing…")).toBeTruthy();
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: "/api/admin/pricing/catalog/refresh", method: "POST", body: {} });
    expect(cat.textContent).toContain("as of Nov 1, 2026, 09:30 UTC");
    expect(status(m.root, "pr-m")).toBe("Fetched tokenulator.dev's prices.");
  });

  it("says when the catalog is empty or names no provider, and when a refresh fails", async () => {
    stubFetch({ "GET /api/admin/pricing": pricing({ catalog: { fetched_at: null, provider: null } }), "POST /api/admin/pricing/catalog/refresh": [502, { error: { code: "tokenulator_error", message: "tokenulator.dev could not be read" } }] });
    const m = mount(AdminPricing, {});
    await settle();
    expect(m.root.querySelector(".pr-cat")!.textContent).toContain("No tokenulator.dev prices fetched yet (the base URL names no tokenulator.dev provider)");
    click(button(m.root, "Refresh"));
    await settle();
    expect(m.root.querySelector("section[aria-labelledby=pr-m] [role=status] .err")!.textContent).toBe("tokenulator.dev could not be read.");
  });

  it("edits a model's rate card below the table, unit by unit at cost plus margin or at fixed credits, keeping its name", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing(), "POST /api/admin/pricing/models": pricing() });
    const m = mount(AdminPricing, {});
    await settle();
    const edit = editOf(m.root, SONNET);
    click(edit);
    expect(edit.getAttribute("aria-expanded")).toBe("true");
    const card = m.root.querySelector("#pr-card")!;
    expect(edit.getAttribute("aria-controls")).toBe("pr-card");
    // Outside the table, so it is not held to the table's width.
    expect(card.closest("table")).toBeNull();
    expect(card.querySelector("h4")!.textContent).toBe("Rate card for Claude Sonnet 5.5");
    const id = "pr-e-claude-sonnet-5-5";
    const how = m.root.querySelector<HTMLSelectElement>(`#${id}-input-how`)!;
    expect(document.activeElement).toBe(how);
    const usd = m.root.querySelector<HTMLInputElement>(`#${id}-input-usd`)!;
    expect([usd.value, m.root.querySelector<HTMLInputElement>(`#${id}-input-margin`)!.value]).toEqual(["3", "40"]);
    // A blank cost is refused before anything is sent.
    type(usd, "");
    click(button(m.root, "Save rates"));
    expect(m.root.querySelector(".rcard [role=alert]")!.textContent).toBe("Input: the cost is US dollars per million tokens, 0 or more.");
    type(usd, "3");
    type(m.root.querySelector<HTMLInputElement>(`#${id}-input-margin`)!, "50");
    const eq = () => usd.closest("fieldset")!.querySelector(".rcard-eq")!.textContent;
    expect(eq()).toBe("= 450 credits per million tokens");
    // The preview is at the saved credit price, not one typed and not saved.
    type(m.root.querySelector<HTMLInputElement>("#pr-price")!, "0.02");
    expect(eq()).toBe("= 450 credits per million tokens");
    choose(m.root.querySelector<HTMLSelectElement>(`#${id}-output-how`)!, "fixed");
    const fixed = m.root.querySelector<HTMLInputElement>(`#${id}-output-credits`)!;
    expect(fixed.value).toBe("2100");
    type(fixed, "2000");
    click(button(m.root, "Save rates"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: "/api/admin/pricing/models", method: "POST", body: {
      model: SONNET,
      input: { cost_micro_usd: 3_000_000, margin_bp: 5000, fixed_credits_milli: null },
      cached_input: { cost_micro_usd: 300_000, margin_bp: 4000, fixed_credits_milli: null },
      cache_write: { cost_micro_usd: 3_750_000, margin_bp: 4000, fixed_credits_milli: null },
      output: { cost_micro_usd: 15_000_000, margin_bp: 4000, fixed_credits_milli: 2_000_000 },
    } });
    expect(m.root.querySelector("#pr-card")).toBeNull();
    expect(document.activeElement).toBe(editOf(m.root, SONNET));
    expect(status(m.root, "pr-m")).toBe("Saved the rates of claude-sonnet-5-5.");
  });

  it("returns focus to Edit when the editor is cancelled", async () => {
    stubFetch({ "GET /api/admin/pricing": pricing() });
    const m = mount(AdminPricing, {});
    await settle();
    click(editOf(m.root, SONNET));
    click(button(m.root.querySelector("#pr-card")!, "Cancel"));
    await settle();
    expect(m.root.querySelector("#pr-card")).toBeNull();
    expect(document.activeElement).toBe(editOf(m.root, SONNET));
  });

  it("starts a default-priced model's card at its fixed credits, and at the token margin once a unit is cost-based", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing({ token_margin_bp: 3500 }), "POST /api/admin/pricing/models": pricing() });
    const m = mount(AdminPricing, {});
    await settle();
    click(editOf(m.root, "acme-small-2"));
    const id = "pr-e-acme-small-2";
    expect(m.root.querySelector<HTMLInputElement>(`#${id}-input-credits`)!.value).toBe("560");
    choose(m.root.querySelector<HTMLSelectElement>(`#${id}-input-how`)!, "cost");
    expect(m.root.querySelector<HTMLInputElement>(`#${id}-input-usd`)!.value).toBe("");
    expect(m.root.querySelector<HTMLInputElement>(`#${id}-input-margin`)!.value).toBe("35");
    type(m.root.querySelector<HTMLInputElement>(`#${id}-input-usd`)!, "0.5");
    click(button(m.root, "Save rates"));
    await settle();
    const body = calls.find(c => c.method === "POST")!.body as Record<string, unknown>;
    expect(body).not.toHaveProperty("name");
    expect(body.input).toEqual({ cost_micro_usd: 500_000, margin_bp: 3500, fixed_credits_milli: null });
    expect(body.output).toEqual({ cost_micro_usd: 0, margin_bp: 3500, fixed_credits_milli: 2_800_000 });
  });

  it("runs one write at a time: Save waits for a rate card in flight, and only the busy action says so", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing(), "POST /api/admin/pricing/models": pricing(), "PUT /api/admin/pricing": pricing() });
    const stubbed = globalThis.fetch;
    let release = () => {};
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      if (init?.method === "POST") await new Promise<void>(r => { release = r; });
      return stubbed(url, init);
    }));
    const m = mount(AdminPricing, {});
    await settle();
    click(editOf(m.root, SONNET));
    click(button(m.root, "Save rates"));
    await settle();
    expect(button(m.root, "Saving…").closest(".rcard")).not.toBeNull();
    expect(button(m.root, "Look up and price")).toBeTruthy();
    expect(button(m.root, "Save").getAttribute("aria-disabled")).toBe("true");
    click(button(m.root, "Save"));
    await settle();
    expect(calls.some(c => c.method === "PUT")).toBe(false);
    release();
    await settle();
    click(button(m.root, "Save"));
    await settle();
    expect((calls.find(c => c.method === "PUT")!.body as { lines: { id: string }[] }).lines.map(l => l.id)).toEqual(["compute", "harness"]);
  });

  it("flags a set cost that drifted from tokenulator.dev's and replaces the card at the catalog's cost and the token margin", async () => {
    const drifting = modelPricing({ drift: true, listing: listing({ input_micro_usd: 2_500_000 }), units: { ...modelPricing().units, input: unitPricing({ catalog_micro_usd: 2_500_000, drift: true }) } });
    const calls = stubFetch({ "GET /api/admin/pricing": pricing({ models: [drifting] }), "POST /api/admin/pricing/tokenulator": pricing() });
    const m = mount(AdminPricing, {});
    await settle();
    const flag = row(m.root, SONNET).querySelector(".pr-drift")!;
    expect(flag.textContent).toContain("tokenulator.dev lists another price: input $2.50 per million tokens. Using it sets every unit at tokenulator.dev's cost plus the 40% token margin.");
    expect(cell(m.root, SONNET, "input").classList.contains("drift")).toBe(true);
    click(button(flag, "Replace the rate card with tokenulator.dev's price"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: "/api/admin/pricing/tokenulator", method: "POST", body: { model: SONNET, provider: "anthropic" } });
    expect(row(m.root, SONNET).querySelector(".pr-drift")).toBeNull();
    expect(status(m.root, "pr-m")).toBe("Priced claude-sonnet-5-5 at tokenulator.dev's price.");
  });

  it("removes a model's rate card once confirmed, naming what charges it next, and says why while a run on it has not ended", async () => {
    let busyRun = true;
    const calls = stubFetch({
      "GET /api/admin/pricing": pricing(),
      "DELETE /api/admin/pricing/models?model=claude-sonnet-5-5": () => busyRun ? [409, { error: { code: "cost_line_in_use", message: "a run is using claude-sonnet-5-5.input" } }] : pricing({ models: [fromCatalog()] }),
    });
    const asked: string[] = [];
    let answer = false;
    vi.stubGlobal("confirm", (q: string) => { asked.push(q); return answer; });
    const m = mount(AdminPricing, {});
    await settle();
    click(removeOf(m.root, SONNET));
    await settle();
    expect(asked).toEqual(["Remove the rates set for claude-sonnet-5-5? Its tokens are then charged at tokenulator.dev's price, an estimate."]);
    expect(calls.some(c => c.method === "DELETE")).toBe(false);
    answer = true;
    click(removeOf(m.root, SONNET));
    await settle();
    expect(status(m.root, "pr-m")).toBe("A run on claude-sonnet-5-5 has not ended yet: remove its rates once it has.");
    busyRun = false;
    click(removeOf(m.root, SONNET));
    await settle();
    expect(calls.filter(c => c.method === "DELETE").map(c => c.url)).toEqual(["/api/admin/pricing/models?model=claude-sonnet-5-5", "/api/admin/pricing/models?model=claude-sonnet-5-5"]);
    expect(status(m.root, "pr-m")).toBe("Removed the rates set for claude-sonnet-5-5: its tokens are charged at tokenulator.dev's price, an estimate.");
    expect(row(m.root, SONNET).querySelector(".bsrc")!.textContent).toBe("tokenulator.dev, estimated");
    expect(removeOf(m.root, SONNET)).toBeNull();
    // The Remove button is gone: focus goes to the row's Edit.
    expect(document.activeElement).toBe(editOf(m.root, SONNET));
  });

  it("says what charges a model in no tier once its card is removed, though the pricing no longer lists it", async () => {
    const haiku = modelPricing({ model: "claude-haiku-5-5", name: "Claude Haiku 5.5", tiers: [], listing: null });
    stubFetch({ "GET /api/admin/pricing": pricing({ models: [modelPricing(), haiku] }), "DELETE /api/admin/pricing/models?model=claude-haiku-5-5": pricing({ models: [modelPricing()] }) });
    vi.stubGlobal("confirm", () => true);
    const m = mount(AdminPricing, {});
    await settle();
    click(removeOf(m.root, "claude-haiku-5-5"));
    await settle();
    expect(row(m.root, "claude-haiku-5-5")).toBeNull();
    expect(status(m.root, "pr-m")).toBe("Removed the rates set for claude-haiku-5-5: its tokens are charged at the default rates, an estimate.");
    // Its row is gone: focus goes to the Models heading.
    expect(document.activeElement).toBe(m.root.querySelector("#pr-m"));
  });

  it("warns that removing a card refuses runs on the model when an explicit price is required", async () => {
    stubFetch({ "GET /api/admin/pricing": pricing({ require_explicit_price: true }) });
    const asked: string[] = [];
    vi.stubGlobal("confirm", (q: string) => { asked.push(q); return false; });
    const m = mount(AdminPricing, {});
    await settle();
    click(removeOf(m.root, SONNET));
    expect(asked[0]).toBe("Remove the rates set for claude-sonnet-5-5? Its tokens are then charged at no price: runs on it are refused.");
  });

  it("adds a model priced from tokenulator.dev and says when it is not listed", async () => {
    const calls = stubFetch({
      "GET /api/admin/pricing": pricing(),
      "POST /api/admin/pricing/tokenulator": (b: unknown) => (b as { model: string }).model === "gemini-3.5-flash"
        ? pricing({ models: [...pricing().models, modelPricing({ model: "gemini-3.5-flash", name: "Gemini 3.5 Flash", tiers: [] })] })
        : [404, { error: { code: "not_on_tokenulator", message: "tokenulator.dev lists no price for zai-glm-5-3-flash from mistral" } }],
    });
    const m = mount(AdminPricing, {});
    await settle();
    click(button(m.root, "Look up and price"));
    expect(status(m.root, "pr-a")).toBe("Give the model's ID.");
    type(m.root.querySelector<HTMLInputElement>("#pr-a-model")!, "gemini-3.5-flash");
    click(button(m.root, "Look up and price"));
    expect(button(m.root, "Looking up…")).toBeTruthy();
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: "/api/admin/pricing/tokenulator", method: "POST", body: { model: "gemini-3.5-flash" } });
    expect(status(m.root, "pr-a")).toBe("Priced gemini-3.5-flash from tokenulator.dev.");
    expect(row(m.root, "gemini-3.5-flash")).not.toBeNull();
    expect(m.root.querySelector<HTMLInputElement>("#pr-a-model")!.value).toBe("");
    type(m.root.querySelector<HTMLInputElement>("#pr-a-model")!, "zai-glm-5-3-flash");
    type(m.root.querySelector<HTMLInputElement>("#pr-a-provider")!, "mistral");
    click(button(m.root, "Look up and price"));
    await settle();
    expect(calls.filter(c => c.method === "POST")[1].body).toEqual({ model: "zai-glm-5-3-flash", provider: "mistral" });
    expect(m.root.querySelector("section[aria-labelledby=pr-a] [role=status] .err")!.textContent).toContain("lists no price");
  });

  it("adds a model with a rate card of its own", async () => {
    const calls = stubFetch({ "GET /api/admin/pricing": pricing(), "POST /api/admin/pricing/models": pricing({ models: [...pricing().models, defaultPriced("acme-large-1")] }) });
    const m = mount(AdminPricing, {});
    await settle();
    pick(m.root.querySelector<HTMLInputElement>('input[name="pr-a-how"][value="card"]')!);
    expect(m.root.querySelector("#pr-a-provider")).toBeNull();
    type(m.root.querySelector<HTMLInputElement>("#pr-a-model")!, "acme-large-1");
    type(m.root.querySelector<HTMLInputElement>("#pr-a-name")!, "Acme Large 1");
    // A new card starts at cost plus the token margin.
    expect(m.root.querySelector<HTMLInputElement>("#pr-a-input-margin")!.value).toBe("40");
    for (const [u, usd] of [["input", "1"], ["cached_input", "0.1"], ["cache_write", "1.25"]]) type(m.root.querySelector<HTMLInputElement>(`#pr-a-${u}-usd`)!, usd);
    choose(m.root.querySelector<HTMLSelectElement>("#pr-a-output-how")!, "fixed");
    type(m.root.querySelector<HTMLInputElement>("#pr-a-output-credits")!, "700");
    click(button(m.root, "Add the model"));
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({
      model: "acme-large-1", name: "Acme Large 1",
      input: { cost_micro_usd: 1_000_000, margin_bp: 4000, fixed_credits_milli: null },
      cached_input: { cost_micro_usd: 100_000, margin_bp: 4000, fixed_credits_milli: null },
      cache_write: { cost_micro_usd: 1_250_000, margin_bp: 4000, fixed_credits_milli: null },
      output: { cost_micro_usd: 0, margin_bp: 4000, fixed_credits_milli: 700_000 },
    });
    expect(status(m.root, "pr-a")).toBe("Added acme-large-1 with the rates set.");
    // The card is blank again for the next model.
    expect(m.root.querySelector<HTMLInputElement>("#pr-a-input-usd")!.value).toBe("");
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
