// The pages beside the gallery load in their final shape: the inbox, the
// Tags page, the report queues (the list, a queue's view, a report's),
// Settings (the overview and every screen this browser may open, ClaxBot's
// with it on), and the pages that moved into Settings. Each is loaded one
// response at a time (`load-stages.ts`, through `gallery-stages.ts`'s
// staged visit), and at every stage: nothing moves (the Layout
// Instability API), the header is where the first frame put it, no line
// says the page is loading (a page is shown once it has its first data,
// in the skeleton's place), and once the page shows, nothing in it grows:
// `main`'s content ends where the settled page's does. On a second visit
// the skeleton is the page's kept shape (`gallery-shape.ts`), so the
// skeleton ends there too. The local owner loads each, whose navigation
// carries no bootstrap block, and a LAN viewer the pages it may open, with
// the block. A page that moved into Settings is sent there by the daemon,
// before any of the shell loads. Moving inside a page (Settings from one
// screen to another, the queues from the list to a queue's view) keeps
// the page's frame and shows the next view once it has its first data:
// no stage shows a loading line.
import type { Page } from "@playwright/test";
import { test, expect, type Daemon, api, startDaemon } from "./fixtures";
import { GALLERY_APART, HEADER_MARKS, type Landmark, formatShifts, landmarks, layoutShifts, overlaps, placesDiffer, recordLayoutShifts, shiftTotal } from "./layout-shift";
import { FORMS, NAME, ORDER, SUB, type Stage, type Who, hasBlock, headerHolds, heightHolds, loadingShown, probeMain, seed, stagedVisit } from "./gallery-stages";
import { stageLoad } from "./load-stages";
import { quietEval } from "./time";

/** What is still missing once a page beside the gallery has settled for
 * `who`: the page's own `here` (a JavaScript expression answering true or
 * what is missing), the page in the skeleton's place, the header's
 * subtitle, and the live stream (unless `stream` is false: a browser
 * that is not a member's opens none on `/inbox` or `/settings`). */
const pageMissing = (here: string, who: Who = "owner", stream = true) => (page: Page) => quietEval<string[]>(page, `(() => {
  const missing = [];
  const here = ${here};
  if (here !== true) missing.push(here);
  if (document.querySelector("main .gpage.pending, main .gskel")) missing.push("the page in place of the skeleton");
  if (!/seen as ${NAME[who]}/.test(document.querySelector(${JSON.stringify(SUB)})?.textContent ?? "")) missing.push("the subtitle naming the viewer");
  if (${stream} && !(window.claxStreamLive > 0)) missing.push("the live stream");
  return missing;
})()`);

/** A Settings screen with its data: its pane shown, and none on its way. */
const screenHere = (id: string) => `location.pathname === "/settings/${id}" && !!document.querySelector("main .stg .stg-body > .stg-pane") && !document.querySelector("main .stg .stg-held") || "the ${id} screen with its data"`;

/** The owner's pages, with what each shows once it has its first data.
 * `qid` is the seeded queue's ID. */
const OWNER_PAGES = (qid: string): Record<string, string> => ({
  "/inbox": `!!document.querySelector("main .inbox .qcard") || "the inbox's question card"`,
  "/tags": `document.querySelectorAll(".tagsp .trow").length === 2 || "the two tags' rows"`,
  "/queues": `(!!document.querySelector("main .queues .qcard .counts") && !/Counting/.test(document.querySelector("main .queues .qcard .counts").textContent)) || "the queue with its counts"`,
  [`/queues/${qid}`]: `!!document.querySelector("main .qview #qv-list .qnone") || "the queue's empty list"`,
  [`/queues/${qid}/t_missing`]: `!!document.querySelector("main .rview [role=alert]") || "the report's refusal"`,
  "/settings": `!!document.querySelector("main .stg .stg-over") || "Settings' screens"`,
  ...Object.fromEntries(["agents", "confirmations", "people", "config", "remotes", "audit"].map(id => [`/settings/${id}`, screenHere(id)])),
});

/** The screens whose content grows with every visit (the audit log
 * records each; listing the provider's models, which no provider answers
 * here, adds a failure): a kept shape is the last visit's, never this one's,
 * so their skeletons are not held to the settled height. */
const GROWS = new Set(["/settings/audit", "/settings/provider"]);

/** ClaxBot's screens, on a daemon with it on. */
const BOT_SCREENS = ["claxbot", "roles", "provider", "web", "pricing", "usage"];

/** The pages a LAN viewer (no inbox, not a member) may open, with the block. */
const VIEWER_PAGES: Record<string, string> = {
  "/inbox": `!!document.querySelector("main.gal > p.empty") || "the line saying the browser has no inbox"`,
  "/tags": `document.querySelectorAll(".tagsp .trow").length === 2 || "the two tags' rows"`,
  "/settings": `!!document.querySelector("main .stg .stg-over") || "Settings' line for a browser that is not a member's"`,
};

/** The pages that moved into Settings, and where each is now. */
const MOVED: Record<string, string> = {
  "/people": "/settings/people",
  "/admin": "/settings/roles",
  "/admin?tab=web": "/settings/web",
  "/me": "/settings/claxbot",
  "/audit": "/settings/audit",
};

/** Makes the owner's queue, with no reports; answers its ID. */
async function seedQueue(d: Daemon): Promise<string> {
  const r = await api(d.base, d.token, "/api/queues", { method: "POST", body: JSON.stringify({ name: "Site reports", origins: ["https://example.test"], reporters_see: "own" }) }) as { queue: { id: string } };
  return r.queue.id;
}

/** Runs `f` on a page of its own in `page`'s context: each staged load
 * starts with none of another's init scripts or held routes. */
async function onOwnPage(page: Page, f: (p: Page) => Promise<void>): Promise<void> {
  const p = await page.context().newPage();
  try { await f(p); } finally { await p.close(); }
}

/** What sticks out past the window's right edge at the settled page (the
 * page then scrolls sideways, and on a phone the browser widens its
 * layout to fit, so the window is the viewport the test set): each such
 * box outside a box that scrolls or clips it. */
const sideways = (page: Page) => quietEval<string[]>(page, `(() => {
  const w = Math.min(document.documentElement.clientWidth, ${page.viewportSize()?.width ?? 1e9});
  const out = [];
  if (document.documentElement.scrollWidth > w) out.push("the page is " + document.documentElement.scrollWidth + " wide in a window of " + w);
  const clipped = e => { for (let n = e.parentElement; n && n !== document.body; n = n.parentElement) { const o = getComputedStyle(n).overflowX; if (o !== "visible") return true; } return false; };
  for (const e of document.querySelectorAll("#app *")) {
    const r = e.getBoundingClientRect();
    if (r.width && r.right > w + 0.5 && !clipped(e)) out.push(e.localName + (e.className && typeof e.className === "string" ? "." + e.className.trim().split(/\\s+/).join(".") : "") + " ends at " + Math.round(r.right) + " of " + w);
  }
  return out.slice(0, 8);
})()`);

/** Every check of a staged load of a page beside the gallery. */
async function holds(page: Page, order: string[], stages: Stage[], again: boolean): Promise<void> {
  const shifts = await layoutShifts(page);
  const released = `released in order:\n  ${order.join("\n  ")}`;
  expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\n${released}`).toBe(0);
  expect.soft(await overlaps(page, GALLERY_APART), "boxes that overlap, or leave the header, at the settled page").toEqual([]);
  expect.soft(headerHolds(stages, again), released).toEqual([]);
  expect.soft(loadingShown(stages), `loading lines shown\n${released}`).toEqual([]);
  expect.soft(heightHolds(stages, again), released).toEqual([]);
  expect.soft(await sideways(page), "boxes past the window's right edge at the settled page").toEqual([]);
}

/** ClaxBot's daemon: its provider an address nothing answers, never asked while a page loads. */
const botDaemon = () => startDaemon({
  config: `[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n[bot]\nbase_url = "http://127.0.0.1:9"\napi_key_env = "CLAX_E2E_BOT_KEY"\n`,
  env: { CLAX_E2E_BOT_KEY: "sk-ant-e2e-not-a-real-key-0000" },
});

/** Moves inside the page with `act`, staged from then on, until `missing`
 * answers nothing; answers the stages, each with the header's landmarks
 * and the place of `frame` (what the move keeps). */
async function stagedMove(page: Page, act: () => Promise<void>, missing: (p: Page) => Promise<string[]>, frame: string): Promise<{ order: string[]; stages: (Stage & { frame: string | null })[] }> {
  const load = await stageLoad(page, ORDER);
  const stages: (Stage & { frame: string | null })[] = [];
  const each = async (stage: string | null) => {
    const places = await landmarks(page, [...HEADER_MARKS, { name: "frame", skeleton: null, content: frame }]);
    const { bottom, loading } = await probeMain(page);
    stages.push({ stage, skeleton: false, cards: 0, skeletonCards: 0, places, bottom, loading, frame: JSON.stringify(places.frame) });
  };
  await act();
  let last: string[] = [];
  const order = await load.drive(async () => (last = await missing(page)).length === 0, { each }).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${last.join(", ")}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
  await each("settled");
  await page.unrouteAll({ behavior: "ignoreErrors" });
  return { order, stages };
}

for (const [form, use] of FORMS) {
  test.describe(`on a ${form}`, () => {
    test.use(use);

    for (const again of [false, true]) {
      const how = again ? " (a second visit)" : "";
      test(`each of the owner's pages beside the gallery loads without moving anything or saying it is loading${how}`, async ({ page, freshDaemon: d }) => {
        await seed(page, d);
        const pages = OWNER_PAGES(await seedQueue(d));
        for (const [path, here] of Object.entries(pages)) {
          await test.step(path, () => onOwnPage(page, async p => {
            const { order, stages } = await stagedVisit(p, d, path, pageMissing(here), [], again);
            expect.soft(await hasBlock(p), "the page's bootstrap block").toBe(false);
            await holds(p, order, stages, again && !GROWS.has(path));
          }));
        }
      });

      test(`ClaxBot's Settings screens load without moving anything or saying they are loading${how}`, async ({ page }) => {
        const d = await botDaemon();
        try {
          await seed(page, d);
          for (const id of BOT_SCREENS) {
            await test.step(id, () => onOwnPage(page, async p => {
              const { order, stages } = await stagedVisit(p, d, `/settings/${id}`, pageMissing(screenHere(id)), [], again);
              await holds(p, order, stages, again && !GROWS.has(`/settings/${id}`));
            }));
          }
        } finally { await d.stop(); }
      });

      test(`the pages a LAN viewer opens load with the block, without moving anything or saying they are loading${how}`, async ({ page, freshDaemon: d }) => {
        await seed(page, d, "viewer");
        for (const [path, here] of Object.entries(VIEWER_PAGES)) {
          await test.step(path, () => onOwnPage(page, async p => {
            const { order, stages } = await stagedVisit(p, d, path, pageMissing(here, "viewer", path === "/tags"), [], again);
            expect.soft(await hasBlock(p), "the page's bootstrap block").toBe(true);
            await holds(p, order, stages, again);
          }));
        }
      });
    }

    test("a page that moved into Settings is sent to its screen by the daemon, before the shell loads", async ({ page }) => {
      const d = await botDaemon();
      try {
        await seed(page, d);
        for (const [from, to] of Object.entries(MOVED)) {
          await test.step(from, () => onOwnPage(page, async p => {
            const id = to.replace("/settings/", "");
            const { order, stages } = await stagedVisit(p, d, from, pageMissing(screenHere(id)));
            expect.soft(new URL(p.url()).pathname, "where the page is").toBe(to);
            await holds(p, order, stages, false);
          }));
          // The daemon's answer for the old address is the redirect itself.
          const r = await fetch(`${d.base}${from}`, { redirect: "manual", headers: { accept: "text/html" } });
          expect.soft(r.status, from).toBe(308);
          expect.soft(r.headers.get("location"), from).toBe(to);
        }
      } finally { await d.stop(); }
    });

    test("moving between Settings screens keeps the navigation and shows the next screen once it has its data", async ({ page, freshDaemon: d }) => {
      await seed(page, d);
      await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
      await recordLayoutShifts(page);
      await page.goto(`${d.base}/settings/agents`);
      await expect.poll(() => pageMissing(screenHere("agents"))(page)).toEqual([]);
      for (const [name, id] of [["Audit log", "audit"], ["People", "people"], ["Configuration", "config"], ["Agents & machines", "agents"]] as const) {
        await test.step(id, async () => {
          const nav = page.locator(".stg-nav");
          const { order, stages } = await stagedMove(page, () => (form === "desktop" ? nav.getByRole("link", { name, exact: true }).click() : page.evaluate(h => { history.pushState(null, "", h); dispatchEvent(new PopStateEvent("popstate")); }, `/settings/${id}`)), pageMissing(screenHere(id)), form === "desktop" ? "main .stg-nav" : "main .stg-back");
          const released = `released in order:\n  ${order.join("\n  ")}`;
          expect.soft(loadingShown(stages), `loading lines shown\n${released}`).toEqual([]);
          expect.soft([...new Set(stages.map(s => s.frame))], `the page's frame\n${released}`).toHaveLength(1);
          // `main` grows or shrinks with the screen shown; it starts where it did.
          const marks: Landmark[] = HEADER_MARKS.map(m => (m.name === "main" ? { ...m, keys: ["x", "y", "w"] } : m));
          expect.soft(stages.flatMap(s => placesDiffer(marks, stages[0].places, s.places).map(x => `after ${s.stage}: ${x}`)), released).toEqual([]);
        });
      }
      expect.soft(shiftTotal(await layoutShifts(page)), formatShifts(await layoutShifts(page))).toBe(0);
    });

    test("moving from the queues to a queue's view and back shows each once it has its data", async ({ page, freshDaemon: d }) => {
      await seed(page, d);
      const qid = await seedQueue(d);
      const pages = OWNER_PAGES(qid);
      await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
      await page.goto(`${d.base}/queues`);
      await expect.poll(() => pageMissing(pages["/queues"])(page)).toEqual([]);
      const there = await stagedMove(page, () => page.getByRole("link", { name: "Site reports" }).click(), pageMissing(pages[`/queues/${qid}`]), "#app > .gbar");
      expect.soft(loadingShown(there.stages), `loading lines shown on the way to the queue\nreleased in order:\n  ${there.order.join("\n  ")}`).toEqual([]);
      const back = await stagedMove(page, () => page.getByRole("link", { name: "Queues", exact: true }).first().click(), pageMissing(pages["/queues"]), "#app > .gbar");
      expect.soft(loadingShown(back.stages), `loading lines shown on the way back\nreleased in order:\n  ${back.order.join("\n  ")}`).toEqual([]);
    });
  });
}
