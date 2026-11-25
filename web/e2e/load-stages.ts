// A page load played one response at a time, in a fixed order, each painted
// before the next arrives, so whatever a late response moves or rewrites is
// on screen as a frame of its own on any machine (`layout-shift.ts` records
// it). Every request of the page is held at a gate (by default all but the
// main frame's navigation); the driver releases the held request that comes
// first in the order the test gives, waits for its response to finish and
// for the page to paint twice and fall quiet, and picks again, until nothing
// is held and the test's condition holds. It releases only what the page
// has asked for, so a later version of the page that no longer makes some
// request (its data in the HTML) runs the same test to the end.
//
// The order is a list of rules; a request takes the first rule it matches,
// and requests matching no rule come after all of them. Ties go by method
// and URL, then by arrival. A response that never finishes (an event
// stream) counts as done once its headers arrive.
import type { Page, Request, Route } from "@playwright/test";
import { expect } from "./fixtures";
import { markStage } from "./layout-shift";
import { settle } from "./time";

export type StageRule = { name: string; match: (url: URL, req: Request) => boolean };

type Held = { req: Request; route: Route; key: string; rule: string | null; rank: number; seq: number; release: () => void };

export type StagedLoad = {
  /** Releases held requests in order until none is held and `done` answers
   * true with the page quiet; answers the labels released, in order.
   * `first` runs once, when the page has painted and fallen quiet before
   * the first release: what the document alone shows. `each` runs every
   * time the page has fallen quiet, the first time included, with the
   * label released last (null before the first). */
  drive(done: () => Promise<boolean>, opts?: { first?: () => Promise<void>; each?: (stage: string | null) => Promise<void> }): Promise<string[]>;
  /** The labels released so far. */
  released: string[];
};

/** Holds `page`'s requests at gates from now on (call before `goto`, which
 * must not wait past `commit`: the load event waits for released requests). */
export async function stageLoad(page: Page, order: StageRule[], opts: { hold?: (req: Request) => boolean } = {}): Promise<StagedLoad> {
  const hold = opts.hold ?? (req => !(req.isNavigationRequest() && req.frame() === page.mainFrame()));
  const held: Held[] = [];
  const released: string[] = [];
  // Released requests whose response has not finished.
  const open = new Map<Request, string>();
  let seq = 0;
  let arrivals = 0;
  const finish = (req: Request) => { open.delete(req); };
  // A shared or service worker's script reaches the route, but its end is
  // reported to the worker, not the page: it is released and not waited on.
  const untracked = (req: Request) => /^(sharedworker|serviceworker)$/.test(req.headers()["sec-fetch-dest"] ?? "");
  page.on("requestfinished", finish);
  page.on("requestfailed", finish);
  page.on("response", res => {
    if ((res.headers()["content-type"] ?? "").startsWith("text/event-stream")) open.delete(res.request());
  });
  await page.route("**/*", async route => {
    const req = route.request();
    if (!hold(req)) return route.fallback();
    const url = new URL(req.url());
    const at = order.findIndex(r => r.match(url, req));
    let release = () => {};
    const gate = new Promise<void>(r => { release = r; });
    held.push({ req, route, key: `${req.method()} ${url.href}`, rule: at < 0 ? null : order[at].name, rank: at < 0 ? order.length : at, seq: seq++, release });
    arrivals++;
    await gate;
    await route.fallback().catch(() => { open.delete(req); });
  });
  const pick = (): Held | undefined => {
    held.sort((a, b) => a.rank - b.rank || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0) || a.seq - b.seq);
    return held.shift();
  };
  // A request's label: its rule, method and path (the full URL off the page's origin).
  const labelOf = (h: Held) => {
    const url = new URL(h.req.url());
    const path = url.origin === new URL(page.url()).origin ? `${url.pathname}${url.search}` : url.href;
    return `${h.rule ? `[${h.rule}] ` : ""}${h.req.method()} ${path}`;
  };
  // The page is quiet once every released response has finished and two
  // painted frames in a row asked for nothing new.
  const finished = () => expect.poll(() => [...open.values()], { message: "released responses to finish" }).toEqual([]);
  const quiet = async () => {
    await finished();
    for (;;) {
      const before = arrivals;
      await settle(page);
      await settle(page);
      if (arrivals === before && open.size === 0) return;
      await finished();
    }
  };
  const drive = async (done: () => Promise<boolean>, at: { first?: () => Promise<void>; each?: (stage: string | null) => Promise<void> } = {}) => {
    let first = at.first;
    for (;;) {
      await quiet();
      await at.each?.(released.at(-1) ?? null);
      if (first) { const f = first; first = undefined; await f(); }
      const next = pick();
      if (next) {
        const label = labelOf(next);
        await markStage(page, label);
        released.push(label);
        if (!untracked(next.req)) open.set(next.req, label);
        next.release();
        continue;
      }
      if (await done()) return released;
      // Nothing held and not done yet: wait for the page to ask for more.
      await expect.poll(async () => held.length > 0 || await done(), { message: `the page to settle; released so far:\n  ${released.join("\n  ")}` }).toBe(true);
    }
  };
  return { drive, released };
}
