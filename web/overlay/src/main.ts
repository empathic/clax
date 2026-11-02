// The embeddable overlay's entry: `/_clax/overlay.js` (global `Clax`),
// `/_clax/overlay.mjs`, and the SDK package's `./overlay`. `overlay()`
// mounts it with the plugins given, or the classic set (the overlay as it
// was before plugins); the mount itself is mount.ts, which the loader
// (loader.ts) uses with only the plugins its config names.
import { type OverlayHandle, mountOverlay } from "./mount";
import type { OverlayOptions } from "./options";
import { classic } from "./plugins";

export * from "./mount";

/** Starts the overlay on `doc` (default this page's), with the launcher in
 * the page's bottom right corner unless `opts.launcher` is false, and the
 * plugins `opts.plugins` (default the classic set). */
export function overlay(opts: OverlayOptions, doc: Document = document): OverlayHandle {
  if (opts?.plugins !== undefined && !Array.isArray(opts.plugins)) throw new TypeError("Clax.overlay: plugins must be a list of plugins");
  return mountOverlay(opts, opts?.plugins ?? classic(), doc);
}

export { install, installQueued, type Installed, type Snippet } from "./loader";

// Loaded by a script tag after a tag manager queued `["install", {...}]`: install it now.
import { installQueued as queued } from "./loader";
if (typeof window !== "undefined") queued(window)?.catch(() => {});
