// The page's own `history.pushState` routes, for the app shell's back: each
// push the page makes, and each move of its own back or forward among them,
// is reported as the route's depth (`clax:route`), the number of the page's
// pushes above the entry it was loaded at. The shell's `clax:back`
// returns the page to the route at a lower depth in place, as the shell's own
// back does: the entry is replaced with that route's URL and state, and the
// page hears a `popstate`, which its router follows. Loaded with the caps
// part on the app shell's welcome, so a page outside the app loads none of
// it.

export type RouteReport = { type: "clax:route"; depth: number; hash: string };

/** One route the page pushed (or loaded at): its URL and its state. */
type Route = { href: string; state: unknown };

export type Routes = {
  /** Returns the page to the route at `depth`, below the one it shows. */
  back(depth: number): void;
};

/** Watches `win`'s pushes and pops, reporting each move with `post`. */
export function appRoutes(win: Window, post: (m: RouteReport) => void): Routes {
  const h = win.history;
  const routes: Route[] = [{ href: win.location.href, state: h.state }];
  let depth = 0;
  const report = () => post({ type: "clax:route", depth, hash: win.location.hash });
  const push = h.pushState.bind(h);
  const replace = h.replaceState.bind(h);
  h.pushState = (state: unknown, unused: string, url?: string | URL | null) => {
    push(state, unused, url);
    routes.splice(depth + 1, routes.length, { href: win.location.href, state });
    depth++;
    report();
  };
  h.replaceState = (state: unknown, unused: string, url?: string | URL | null) => {
    replace(state, unused, url);
    routes[depth] = { href: win.location.href, state };
  };
  // A move of the page's own back or forward: to the known route nearest the
  // one it showed with this URL. A fragment's own entry is none of them (the
  // bridge's `clax:hash` reports it).
  win.addEventListener("popstate", () => {
    const href = win.location.href;
    let at = -1;
    for (let d = 0; d < routes.length; d++) {
      if (routes[d].href === href && (at < 0 || Math.abs(d - depth) < Math.abs(at - depth))) at = d;
    }
    if (at < 0 || at === depth) return;
    depth = at;
    report();
  });
  return {
    back(to: number) {
      if (!Number.isInteger(to) || to < 0 || to >= depth) return;
      const r = routes[to];
      depth = to;
      replace(r.state, "", r.href);
      win.dispatchEvent(new PopStateEvent("popstate", { state: r.state }));
    },
  };
}
