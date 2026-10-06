// Same-document navigations, as the loader and the overlay report them to
// the worker (spec 2026-10-05 §10.3, §11 "SPA route change"). A page can
// dispatch `navigatesuccess`, `popstate` and `hashchange` itself, and those
// reach the isolated world too: only the browser's own events count, the
// URL is read from `location`, an unchanged one is not reported, and a
// burst reports its last URL once, at most one every ROUTE_MS.

/** The least time between two reports: a burst of navigations reports the last URL once, on the trailing edge. */
export const ROUTE_MS = 250;

export type RouteTimers = { set(fn: () => void, ms: number): unknown; clear(h: unknown): void };
const realTimers: RouteTimers = { set: (fn, ms) => setTimeout(fn, ms), clear: h => clearTimeout(h as ReturnType<typeof setTimeout>) };

/** Calls `report` with the page's new URL after each same-document
 * navigation of `win`, as above; returns what stops it. */
export function watchRoutes(win: Window, report: (href: string) => void, timers: RouteTimers = realTimers, now: () => number = () => performance.now()): () => void {
  let last = win.location.href;
  let lastAt = now();
  let timer: unknown = null;
  let live = true;
  const fire = () => {
    timer = null;
    const href = win.location.href;
    if (!live || href === last) return;
    last = href;
    lastAt = now();
    report(href);
  };
  const onNav = (e: Event) => {
    if (!e.isTrusted || timer !== null) return;
    timer = timers.set(fire, Math.max(0, lastAt + ROUTE_MS - now()));
  };
  const nav = (win as { navigation?: EventTarget }).navigation;
  const target: EventTarget = nav ?? win;
  const name = nav ? "navigatesuccess" : "popstate";
  target.addEventListener(name, onNav);
  win.addEventListener("hashchange", onNav);
  return () => {
    live = false;
    target.removeEventListener(name, onNav);
    win.removeEventListener("hashchange", onNav);
    if (timer !== null) timers.clear(timer);
    timer = null;
  };
}
