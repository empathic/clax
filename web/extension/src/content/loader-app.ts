// The loader's work (spec 2026-10-05 §12): it tells the worker the page's
// URL on load (`hello`) and after each same-document navigation (`route`),
// filtered and throttled as the overlay's own reports are (`watchRoutes`).
// Once the overlay runs in this document it reports the routes itself, so
// the loader then stays quiet. An address over MAX_URL goes as null.
import { type OverlayToWorker, pageUrl } from "../messages";
import { overlayRunning } from "./presence";
import { type RouteTimers, watchRoutes } from "./routes";

export type LoaderEnv = {
  win: Window;
  runtime: { readonly id: string | undefined; sendMessage(m: OverlayToWorker): Promise<unknown> };
  /** The isolated world's global, where a running overlay marks itself. */
  g?: object;
  timers?: RouteTimers;
  now?(): number;
};

/** Starts the loader; returns what stops it. A context the extension left
 * (it was reloaded or removed) stops it too. */
export function startLoader(env: LoaderEnv): () => void {
  const g = env.g ?? globalThis;
  let stop = () => {};
  const tell = (t: "hello" | "route", href: string) => {
    try {
      if (!env.runtime.id) throw new Error("extension context invalidated");
      env.runtime.sendMessage({ t, url: pageUrl(href) }).catch(() => {});
    } catch {
      stop();
    }
  };
  tell("hello", env.win.location.href);
  stop = watchRoutes(env.win, href => { if (!overlayRunning(g)) tell("route", href); }, env.timers, env.now);
  return () => stop();
}
