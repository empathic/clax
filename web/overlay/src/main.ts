// The embeddable overlay, built as one classic script (`/_clax/overlay.js`,
// global `Clax`) and the same entry as an ES module (`/_clax/overlay.mjs`)
// for bundlers. A deployer's page calls `Clax.overlay({instance, queue,
// auth, …})` and its visitors file reports into the queue (spec 2026-10-27
// §5.3, 2026-10-29-feedback-loop §4.6). Nothing in it is tied to a
// framework: a plain page, a React or a SvelteKit app each start it once
// (in the browser, after load) and call the handle from their own controls.
//
// Its interface lives in a shadow root on <html>: the page's rules do not
// reach it and its own do not reach the page. Keys typed into it stop at
// that root, so the page's shortcuts do not hear them. Its styles are set
// through the CSSOM (the host's `style`, the root's adopted sheets), so a
// page whose CSP `style-src` lacks 'unsafe-inline' does not refuse them.
import { mount, unmount } from "svelte";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { metaRefusesInlineStyle, withoutWebFonts } from "../../bridge/src/clip";
import { addSheet } from "../../bridge/src/sheet";
import { type Recorder, installDiagnostics } from "../../sdk/src/diagnostics";
import { ReporterClient, type ReporterAuth } from "../../sdk/src/reporter";
import type { DiagLine, Report } from "../../sdk/src/types";
import type { Mode } from "./capture";
import { CONTEXT_TIMEOUT_MS, type Given, type HostContext, type OverlayOptions, Session, externalIdOf } from "./model";
import Overlay from "./Overlay.svelte";
import { commentPicker, type Picked } from "./picker";
import TOKENS from "./tokens.css?inline";

export type { DiagLine, Given, HostContext, Mode, OverlayOptions, Recorder, Report, ReporterAuth };
export { CONTEXT_TIMEOUT_MS };

/** What a running overlay tells its host. */
export interface OverlayEvents {
  /** How many of the reporter's reports wait on them (a fix to check, or a
   * reply not yet read): told once the first count is known, and again on
   * each change; a listener added later hears the last count at once. */
  badge: number;
  /** A report the reporter just filed, as Clax answered it. */
  filed: Report;
}

/** A running overlay. */
export interface OverlayHandle {
  /** Starts a report: the mode bar, in `mode` (default `"element"`;
   * `"screen"` captures the visible page at once). Does nothing when
   * `canFile` is false. */
  report(opts?: { mode?: Mode }): void;
  /** Opens "Your reports". */
  showReports(): void;
  /** Listens for `event`; the result stops listening. */
  on(event: "badge", fn: (n: number) => void): () => void;
  on(event: "filed", fn: (r: Report) => void): () => void;
  /** Removes the overlay and ends comment mode. The page's diagnostics
   * recorder keeps recording, for a later overlay. */
  close(): void;
}

/** The overlay's mounted component: what `Overlay.svelte` exports. */
interface Mounted {
  report(opts?: { mode?: Mode }): void;
  showReports(): void;
}

/** The components' stylesheet: written in at build time from the CSS the
 * Svelte components compile to (`vite.overlay.config.ts`). Run from source,
 * the overlay keeps this placeholder, which styles nothing. */
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

/** Starts the overlay on `doc` (default this page's), with the launcher in
 * the page's bottom right corner unless `opts.launcher` is false. */
export function overlay(opts: OverlayOptions, doc: Document = document): OverlayHandle {
  for (const k of ["instance", "queue"] as const) if (typeof opts?.[k] !== "string" || !opts[k]) throw new TypeError(`Clax.overlay: ${k} must be a non-empty string`);
  if (typeof opts.auth !== "function") throw new TypeError("Clax.overlay: auth must be a function");
  if (opts.launcher !== undefined && opts.launcher !== "corner" && opts.launcher !== false) throw new TypeError('Clax.overlay: launcher must be "corner" or false');
  if (opts.nonce !== undefined && (typeof opts.nonce !== "string" || !opts.nonce)) throw new TypeError("Clax.overlay: nonce must be a non-empty string");
  const instance = opts.instance.replace(/\/+$/, "");
  const win = doc.defaultView ?? window;
  if (opts.user !== undefined && (typeof opts.user !== "string" || !opts.user)) throw new TypeError("Clax.overlay: user must be a non-empty string");
  const client = new ReporterClient({ instance, queue: opts.queue, auth: opts.auth, user: opts.user });
  const recorder = installDiagnostics(win, { instance });
  // A page refusing inline style: its clips never try the web fonts' <style>.
  if (opts.nonce !== undefined || metaRefusesInlineStyle(doc)) withoutWebFonts(doc);
  const session = new Session({ context: opts.context, metadata: opts.metadata, diagnostics: opts.diagnostics, recorder }, win);

  const listeners = { badge: new Set<(n: number) => void>(), filed: new Set<(r: Report) => void>() };
  let lastBadge: number | null = null;
  const emit = <K extends keyof OverlayEvents>(event: K, value: OverlayEvents[K]) => {
    for (const fn of [...listeners[event]] as ((v: OverlayEvents[K]) => void)[]) {
      try { fn(value); } catch (e) { console.error(`Clax.overlay: a ${event} listener threw`, e); }
    }
  };

  const host = doc.createElement(OVERLAY_TAG);
  host.style.cssText = HOST_CSS;
  const root = host.attachShadow({ mode: "open" });
  addSheet(root, [TOKENS, COMPONENT_CSS].join("\n"));
  const target = doc.createElement("div");
  root.append(target);
  for (const k of ["keydown", "keyup", "keypress"]) root.addEventListener(k, e => e.stopPropagation());
  doc.documentElement.appendChild(host);

  const app = mount(Overlay, {
    target,
    props: {
      client, doc, host, session,
      picker: (onPick: (p: Picked) => void, onCancel: () => void, onHover?: (what: string | null) => void) => commentPicker(doc, onPick, onCancel, onHover),
      externalId: externalIdOf(doc, opts.externalId),
      launcher: opts.launcher ?? "corner",
      canFile: opts.canFile ?? true,
      onBadge: (n: number) => { lastBadge = n; emit("badge", n); },
      onFiled: (r: Report) => emit("filed", r),
    },
  }) as unknown as Mounted;
  let open = true;
  return {
    report(o) { if (open) app.report(o); },
    showReports() { if (open) app.showReports(); },
    on(event: keyof OverlayEvents, fn: ((n: number) => void) | ((r: Report) => void)) {
      if (event !== "badge" && event !== "filed") throw new TypeError(`Clax.overlay: no event ${String(event)}`);
      const set = listeners[event] as Set<typeof fn>;
      set.add(fn);
      if (event === "badge" && lastBadge !== null) (fn as (n: number) => void)(lastBadge);
      return () => { set.delete(fn); };
    },
    close() {
      if (!open) return;
      open = false;
      listeners.badge.clear();
      listeners.filed.clear();
      void unmount(app as never);
      host.remove();
    },
  };
}
