// The page's own `history.pushState` routes, for the app shell's back: each
// push the page makes, and each move of its own back or forward among them,
// is reported as the route's depth (`clax:route`), the number of the page's
// pushes above the entry it was loaded at. The shell's `clax:back` takes the
// page to the route at a lower depth: back through its own entries
// (`history.go`), so its own back and forward go on from there, while the
// page's entries since it loaded are its routes' alone. Once a fragment
// entry lies among them (a `hashchange` to no route), the count of entries
// is unknown, and the move is made in place, as the shell's own back is: the
// entry is replaced with that route's URL and state, and the page hears a
// `popstate`, which its router follows. Loaded with the caps part on the app
// shell's welcome, so a page outside the app loads none of it.

export type RouteReport = { type: "clax:route"; depth: number; hash: string };

/** One route the page pushed (or loaded at): its URL and its state. */
type Route = { href: string; state: unknown };

export type Routes = {
  /** Takes the page to the route at `depth`, below the one it shows. */
  back(depth: number): void;
  /** Reports the route the page shows again: the page greeted again, and a
   * report the shell got while it awaited that hello was not taken. */
  report(): void;
};

/** Watches `win`'s pushes and pops, reporting each move with `post`. */
export function appRoutes(win: Window, post: (m: RouteReport) => void): Routes {
  const h = win.history;
  const routes: Route[] = [{ href: win.location.href, state: h.state }];
  // The route the frame's entry shows.
  let depth = 0;
  // Whether the page's entries since it loaded are its routes' alone.
  let plain = true;
  // The shell's back under way: the route its `history.go` lands on, and
  // the one the shell last asked for.
  let landing: number | null = null;
  let target = 0;
  const report = () => post({ type: "clax:route", depth, hash: win.location.hash });
  const push = h.pushState.bind(h);
  const replace = h.replaceState.bind(h);
  const go = () => { landing = target; h.go(target - depth); };
  h.pushState = (state: unknown, unused: string, url?: string | URL | null) => {
    push(state, unused, url);
    routes.splice(depth + 1, routes.length, { href: win.location.href, state });
    depth++;
    landing = null;
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
    if (landing !== null) {
      depth = landing;
      landing = null;
      if (target < depth) go();
      return;
    }
    const href = win.location.href;
    let at = -1;
    for (let d = 0; d < routes.length; d++) {
      if (routes[d].href === href && (at < 0 || Math.abs(d - depth) < Math.abs(at - depth))) at = d;
    }
    if (at < 0 || at === depth) return;
    depth = at;
    report();
  });
  win.addEventListener("hashchange", () => {
    if (win.location.href !== routes[depth].href) plain = false;
  });
  return {
    report,
    back(to: number) {
      if (!Number.isInteger(to) || to < 0 || to >= (landing ?? depth)) return;
      target = to;
      if (landing !== null) return;
      if (plain) return go();
      const r = routes[to];
      depth = to;
      replace(r.state, "", r.href);
      win.dispatchEvent(new PopStateEvent("popstate", { state: r.state }));
    },
  };
}
