// docs/feedback.md's typed dynamic import, kept compiling by //web:typecheck.
// In a product, `../overlay` is the copy of `<instance>/_clax/overlay.d.ts`
// saved beside this file (`./clax-overlay`, or `$lib/clax-overlay` in
// SvelteKit); everything else is as the guide prints it.
import type { OverlayHandle, OverlayOptions } from "../overlay";

type Overlay = typeof import("../overlay");

const OVERLAY_URL = "https://clax.dev/_clax/overlay.mjs";

/** Loads the overlay from the instance and starts it with `opts`. */
export async function startOverlay(opts: OverlayOptions): Promise<OverlayHandle> {
  const { overlay } = (await import(/* @vite-ignore */ OVERLAY_URL)) as Overlay;
  return overlay(opts);
}
