import { afterEach, describe, expect, it, vi } from "vitest";
import { type RouteReport, appRoutes } from "../src/routes";

// Each test watches the same window's history: put its own pushState and
// replaceState back after each, so the next test's watch is the only one.
afterEach(() => {
  delete (history as Partial<History>).pushState;
  delete (history as Partial<History>).replaceState;
});

const watch = (start: string) => {
  history.replaceState(null, "", start);
  const posted: RouteReport[] = [];
  const routes = appRoutes(window, m => posted.push(m));
  const pops: unknown[] = [];
  const onPop = (e: PopStateEvent) => pops.push(e.state);
  addEventListener("popstate", onPop);
  return { posted, routes, pops, stop: () => removeEventListener("popstate", onPop) };
};

describe("appRoutes", () => {
  it("reports the page's pushes and its own back, and takes the page back through its own entries on the shell's back", async () => {
    const { posted, routes, pops, stop } = watch("/c/a1/v/1/");

    history.pushState({ run: 3 }, "", "/c/a1/v/1/runs/3");
    history.pushState({ run: 4 }, "", "/c/a1/v/1/runs/4#laps");
    expect(posted).toEqual([
      { type: "clax:route", depth: 1, hash: "" },
      { type: "clax:route", depth: 2, hash: "#laps" },
    ]);
    // A replace is the same route.
    history.replaceState({ run: 4, tab: "map" }, "", "/c/a1/v/1/runs/4?tab=map");
    expect(posted).toHaveLength(2);

    // The page's own back.
    history.back();
    await vi.waitFor(() => expect(posted).toHaveLength(3));
    expect(posted[2]).toEqual({ type: "clax:route", depth: 1, hash: "" });
    expect(location.pathname).toBe("/c/a1/v/1/runs/3");

    // The page pushes again, then the shell goes back: to the route's own
    // entry, with its state, and no entry made.
    history.pushState({ run: 5 }, "", "/c/a1/v/1/runs/5");
    expect(posted.at(-1)).toEqual({ type: "clax:route", depth: 2, hash: "" });
    const entries = history.length;
    pops.length = 0;
    routes.back(1);
    await vi.waitFor(() => expect(location.pathname).toBe("/c/a1/v/1/runs/3"));
    expect(history.state).toEqual({ run: 3 });
    await vi.waitFor(() => expect(pops).toEqual([{ run: 3 }]));
    expect(history.length).toBe(entries);

    // The page's own back after the shell's: one press reaches the route
    // below, not another entry of the route it shows.
    history.back();
    await vi.waitFor(() => expect(location.pathname).toBe("/c/a1/v/1/"));
    expect(history.state).toBeNull();
    expect(posted.at(-1)).toEqual({ type: "clax:route", depth: 0, hash: "" });

    // Nothing below the first, and no report for the shell's own moves.
    const reports = posted.length;
    routes.back(0);
    routes.back(-1);
    expect(pops).toHaveLength(2);
    expect(posted).toHaveLength(reports);
    stop();
  });

  it("goes on to a lower route the shell asks for while its traversal is under way", async () => {
    const { posted, routes, stop } = watch("/c/a2/v/1/");
    history.pushState({ n: 1 }, "", "/c/a2/v/1/1");
    history.pushState({ n: 2 }, "", "/c/a2/v/1/2");
    history.pushState({ n: 3 }, "", "/c/a2/v/1/3");
    const reports = posted.length;
    routes.back(2);
    routes.back(1);
    routes.back(0);
    await vi.waitFor(() => expect(location.pathname).toBe("/c/a2/v/1/"));
    expect(history.state).toBeNull();
    expect(posted).toHaveLength(reports);
    // The page's forward is the route above, as after its own back.
    history.forward();
    await vi.waitFor(() => expect(posted.at(-1)).toEqual({ type: "clax:route", depth: 1, hash: "" }));
    expect(location.pathname).toBe("/c/a2/v/1/1");
    stop();
  });

  it("returns the page in place once a fragment entry lies among its routes", async () => {
    const { routes, pops, stop } = watch("/c/a3/v/1/");
    history.pushState({ n: 1 }, "", "/c/a3/v/1/1");
    // A link to a fragment: an entry of the page's that is none of its routes.
    const changed = new Promise(r => addEventListener("hashchange", r, { once: true }));
    location.hash = "#notes";
    await changed;
    history.pushState({ n: 2 }, "", "/c/a3/v/1/2");
    const entries = history.length;
    pops.length = 0;
    routes.back(1);
    expect(location.pathname).toBe("/c/a3/v/1/1");
    expect(history.state).toEqual({ n: 1 });
    expect(pops).toEqual([{ n: 1 }]);
    expect(history.length).toBe(entries);
    stop();
  });
});
