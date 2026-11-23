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
// already in it); the pages beside it by the owner; and the gallery again
// after going to an artifact and coming back.
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
import { test, expect, type Daemon, api, asLocalViewer, proofHeaders, publishAs, publishNext, registerSession } from "./fixtures";
import { ask } from "./questions-helpers";
import { CONTENT_MARKS, GALLERY_APART, HEADER_MARKS, type Landmark, type Place, formatShifts, overlaps, formatTexts, landmarks, layoutShifts, placesDiffer, recordLayoutShifts, recordTexts, shiftTotal, shownTexts, textHistory } from "./layout-shift";
import { type StageRule, stageLoad } from "./load-stages";
import { quietEval } from "./time";

const PAGE = "<main><h2>Quarterly goals</h2></main>";
const SUB = ".gbar .sub";
const HEADINGS = ["main.gal > .grp.needs h2", "main.gal > .grp.rest h2"];

/** The order held requests are released in, when several wait at once: the
 * page's own code and styles, then its data in the order the gallery reads
 * it, then the rest (the event stream last). */
const ORDER: StageRule[] = [
  { name: "css", match: (_u, r) => r.resourceType() === "stylesheet" },
  { name: "js", match: (_u, r) => r.resourceType() === "script" },
  { name: "font", match: (_u, r) => r.resourceType() === "font" },
  { name: "token", match: u => u.pathname === "/api/token" },
  { name: "viewer", match: u => u.pathname === "/api/viewers/me" },
  { name: "hosted", match: u => u.pathname === "/api/hosted/me" },
  { name: "list", match: u => u.pathname === "/api/artifacts" },
  { name: "attention", match: u => u.pathname === "/api/viewers/me/attention" },
  { name: "feedback", match: u => u.pathname.startsWith("/_app/feedback/") },
  { name: "api", match: u => u.pathname.startsWith("/api/") && !u.pathname.startsWith("/api/stream") },
  { name: "stream", match: u => u.pathname.startsWith("/api/stream") },
];

/** Who loads the page: the local owner (no bootstrap block) or a LAN viewer (the block). */
type Who = "owner" | "viewer";
const NAME: Record<Who, string> = { owner: "Alex", viewer: "Bea" };

/** Seeds `d` with a gallery of five artifacts by an agent: "Roadmap" needs
 * the viewer (a thread of theirs addressed in a version they have not
 * seen), "Budget" is pinned, "Design notes" and "Launch checklist" carry
 * tags, and the agent asks the owner a question about the roadmap (the
 * inbox's unread summary). The viewer is named (Alex for the owner, Bea for
 * a LAN viewer, which the context becomes), so the subtitle reads "… · seen
 * as <name>". The context's own requests seed it, from a page that runs
 * none of the shell. Answers the artifacts' IDs. */
async function seed(page: Page, d: Daemon, who: Who = "owner"): Promise<string[]> {
  if (who === "viewer") await asLocalViewer(page.context(), d);
  const s = await registerSession(d.base, d.token, "claude", "smooth");
  const pub = async (title: string) => (await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE })).artifact.id;
  const roadmap = await pub("Roadmap");
  const budget = await pub("Budget");
  const design = await pub("Design notes");
  const launch = await pub("Launch checklist");
  const offsite = await pub("Team offsite");
  await api(d.base, d.token, `/api/artifacts/${budget}`, { method: "PATCH", body: JSON.stringify({ pinned: true }) });
  for (const [name, colour] of [["design", "blue"], ["q4", "green"]]) await api(d.base, d.token, "/api/tags", { method: "POST", body: JSON.stringify({ name, colour }) });
  await api(d.base, d.token, `/api/artifacts/${design}/tags`, { method: "PUT", body: JSON.stringify({ tags: ["design", "q4"] }) });
  await api(d.base, d.token, `/api/artifacts/${launch}/tags`, { method: "PUT", body: JSON.stringify({ tags: ["q4"] }) });
  const side = await page.context().newPage();
  await side.goto(`${d.base}/healthz`);
  const tid = await side.evaluate(async ([id, proof, name]) => {
    const h = { ...proof, "content-type": "application/json" };
    await fetch("/api/viewers/me", { headers: proof });
    await fetch("/api/viewers/me", { method: "PUT", headers: h, body: JSON.stringify({ display_name: name }) });
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", "Two columns, please"); f.set("version", "1");
    const t = (await (await fetch(`/api/artifacts/${id}/threads`, { method: "POST", headers: proof, body: f })).json()).thread.id as string;
    await fetch("/api/viewers/me/seen", { method: "PUT", headers: h, body: JSON.stringify({ artifact_id: id, version: 1 }) });
    return t;
  }, [roadmap, who === "owner" ? proofHeaders(d.base) : {}, NAME[who]] as const);
  await side.close();
  await publishNext(d.base, d.token, s.id, roadmap, 1, { addresses: [tid] });
  await ask(d.base, d.token, s.id, { source: "ask", artifact_id: roadmap, questions: [{ question: "Which quarter should the roadmap lead with?", header: "Quarter", options: [{ label: "Q3" }, { label: "Q4" }] }] });
  return [roadmap, budget, design, launch, offsite];
}

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

/** What each page beside the gallery shows once it has its first data, for the owner. */
const PAGES: Record<string, string> = {
  "/tags": `document.querySelectorAll(".tagsp .trow").length === 2 || "the two tags' rows"`,
  "/inbox": `!!document.querySelector("main .inbox .qcard") || "the inbox's question card"`,
  "/queues": `!!document.querySelector("main .queues .qh") || "the queues' heading"`,
  "/settings": `!!document.querySelector("main .stg .stg-over") || "Settings' screens"`,
  "/settings/account": `!!document.querySelector("main .stg .stg-body .ppl") && !document.querySelector("main .stg [role=status]") || "the Account screen with its data"`,
};
const pageMissing = (path: string) => (page: Page) => quietEval<string[]>(page, `(() => {
  const missing = [];
  const here = ${PAGES[path]};
  if (here !== true) missing.push(here);
  if (document.querySelector("main .gpage.pending, main .gskel")) missing.push("the page in place of the skeleton");
  if (!document.querySelector(".gbar .settings-link")) missing.push("Settings in the header");
  if (!/seen as Alex/.test(document.querySelector(${JSON.stringify(SUB)})?.textContent ?? "")) missing.push("the subtitle naming the viewer");
  if (!(window.claxStreamLive > 0)) missing.push("the live stream");
  return missing;
})()`);

/** The landmarks recorded at each stage of a load. */
type Stage = { stage: string | null; skeleton: boolean; cards: number; skeletonCards: number; places: Record<string, Place | null> };

/** Whether any skeleton card shows. */
const skeletonCards = (page: Page) => quietEval<number>(page, `[...document.querySelectorAll("main.gal .gskel .sk-c")].filter(e => e.getBoundingClientRect().height > 0).length`);

/** Loads `path` staged, until `missing` answers nothing; answers the release
 * order and the landmarks at each stage. Before the first release, the
 * page must show the header and the skeleton: the document alone, its
 * scripts held. `again`: the page was loaded once before, unstaged, so the
 * browser keeps its shape. */
async function stagedVisit(page: Page, d: Daemon, path: string, missing: (p: Page) => Promise<string[]>, texts: string[] = [], again = false): Promise<{ order: string[]; stages: Stage[] }> {
  // The page holds the event stream itself, as where shared workers are
  // missing (shell/src/stream.ts): a shared worker's requests reach no event
  // of the page's, so the stream's would land at no fixed stage.
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  if (again) {
    await page.goto(`${d.base}${path}`);
    await expect.poll(() => missing(page)).toEqual([]);
    // Its shape kept once its first render is painted (view/gallery-shape.ts).
    const top = path.replace(/^\/(queues|settings)\/.+/, "/$1");
    await expect.poll(() => quietEval<boolean>(page, `(() => { try { return ${JSON.stringify(top)} in JSON.parse(localStorage.getItem("clax.shape") ?? "{}"); } catch { return false; } })()`)).toBe(true);
    // Off the page, so none of its requests reach the staged load.
    await page.goto("about:blank");
  }
  await recordLayoutShifts(page);
  await recordTexts(page, [SUB, ...texts]);
  const load = await stageLoad(page, ORDER);
  await page.goto(`${d.base}${path}`, { waitUntil: "commit" });
  let last: string[] = [];
  const stages: Stage[] = [];
  const first = async () => {
    const shows = await quietEval<Record<string, boolean>>(page, `(() => {
      const seen = s => { const e = document.querySelector(s); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== "hidden"; };
      return { header: seen(".gbar") && seen(".gbar .theme-switch") && seen(".gbar .home"), skeleton: seen("main.gal .gskel"), busy: document.querySelector("main.gal")?.getAttribute("aria-busy") === "true" };
    })()`);
    expect(shows, "the first frame, before any script ran: the header and the skeleton, main busy").toEqual({ header: true, skeleton: true, busy: true });
  };
  const each = async (stage: string | null) => {
    const places = await landmarks(page, [...HEADER_MARKS, ...CONTENT_MARKS]);
    const [skeleton, cards] = await quietEval<[boolean, number]>(page, `[!!document.querySelector("main.gal .gskel"), ${"document.querySelectorAll(\"main.gal > .grp .card-wrap\").length"}]`);
    stages.push({ stage, skeleton, cards, skeletonCards: await skeletonCards(page), places });
  };
  const order = await load.drive(async () => (last = await missing(page)).length === 0, { first, each }).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${last.join(", ")}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
  await each("settled");
  return { order, stages };
}

/** The header's landmarks at every stage against the first frame's; the
 * links slot by its right edge unless its width was known (`slot`). */
function headerHolds(stages: Stage[], slot: boolean): string[] {
  const marks: Landmark[] = HEADER_MARKS.map(m => (m.name === "links slot" && !slot ? { ...m, keys: [] } : m.name === "main" ? { ...m, keys: ["x", "y", "w"] } : m));
  const right = (p: Place | null) => (p ? p.x + p.w : null);
  const out: string[] = [];
  for (const s of stages.slice(1)) {
    for (const d of placesDiffer(marks, stages[0].places, s.places)) out.push(`after ${s.stage}: ${d}`);
    const a = right(stages[0].places["links slot"]), b = right(s.places["links slot"]);
    if (a !== null && b !== null && a !== b) out.push(`after ${s.stage}: the links slot's right edge ${a} -> ${b}`);
  }
  return out;
}

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

/** Whether the page holds the daemon's bootstrap block. */
const hasBlock = (page: Page) => quietEval<boolean>(page, `!!document.getElementById("clax-boot")`);

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

const FORMS = [
  ["desktop", { viewport: { width: 1280, height: 900 } }],
  ["phone", { viewport: { width: 390, height: 844 } }],
  ["phone with touch", { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true }],
] as const;

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
        // The content in the skeleton's place: predicted by a kept shape or the block.
        if (again || who === "viewer") expect.soft(contentHolds(stages), released).toEqual([]);
      });
    }

    for (const path of Object.keys(PAGES)) {
      for (const again of [false, true]) {
        test(`the page at ${path} loads without moving anything${again ? " (a second visit)" : ""}`, async ({ page, freshDaemon: d }) => {
          await seed(page, d);
          const { order, stages } = await stagedVisit(page, d, path, pageMissing(path), [], again);
          const shifts = await layoutShifts(page);
          const released = `released in order:\n  ${order.join("\n  ")}`;
          expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\n${released}`).toBe(0);
          expect.soft(await overlaps(page, GALLERY_APART), "boxes that overlap, or leave the header, at the settled page").toEqual([]);
          expect.soft(headerHolds(stages, again), released).toEqual([]);
        });
      }
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
