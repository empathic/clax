// The overlay's mount, with its plugins given (main.ts gives the classic
// set; the loader, loader.ts, only those its config names). The embeddable
// overlay is built as one classic script (`/_clax/overlay.js`,
// global `Clax`), the same entry as an ES module (`/_clax/overlay.mjs`)
// for bundlers, and the `./overlay` entry of the SDK package
// (scripts/build-sdk.mjs). A deployer's page calls `Clax.overlay({instance,
// queue, auth, …})` and its visitors file reports into the queue (spec
// 2026-10-27 §5.3, 2026-10-29-feedback-loop §4.6). Nothing in it is tied to
// a framework: a plain page, a React or a SvelteKit app each start it once
// (in the browser, after load) and call the handle from their own controls.
//
// Its interface lives in a shadow root on <html>: the page's rules do not
// reach it and its own do not reach the page. Keys typed into it stop at
// that root, so the page's shortcuts do not hear them. Its styles are set
// through the CSSOM (the host's `style`, the root's adopted sheets), so a
// page whose CSP `style-src` lacks 'unsafe-inline' does not refuse them.
import { mount, unmount } from "svelte";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { addSheet } from "../../bridge/src/sheet";
import { TOKEN_NAME } from "../../sdk/src/config";
import { type Recorder, installDiagnostics } from "../../sdk/src/diagnostics";
import { forgetReporter as forgetKept } from "../../sdk/src/keystore";
import { initialLifecycleState, type AuthStatus, type ReporterLifecycleState } from "../../sdk/src/lifecycle";
import { ReporterClient, type ReporterAuth } from "../../sdk/src/reporter";
import type { DiagLine, Report } from "../../sdk/src/types";
import { type Settings, fetchInstall, installIdOf, settingsOf } from "./install";
import { Session, externalIdOf, reportsModeOf } from "./model";
import { CONTEXT_TIMEOUT_MS, type Given, type HostContext, type Mode, type OverlayOptions, type ReportsMode } from "./options";
import Overlay from "./Overlay.svelte";
import { type ClaxPlugin, PluginHost } from "../../sdk/src/plugin";
import { commentPicker, type Picked } from "./picker";
import TOKENS from "./tokens.css?inline";

export type { AuthStatus, DiagLine, Given, HostContext, Mode, OverlayOptions, Recorder, Report, ReporterAuth, ReporterLifecycleState, ReportsMode };
export { CONTEXT_TIMEOUT_MS };

/** What a running overlay tells its host. */
export interface OverlayEvents {
  /** How many of the reporter's reports wait on them (a fix to check, or a
   * reply not yet read): told once the first count is known, and again on
   * each change; a listener added later hears the last count at once. */
  badge: number;
  /** A report the reporter just filed, as Clax answered it. */
  filed: Report;
  /** Whether a report is under way (the mode bar, a pick, or the form open),
   * told on each change; a listener added later hears the current value at
   * once. The host's own trigger shows it (`aria-pressed`). */
  active: boolean;
}

/** A running overlay. */
export interface OverlayHandle {
  /** Starts a report: the mode bar, in `mode` (default `"element"`;
   * `"screen"` captures the visible page at once). Does nothing when
   * `canFile` is false. */
  report(opts?: { mode?: Mode | "note" }): void;
  /** Opens "Your reports". */
  showReports(): void;
  /** What the host's command palette may offer: Send feedback, and Your reports. */
  actions(): { id: string; label: string; run(): void }[];
  /** Listens for `event`; the result stops listening. */
  on(event: "badge", fn: (n: number) => void): () => void;
  on(event: "filed", fn: (r: Report) => void): () => void;
  on(event: "active", fn: (on: boolean) => void): () => void;
  /** The reporter lifecycle's state (sdk/src/lifecycle.ts): auth, "Your
   * reports" and the waiting count (`waiting`, the same the launcher's
   * badge and `on("badge")` show) — for a host with `launcher: false` to
   * read for its own UI, such as a "needs your review" dot on its own
   * button. A Svelte store's `subscribe`: `fn` is called at once with the
   * current state (`sdk/src/lifecycle.ts`'s `initialLifecycleState` before
   * the overlay has started), and again on each change. */
  subscribe(fn: (s: ReporterLifecycleState) => void): () => void;
  /** Removes the overlay and ends comment mode. The page's diagnostics
   * recorder keeps recording, for a later overlay. */
  close(): void;
  /** Forgets the reporter, as the host does when its user signs out:
   * deletes the key and token kept for `user` and drops them from this
   * overlay, which keeps nothing more. */
  forget(): Promise<void>;
}

/** Whose kept key and token `forgetReporter` deletes: the overlay's
 * `instance`, `queue` and `user` options. */
export interface ReporterRef {
  instance: string;
  queue: string;
  user: string;
}

/** Forgets what the overlay keeps for `r.user` (`OverlayOptions.user`):
 * the reporter's key and token, so the next user on this browser shares
 * neither. Call it on sign-out; it needs no running overlay. */
export async function forgetReporter(r: ReporterRef): Promise<void> {
  for (const k of ["instance", "queue", "user"] as const) if (typeof r?.[k] !== "string" || !r[k]) throw new TypeError(`Clax.forgetReporter: ${k} must be a non-empty string`);
  await forgetKept(r.instance.replace(/\/+$/, ""), r.queue, r.user);
}

/** The overlay's mounted component: what `Overlay.svelte` exports. */
interface Mounted {
  report(opts?: { mode?: Mode | "note" }): void;
  showReports(): void;
  onLifecycle(fn: (s: ReporterLifecycleState) => void): () => void;
}

/** The components' stylesheet: written in at build time from the CSS the
 * Svelte components compile to (`scripts/overlay-component-css.mjs`, in
 * both builds). Run from source, the overlay keeps this placeholder, which
 * styles nothing. */
const COMPONENT_CSS: string = "__CLAX_OVERLAY_COMPONENT_CSS__";

/** Inline style of the host: fixed, sized to nothing and passing every
 * pointer event, so the page's rules can neither hide nor move it. */
const HOST_CSS = "all:initial!important;position:fixed!important;left:0!important;top:0!important;width:0!important;height:0!important;display:block!important;pointer-events:none!important;visibility:visible!important;opacity:1!important;z-index:2147483647!important;";

/** Starts recording the page's recent errors (spec 2026-10-29-feedback-loop
 * §4.4) before the overlay starts, so errors from the page's own start are
 * kept; `overlay()` starts it too. Nothing recorded leaves the page unless
 * the reporter ticks "Include recent errors". */
export const diagnostics = {
  install(win: Window = window): Recorder {
    return installDiagnostics(win);
  },
};

/** The overlay takes the page's own light or dark: the color-scheme the page computes on its root (a site's theme switch
 * sets it, as Changeling's does), as `data-scheme` on the host, which tokens.css follows over the system's. A page that
 * names both, or neither, leaves it to the system. Followed as the page switches; returns the stop. */
export function followScheme(doc: Document, host: HTMLElement): () => void {
  const win = doc.defaultView;
  const read = () => {
    const s = win ? win.getComputedStyle(doc.documentElement).colorScheme.trim() : "";
    const one = s === "dark" || s === "only dark" ? "dark" : s === "light" || s === "only light" ? "light" : null;
    if (one) host.setAttribute("data-scheme", one); else host.removeAttribute("data-scheme");
  };
  read();
  const mo = new MutationObserver(read);
  mo.observe(doc.documentElement, { attributes: true, attributeFilter: ["class", "style", "data-theme", "data-color-scheme"] });
  const mq = win?.matchMedia?.("(prefers-color-scheme: dark)");
  mq?.addEventListener?.("change", read);
  return () => { mo.disconnect(); mq?.removeEventListener?.("change", read); };
}

/** `theme`'s `--clax-*` entries; anything else is left out. */
function themeOf(theme: Record<string, string> | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(theme ?? {})) if (TOKEN_NAME.test(k) && typeof v === "string") out[k] = v;
  return out;
}

/** Starts the overlay on `doc` with `plugins` (the overlay's features; `overlay()` in main.ts defaults them), with the launcher in
 * the page's bottom right corner unless `opts.launcher` is false. */
export function mountOverlay(opts: OverlayOptions, pluginList: readonly ClaxPlugin[], doc: Document = document): OverlayHandle {
  if (typeof opts?.instance !== "string" || !opts.instance) throw new TypeError("Clax.overlay: instance must be a non-empty string");
  const installId = installIdOf(doc, opts.install);
  if (opts.queue !== undefined || !installId) if (typeof opts.queue !== "string" || !opts.queue) throw new TypeError("Clax.overlay: queue must be a non-empty string");
  if (typeof opts.auth !== "function") throw new TypeError("Clax.overlay: auth must be a function");
  if (opts.launcher !== undefined && opts.launcher !== "corner" && opts.launcher !== false) throw new TypeError('Clax.overlay: launcher must be "corner" or false');
  if (opts.nonce !== undefined && (typeof opts.nonce !== "string" || !opts.nonce)) throw new TypeError("Clax.overlay: nonce must be a non-empty string");
  const instance = opts.instance.replace(/\/+$/, "");
  const win = doc.defaultView ?? window;
  if (opts.user !== undefined && (typeof opts.user !== "string" || !opts.user)) throw new TypeError("Clax.overlay: user must be a non-empty string");
  // Without an install the page's own options start it at once (a bad
  // `reports` is refused here); with one, once its config is fetched.
  if (!installId) reportsModeOf(doc, opts.reports);
  const recorder = installDiagnostics(win, { instance });
  const session = new Session({ context: opts.context, metadata: opts.metadata, diagnostics: opts.diagnostics, recorder }, win);

  const listeners = { badge: new Set<(n: number) => void>(), filed: new Set<(r: Report) => void>(), active: new Set<(on: boolean) => void>() };
  let lastBadge: number | null = null;
  let lastActive = false;
  // The reporter lifecycle's own state, relayed from the component once it
  // starts (`start`, below); `handle.subscribe` works before that too,
  // with `initialLifecycleState`, the same way `on("badge")` gives `null`
  // until the first count (here, every subscriber instead gets the idle
  // state at once).
  const lifecycleListeners = new Set<(s: ReporterLifecycleState) => void>();
  let lifecycleState: ReporterLifecycleState = initialLifecycleState;
  let unsubLifecycle: (() => void) | null = null;
  const emit = <K extends keyof OverlayEvents>(event: K, value: OverlayEvents[K]) => {
    for (const fn of [...listeners[event]] as ((v: OverlayEvents[K]) => void)[]) {
      try { fn(value); } catch (e) { console.error(`Clax.overlay: a ${event} listener threw`, e); }
    }
  };

  if (!Array.isArray(pluginList)) throw new TypeError("Clax.overlay: plugins must be a list of plugins");
  const plugins = new PluginHost(pluginList);
  // A page refusing inline style: its clips never try the web fonts' <style>.
  // The clip code is loaded only where a capture plugin is in (a site with
  // screenshots off never fetches it); this runs before any clip, whose
  // import waits on the same module.
  if (plugins.list("capture").length) void import("../../bridge/src/clip").then(m => { if (opts.nonce !== undefined || m.metaRefusesInlineStyle(doc)) m.withoutWebFonts(doc); });
  for (const r of plugins.refused) console.warn(`Clax.overlay: plugin ${r.name} refused: ${r.reason}`);

  const host = doc.createElement(OVERLAY_TAG);
  host.style.cssText = HOST_CSS;
  // The site's look: --clax-* custom properties on the host reach into its shadow root (tokens.css).
  for (const [k, v] of Object.entries({ ...plugins.theme(), ...themeOf(opts.theme) })) host.style.setProperty(k, v);
  const root = host.attachShadow({ mode: "open" });
  const css = [TOKENS, COMPONENT_CSS].join("\n");
  addSheet(root, css);
  const target = doc.createElement("div");
  root.append(target);
  for (const k of ["keydown", "keyup", "keypress"]) root.addEventListener(k, e => e.stopPropagation());
  doc.documentElement.appendChild(host);
  const stopScheme = followScheme(doc, host);

  let open = true;
  let app: Mounted | null = null;
  let client: ReporterClient | null = null;
  let settings: Settings | null = null;
  // What the host asked before the overlay started, done once it has.
  let asked: ((a: Mounted) => void) | null = null;
  const start = (s: Settings) => {
    if (!open) return;
    settings = s;
    client = new ReporterClient({ instance, queue: s.queue, auth: opts.auth, user: opts.user });
    if (s.brandColor) addSheet(root, `:host{--you:${s.brandColor};--you-ink:${s.brandColor};}`);
    app = mount(Overlay, {
      target,
      props: {
        client, doc, host, session,
        picker: (onPick: (p: Picked) => void, onCancel: () => void, onHover?: (what: string | null) => void) => commentPicker(doc, onPick, onCancel, onHover),
        externalId: externalIdOf(doc, opts.externalId),
        launcher: opts.launcher ?? "corner",
        canFile: opts.canFile ?? true,
        reports: s.reports,
        elementPicker: s.picker,
        wording: s.wording,
        queueInfo: s.queueInfo,
        onBadge: (n: number) => { lastBadge = n; emit("badge", n); },
        onFiled: (r: Report) => emit("filed", r),
        onActive: (on: boolean) => { if (on === lastActive) return; lastActive = on; emit("active", on); },
        plugins, presentation: opts.presentation, css, defaults: opts.defaults ?? {}, copy: opts.copy ?? {},
      },
    }) as unknown as Mounted;
    unsubLifecycle = app.onLifecycle(st => {
      lifecycleState = st;
      for (const fn of [...lifecycleListeners]) { try { fn(st); } catch (e) { console.error("Clax.overlay: a subscribe listener threw", e); } }
    });
    asked?.(app);
    asked = null;
  };
  const ready = installId
    ? fetchInstall(instance, installId, (win.fetch ?? fetch).bind(win)).then(cfg => {
      if (!cfg) console.warn(`Clax.overlay: install ${installId}'s config could not be fetched; starting on the page's own options`);
      let s: Settings;
      try { s = settingsOf(cfg, opts, doc); } catch (e) { console.error(e); return; }
      if (!s.queue) { console.error("Clax.overlay: no queue: the page names none and the install's config could not be fetched"); return; }
      start(s);
    })
    : (start(settingsOf(null, opts, doc)), Promise.resolve());
  const whenUp = (fn: (a: Mounted) => void) => { if (!open) return; if (app) fn(app); else asked = fn; };
  return {
    report(o) { whenUp(a => a.report(o)); },
    showReports() { whenUp(a => a.showReports()); },
    actions: () => [
      ...(opts.canFile ?? true ? [{ id: "clax.report", label: "Send feedback", run: () => whenUp(a => a.report()) }] : []),
      ...(settings?.reports === "off" ? [] : [{ id: "clax.reports", label: "Your reports", run: () => whenUp(a => a.showReports()) }]),
    ],
    on(event: keyof OverlayEvents, fn: ((n: number) => void) | ((r: Report) => void) | ((on: boolean) => void)) {
      if (event !== "badge" && event !== "filed" && event !== "active") throw new TypeError(`Clax.overlay: no event ${String(event)}`);
      const set = listeners[event] as Set<typeof fn>;
      set.add(fn);
      if (event === "badge" && lastBadge !== null) (fn as (n: number) => void)(lastBadge);
      if (event === "active") (fn as (on: boolean) => void)(lastActive);
      return () => { set.delete(fn); };
    },
    subscribe(fn: (s: ReporterLifecycleState) => void) {
      lifecycleListeners.add(fn);
      fn(lifecycleState);
      return () => { lifecycleListeners.delete(fn); };
    },
    forget: () => ready.then(() => client?.forget()),
    close() {
      if (!open) return;
      open = false;
      listeners.badge.clear();
      listeners.filed.clear();
      if (lastActive) { lastActive = false; for (const fn of [...listeners.active]) { try { fn(false); } catch (e) { console.error("Clax.overlay: an active listener threw", e); } } }
      listeners.active.clear();
      unsubLifecycle?.();
      lifecycleListeners.clear();
      if (app) void unmount(app as never);
      plugins.destroy();
      stopScheme();
      host.remove();
    },
  };
}
