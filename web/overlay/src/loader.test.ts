// The loader installs only what the resolved config names, and wires the
// targets the config turns on (Addenda 2 and 3).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { definePlugin, PLUGIN_API_VERSION, type ClaxPlugin } from "../../sdk/src/plugin";
import { INSTALL_KEY, overrideCookie } from "../../sdk/src/config";
import { authOf, CARRIER_ATTR, claim, containerUrl, install, installQueued, snippetFromTag, type PluginName, pluginsFor, type Registry, type Snippet } from "./loader";
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
  subscribe: () => () => {},
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
beforeEach(() => { loaded = []; mounted = []; calls = []; installed = null; document.body.innerHTML = ""; document.documentElement.removeAttribute(CARRIER_ATTR); document.cookie = `${overrideCookie(location.port)}=; Max-Age=0; Path=/`; });
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
    expect(document.cookie).toMatch(new RegExp(`${overrideCookie(location.port)}=`));
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
    await authOf("/clax/reporter-token", win)("PUBKEY", { traceparent: "00-a-b-01" });
    expect(seen[1][1]?.headers).toEqual({ "content-type": "text/plain", traceparent: "00-a-b-01" });
  });

  it("installs what a tag manager queued before the script ran", async () => {
    (window as Window & { clax?: unknown }).clax = [["install", snippet()]];
    installed = await installQueued(window);
    expect(mounted).toHaveLength(1);
    expect(installQueued({} as Window)).toBeNull();
  });
});

describe("one install, two carriers", () => {
  const tag = (attrs: Record<string, string>) => {
    const t = document.createElement("script");
    for (const [k, v] of Object.entries(attrs)) t.setAttribute(k, v);
    return t;
  };
  it("a tag names its instance by where the script came from, and its container and token route", () => {
    expect(snippetFromTag(tag({ src: "https://clax.example/_clax/overlay.js?v=0123", "data-container": "q1", "data-auth": "/clax/token", "data-deployment": "render" }), "https://site.example/app/")).toEqual({
      instance: "https://clax.example", container: "q1", auth: "https://site.example/clax/token", deployment: "render", script: "https://clax.example/_clax/overlay.js?v=0123",
    });
    expect(snippetFromTag(tag({ src: "https://example.com/clax/_clax/overlay.js", "data-container": "q1", "data-queue": "q2" }), "https://site.example/")).toEqual({
      instance: "https://example.com/clax", container: "q1", queue: "q2", script: "https://example.com/clax/_clax/overlay.js",
    });
    expect(snippetFromTag(tag({ src: "https://clax.example/_clax/overlay.js" }), "https://site.example/")).toBeNull();
    expect(snippetFromTag(null, "https://site.example/")).toBeNull();
  });
  it("the extension installs only where the site has not; the site's install wins and the extension's goes", async () => {
    const ext = await install(snippet({ carrier: "extension" }));
    expect(document.documentElement.getAttribute(CARRIER_ATTR)).toBe("extension");
    await expect(install(snippet({ carrier: "extension" }))).rejects.toThrow(/already has Clax/);
    installed = await install(snippet());
    expect(calls).toContain("close");
    expect(document.documentElement.getAttribute(CARRIER_ATTR)).toBe("site");
    expect(claim(document, "extension")).toBe(false);
    ext.uninstall();
    expect(document.documentElement.getAttribute(CARRIER_ATTR)).toBe("site");
    installed.uninstall();
    installed = null;
    expect(document.documentElement.hasAttribute(CARRIER_ATTR)).toBe(false);
  });
});

describe("the DevTools panel's integration hook", () => {
  type Hook = { v: 1; info(): string; set(f: string, v: unknown): { ok: boolean; reason?: string } };
  const hook = () => (window as unknown as Record<string, Hook | undefined>)[INSTALL_KEY];
  it("says which container, version, carrier and deployment put Clax here, and how each field resolved, with no host code", async () => {
    installed = await install(snippet({
      deployment: "edge-caddy", script: "https://clax.example/_clax/overlay.js?v=0123456789ab",
      site: { start: "screenshot" },
      fetchContainer: async () => ({ id: "q1", version: 3, config: { presentation: "drawer" } }),
      registry: registry({ presentations: async () => fake("presentations", { presentations: [{ id: "floating", label: "F", layout: "dock" }, { id: "drawer", label: "D", layout: "drawer" }] }) }),
    }));
    const info = JSON.parse(hook()!.info());
    expect(info).toMatchObject({
      v: 1, instance: "https://clax.example", container: "q1", version: 3, carrier: "site", deployment: "edge-caddy",
      script: "https://clax.example/_clax/overlay.js?v=0123456789ab", applies: true, error: null,
    });
    expect(info.config).toMatchObject({ presentation: "drawer", start: "screenshot" });
    expect(info.source).toMatchObject({ presentation: "clax", start: "site", enabled: "default" });
    expect(info.built).toEqual(expect.arrayContaining(["floating", "drawer"]));
    expect(info.built).not.toContain("pins");
    expect(Object.keys(window)).not.toContain(INSTALL_KEY);
    installed.uninstall();
    installed = null;
    expect(hook()).toBeUndefined();
  });
  it("keeps the viewer's override in the page's cookie, checked, for the next load; refuses what it may not take", async () => {
    installed = await install(snippet());
    expect(hook()!.set("presentation", "sheet")).toEqual({ ok: true });
    expect(hook()!.set("presentation", "sideways")).toMatchObject({ ok: false, reason: expect.stringContaining("Not a value it takes") });
    expect(hook()!.set("overrides.viewer", "always")).toMatchObject({ ok: false });
    expect(hook()!.set("no.such", true)).toMatchObject({ ok: false });
    expect(hook()!.set("enabled", false)).toEqual({ ok: true });
    expect(hook()!.set("enabled", null)).toEqual({ ok: true });
    installed.uninstall();
    installed = await install(snippet());
    expect(installed.source.presentation).toBe("you");
    expect(installed.config.presentation).toBe("sheet");
    expect(installed.source.enabled).toBe("default");
  });
  it("is read-only where viewer overrides do not apply", async () => {
    installed = await install(snippet({ fetchContainer: async () => ({ id: "q1", version: 1, config: { overrides: { viewer: "never" } } }) }));
    expect(JSON.parse(hook()!.info()).applies).toBe(false);
    expect(hook()!.set("presentation", "sheet")).toEqual({ ok: false, reason: "Viewer overrides are off for this site." });
  });
});

describe("the host's data from page-level hooks the container names", () => {
  afterEach(() => { delete (window as unknown as Record<string, unknown>).app; });
  it("reads logs, the app's version and the signed-in person from the globals the container names, with no host code", async () => {
    class App { lines = [{ at: "2026-11-02T10:00:00Z", level: "error", text: "boom" }]; feedbackLogs() { return this.lines; } }
    (window as unknown as Record<string, unknown>).app = Object.assign(new App(), { release: { version: "0.6.1", commit: "abc" }, user: { name: () => "Ana" } });
    const seen: Record<string, unknown> = {};
    installed = await install(snippet({
      fetchContainer: async () => ({ id: "q1", version: 2, config: { hooks: { logs: "app.feedbackLogs", release: "app.release", user: "app.user.name" } } }),
      registry: registry({
        logs: async s => { seen.logs = s.logs?.(); return fake("logs"); },
        "app-version": async s => { seen.release = typeof s.release === "function" ? s.release() : s.release; return fake("app-version"); },
      }),
    }));
    expect(installed.plugins).toEqual(expect.arrayContaining(["logs", "app-version"]));
    expect(seen.logs).toEqual([{ at: "2026-11-02T10:00:00Z", level: "error", text: "boom" }]);
    expect(seen.release).toEqual({ version: "0.6.1", commit: "abc" });
    expect(mounted[0].opts.user).toBe("Ana");
  });
  it("names nothing it can't read: a missing global, a malformed path, the prototype chain", async () => {
    installed = await install(snippet({ fetchContainer: async () => ({ id: "q1", version: 2, config: { hooks: { logs: "app.nope", release: "constructor.prototype", user: "a..b" } } }) }));
    expect(installed.plugins).toContain("logs");
    expect(installed.plugins).not.toContain("app-version");
    expect(mounted[0].opts.user).toBeUndefined();
  });
});

describe("hooks are the site's and its container's only", () => {
  afterEach(() => { delete (window as unknown as Record<string, unknown>).app; history.replaceState(null, "", "/"); });
  it("a link or a viewer naming a hook is ignored, and the page code it names is never called", async () => {
    let called = 0;
    (window as unknown as Record<string, unknown>).app = { wipe: () => { called++; return "x"; } };
    history.replaceState(null, "", "/?clax.hooks.user=app.wipe&clax.hooks.logs=app.wipe");
    installed = await install(snippet());
    expect(called).toBe(0);
    expect(installed.ignored).toEqual(expect.arrayContaining([expect.objectContaining({ layer: "you", field: "hooks.user", reason: "a viewer cannot override it" })]));
    expect(mounted[0].opts.user).toBeUndefined();
    expect(document.cookie).not.toMatch(/hooks/);
    const hook = (window as unknown as Record<string, { set(f: string, v: unknown): { ok: boolean } }>)[INSTALL_KEY];
    expect(hook.set("hooks.user", "app.wipe").ok).toBe(false);
  });
});
