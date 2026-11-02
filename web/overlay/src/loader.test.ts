// The loader installs only what the resolved config names, and wires the
// targets the config turns on (Addenda 2 and 3).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { definePlugin, PLUGIN_API_VERSION, type ClaxPlugin } from "../../sdk/src/plugin";
import { authOf, containerUrl, install, installQueued, type PluginName, pluginsFor, type Registry, type Snippet } from "./loader";
import type { OverlayHandle } from "./mount";
import { resolve } from "../../sdk/src/config";

let loaded: PluginName[];
let mounted: { opts: Record<string, unknown>; plugins: string[] }[];
let calls: string[];

const fake = (name: string, extra: Partial<ClaxPlugin["contributes"]> = {}) => definePlugin({ name, version: "1", apiVersion: PLUGIN_API_VERSION, capabilities: [], contributes: extra });
const registry = (over: Partial<Registry> = {}): Registry => new Proxy({} as Registry, {
  get: (_, k: string) => over[k as PluginName] ?? (async () => { loaded.push(k as PluginName); return fake(k); }),
});
const handle = (): OverlayHandle => ({
  report: o => { calls.push(`report:${o?.mode ?? ""}`); },
  showReports: () => { calls.push("reports"); },
  actions: () => [{ id: "clax.report", label: "Send feedback", run: () => {} }],
  on: () => () => {},
  close: () => { calls.push("close"); },
  forget: async () => {},
});
const snippet = (over: Partial<Snippet> = {}): Snippet => ({
  instance: "https://clax.example", container: "q1", auth: async () => ({ token: "t", expires_at: "" }),
  fetchContainer: async () => null, registry: registry(),
  mount: (opts, plugins) => { mounted.push({ opts: opts as unknown as Record<string, unknown>, plugins: plugins.map(p => p.name) }); return handle(); },
  ...over,
});

let installed: Awaited<ReturnType<typeof install>> | null;
beforeEach(() => { loaded = []; mounted = []; calls = []; installed = null; document.body.innerHTML = ""; document.cookie = "clax_override=; Max-Age=0; Path=/"; });
afterEach(() => { installed?.uninstall(); localStorage.clear(); history.replaceState(null, "", "/"); });

describe("pluginsFor", () => {
  const base = resolve({}, "https://site.example").config;
  it("names capture, annotate and code-blur only with screenshots on", () => {
    expect(pluginsFor(base, {})).toEqual(["capture-dom", "capture-display", "annotate", "code-blur", "picker-element", "picker-area", "picker-page", "browser", "presentations"]);
    const off = resolve({ site: { attachments: { screenshot: false } } }, "https://site.example").config;
    expect(pluginsFor(off, {}).filter(n => /capture|annotate|blur/.test(n))).toEqual([]);
  });
  it("names the site's sources' plugins only when the site supplies them, and nothing with Clax off", () => {
    const c = resolve({ site: { pickers: { "code-line": true }, presentation: "window" } }, "https://site.example").config;
    expect(pluginsFor(c, { logs: () => [], release: { version: "1" } })).toEqual(expect.arrayContaining(["picker-code-line", "logs", "app-version", "presentation-window"]));
    expect(pluginsFor(resolve({ clax: { enabled: false } }, "https://site.example").config, {})).toEqual([]);
  });
});

describe("install", () => {
  it("lays the container over the site's defaults, reports each field's source, and loads and mounts only what they name", async () => {
    let asked = "";
    installed = await install(snippet({
      site: { presentation: "drawer", attachments: { logs: true } },
      fetchContainer: async url => { asked = url; return { id: "q1", version: 4, config: { presentation: "dialog", attachments: { screenshot: false }, theme: { "--clax-bg": "#101010" } } }; },
    }));
    expect(asked).toBe(containerUrl("https://clax.example", "q1"));
    expect(asked).toBe("https://clax.example/_clax/containers/q1/config.json");
    expect(installed.container).toEqual({ id: "q1", version: 4 });
    expect(installed.source).toMatchObject({ presentation: "clax", "attachments.logs": "site", "attachments.screenshot": "clax", start: "default" });
    expect(loaded).not.toContain("capture-dom");
    expect(loaded).not.toContain("annotate");
    expect(mounted).toHaveLength(1);
    expect(mounted[0].plugins).toEqual(loaded);
    expect(mounted[0].opts).toMatchObject({ queue: "q1", presentation: "dialog", theme: { "--clax-bg": "#101010" }, launcher: "corner", defaults: { logs: true } });
  });

  it("keeps the last container this browser read when Clax cannot be reached, and says so", async () => {
    installed = await install(snippet({ fetchContainer: async () => ({ id: "q1", version: 2, config: { presentation: "sheet" } }) }));
    installed.uninstall();
    installed = await install(snippet({ fetchContainer: async () => { throw new Error("offline"); } }));
    expect(installed.config.presentation).toBe("sheet");
    expect(installed.error).toMatch(/offline.*version 2 kept in this browser/);
  });

  it("with Clax off, loads and mounts nothing and wires no target", async () => {
    document.body.innerHTML = `<button data-clax="report">Feedback</button>`;
    installed = await install(snippet({ site: { enabled: false } }));
    expect(loaded).toEqual([]);
    expect(mounted).toEqual([]);
    document.querySelector("button")!.click();
    expect(calls).toEqual([]);
  });

  it("any element can be a target: data-clax, or a selector from the config", async () => {
    document.body.innerHTML = `<header><button data-clax="report"><span id=inner>Feedback</span></button><a id=mine href="#">Mine</a><button class=help>?</button></header>`;
    installed = await install(snippet({ site: { start: "note", targets: [{ selector: ".help", action: "reports" }] } }));
    document.getElementById("inner")!.click();
    document.querySelector<HTMLElement>(".help")!.click();
    expect(calls).toEqual(["report:note", "reports"]);
  });

  it("the key starts a report, never in a text field or with a modifier", async () => {
    document.body.innerHTML = `<input id=f>`;
    installed = await install(snippet({ site: { entry: { key: "f" } } }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "f" }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "f", metaKey: true }));
    document.getElementById("f")!.dispatchEvent(new KeyboardEvent("keydown", { key: "f", bubbles: true, composed: true }));
    expect(calls).toEqual(["report:element"]);
  });

  it("drains commands pushed before it ran and takes later ones; ?clax=reports opens Your reports", async () => {
    history.replaceState(null, "", "/?clax=reports");
    (window as Window & { clax?: unknown }).clax = [["report", { mode: "screen" }], ["install", {}]];
    installed = await install(snippet());
    (window as unknown as { clax: { push(c: unknown): void } }).clax.push(["close"]);
    expect(calls).toEqual(["reports", "report:screen", "close"]);
  });

  it("a ?clax.<field>= link overrides on a dev origin and is kept in the viewer's cookie", async () => {
    history.replaceState(null, "", "/?clax.presentation=sheet");
    installed = await install(snippet());
    expect(installed.config.presentation).toBe("sheet");
    expect(installed.source.presentation).toBe("you");
    expect(document.cookie).toMatch(/clax_override=/);
    installed.uninstall();
    history.replaceState(null, "", "/");
    installed = await install(snippet());
    expect(installed.source.presentation).toBe("you");
  });

  it("presentation extension, with the extension there: no composer is loaded; reports are handed to it", async () => {
    const handed: unknown[] = [];
    const ext = async () => fake("presentation-extension", { presentations: [{ id: "extension", label: "x", layout: "dialog", available: () => true, handoff: req => { handed.push(req.mode); return true; } }] });
    document.body.innerHTML = `<button data-clax="report">F</button>`;
    installed = await install(snippet({ site: { presentation: "extension" }, registry: registry({ "presentation-extension": ext }) }));
    expect(mounted).toEqual([]);
    expect(loaded).toEqual([]);
    expect(installed.plugins).toEqual(["presentation-extension"]);
    document.querySelector("button")!.click();
    expect(handed).toEqual(["element"]);
  });

  it("presentation extension, without it: a dialog", async () => {
    const ext = async () => fake("presentation-extension", { presentations: [{ id: "extension", label: "x", layout: "dialog", available: () => false }] });
    installed = await install(snippet({ site: { presentation: "extension" }, registry: registry({ "presentation-extension": ext }) }));
    expect(mounted[0].opts.presentation).toBe("dialog");
  });

  it("offers palette actions only with entry.palette on", async () => {
    installed = await install(snippet());
    expect(installed.actions()).toEqual([]);
    installed.uninstall();
    installed = await install(snippet({ site: { entry: { palette: true } } }));
    expect(installed.actions().map(a => a.label)).toEqual(["Send feedback"]);
  });

  it("refuses a snippet without an instance, a container or auth", async () => {
    await expect(install({ ...snippet(), container: "" })).rejects.toThrow(/container/);
    await expect(install({ ...snippet(), auth: 3 as never })).rejects.toThrow(/auth/);
  });
});

describe("authOf and the queued install", () => {
  it("a URL auth POSTs the public key to the site's token route", async () => {
    const seen: [string, RequestInit | undefined][] = [];
    const win = { fetch: async (u: string, i?: RequestInit) => { seen.push([u, i]); return { ok: true, json: async () => ({ token: "t", expires_at: "x" }) }; } } as unknown as Window;
    expect(await authOf("/clax/reporter-token", win)("PUBKEY")).toEqual({ token: "t", expires_at: "x" });
    expect(seen[0][0]).toBe("/clax/reporter-token");
    expect(seen[0][1]).toMatchObject({ method: "POST", body: "PUBKEY", credentials: "same-origin" });
  });

  it("installs what a tag manager queued before the script ran", async () => {
    (window as Window & { clax?: unknown }).clax = [["install", snippet()]];
    installed = await installQueued(window);
    expect(mounted).toHaveLength(1);
    expect(installQueued({} as Window)).toBeNull();
  });
});
