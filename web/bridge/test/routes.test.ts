import { describe, expect, it, vi } from "vitest";
import { type RouteReport, appRoutes } from "../src/routes";

describe("appRoutes", () => {
  it("reports the page's pushes and its own back, and returns the page to a lower route in place on the shell's back", async () => {
    history.replaceState(null, "", "/c/a1/v/1/");
    const posted: RouteReport[] = [];
    const routes = appRoutes(window, m => posted.push(m));
    const pops: unknown[] = [];
    addEventListener("popstate", e => pops.push(e.state));

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

    // The page pushes again, then the shell goes back: in place, with the
    // route's state, and no entry.
    history.pushState({ run: 5 }, "", "/c/a1/v/1/runs/5");
    expect(posted.at(-1)).toEqual({ type: "clax:route", depth: 2, hash: "" });
    const entries = history.length;
    pops.length = 0;
    routes.back(1);
    expect(location.pathname).toBe("/c/a1/v/1/runs/3");
    expect(history.state).toEqual({ run: 3 });
    expect(pops).toEqual([{ run: 3 }]);
    expect(history.length).toBe(entries);
    routes.back(0);
    expect(location.pathname).toBe("/c/a1/v/1/");
    expect(history.state).toBeNull();
    // No report for the shell's own moves, and nothing below the first.
    expect(posted).toHaveLength(4);
    routes.back(0);
    routes.back(-1);
    expect(pops).toHaveLength(2);
  });
});
