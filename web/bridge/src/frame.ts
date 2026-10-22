// A wait for the next animation frame that ends anyway after `FRAME_FALLBACK_MS`
// when no frame comes first: a document the browser treats as hidden runs
// none, and the Clax app's WebView may treat the page as hidden while the app
// shows it. For work that only needs to come soon (re-placing pins after a
// scroll, an outline under the pointer), never for a wait that guards consent.

/** How long a wait for an animation frame lasts where frames may not come. */
export const FRAME_FALLBACK_MS = 80;

/** Runs `fn` once, at the next animation frame or after `FRAME_FALLBACK_MS`,
 * whichever comes first. Returns the cancel. */
export function nextFrame(fn: () => void): () => void {
  const cancel = () => { cancelAnimationFrame(raf); clearTimeout(timer); };
  const run = () => { cancel(); fn(); };
  const timer = setTimeout(run, FRAME_FALLBACK_MS);
  const raf = requestAnimationFrame(run);
  return cancel;
}
