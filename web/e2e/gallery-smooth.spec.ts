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
import type { Page } from "@playwright/test";
import { test, expect, type Daemon, api, asLocalViewer, proofHeaders, publishAs, publishNext, registerSession } from "./fixtures";
import { ask } from "./questions-helpers";
import { formatShifts, formatTexts, layoutShifts, recordLayoutShifts, recordTexts, shiftTotal, shownTexts, textHistory } from "./layout-shift";
import { type StageRule, stageLoad } from "./load-stages";
import { quietEval } from "./time";

const PAGE = "<main><h2>Quarterly goals</h2></main>";
const SUB = ".gbar .sub";
const HEADINGS = [".grp.needs h2", ".grp.rest h2"];

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

/** Loads `path` staged, until `missing` answers nothing; answers the release
 * order. Before the first release, the page must show the header and the
 * skeleton: the document alone, its scripts held. */
async function stagedVisit(page: Page, d: Daemon, path: string, missing: (p: Page) => Promise<string[]>, texts: string[] = []): Promise<string[]> {
  // The page holds the event stream itself, as where shared workers are
  // missing (shell/src/stream.ts): a shared worker's requests reach no event
  // of the page's, so the stream's would land at no fixed stage.
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  await recordLayoutShifts(page);
  await recordTexts(page, [SUB, ...texts]);
  const load = await stageLoad(page, ORDER);
  await page.goto(`${d.base}${path}`, { waitUntil: "commit" });
  let last: string[] = [];
  const first = async () => {
    const shows = await quietEval<Record<string, boolean>>(page, `(() => {
      const seen = s => { const e = document.querySelector(s); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== "hidden"; };
      return { header: seen(".gbar") && seen(".gbar .theme-switch") && seen(".gbar .home"), skeleton: seen("main.gal .gskel") };
    })()`);
    expect(shows, "the first frame, before any script ran: the header and the skeleton").toEqual({ header: true, skeleton: true });
  };
  return load.drive(async () => (last = await missing(page)).length === 0, { first }).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${last.join(", ")}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
}

/** Whether the page holds the daemon's bootstrap block. */
const hasBlock = (page: Page) => quietEval<boolean>(page, `!!document.getElementById("clax-boot")`);

for (const [form, viewport] of [["desktop", { width: 1280, height: 900 }], ["phone", { width: 390, height: 844 }]] as const) {
  test.describe(`on a ${form}`, () => {
    test.use({ viewport });

    for (const who of ["owner", "viewer"] as const) {
      test(`the gallery loads without moving anything, and writes its subtitle, titles and headings once (${who === "owner" ? "the owner, no bootstrap block" : "a LAN viewer, with the block"})`, async ({ page, freshDaemon: d }) => {
        const ids = await seed(page, d, who);
        const titles = ids.map(id => `a.card[href="/a/${id}"] h3`);
        const order = await stagedVisit(page, d, "/", galleryMissing(who), [...HEADINGS, ...titles]);
        expect(await hasBlock(page), "the page's bootstrap block").toBe(who === "viewer");
        const shifts = await layoutShifts(page);
        const texts = await textHistory(page);
        expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\nreleased in order:\n  ${order.join("\n  ")}`).toBe(0);
        expect.soft(shownTexts(texts[SUB]), formatTexts(SUB, texts[SUB])).toHaveLength(1);
        for (const sel of [...HEADINGS, ...titles]) expect.soft(shownTexts(texts[sel]), formatTexts(sel, texts[sel])).toHaveLength(1);
      });
    }

    for (const path of Object.keys(PAGES)) {
      test(`the page at ${path} loads without moving anything`, async ({ page, freshDaemon: d }) => {
        await seed(page, d);
        const order = await stagedVisit(page, d, path, pageMissing(path));
        const shifts = await layoutShifts(page);
        expect(shiftTotal(shifts), `${formatShifts(shifts)}\nreleased in order:\n  ${order.join("\n  ")}`).toBe(0);
      });
    }

    test("the gallery comes back from an artifact without moving anything", async ({ page, freshDaemon: d }) => {
      const [roadmap] = await seed(page, d);
      await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
      await recordLayoutShifts(page);
      await page.goto(`${d.base}/`);
      const settled = galleryMissing("owner");
      await expect.poll(() => settled(page)).toEqual([]);
      await page.locator(`a.card[href="/a/${roadmap}"]`).click();
      await page.waitForURL(`**/a/${roadmap}**`);
      await page.goBack();
      await page.waitForURL(`${d.base}/`);
      await expect.poll(() => settled(page)).toEqual([]);
      const shifts = await layoutShifts(page);
      expect(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
    });
  });
}
