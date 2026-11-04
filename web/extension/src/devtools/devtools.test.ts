// The DevTools panel: the settings link and view against the SDK's real
// hook (each expression evaluated in a page as `inspectedWindow.eval`
// would), the network log and view, and the host's tab list and notices zone.
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HOOK_KEY, type OverrideStore, type Value, registerDevSettings } from "../../../sdk/src/devtools";
import { changelingStyle, feedbackAxes } from "../../../sdk/src/devtools-fixtures";
import { INSTALL_KEY, type InstallInfo, integrationHook, isDevOrigin, resolve } from "../../../sdk/src/config";
import AppSettings from "./AppSettings.svelte";
import { CHANGELING_PANEL, IntegrationLink, groups, parseInfo } from "./integration.svelte";
import IntegrationView from "./IntegrationView.svelte";
import DevtoolsPanel from "./DevtoolsPanel.svelte";
import { type HarEntry, NetworkLog, RING, asText, elide, loadBody, matches, shownHeaders, toRow } from "./network.svelte";
import NetworkView from "./NetworkView.svelte";
import { type InspectEnv, POLL_MS, SettingsLink } from "./settings.svelte";

function memory(): OverrideStore {
  const data = new Map<string, Value>();
  return { get: k => data.get(k), set: (k, v) => { data.set(k, v); return true; }, delete: k => { data.delete(k); } };
}
/** A page at `href` the panel inspects: `eval` runs an expression with this page as `window`. */
function inspected(href = "http://127.0.0.1:7700/") {
  const u = new URL(href);
  const win = { location: { origin: u.origin, search: u.search }, document: { cookie: "" } } as unknown as Window & typeof globalThis;
  const nav: (() => void)[] = [];
  const evals: string[] = [];
  const env: InspectEnv = {
    eval: async expr => { evals.push(expr); return new Function("window", `return ${expr};`)(win); },
    onNavigated: f => { nav.push(f); return () => nav.splice(nav.indexOf(f), 1); },
  };
  return { win, env, evals, navigate: () => nav.forEach(f => f()) };
}
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("SettingsLink", () => {
  it("says a page without the hook has none, and reads one that registers later on navigation", async () => {
    const p = inspected();
    const link = new SettingsLink(p.env);
    await link.refresh();
    expect(link.status).toBe("none");
    registerDevSettings(changelingStyle, { win: p.win, store: memory() });
    p.navigate();
    await vi.waitFor(() => expect(link.status).toBe("ready"));
    expect(link.snapshot?.sections[0].title).toBe("Feedback");
  });

  it("reads the snapshot again only when the page's rev moved", async () => {
    const p = inspected();
    const h = registerDevSettings(feedbackAxes, { win: p.win, store: memory() })!;
    const link = new SettingsLink(p.env);
    await link.refresh();
    const snaps = () => p.evals.filter(e => e.includes("snapshot()")).length;
    expect(snaps()).toBe(1);
    await link.refresh();
    expect(snaps()).toBe(1);
    h.setBase("presentation", { value: "sheet", source: "site" });
    await link.refresh();
    expect(snaps()).toBe(2);
    expect(link.snapshot?.sections[0].fields.find(f => f.id === "presentation")).toMatchObject({ value: "sheet", source: "site" });
  });

  it("polls only while active", async () => {
    vi.useFakeTimers();
    const p = inspected();
    const link = new SettingsLink(p.env);
    link.active(true);
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    const n = p.evals.length;
    expect(n).toBeGreaterThanOrEqual(3);
    link.active(false);
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(p.evals.length).toBe(n);
  });

  it("sets and resets through the page's hook, and reads the result", async () => {
    const p = inspected();
    registerDevSettings(feedbackAxes, { win: p.win, store: memory() });
    const link = new SettingsLink(p.env);
    expect(await link.set("clax", "presentation", "window")).toMatchObject({ ok: true, field: { source: "you" } });
    expect(link.snapshot?.sections[0].fields.find(f => f.id === "presentation")?.value).toBe("window");
    expect(await link.set("clax", "attach-logs", true)).toMatchObject({ ok: false });
    expect(await link.reset("clax", "presentation")).toMatchObject({ ok: true, field: { value: "dialog", source: "clax" } });
  });

  it("treats a page whose hook answers something off as having none", async () => {
    const p = inspected();
    (p.win as unknown as Record<string, unknown>)[HOOK_KEY] = { v: 1, rev: 1, snapshot: () => ({ v: 1, rev: 1, origin: "x", sections: [{ id: "<img>", title: "x", fields: [] }] }) };
    const link = new SettingsLink(p.env);
    await link.refresh();
    expect(link.status).toBe("none");
    expect(await link.set("a", "b", true)).toMatchObject({ ok: false, reason: expect.stringMatching(/did not answer/) });
  });
});

/** The view over a live link to a page that registered `sections`. */
async function settingsView(...sections: Parameters<typeof registerDevSettings>[0][]) {
  const p = inspected();
  for (const s of sections) registerDevSettings(s, { win: p.win, store: memory() });
  const link = new SettingsLink(p.env);
  await link.refresh();
  const said: [string, boolean | undefined][] = [];
  render(AppSettings, { props: { link, say: (t: string, r?: boolean) => said.push([t, r]) } });
  return { p, link, said };
}

describe("AppSettings", () => {
  it("renders Changeling's feedback style as one radio group, unusable styles disabled with what they need", async () => {
    await settingsView(changelingStyle);
    const group = screen.getByRole("group", { name: "Feedback style" });
    const radios = within(group).getAllByRole("radio") as HTMLInputElement[];
    expect(radios).toHaveLength(10);
    expect(within(group).getByRole("radio", { name: "Changeling’s Send feedback (prod)" })).toHaveProperty("checked", true);
    const drawer = within(group).getByRole("radio", { name: "Side drawer" }) as HTMLInputElement;
    expect(drawer.disabled).toBe(true);
    expect(document.getElementById(drawer.getAttribute("aria-describedby")!)?.textContent).toContain('Needs from the Clax SDK: `presentation: "drawer"`');
    expect(screen.getByText(/From: Site default/)).toBeTruthy();
  });

  it("chooses an option, says so with its source, shows it as yours, and resets it by its full name", async () => {
    const { said } = await settingsView(changelingStyle);
    await fireEvent.click(screen.getByRole("radio", { name: "Floating launcher" }));
    await waitFor(() => expect(said.at(-1)?.[0]).toBe("Feedback style: Floating launcher (You)."));
    const reset = await screen.findByRole("button", { name: "Reset Feedback style to site default (Changeling’s Send feedback (prod))" });
    expect(screen.getByText(/From: You, over site default/)).toBeTruthy();
    await fireEvent.click(reset);
    await waitFor(() => expect(said.at(-1)?.[0]).toBe("Feedback style: Changeling’s Send feedback (prod) (Site default)."));
    expect(screen.queryByRole("button", { name: /^Reset/ })).toBeNull();
  });

  it("renders the separate axes: switches, a radio group, a text field, a locked field, and asks for a reload where it applies on reload", async () => {
    const { said } = await settingsView(feedbackAxes);
    const on = screen.getByRole("switch", { name: "Clax on" }) as HTMLInputElement;
    expect(on.checked).toBe(true);
    expect(screen.getByRole("switch", { name: "Attach logs by default" })).toHaveProperty("disabled", true);
    expect(screen.getByText(/Set by the Clax config; not changeable here/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("radio", { name: "Sheet" }));
    await waitFor(() => expect(said.at(-1)).toEqual(["Presentation: Sheet (You). Reload the page to apply.", true]));
    const note = screen.getByRole("textbox", { name: "Note placeholder" }) as HTMLInputElement;
    await fireEvent.input(note, { target: { value: "Tell us" } });
    await fireEvent.keyDown(note, { key: "Enter" });
    await waitFor(() => expect(said.at(-1)?.[0]).toBe("Note placeholder: “Tell us” (You)."));
    await fireEvent.click(on);
    await waitFor(() => expect(said.at(-1)?.[0]).toBe("Clax on: Off (You). Reload the page to apply."));
  });

  it("puts a control back and says why when the page refuses to keep the change", async () => {
    const p = inspected();
    const refusing: OverrideStore = { get: () => undefined, set: () => false, delete: () => {} };
    registerDevSettings(feedbackAxes, { win: p.win, store: refusing });
    const link = new SettingsLink(p.env);
    await link.refresh();
    const said: string[] = [];
    render(AppSettings, { props: { link, say: (t: string) => said.push(t) } });
    const sheet = screen.getByRole("radio", { name: "Sheet" }) as HTMLInputElement;
    await fireEvent.click(sheet);
    await waitFor(() => expect(said.at(-1)).toBe("Presentation not changed: This browser refused to keep it (cookies blocked or full)."));
    expect(sheet.checked).toBe(false);
    expect((screen.getByRole("radio", { name: "Dialog" }) as HTMLInputElement).checked).toBe(true);
    const on = screen.getByRole("switch", { name: "Clax on" }) as HTMLInputElement;
    await fireEvent.click(on);
    await waitFor(() => expect(said.at(-1)).toMatch(/^Clax on not changed/));
    expect(on.checked).toBe(true);
  });

  it("says when the page registers nothing", async () => {
    await settingsView();
    expect(screen.getByText("This page registers no app dev settings.")).toBeTruthy();
  });

  it("shows the page's strings as text, never markup", async () => {
    await settingsView({ id: "x", title: "<img src=x onerror=alert(1)>", fields: [{ id: "f", label: "<b>bold</b>", kind: "toggle", base: { value: true, source: "site" } }] });
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByRole("switch", { name: "<b>bold</b>" })).toBeTruthy();
  });
});

const entry = (over: Partial<HarEntry> & { url?: string; status?: number; type?: string } = {}): HarEntry => ({
  startedDateTime: "2026-11-02T10:00:00.000Z", time: 42.4,
  request: { method: "GET", url: over.url ?? "http://127.0.0.1:7700/api/x?token=abc&page=2#frag", headers: [{ name: "Authorization", value: "Bearer s3cret" }, { name: "Accept", value: "*/*" }] },
  response: { status: over.status ?? 200, headers: [{ name: "Set-Cookie", value: "sid=1" }, { name: "Content-Type", value: "application/json" }], _transferSize: 1536 },
  timings: { blocked: 1, dns: -1, connect: -1, ssl: -1, send: 0.2, wait: 30, receive: 2 },
  _resourceType: over.type ?? "fetch",
  ...over,
});

describe("the network log", () => {
  it("elides query values and the fragment, and hides credentials even when headers are shown", () => {
    expect(elide("http://a.test/p?token=abc&page=2#f")).toBe("http://a.test/p?token=…&page=…");
    expect(shownHeaders(entry().request.headers)).toEqual([{ name: "Authorization", value: "[hidden]" }, { name: "Accept", value: "*/*" }]);
    expect(shownHeaders(entry().response.headers)[0]).toEqual({ name: "Set-Cookie", value: "[hidden]" });
  });

  it("makes a row: failed for 4xx/5xx, status 0 and network errors; other site by origin; phases Chrome measured", () => {
    const r = toRow(entry(), 1, "http://127.0.0.1:7700", Date.parse("2026-11-02T09:59:59.900Z"));
    expect(r).toMatchObject({ method: "GET", url: "http://127.0.0.1:7700/api/x?token=…&page=…", status: 200, failed: false, kind: "fetch", size: 1536, ms: 42, at: 100, otherSite: false });
    expect(r.timings).toEqual([{ phase: "blocked", ms: 1 }, { phase: "send", ms: 0 }, { phase: "wait", ms: 30 }, { phase: "receive", ms: 2 }]);
    expect(toRow(entry({ status: 404 }), 2, null, 0).failed).toBe(true);
    expect(toRow(entry({ status: 0 }), 3, null, 0).failed).toBe(true);
    expect(toRow(entry({ url: "https://cdn.example/x.js" }), 4, "http://127.0.0.1:7700", 0).otherSite).toBe(true);
  });

  it("filters by kind, failure and site, and searches", () => {
    const rows = [entry(), entry({ status: 500 }), entry({ type: "document", url: "http://127.0.0.1:7700/" }), entry({ url: "https://cdn.example/a.js", type: "script" })]
      .map((e, i) => toRow(e, i, "http://127.0.0.1:7700", 0));
    expect(rows.filter(r => matches(r, "failed", "")).length).toBe(1);
    expect(rows.filter(r => matches(r, "fetch", "")).length).toBe(2);
    expect(rows.filter(r => matches(r, "document", "")).length).toBe(1);
    expect(rows.filter(r => matches(r, "other", "")).length).toBe(1);
    expect(rows.filter(r => matches(r, "all", "cdn")).length).toBe(1);
  });

  it("keeps the last RING requests, starts again on navigation unless kept", () => {
    const log = new NetworkLog();
    for (let i = 0; i < RING + 5; i++) log.add(entry());
    expect(log.rows).toHaveLength(RING);
    expect(log.rows[0].n).toBe(6);
    log.keep = true;
    log.navigated("http://127.0.0.1:7700");
    expect(log.rows).toHaveLength(RING);
    log.keep = false;
    log.navigated("http://127.0.0.1:7700");
    expect(log.rows).toHaveLength(0);
  });

  it("copies only what a row shows", () => {
    const text = asText([toRow(entry({ status: 404 }), 1, null, 0)]);
    expect(text).toBe("GET 404 http://127.0.0.1:7700/api/x?token=…&page=… fetch 1.5 kB 42 ms");
    expect(text).not.toMatch(/s3cret|sid=1|abc/);
  });

  it("loads a body only when asked: text cut at 64 KiB, binary not shown", async () => {
    expect(await loadBody({ ...entry(), getContent: f => f("x".repeat(70_000), "") })).toMatch(/^x{65536}\n… \(cut at 64\.0 kB\)$/);
    expect(await loadBody({ ...entry(), getContent: f => f("AAAA", "base64") })).toBe("Binary body (3 B), not shown.");
    expect(await loadBody(entry())).toBe("No body.");
  });
});

describe("NetworkView", () => {
  it("lists requests, shows a selected one's details in the reserved region, headers only when asked, and its body only on its click", async () => {
    const log = new NetworkLog();
    log.origin = "http://127.0.0.1:7700";
    const getContent = vi.fn((f: (c: string, e: string) => void) => f('{"ok":true}', ""));
    log.add({ ...entry({ status: 404 }), getContent });
    const said: string[] = [];
    render(NetworkView, { props: { log, say: (t: string) => said.push(t), copy: async () => true } });
    const details = screen.getByRole("region", { name: "Request details" });
    expect(within(details).getByText("Select a request to see its details.")).toBeTruthy();
    const row = screen.getByRole("button", { name: "http://127.0.0.1:7700/api/x?token=…&page=…" });
    await fireEvent.click(row);
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(within(details).getByText(/Status 404/)).toBeTruthy();
    expect(within(details).queryByText("Bearer s3cret")).toBeNull();
    await fireEvent.click(within(details).getByRole("button", { name: "Show request headers" }));
    expect(within(details).getByText("[hidden]")).toBeTruthy();
    expect(within(details).queryByText(/s3cret/)).toBeNull();
    expect(getContent).not.toHaveBeenCalled();
    await fireEvent.click(within(details).getByRole("button", { name: "Load body" }));
    expect(await within(details).findByText('{"ok":true}')).toBeTruthy();
    await fireEvent.click(row);
    expect(within(details).queryByText('{"ok":true}')).toBeNull();
    await fireEvent.click(screen.getByRole("button", { name: "Failed" }));
    expect(screen.getByRole("button", { name: "Failed" }).getAttribute("aria-pressed")).toBe("true");
    await fireEvent.click(screen.getByRole("button", { name: "Copy as text" }));
    await waitFor(() => expect(said.at(-1)).toBe("Copied 1 request as text (no headers, cookies or bodies)."));
  });
});

describe("DevtoolsPanel", () => {
  const panelLink = () => ({ state: null, post: () => {} });
  const noInstall = () => ({ status: "none" as const, info: null, set: vi.fn(), refresh: vi.fn(async () => {}) });
  /** No queues: the Queues view reads nothing until it is first opened. */
  const queues = () => ({ base: null, queues: vi.fn(async () => []), reports: vi.fn(), report: vi.fn(), clip: vi.fn(), reply: vi.fn(), resolve: vi.fn(), watch: () => () => {} });
  it("has five views in a tab list the arrows move through, a notices zone always there, and reads settings only while they are in sight", async () => {
    const settings = { status: "none" as const, snapshot: null, set: vi.fn(), reset: vi.fn(), active: vi.fn() };
    const q = queues();
    const { rerender } = render(DevtoolsPanel, { props: { link: panelLink(), queues: q, settings, integration: noInstall(), network: new NetworkLog(), reload: () => {} } });
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map(t => t.textContent)).toEqual(["Threads", "Queues", "Integration", "App settings", "Network"]);
    expect(screen.getByRole("status")).toBeTruthy();
    expect(screen.getByText("Connecting to Clax…")).toBeTruthy();
    expect(settings.active).toHaveBeenLastCalledWith(false);
    expect(q.queues).not.toHaveBeenCalled();
    await fireEvent.keyDown(tabs[0], { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Queues" }).getAttribute("aria-selected")).toBe("true");
    expect(await screen.findByText("You have no queues.", { exact: false })).toBeTruthy();
    await fireEvent.keyDown(screen.getByRole("tab", { name: "Queues" }), { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Integration" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText("Clax is not installed on this page.")).toBeTruthy();
    await fireEvent.keyDown(screen.getByRole("tab", { name: "Integration" }), { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "App settings" }).getAttribute("aria-selected")).toBe("true");
    expect(settings.active).toHaveBeenLastCalledWith(true);
    expect(screen.getByText("This page registers no app dev settings.")).toBeTruthy();
    await rerender({ shown: false });
    expect(settings.active).toHaveBeenLastCalledWith(false);
    await fireEvent.keyDown(screen.getByRole("tab", { name: "App settings" }), { key: "End" });
    expect(screen.getByRole("tab", { name: "Network" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText(/No requests yet/)).toBeTruthy();
  });

  it("offers a reload in the notices zone after a change that applies on reload", async () => {
    const p = inspected();
    registerDevSettings(feedbackAxes, { win: p.win, store: memory() });
    const settings = new SettingsLink(p.env);
    await settings.refresh();
    const reload = vi.fn();
    render(DevtoolsPanel, { props: { link: panelLink(), queues: queues(), settings, integration: noInstall(), network: new NetworkLog(), reload } });
    await fireEvent.click(screen.getByRole("tab", { name: "App settings" }));
    await fireEvent.click(screen.getByRole("radio", { name: "Drawer" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Presentation: Drawer (You). Reload the page to apply."));
    await fireEvent.click(screen.getByRole("button", { name: "Reload page" }));
    expect(reload).toHaveBeenCalledOnce();
    settings.close();
  });
});

describe("the Integration view", () => {
  /** A page whose install is `info` (the real hook over the real resolver), inspected as `inspectedWindow.eval` would. */
  function installedPage(layers: { site?: unknown; clax?: unknown }, origin = "http://127.0.0.1:7700", extra: Partial<InstallInfo> = {}) {
    const doc = { cookie: "" } as Document;
    const r = resolve(layers, origin);
    const mode = r.config["overrides.viewer"];
    const info = (): InstallInfo => ({
      v: 1, origin, instance: "https://clax.example", container: "q1", version: 0, error: null, carrier: "site", deployment: "render",
      script: "https://clax.example/_clax/overlay.js?v=0123456789ab", applies: mode === "always" || (mode === "local" && isDevOrigin(origin)),
      config: r.config, source: r.source, ignored: r.ignored, plugins: ["presentations"], built: ["floating", "dialog", "drawer", "sheet", "window", "extension"], ...extra,
    });
    const win = { [INSTALL_KEY]: integrationHook(info, doc) };
    const nav: (() => void)[] = [];
    const env: InspectEnv = {
      eval: async expr => new Function("window", `return ${expr};`)(win),
      onNavigated: f => { nav.push(f); return () => nav.splice(nav.indexOf(f), 1); },
    };
    return { doc, env };
  }

  it("reads the install with no host code, checks it whole, and says when there is none", async () => {
    const p = installedPage({ clax: { presentation: "dialog" } });
    const link = new IntegrationLink(p.env);
    await link.refresh();
    expect(link.status).toBe("ready");
    expect(link.info).toMatchObject({ container: "q1", deployment: "render", source: { presentation: "clax" } });
    expect(parseInfo('{"v":1}')).toBeNull();
    expect(parseInfo(JSON.stringify({ ...link.info, carrier: "elsewhere" }))).toBeNull();
    const none = new IntegrationLink({ eval: async () => null, onNavigated: () => () => {} });
    await none.refresh();
    expect(none.status).toBe("none");
  });

  it("maps every option of Changeling's on-page panel to a row, and shows what is not built as such", async () => {
    const p = installedPage({});
    const link = new IntegrationLink(p.env);
    await link.refresh();
    const rows = groups(link.info!).flatMap(g => g.rows);
    for (const o of CHANGELING_PANEL) expect(rows.map(r => r.field), o.option).toContain(o.field);
    expect(rows.find(r => r.field === "pins")?.notBuilt).toBeTruthy();
    const partial = groups({ ...link.info!, built: ["floating", "dialog"] }).flatMap(g => g.rows).find(r => r.field === "presentation")!;
    expect(partial.choices!.filter(c => !c.built).map(c => c.value)).toEqual(["drawer", "sheet", "window", "extension"]);
  });

  it("shows each field's value and source, sets the viewer's override in the page's cookie, and says to reload", async () => {
    const p = installedPage({ site: { start: "screenshot" } });
    const link = new IntegrationLink(p.env);
    await link.refresh();
    const said: [string, boolean | undefined][] = [];
    render(IntegrationView, { props: { link, say: (t: string, r?: boolean) => { said.push([t, r]); } } });
    expect(screen.getByText("render")).toBeTruthy();
    expect(screen.getByText("pinned, version 0123456789ab")).toBeTruthy();
    const start = screen.getByRole("group", { name: "Start mode" });
    expect((within(start).getByRole("radio", { name: "Screenshot then annotate" }) as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(screen.getByRole("radio", { name: "Side drawer" }));
    await waitFor(() => expect(said.at(-1)).toEqual(["Presentation: Side drawer (You). Reload the page to apply.", true]));
    expect(decodeURIComponent(p.doc.cookie)).toContain('"presentation":"drawer"');
    expect(screen.getByText(/You chose Side drawer: reload to apply/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reset Presentation" })).toBeTruthy();
    expect(screen.getAllByText("Not built").length).toBeGreaterThan(0);
  });

  it("is read-only off a local or tailnet origin", async () => {
    const p = installedPage({}, "https://changeling.io");
    const link = new IntegrationLink(p.env);
    await link.refresh();
    render(IntegrationView, { props: { link, say: () => {} } });
    expect(screen.getByText(/Read-only here/)).toBeTruthy();
    expect((screen.getByRole("radio", { name: "Dialog" }) as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByRole("switch", { name: "Floating launcher" }) as HTMLInputElement).disabled).toBe(true);
    expect((await link.set("presentation", "dialog")).ok).toBe(false);
  });
});
