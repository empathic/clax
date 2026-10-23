import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import AuditPage from "./AuditPage.svelte";
import { meBot, settings, week } from "./fixtures";
import HeaderBot from "./HeaderBot.svelte";
import { forget } from "./me";
import MePage from "./MePage.svelte";
import SettingsPage from "./SettingsPage.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const choose = (el: HTMLSelectElement, value: string) => flush(() => { el.value = value; el.dispatchEvent(new Event("change", { bubbles: true })); });
const button = (root: Element, text: string | RegExp) => [...root.querySelectorAll("button")].find(b => (typeof text === "string" ? b.textContent?.trim() === text : text.test(b.textContent ?? "")))!;

beforeEach(() => { forget(); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Settings", () => {
  it("shows every field with its source and pending value, read-only from the environment, and writes what changed to config.toml", async () => {
    const calls = stubFetch({
      "GET /api/settings": settings({ fields: settings().fields.map(f => (f.key === "serve.port" ? { ...f, note: "the daemon listens on 7480, not the configured 7481" } : f)) }),
      "PUT /api/settings": { ...settings(), written: ["bot.api_key_env", "bot.enabled"], restart_required: true },
      "GET /api/admin/provider/models": { models: [{ id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5" }, { id: "claude-opus-5-5", display_name: "Claude Opus 5.5" }], fetched_at: "x" },
      "POST /api/admin/provider/test": { ok: false, latency_ms: 0, error: "401 invalid x-api-key" },
    });
    const m = mount(SettingsPage, {});
    await settle();
    expect([...m.root.querySelectorAll(".bcard h3")].map(h => h.textContent)).toEqual(["[serve]", "[bot]"]);
    const port = m.root.querySelector<HTMLInputElement>("#set-serve-port")!;
    expect(port.readOnly).toBe(true);
    const portField = port.closest(".field")!.textContent!;
    expect(portField).toContain("env CLAX_SERVE__PORT");
    expect(portField).toContain("the daemon listens on 7480, not the configured 7481");
    // A value the next start takes is marked.
    const keyEnv = m.root.querySelector<HTMLInputElement>("#set-bot-api-key-env")!;
    expect(keyEnv.closest(".field")!.querySelector(".pend")!.textContent).toBe("after restart: CLAX_KEY");
    expect(keyEnv.closest(".field")!.textContent).toContain("Or set CLAX_BOT__API_KEY_ENV");
    // A model field is the provider's model selector, with Test connection.
    const model = m.root.querySelector<HTMLSelectElement>("select#set-bot-models-default")!;
    expect([...model.options].map(o => o.value)).toContain("claude-opus-5-5");
    click(button(m.root, "Test connection"));
    await settle();
    expect(m.root.querySelector('[role="status"] .err')!.textContent).toBe("The call failed: 401 invalid x-api-key");
    // Nothing changed yet: Save does nothing.
    click(button(m.root, "Save to config.toml"));
    expect(calls.some(c => c.method === "PUT")).toBe(false);
    type(keyEnv, "CLAX_KEY");
    click(m.root.querySelector('[role="switch"][aria-label="bot.enabled"]')!);
    click(button(m.root, "Save to config.toml"));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({ url: "/api/settings", method: "PUT", body: { set: { "bot.enabled": false, "bot.api_key_env": "CLAX_KEY" } } });
    expect(m.root.querySelector('[role="status"]')!.textContent).toBe("Saved 2 to config.toml. Restart Clax for them to take effect.");
  });

  it("says when Clax started without its config.toml, and which tables do not check", async () => {
    stubFetch({ "GET /api/settings": settings({ started_with_invalid_config: true, start_error: "config.toml line 3: expected a table", problems: [{ table: "sample", message: "max_tokens must be at least 16" }] }) });
    const m = mount(SettingsPage, {});
    await settle();
    const alerts = [...m.root.querySelectorAll('[role="alert"]')].map(a => a.textContent);
    expect(alerts[0]).toContain("Clax started without its config.toml: config.toml line 3: expected a table");
    expect(alerts[1]).toBe("[sample] is not used: max_tokens must be at least 16");
  });

  it("on a daemon without a settings screen, says where it is configured", async () => {
    stubFetch({});
    const m = mount(SettingsPage, {});
    await settle();
    expect(m.root.textContent).toContain("This Clax has no settings screen");
  });
});

describe("You", () => {
  it("shows the week, sets the person's own key write-only, and turns using it on", async () => {
    let me = meBot();
    const calls = stubFetch({
      "GET /api/me/bot": () => me,
      "GET /api/me/usage": { week: { start: "2026-10-19T00:00:00Z", end: "2026-10-26T00:00:00Z" }, totals: [{ line: "compute", label: "Script compute", unit: "cpu_gb_s", qty: 412, credits_milli: 310_000 }], runs: [{ id: "r1", artifact_id: "7q3k9mzx2b4t", artifact_title: "Grocery splitter", started_at: "2026-10-21T14:02:00Z", status: "finished", scripts: 3, cpu_gb_s: 8, key_source: "own", credits_milli: 6000, tokens: 38_200, lines: [] }] },
      "PUT /api/me/bot/key": () => { me = meBot({ key: { own: { set: true, last4: "4f2c", set_at: "2026-10-21T14:00:00Z", last_ok_at: null }, use_own: false } }); return me; },
      "PUT /api/me/bot": (b: { use_own: boolean }) => { me = { ...me, key: { ...me.key, use_own: b.use_own } }; return me; },
    });
    const m = mount(MePage, { now: () => new Date("2026-10-21T14:05:00Z") });
    await settle();
    expect(m.root.querySelector(".mew")!.textContent).toContain("310 of 2,000 credits");
    expect(m.root.querySelector(".mew")!.textContent).toContain("Resets Monday, Oct 26");
    const use = m.root.querySelector<HTMLElement>('[role="switch"][aria-label="Use my own Anthropic key"]')!;
    expect(use.getAttribute("aria-disabled")).toBe("true");
    const run = m.root.querySelector<HTMLAnchorElement>(".mer tbody a")!;
    expect(run.textContent).toBe("Grocery splitter");
    expect(run.getAttribute("href")).toBe("/a/7q3k9mzx2b4t#run=r1");
    expect(run.closest("tr")!.textContent).toContain("8 CPU·GB-s");
    click(button(m.root, /^Set/));
    const input = m.root.querySelector<HTMLInputElement>('input[type="password"]')!;
    type(input, "sk-ant-mine-4f2c");
    flush(() => input.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    await settle();
    expect(calls.find(c => c.url === "/api/me/bot/key")!.body).toEqual({ key: "sk-ant-mine-4f2c" });
    expect(m.root.querySelector(".bkey-s")!.textContent).toContain("ending 4f2c");
    click(m.root.querySelector('[role="switch"][aria-label="Use my own Anthropic key"]')!);
    await settle();
    expect(calls.find(c => c.url === "/api/me/bot" && c.method === "PUT")!.body).toEqual({ use_own: true });
    expect(m.root.querySelector('[role="switch"][aria-label="Use my own Anthropic key"]')!.getAttribute("aria-checked")).toBe("true");
  });

  it("shows the provider host a key is bound to before it is saved, and a key set for another", async () => {
    let me = meBot({ provider_host: "https://api.anthropic.com" });
    stubFetch({
      "GET /api/me/bot": () => me,
      "GET /api/me/usage": { week: { start: "2026-10-19T00:00:00Z", end: "2026-10-26T00:00:00Z" }, totals: [], runs: [] },
    });
    let m = mount(MePage, { now: () => new Date("2026-10-21T14:05:00Z") });
    await settle();
    expect(m.root.querySelector(".mek-host")!.textContent).toContain("https://api.anthropic.com");
    expect(m.root.querySelector(".mek-problem")).toBeNull();
    me = meBot({
      provider_host: "https://collector.example.com",
      key: { own: { set: true, last4: "4f2c", set_at: "2026-10-21T14:00:00Z", last_ok_at: null, host: "https://api.anthropic.com", problem: "your key: it was set for https://api.anthropic.com; set it again" }, use_own: true },
    });
    m = mount(MePage, { now: () => new Date("2026-10-21T14:05:00Z") });
    await settle();
    expect(m.root.querySelector(".mek-host")!.textContent).toContain("https://collector.example.com");
    expect(m.root.querySelector(".mek-problem")!.textContent).toBe("your key: it was set for https://api.anthropic.com; set it again");
  });
});

describe("Audit", () => {
  const EV = { seq: 48212, at: "2026-10-21T14:05:58Z", actor: { id: "agent:claxbot", name: "ClaxBot" }, on_behalf_of: { id: "p_sam", name: "Sam" }, kind: "bot.script", summary: "\"Fix rounding\" · 3 calls · 2 credits", artifact_id: "7q3k9mzx2b4t", run: "01M5GR7" };

  it("filters by the address's query, shows an event's stored body, and reads the next page by its cursor", async () => {
    history.replaceState(null, "", "/audit?artifact=7q3k9mzx2b4t&q=01M5GR7");
    const now = () => new Date("2026-10-22T15:00:00Z");
    const since = encodeURIComponent("2026-10-15T15:00:00.000Z");
    const calls = stubFetch({
      "GET /api/artifacts": { artifacts: [{ id: "7q3k9mzx2b4t", title: "Grocery splitter" }] },
      [`GET /api/audit?q=01M5GR7&artifact=7q3k9mzx2b4t&from=${since}`]: { events: [EV], next_cursor: "c2" },
      [`GET /api/audit?q=01M5GR7&artifact=7q3k9mzx2b4t&from=${since}&cursor=c2`]: { events: [{ ...EV, seq: 48211, kind: "file.read", summary: "index.html" }], next_cursor: null },
      [`GET /api/audit?q=01M5GR7&group=bot&artifact=7q3k9mzx2b4t&from=${since}`]: { events: [], next_cursor: null },
      "GET /api/audit/48212": { ...EV, body: { v: 1, via: "bot", run: "01M5GR7" } },
    });
    const m = mount(AuditPage, { now });
    await settle();
    const item = m.root.querySelector(".aud-l li button")!;
    expect(item.textContent).toContain("ClaxBot for Sam");
    expect(item.textContent).toContain("bot.script");
    expect(item.textContent).toContain("Grocery splitter");
    click(item);
    await settle();
    expect(m.root.querySelector('pre[aria-label="Stored body"]')!.textContent).toContain("\"via\": \"bot\"");
    // The detail takes the focus; Export Toolpath is not offered to one the route refuses.
    expect(document.activeElement).toBe(m.root.querySelector(".aud-d"));
    expect(button(m.root, "Export Toolpath")).toBeUndefined();
    expect(m.root.querySelector<HTMLAnchorElement>(".aud-d a")!.getAttribute("href")).toBe("/a/7q3k9mzx2b4t#run=01M5GR7");
    click(button(m.root, "More"));
    await settle();
    expect(m.root.querySelectorAll(".aud-l li")).toHaveLength(2);
    choose(m.root.querySelector<HTMLSelectElement>('select[aria-label="Kind"]')!, "bot");
    await settle();
    expect(m.root.textContent).toContain("Nothing matches.");
    expect(location.search).toBe("?q=01M5GR7&group=bot&artifact=7q3k9mzx2b4t");
    expect(calls.filter(c => c.url.startsWith("/api/audit?")).length).toBe(3);
  });
});

describe("Audit's export", () => {
  it("is offered to whoever the export route admits", async () => {
    history.replaceState(null, "", "/audit");
    stubFetch({ "GET /api/me/bot": meBot({ role: "owner", can_export: true }) });
    const m = mount(AuditPage, {});
    await settle();
    expect(button(m.root, "Export Toolpath")).toBeDefined();
  });
});

describe("the gallery header's ClaxBot", () => {
  it("shows the week's meter and Admin for an admin, and the banner at the limit", async () => {
    const main = document.createElement("main");
    main.className = "gal";
    document.body.append(main);
    stubFetch({ "GET /api/me/bot": meBot({ role: "admin", week: week({ spent_milli: 2_000_000 }) }) });
    const m = mount(HeaderBot, {});
    await settle();
    const meter = m.root.querySelector<HTMLAnchorElement>(".bot-meter")!;
    expect(meter.getAttribute("aria-label")).toBe("ClaxBot: 2,000 of 2,000 credits used this week");
    expect(meter.classList.contains("out")).toBe(true);
    expect(m.root.querySelector(".admin-link")!.getAttribute("href")).toBe("/admin");
    const banner = main.querySelector('.bbanner[role="alert"]')!;
    expect(banner.textContent).toContain("This week's credits are used up.");
    click(button(banner, "Ask an admin for a lift"));
    expect(banner.querySelector("#bbanner-ask")!.textContent).toContain("Admin › People & flags");
    m.unmount();
    expect(main.querySelector(".bbanner")).toBeNull();
  });

  it("shows nothing where the daemon has no ClaxBot", async () => {
    stubFetch({});
    const m = mount(HeaderBot, {});
    await settle();
    expect(m.root.querySelector("a")).toBeNull();
  });
});
