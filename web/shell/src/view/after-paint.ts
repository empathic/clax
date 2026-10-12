import { visibility } from "../visibility";

/** How long a wait for an animation frame lasts where frames may not come. */
export const FRAME_FALLBACK_MS = 100;

/** Runs `fn` at the next animation frame, or after `FRAME_FALLBACK_MS` when
 * none comes first (a hidden tab, or the Clax app's WebView, which may run no
 * frames while the app shows it). With `whenShown`, the timer stands in only
 * where the app's host says the page is shown (`visibility.shownByHost`): a
 * wait that must not end before the viewer can see the page (an armed Allow)
 * then still waits for a frame in a hidden browser tab. Returns the cancel. */
export function nextFrame(fn: () => void, whenShown = false): () => void {
  let raf = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cancel = () => { clearTimeout(timer); if (raf) cancelAnimationFrame(raf); };
  const run = () => { cancel(); fn(); };
  if (!whenShown || visibility.shownByHost()) timer = setTimeout(run, FRAME_FALLBACK_MS);
  if (typeof requestAnimationFrame === "function") raf = requestAnimationFrame(run);
  return cancel;
}

/** Runs `fn` once the page has painted: after the next animation frame, or
 * after `FRAME_FALLBACK_MS` where none comes (a hidden tab). The result
 * cancels it. */
export function afterPaint(fn: () => void): () => void {
  let later: ReturnType<typeof setTimeout> | undefined;
  const cancel = nextFrame(() => { later = setTimeout(fn); });
  return () => { cancel(); clearTimeout(later); };
}
