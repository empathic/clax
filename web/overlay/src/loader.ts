// The loader: Clax installed the way a tag manager installs a container.
// One snippet (or one npm call) names the instance and the container (the
// site in Clax); the loader fetches the container's published config from
// Clax, lays the site's defaults under it and the viewer's override over it
// (sdk/src/config.ts `resolve`), and then installs only what the resolved
// config names: the plugins it needs, the presentation, the theme, and the
// targets that start Clax (elements marked `data-clax`, selectors, a key, a
// `?clax=` link, `clax.push([...])` commands). A site with screenshots off
// never loads capture; a site whose presentation is the Clax extension,
// with the extension there, loads no composer at all.
//
// One code path for every install: npm (`install({...})`), the script tag
// and a tag manager (`Clax.install({...})`, or `window.clax = [["install",
// {...}]]` before the script).
import type { ClaxConfig, ConfigLayer, Container, FieldName, Source } from "../../sdk/src/config";
import { INSTALL_KEY, integrationHook, isDevOrigin, linkOverride, pageHook, parseContainer, readOverride, resolve, writeOverride } from "../../sdk/src/config";
import type { ClaxPlugin, HandoffRequest } from "../../sdk/src/plugin";
import type { ReporterAuth } from "../../sdk/src/reporter";
import type { Release } from "../../sdk/src/types";
import type { Mode } from "./options";
import type { OverlayHandle } from "./mount";
import type { Given, HostContext } from "./options";
import type { LogLine } from "./plugins/attachments";
import type { CodeLocation } from "./plugins/pickers";

/** What a site's snippet gives the loader. Everything else is config. */
export interface Snippet {
  /** The Clax instance, such as `https://clax.example.com`. */
  instance: string;
  /** The container: the site in Clax, whose config Clax serves and versions. Its queue too, unless `queue` says otherwise. */
  container: string;
  queue?: string;
  /** Gets a reporter token for the reporter's key, from the site's backend:
   * a function, or the URL of the site's token route (a tag manager can
   * carry no function), which the reporter's public key is POSTed to. */
  auth: ReporterAuth | string;
  /** The site's defaults, under the container's config. */
  site?: ConfigLayer;
  /** The site's own sources, which the config's plugins read from. */
  logs?: () => readonly LogLine[];
  release?: Release | (() => Release);
  locateCode?: (target: Element | Range | null, doc: Document) => CodeLocation | null;
  extension?: { detect?: (win: Window) => boolean; handoff?: (req: HandoffRequest, win: Window) => boolean };
  /** Passed to the overlay as they are. */
  user?: string;
  context?: Given<HostContext>;
  metadata?: Given<unknown>;
  externalId?: string;
  nonce?: string;
  doc?: Document;
  /** Who installs: the site (default) or the Clax extension, which yields to the site ([`claim`]). */
  carrier?: Carrier;
  /** Which of the integration's deployments put it here (the tag's `data-deployment`), and the tag's script address: shown in DevTools. */
  deployment?: string;
  script?: string;
  /** Fetches the container (tests replace it): default a GET of `containerUrl`. */
  fetchContainer?: (url: string) => Promise<unknown>;
  /** The plugins by name (tests replace it). */
  registry?: Registry;
  /** Mounts the overlay (tests replace it). */
  mount?: (opts: Parameters<typeof import("./mount").mountOverlay>[0], plugins: ClaxPlugin[], doc: Document) => OverlayHandle;
}

/** The plugins the loader can install, by name. */
export type PluginName =
  | "capture-dom" | "capture-display" | "annotate" | "code-blur" | "logs" | "app-version" | "browser"
  | "picker-element" | "picker-area" | "picker-page" | "picker-code-line"
  | "presentations" | "presentation-window" | "presentation-extension";

/** Loads a plugin, configured from the snippet and the resolved config. */
export type Registry = Record<PluginName, (s: Snippet, c: ClaxConfig) => Promise<ClaxPlugin>>;

/** Clax's first-party plugins, each its own import: only those named are fetched. */
export const REGISTRY: Registry = {
  "capture-dom": () => import("./plugins/capture").then(m => m.domCapture()),
  "capture-display": () => import("./plugins/capture").then(m => m.displayCapture()),
  annotate: () => import("./plugins/annotate").then(m => m.annotate()),
  "code-blur": (_s, c) => import("./plugins/code-blur").then(m => m.codeBlur({ selector: c["filters.code-selector"] })),
  logs: s => import("./plugins/attachments").then(m => m.logs({ source: s.logs! })),
  "app-version": s => import("./plugins/attachments").then(m => m.appVersion({ release: s.release! })),
  browser: () => import("./plugins/attachments").then(m => m.browser()),
  "picker-element": () => import("./plugins/pickers").then(m => m.elementPicker()),
  "picker-area": () => import("./plugins/pickers").then(m => m.areaPicker()),
  "picker-page": () => import("./plugins/pickers").then(m => m.pagePicker()),
  "picker-code-line": s => import("./plugins/pickers").then(m => m.codeLine({ locate: s.locateCode })),
  presentations: () => import("./plugins/presentations").then(m => m.presentations()),
  "presentation-window": () => import("./plugins/presentations").then(m => m.windowPresentation()),
  "presentation-extension": s => import("./plugins/presentations").then(m => m.extensionPresentation(s.extension ?? {})),
};

/** The plugins the resolved config `c` needs, given what the site supplies. */
export function pluginsFor(c: ClaxConfig, s: Pick<Snippet, "logs" | "release">): PluginName[] {
  if (!c.enabled) return [];
  const out: PluginName[] = [];
  if (c["attachments.screenshot"]) {
    out.push("capture-dom", "capture-display");
    if (c.annotate) out.push("annotate");
    if (c["filters.code-blur"]) out.push("code-blur");
  }
  out.push("picker-element", "picker-area", "picker-page");
  if (c["pickers.code-line"]) out.push("picker-code-line");
  if (s.logs) out.push("logs");
  if (s.release) out.push("app-version");
  out.push("browser", "presentations");
  if (c.presentation === "window") out.push("presentation-window");
  return out;
}

/** Where Clax serves a container's published config. */
export const containerUrl = (instance: string, id: string) => `${instance.replace(/\/+$/, "")}/_clax/containers/${encodeURIComponent(id)}/config.json`;

const CACHE_PREFIX = "feedback.container:";

/** The container from Clax, else the last one this browser kept, else none. */
async function fetchContainer(s: Snippet, win: Window): Promise<{ container: Container | null; error: string | null }> {
  const url = containerUrl(s.instance, s.container);
  const key = `${CACHE_PREFIX}${url}`;
  const get = s.fetchContainer ?? (async (u: string) => {
    const res = await win.fetch(u, { credentials: "omit", headers: { accept: "application/json" } });
    if (res.status === 404) return null;
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json();
  });
  try {
    const raw = await get(url);
    if (raw === null || raw === undefined) return { container: null, error: null };
    const container = parseContainer(raw);
    try { win.localStorage?.setItem(key, JSON.stringify(container)); } catch { /* storage refused */ }
    return { container, error: null };
  } catch (e) {
    let kept: Container | null = null;
    try { const v = win.localStorage?.getItem(key); kept = v ? parseContainer(JSON.parse(v)) : null; } catch { kept = null; }
    return { container: kept, error: `the container could not be read from Clax (${(e as Error)?.message ?? String(e)})${kept ? `; version ${kept.version} kept in this browser is used` : ""}` };
  }
}

/** A command a site (or its tag manager) pushes: `clax.push(["report", {mode}])`. */
export type Command = ["report" | "open", { mode?: Mode | "note" }?] | ["reports"] | ["close"];

/** An installed container. */
export interface Installed {
  config: ClaxConfig;
  /** Where each field's value came from: the container ("clax"), the site, the viewer ("you"), or Clax's default. */
  source: Record<FieldName, Source>;
  ignored: { layer: Source; field: string; reason: string }[];
  container: { id: string; version: number } | null;
  /** Why the container could not be read, when it could not. */
  error: string | null;
  /** The plugins installed. */
  plugins: PluginName[];
  /** The overlay, when one was mounted (none with Clax off, or handed to the extension). */
  handle: OverlayHandle | null;
  /** Runs a command, as `clax.push` does. */
  push(cmd: Command): void;
  /** What the site's command palette may offer (with `entry.palette` on). */
  actions(): { id: string; label: string; run(): void }[];
  /** Sets the viewer's override (null clears it); it applies at the next install (a reload). */
  override(layer: ConfigLayer | null): void;
  /** Removes the overlay and every target's listener. */
  uninstall(): void;
}

const START: Record<ClaxConfig["start"], Mode | "note"> = { note: "note", screenshot: "screen", element: "element" };

/** Installs the container `s` names: reads its config, loads what it names, mounts and wires the targets. */
export async function install(s: Snippet): Promise<Installed> {
  for (const k of ["instance", "container"] as const) if (typeof s?.[k] !== "string" || !s[k]) throw new TypeError(`Clax.install: ${k} must be a non-empty string`);
  if (typeof s.auth !== "function" && (typeof s.auth !== "string" || !s.auth)) throw new TypeError("Clax.install: auth must be a function or the URL of the site's token route");
  const doc = s.doc ?? document;
  const win = doc.defaultView ?? window;
  const origin = win.location?.origin ?? "";
  const carrier = s.carrier ?? "site";
  if (!claim(doc, carrier)) throw new Error("Clax.install: this page already has Clax; the site's install wins");

  const { container, error } = await fetchContainer(s, win);
  const link = linkOverride(win.location?.search ?? "");
  const kept = readOverride(doc.cookie ?? "", win.location?.port);
  const you = link || kept ? { ...kept, ...link } : undefined;
  const r = resolve({ site: s.site, clax: container?.config, you }, origin);
  const config = r.config;
  // The host's data from what the page already exposes, by the globals the config names, where the snippet gives none (no host code).
  const named = (f: "hooks.logs" | "hooks.release" | "hooks.user") => (config[f] ? pageHook(win, config[f]) : undefined);
  const logsAt = named("hooks.logs"), releaseAt = named("hooks.release"), userAt = named("hooks.user");
  const userSaid = userAt?.();
  s = {
    ...s,
    logs: s.logs ?? (logsAt && (() => { const v = logsAt(); return Array.isArray(v) ? (v as LogLine[]) : []; })),
    release: s.release ?? (releaseAt && (() => { const v = releaseAt(); return typeof v === "string" ? { version: v } : v && typeof v === "object" ? (v as Release) : {}; })),
    user: s.user ?? (typeof userSaid === "string" && userSaid ? userSaid : undefined),
  };
  // A link's override is kept for this viewer, where overrides apply at all.
  if (link && !r.ignored.some(i => i.layer === "you")) writeOverride(doc, { ...kept, ...link });

  const names = pluginsFor(config, s);
  const registry = s.registry ?? REGISTRY;
  const teardown: (() => void)[] = [];
  let handle: OverlayHandle | null = null;

  // The Clax extension takes reports whole: no composer is loaded.
  let handoff: ((req: HandoffRequest) => boolean) | null = null;
  if (config.enabled && config.presentation === "extension") {
    const ext = await registry["presentation-extension"](s, config);
    const p = ext.contributes.presentations?.[0];
    if (p?.available?.(win) && p.handoff) {
      handoff = req => p.handoff!(req, win);
      names.splice(0, names.length, "presentation-extension");
    }
  }

  // What the overlay can present, from the plugins it loaded and those it would load (null: it loaded none to ask).
  let built: string[] | null = null;
  if (config.enabled && !handoff) {
    const plugins = await Promise.all(names.map(n => registry[n](s, config)));
    built = [...plugins.flatMap(p => p.contributes.presentations?.map(x => x.id) ?? []), ...Object.keys(registry).filter(k => /^(presentation-|pins$)/.test(k)).map(k => k.replace("presentation-", ""))];
    const mount = s.mount ?? (await import("./mount")).mountOverlay;
    handle = mount({
      instance: s.instance, queue: s.queue ?? s.container, auth: authOf(s.auth, win), user: s.user, context: s.context, metadata: s.metadata, externalId: s.externalId, nonce: s.nonce,
      launcher: config["entry.launcher"] ? "corner" : false,
      diagnostics: { default: config["attachments.errors"] },
      presentation: config.presentation === "extension" ? "dialog" : config.presentation,
      theme: config.theme,
      copy: { title: config["copy.title"], intro: config["copy.intro"], retention: config["copy.retention"], reportsLabel: config["copy.reports-label"], reportsUrl: config["copy.reports-url"] },
      defaults: { "code-blur": config["filters.code-blur"], logs: config["attachments.logs"], version: config["attachments.version"], browser: config["attachments.browser"] },
    }, plugins, doc);
    teardown.push(() => handle?.close());
  }

  const report = (mode?: Mode | "note") => {
    if (!config.enabled) return;
    const m = mode ?? START[config.start];
    if (handoff) { handoff({ mode: m, url: win.location?.href ?? "", title: doc.title }); return; }
    handle?.report({ mode: m });
  };
  const push = (cmd: Command) => {
    if (!Array.isArray(cmd)) return;
    if (cmd[0] === "report" || cmd[0] === "open") report(cmd[1]?.mode);
    else if (cmd[0] === "reports") handle?.showReports();
    else if (cmd[0] === "close") handle?.close();
  };

  if (config.enabled) {
    // Targets: any element marked data-clax="report|reports", and the config's selectors.
    if (config["entry.targets"]) {
      const onClick = (e: MouseEvent) => {
        const at = e.target instanceof Element ? e.target : null;
        if (!at) return;
        const marked = at.closest<HTMLElement>("[data-feedback], [data-clax]");
        let action = marked?.dataset.feedback ?? marked?.dataset.clax;
        if (!action) action = config.targets.find(t => { try { return !!at.closest(t.selector); } catch { return false; } })?.action;
        if (action !== "report" && action !== "reports" && action !== "close") return;
        e.preventDefault();
        push([action] as Command);
      };
      doc.addEventListener("click", onClick, true);
      teardown.push(() => doc.removeEventListener("click", onClick, true));
    }
    // The key, never in a text field or with a modifier.
    const key = config["entry.key"].toLowerCase();
    if (key) {
      const onKey = (e: KeyboardEvent) => {
        if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing || e.key.toLowerCase() !== key) return;
        const at = e.composedPath()[0];
        if (at instanceof HTMLElement && (at.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(at.tagName))) return;
        // The overlay's hosts (bridge/src/anchor.ts OVERLAY_HOSTS, not imported: the loader stays small).
        if (at instanceof Element && at.closest("feedback-overlay, clax-overlay")) return;
        e.preventDefault();
        report();
      };
      win.addEventListener("keydown", onKey);
      teardown.push(() => win.removeEventListener("keydown", onKey));
    }
    // A link that starts Clax: ?feedback=report or ?feedback=reports (?clax= still read).
    if (config["entry.link"]) {
      const params = new URLSearchParams(win.location?.search ?? "");
      const asked = params.get("feedback") ?? params.get("clax");
      if (asked === "report") report();
      else if (asked === "reports") handle?.showReports();
    }
  }

  // The command queue: what was pushed before the loader ran, then each push.
  // `window.feedback`, vendor-neutral (white label); a tag manager's older `window.clax` queue is still read and, if it was there, still answered.
  const w = win as Window & { feedback?: unknown; clax?: unknown };
  const queued = [...(Array.isArray(w.feedback) ? w.feedback : []), ...(Array.isArray(w.clax) ? w.clax : [])] as unknown[];
  const legacy = Array.isArray(w.clax);
  const q = { push: (cmd: Command) => { push(cmd); return 0; } };
  w.feedback = q;
  if (legacy) w.clax = q;
  for (const cmd of queued) if (Array.isArray(cmd) && cmd[0] !== "install") push(cmd as Command);
  teardown.push(() => { if (w.feedback === q) delete w.feedback; if (w.clax === q) delete w.clax; });
  teardown.push(() => { if (doc.documentElement.getAttribute(CARRIER_ATTR) === carrier) doc.documentElement.removeAttribute(CARRIER_ATTR); });

  // The DevTools panel's Integration view reads this, with no host code.
  const mode = config["overrides.viewer"];
  const hook = integrationHook(() => ({
    v: 1, origin, instance: s.instance, container: s.container, version: container?.version ?? null, error, carrier,
    deployment: s.deployment ?? null, script: s.script ?? null, applies: mode === "always" || (mode === "local" && isDevOrigin(origin)),
    config, source: r.source, ignored: r.ignored, plugins: names, built,
  }), doc);
  Object.defineProperty(win, INSTALL_KEY, { value: hook, configurable: true });
  teardown.push(() => { if ((win as unknown as Record<string, unknown>)[INSTALL_KEY] === hook) delete (win as unknown as Record<string, unknown>)[INSTALL_KEY]; });

  const uninstall = () => { for (const off of teardown.splice(0).reverse()) { try { off(); } catch (e) { console.error("Clax.install: teardown threw", e); } } };
  // The extension's overlay goes when the site installs its own.
  if (carrier === "extension") {
    const yieldToSite = () => uninstall();
    doc.addEventListener(SITE_INSTALLED, yieldToSite, { once: true });
    teardown.unshift(() => doc.removeEventListener(SITE_INSTALLED, yieldToSite));
  }

  return {
    config, source: r.source, ignored: r.ignored, error,
    container: container ? { id: container.id, version: container.version } : null,
    plugins: names, handle, push,
    actions: () => (config.enabled && config["entry.palette"] ? (handle?.actions() ?? (handoff ? [{ id: "clax.report", label: "Send feedback", run: () => report() }] : [])) : []),
    override: layer => writeOverride(doc, layer),
    uninstall,
  };
}

/** `auth` as the overlay takes it: a URL becomes a POST of the reporter's
 * public key to it (same origin, with the site's cookies), answered with
 * `{ token, expires_at }`, with the report attempt's `traceparent`. */
export function authOf(auth: ReporterAuth | string, win: Window): ReporterAuth {
  if (typeof auth === "function") return auth;
  return async (publicKey, trace) => {
    const headers = { "content-type": "text/plain", ...(trace ? { traceparent: trace.traceparent } : {}) };
    const res = await win.fetch(auth, { method: "POST", credentials: "same-origin", headers, body: publicKey });
    if (!res.ok) throw new Error(`the site's token route answered HTTP ${res.status}`);
    return res.json();
  };
}

/** The script tag's start: a tag manager's `window.clax = [["install", {...}]]`
 * pushed before the script ran is installed now. */
export function installQueued(win: Window = window): Promise<Installed> | null {
  const g = win as Window & { feedback?: unknown; clax?: unknown };
  const q = Array.isArray(g.feedback) ? g.feedback : g.clax;
  if (!Array.isArray(q)) return null;
  const cmd = q.find(c => Array.isArray(c) && c[0] === "install");
  if (!cmd) return null;
  return install({ ...(cmd[1] as Snippet) }).catch(e => { console.error("Clax.install:", e); throw e; });
}

/** The snippet a tag carries: `<script src="{instance}/_clax/overlay.js"
 * data-container="…" data-auth="/clax/token">`. The instance is where the
 * script came from; `data-auth` is the site's token route, resolved against
 * the page. `null` for a tag with no `data-container` (a page that calls
 * `Clax.install` itself). */
export function snippetFromTag(tag: HTMLScriptElement | null | undefined, base: string): Partial<Snippet> | null {
  const d = tag?.dataset;
  if (!tag || !d?.container) return null;
  let instance: string;
  try {
    const src = new URL(tag.src, base);
    instance = src.origin + src.pathname.replace(/\/_clax\/[^/]*$/, "");
  } catch { return null; }
  const s: Partial<Snippet> = { instance: instance.replace(/\/+$/, ""), container: d.container };
  if (d.queue) s.queue = d.queue;
  if (d.deployment) s.deployment = d.deployment;
  s.script = tag.src;
  if (d.auth) { try { s.auth = new URL(d.auth, base).href; } catch { /* left out: install says auth is missing */ } }
  return s;
}

/** Who put the overlay on the page: the site's own tag (or its own
 * `Clax.install`), or the Clax extension. */
export type Carrier = "site" | "extension";

/** The mark on `<html>` naming the carrier that installed Clax. Both worlds
 * see it: the page's and an extension's content script share the DOM, not
 * their globals. */
export const CARRIER_ATTR = "data-feedback-installed";
/** Dispatched on the document when the site installs, so an extension's
 * overlay already there removes itself: the site's install wins. */
export const SITE_INSTALLED = "feedback:site-installed";

/** Whether `carrier` may install on `doc`, and marks it when it may: the
 * site always may (an extension's overlay is told to go); the extension may
 * only where nothing is installed. */
export function claim(doc: Document, carrier: Carrier): boolean {
  const html = doc.documentElement;
  const held = html.getAttribute(CARRIER_ATTR);
  if (carrier === "extension") {
    if (held) return false;
    html.setAttribute(CARRIER_ATTR, "extension");
    return true;
  }
  html.setAttribute(CARRIER_ATTR, "site");
  if (held === "extension") doc.dispatchEvent(new Event(SITE_INSTALLED));
  return true;
}

/** The tag's install, from the script now running: what `/_clax/overlay.js`
 * does when it is loaded with `data-container`. */
export function installFromTag(doc: Document = document): Promise<Installed> | null {
  const tag = doc.currentScript as HTMLScriptElement | null;
  const s = snippetFromTag(tag, doc.baseURI);
  if (!s) return null;
  if (!s.auth) {
    console.error(`Clax: the tag for container ${s.container} has no data-auth (the site's token route); Clax reports need a reporter token`);
    return null;
  }
  return install({ ...(s as Snippet), doc }).catch(e => { console.error("Clax.install:", e); throw e; });
}
