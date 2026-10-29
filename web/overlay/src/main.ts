// The embeddable overlay, built as one classic script (`/_clax/overlay.js`,
// global `Clax`): a deployer's page calls `Clax.overlay({instance, queue,
// auth})` and its visitors file reports into the queue (spec 2026-10-27
// §5.3). Its interface lives in a shadow root on <html>: the page's rules
// do not reach it and its own do not reach the page. Keys typed into it stop
// at that root, so the page's shortcuts do not hear them. Its styles are
// set through the CSSOM (the host's `style`, the root's adopted sheets), so a
// page whose CSP `style-src` lacks 'unsafe-inline' does not refuse them.
import { mount, unmount } from "svelte";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { addSheet } from "../../bridge/src/sheet";
import { ReporterClient } from "../../sdk/src/reporter";
import { externalIdOf, type OverlayOptions } from "./model";
import Overlay from "./Overlay.svelte";
import { commentPicker, type Picked } from "./picker";
import TOKENS from "./tokens.css?inline";

export type { OverlayOptions };

/** A running overlay. */
export interface OverlayHandle {
  /** Removes the overlay and ends comment mode. */
  close(): void;
}

/** The components' stylesheet: written in at build time from the CSS the
 * Svelte components compile to (`vite.overlay.config.ts`). Run from source,
 * the overlay keeps this placeholder, which styles nothing. */
const COMPONENT_CSS: string = "__CLAX_OVERLAY_COMPONENT_CSS__";

/** Inline style of the host: fixed, sized to nothing and passing every
 * pointer event, so the page's rules can neither hide nor move it. */
const HOST_CSS = "all:initial!important;position:fixed!important;left:0!important;top:0!important;width:0!important;height:0!important;display:block!important;pointer-events:none!important;visibility:visible!important;opacity:1!important;z-index:2147483647!important;";

/** Starts the overlay on `doc` (default this page's): the launcher in the
 * page's bottom right corner. */
export function overlay(opts: OverlayOptions, doc: Document = document): OverlayHandle {
  for (const k of ["instance", "queue"] as const) if (typeof opts?.[k] !== "string" || !opts[k]) throw new TypeError(`Clax.overlay: ${k} must be a non-empty string`);
  if (typeof opts.auth !== "function") throw new TypeError("Clax.overlay: auth must be a function");
  const client = new ReporterClient({ instance: opts.instance.replace(/\/+$/, ""), queue: opts.queue, auth: opts.auth });

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
      client, doc, host,
      picker: (onPick: (p: Picked) => void, onCancel: () => void) => commentPicker(doc, onPick, onCancel),
      externalId: externalIdOf(doc, opts.externalId),
      metadata: opts.metadata,
    },
  });
  let open = true;
  return {
    close() {
      if (!open) return;
      open = false;
      void unmount(app);
      host.remove();
    },
  };
}
