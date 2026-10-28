// The embeddable overlay, built as one classic script (`/_clax/overlay.js`,
// global `Clax`): a deployer's page calls `Clax.overlay({instance, queue,
// auth})` and its visitors file reports into the queue (spec 2026-10-27
// §5.3). Its interface lives in a shadow root on <html>: the page's rules
// do not reach it and its own do not reach the page. Keys typed into it stop
// at that root, so the page's shortcuts do not hear them.
import { mount, unmount } from "svelte";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
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
  host.setAttribute("style", HOST_CSS);
  const root = host.attachShadow({ mode: "open" });
  const style = doc.createElement("style");
  style.textContent = TOKENS;
  const target = doc.createElement("div");
  root.append(style, target);
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
