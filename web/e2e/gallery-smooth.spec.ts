// The gallery loads in its final shape: nothing on it moves, the header's
// subtitle is written once, no card's title or group's heading is shown and
// then replaced, and the page is never blank: its first frame, before any
// script or data, is the header and the skeleton. Each load is played one
// response at a time (`load-stages.ts`), every request held and released
// in a fixed order with two painted frames between, so each late piece (the
// entry's script, the viewer, the list, the attention that groups it, the
// lazy module, the inbox's summary, the stream) lands as a frame of its own
// on any machine, and the Layout Instability API sees whatever it moves
// (`layout-shift.ts`). A gallery seeded with every such piece: a card that
// needs the viewer, a pinned one, tagged ones, a named viewer and an
// agent's open question. The gallery is loaded by the local owner, whose
// navigation carries no bootstrap block (the shell asks every route), and
// by a LAN viewer, whose page carries the block (the routes' answers
// already in it); and again after going to an artifact and coming back.
// The pages beside the gallery have their own spec (`pages-smooth.spec.ts`);
// both share their staged visit (`gallery-stages.ts`).
//
// The Layout Instability API sees only nodes that move, and the frame's
// header, `main` and skeleton are replaced, not moved, by the mounted page
// and its content. So each stage of a load also records where the page's
// landmarks are (`layout-shift.ts` `landmarks`): the header's (its bar,
// mark, title, search box, theme switch, links slot, and `main`) must be
// in the same place at every stage, the frame's included, and the content
// the skeleton predicts (the first row of cards in each group, the groups'
// headings, the summary, the tag bar) where the skeleton put it. A first
// visit without the block has nothing to predict the content from; a
// second visit has the shape the first left (`gallery-shape.ts`), and a
// page with the block has the block. Each phone case runs with a fine
// pointer and with touch, whose controls are taller.
import type { Page } from "@playwright/test";
import { test, expect, type Daemon, asLocalViewer, proofHeaders } from "./fixtures";
import { CONTENT_MARKS, GALLERY_APART, formatShifts, overlaps, formatTexts, layoutShifts, placesDiffer, recordLayoutShifts, shiftTotal, shownTexts, textHistory } from "./layout-shift";
import { FORMS, NAME, SUB, type Stage, type Who, hasBlock, headerHolds, loadingShown, seed, stagedVisit } from "./gallery-stages";
import { quietEval } from "./time";

const HEADINGS = ["main.gal > .grp.needs h2", "main.gal > .grp.rest h2"];

/** What is still missing of the gallery's late pieces for `who` (none once it has settled). */
const galleryMissing = (who: Who) => (page: Page) => quietEval<string[]>(page, `(() => {
  const q = s => document.querySelector(s);
  const n = s => document.querySelectorAll(s).length;
  const missing = [];
  if (!/seen as ${NAME[who]}/.test(q(${JSON.stringify(SUB)})?.textContent ?? "")) missing.push("the subtitle naming the viewer");
  if (n(".grp.needs .card-wrap:not(.new-tile)") !== 1) missing.push("Roadmap under Needs your eyes");
  if (n(".grp.rest .card-wrap:not(.new-tile)") !== 4) missing.push("four cards under Everything else");
  if (!q(".tagbar")) missing.push("the tag bar");
  if (!q(".gfoot")) missing.push("the haiku");
  if (!(window.claxStreamLive > 0)) missing.push("the live stream");
  if (${JSON.stringify(who)} === "owner") {
    if (n(".new-tile") !== 1) missing.push("the New tile");
    if (!q(".gbar .inbox-link")) missing.push("Inbox in the header");
    if (!q(".gbar .queues-link")) missing.push("Queues in the header");
    if (!q(".gbar .settings-link")) missing.push("Settings in the header");
    if (!q(".inbox-sum:not([hidden]) .qcard")) missing.push("the inbox's unread summary");
  }
  return missing;
})()`);



/** The content the skeleton predicted against where it landed: each
 * skeleton stage's places of the content's landmarks against the settled
 * page's, the cards of each group's first row only. */
function contentHolds(stages: Stage[]): string[] {
  const end = stages.at(-1)!.places;
  const out: string[] = [];
  for (const s of stages.filter(x => x.skeleton)) {
    const marks = CONTENT_MARKS.filter(m => {
      const g = /^(needs|rest) card (\d)$/.exec(m.name);
      if (!g || g[2] === "0") return true;
      return s.places[m.name]?.y === s.places[`${g[1]} card 0`]?.y;
    });
    for (const d of placesDiffer(marks, s.places, end)) out.push(`skeleton after ${s.stage ?? "(the first frame)"}: ${d}`);
    // A landmark the skeleton showed and the content does not, or the reverse.
    for (const m of ["needs heading", "rest heading", "summary", "tag bar", "needs card 0", "rest card 0"]) {
      if (!!s.places[m] !== !!end[m]) out.push(`skeleton after ${s.stage ?? "(the first frame)"}: ${m} ${s.places[m] ? "shown, but not in the content" : "not shown, but in the content"}`);
    }
  }
  return out;
}

/** Seeds an empty gallery: only the viewer's name. */
async function seedEmpty(page: Page, d: Daemon, who: Who): Promise<void> {
  if (who === "viewer") await asLocalViewer(page.context(), d);
  const side = await page.context().newPage();
  await side.goto(`${d.base}/healthz`);
  await side.evaluate(async ([proof, name]) => {
    const h = { ...proof, "content-type": "application/json" };
    await fetch("/api/viewers/me", { headers: proof });
    await fetch("/api/viewers/me", { method: "PUT", headers: h, body: JSON.stringify({ display_name: name }) });
  }, [who === "owner" ? proofHeaders(d.base) : {}, NAME[who]] as const);
  await side.close();
}
const emptyMissing = (who: Who) => (page: Page) => quietEval<string[]>(page, `(() => {
  const missing = [];
  if (!document.querySelector("main.gal > .empty-gallery")) missing.push("the empty gallery");
  if (!/seen as ${NAME[who]}/.test(document.querySelector(${JSON.stringify(SUB)})?.textContent ?? "")) missing.push("the subtitle naming the viewer");
  return missing;
})()`);

for (const [form, use] of FORMS) {
  test.describe(`on a ${form}`, () => {
    test.use(use);

    for (const [who, again] of [["owner", false], ["owner", true], ["viewer", false]] as const) {
      const how = who === "owner" ? `the owner, no bootstrap block${again ? ", a second visit" : ""}` : "a LAN viewer, with the block";
      test(`the gallery loads without moving anything, and writes its subtitle, titles and headings once (${how})`, async ({ page, freshDaemon: d }) => {
        const ids = await seed(page, d, who);
        const titles = ids.map(id => `a.card[href="/a/${id}"] h3`);
        const { order, stages } = await stagedVisit(page, d, "/", galleryMissing(who), [...HEADINGS, ...titles], again);
        expect(await hasBlock(page), "the page's bootstrap block").toBe(who === "viewer");
        const shifts = await layoutShifts(page);
        const texts = await textHistory(page);
        const released = `released in order:\n  ${order.join("\n  ")}`;
        expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\n${released}`).toBe(0);
        expect.soft(await overlaps(page, GALLERY_APART), "boxes that overlap, or leave the header, at the settled page").toEqual([]);
        expect.soft(shownTexts(texts[SUB]), formatTexts(SUB, texts[SUB])).toHaveLength(1);
        for (const sel of [...HEADINGS, ...titles]) expect.soft(shownTexts(texts[sel]), formatTexts(sel, texts[sel])).toHaveLength(1);
        expect.soft(headerHolds(stages, again), released).toEqual([]);
        expect.soft(loadingShown(stages), `loading lines shown\n${released}`).toEqual([]);
        // The content in the skeleton's place: predicted by a kept shape or the block.
        if (again || who === "viewer") expect.soft(contentHolds(stages), released).toEqual([]);
      });
    }


    for (const [who, again] of [["owner", true], ["viewer", false], ["viewer", true]] as const) {
      test(`an empty gallery never shows a skeleton card (${who === "owner" ? "the owner" : "a LAN viewer, with the block"}${again ? ", a second visit" : ""})`, async ({ page, freshDaemon: d }) => {
        await seedEmpty(page, d, who);
        const { stages } = await stagedVisit(page, d, "/", emptyMissing(who), [], again);
        expect.soft(Math.max(...stages.map(s => s.skeletonCards)), "skeleton cards seen at any stage").toBe(0);
        expect.soft(headerHolds(stages, again)).toEqual([]);
        expect.soft(shiftTotal(await layoutShifts(page))).toBe(0);
        expect.soft(await overlaps(page, GALLERY_APART), "boxes that overlap, or leave the header, at the settled page").toEqual([]);
      });
    }

    test("the gallery comes back from an artifact without moving anything, reading none of its block", async ({ page, freshDaemon: d }) => {
      const [roadmap] = await seed(page, d, "viewer");
      await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
      await recordLayoutShifts(page);
      await page.goto(`${d.base}/`);
      const settled = galleryMissing("viewer");
      await expect.poll(() => settled(page)).toEqual([]);
      await page.locator(`a.card[href="/a/${roadmap}"]`).click();
      await page.waitForURL(`**/a/${roadmap}**`);
      // Whatever comes back from the cache, the gallery asks its routes itself.
      const asked: string[] = [];
      page.on("request", r => { const u = new URL(r.url()); if (u.pathname.startsWith("/api/")) asked.push(u.pathname); });
      await page.goBack();
      await page.waitForURL(`${d.base}/`);
      await expect.poll(() => settled(page)).toEqual([]);
      // A page restored whole from the back/forward cache loads nothing; one loaded again asks.
      if (await quietEval<string>(page, `performance.getEntriesByType("navigation")[0].type`) === "back_forward") {
        for (const route of ["/api/viewers/me", "/api/artifacts", "/api/viewers/me/attention"]) expect(asked, route).toContain(route);
      }
      const shifts = await layoutShifts(page);
      expect(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
      expect.soft(await overlaps(page, GALLERY_APART), "boxes that overlap, or leave the header, at the settled page").toEqual([]);
    });
  });
}

// A document painted while it streams in (the browser paints what it has
// parsed when the network pauses) shows none of `#app` until the parser
// is past it (gallery.css, the inline `clax-framed` script after it): the
// first frame painted with any of the frame is the whole frame, never a
// header bar with nothing in it. The page's own HTML is served cut at
// points inside the frame, each a document the parser has finished, and
// whole, with every script held.
test("a gallery document cut inside its frame shows none of it, and whole shows all of it", async ({ page, freshDaemon: d }) => {
  const html = await (await fetch(`${d.base}/`, { headers: { accept: "text/html" } })).text();
  const app = html.indexOf('<div id="app">');
  const framed = html.indexOf('<script id="clax-framed">');
  expect(app, "the frame").toBeGreaterThan(0);
  expect(framed, "the script after the frame").toBeGreaterThan(app);
  const cuts = [
    html.indexOf("<header", app) + "<header".length + 20,
    html.indexOf("</svg>", app) + "</svg>".length,
    html.indexOf('<input type="search"', app),
    html.indexOf('<main class="gal"', app) + 400,
    html.indexOf("</main>", app),
    framed,
  ];
  await page.route("**/*", r => (r.request().isNavigationRequest() ? r.fallback() : r.abort()));
  const shows = async (body: string) => {
    await page.route(`${d.base}/`, r => r.fulfill({ status: 200, contentType: "text/html", body }), { times: 1 });
    await page.goto(`${d.base}/`);
    return quietEval<string>(page, `getComputedStyle(document.getElementById("app")).visibility`);
  };
  for (const at of cuts) expect.soft(await shows(html.slice(0, at)), `cut at ${at}: ${JSON.stringify(html.slice(at - 40, at))}`).toBe("hidden");
  expect(await shows(html)).toBe("visible");
  expect(await quietEval<boolean>(page, `(() => { const r = document.querySelector("#app > .gbar .home .mark").getBoundingClientRect(); return r.width > 0 && r.height > 0; })()`), "the mark, painted").toBe(true);
});
