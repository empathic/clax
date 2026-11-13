// The gallery loads in its final shape: nothing on it moves, and the
// header's subtitle is written once. Each load is played one response at a
// time (`load-stages.ts`), every request held and released in a fixed
// order with two painted frames between, so each late piece (the viewer,
// the list, the attention that groups it, the lazy module's New tile and
// header links, the inbox's summary, the stream) lands as a frame of its
// own on any machine, and the Layout Instability API sees whatever it moves
// (`layout-shift.ts`). A gallery seeded with every such piece: a card that
// needs the viewer, a pinned one, tagged ones, a named viewer and an agent's
// open question.
import type { Page } from "@playwright/test";
import { test, expect, type Daemon, api, proofHeaders, publishAs, publishNext, registerSession } from "./fixtures";
import { ask } from "./questions-helpers";
import { formatShifts, formatTexts, layoutShifts, recordLayoutShifts, recordTexts, shiftTotal, shownTexts, textHistory } from "./layout-shift";
import { type StageRule, stageLoad } from "./load-stages";
import { quietEval } from "./time";

const PAGE = "<main><h2>Quarterly goals</h2></main>";
const SUB = ".gbar .sub";

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

/** Seeds `d` with a gallery of five artifacts by an agent: "Roadmap" needs
 * the viewer (a thread of theirs addressed in a version they have not
 * seen), "Budget" is pinned, "Design notes" and "Launch checklist" carry
 * tags, and the agent asks the owner a question about the roadmap (the
 * inbox's unread summary). The viewer is named Alex, so the subtitle reads
 * "… · seen as Alex". The context's own requests seed it, from a page that
 * runs none of the shell. */
async function seed(page: Page, d: Daemon): Promise<void> {
  const s = await registerSession(d.base, d.token, "claude", "smooth");
  const pub = async (title: string) => (await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE })).artifact.id;
  const roadmap = await pub("Roadmap");
  const budget = await pub("Budget");
  const design = await pub("Design notes");
  const launch = await pub("Launch checklist");
  await pub("Team offsite");
  await api(d.base, d.token, `/api/artifacts/${budget}`, { method: "PATCH", body: JSON.stringify({ pinned: true }) });
  for (const [name, colour] of [["design", "blue"], ["q4", "green"]]) await api(d.base, d.token, "/api/tags", { method: "POST", body: JSON.stringify({ name, colour }) });
  await api(d.base, d.token, `/api/artifacts/${design}/tags`, { method: "PUT", body: JSON.stringify({ tags: ["design", "q4"] }) });
  await api(d.base, d.token, `/api/artifacts/${launch}/tags`, { method: "PUT", body: JSON.stringify({ tags: ["q4"] }) });
  const side = await page.context().newPage();
  await side.goto(`${d.base}/healthz`);
  const tid = await side.evaluate(async ([id, proof]) => {
    const h = { ...proof, "content-type": "application/json" };
    await fetch("/api/viewers/me", { headers: proof });
    await fetch("/api/viewers/me", { method: "PUT", headers: h, body: JSON.stringify({ display_name: "Alex" }) });
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", "Two columns, please"); f.set("version", "1");
    const t = (await (await fetch(`/api/artifacts/${id}/threads`, { method: "POST", headers: proof, body: f })).json()).thread.id as string;
    await fetch("/api/viewers/me/seen", { method: "PUT", headers: h, body: JSON.stringify({ artifact_id: id, version: 1 }) });
    return t;
  }, [roadmap, proofHeaders(d.base)] as const);
  await side.close();
  await publishNext(d.base, d.token, s.id, roadmap, 1, { addresses: [tid] });
  await ask(d.base, d.token, s.id, { source: "ask", artifact_id: roadmap, questions: [{ question: "Which quarter should the roadmap lead with?", header: "Quarter", options: [{ label: "Q3" }, { label: "Q4" }] }] });
}

/** What is still missing of the gallery's late pieces (none once it has settled). */
const galleryMissing = (page: Page) => quietEval<string[]>(page, `(() => {
  const q = s => document.querySelector(s);
  const n = s => document.querySelectorAll(s).length;
  const missing = [];
  if (!/seen as Alex/.test(q(${JSON.stringify(SUB)})?.textContent ?? "")) missing.push("the subtitle naming the viewer");
  if (n(".grp.needs .card-wrap:not(.new-tile)") !== 1) missing.push("Roadmap under Needs your eyes");
  if (n(".grp.rest .card-wrap:not(.new-tile)") !== 4) missing.push("four cards under Everything else");
  if (n(".new-tile") !== 1) missing.push("the New tile");
  if (!q(".tagbar")) missing.push("the tag bar");
  if (!q(".gbar .inbox-link")) missing.push("Inbox in the header");
  if (!q(".gbar .queues-link")) missing.push("Queues in the header");
  if (!q(".gbar .settings-link")) missing.push("Settings in the header");
  if (!q(".inbox-sum:not([hidden])")) missing.push("the inbox's unread summary");
  if (!q(".gfoot")) missing.push("the haiku");
  if (!(window.claxStreamLive > 0)) missing.push("the live stream");
  return missing;
})()`);

/** What is still missing of the Tags page's pieces. */
const tagsMissing = (page: Page) => quietEval<string[]>(page, `(() => {
  const missing = [];
  if (document.querySelectorAll(".tagsp .trow").length !== 2) missing.push("the two tags' rows");
  if (!document.querySelector(".gbar .settings-link")) missing.push("Settings in the header");
  if (!/seen as Alex/.test(document.querySelector(${JSON.stringify(SUB)})?.textContent ?? "")) missing.push("the subtitle naming the viewer");
  return missing;
})()`);

/** Loads `path` staged, until `missing` answers nothing; answers the release order. */
async function stagedVisit(page: Page, d: Daemon, path: string, missing: (p: Page) => Promise<string[]>): Promise<string[]> {
  // The page holds the event stream itself, as where shared workers are
  // missing (shell/src/stream.ts): a shared worker's requests reach no event
  // of the page's, so the stream's would land at no fixed stage.
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  await recordLayoutShifts(page);
  await recordTexts(page, [SUB]);
  const load = await stageLoad(page, ORDER);
  await page.goto(`${d.base}${path}`, { waitUntil: "commit" });
  let last: string[] = [];
  return load.drive(async () => (last = await missing(page)).length === 0).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${last.join(", ")}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
}

for (const [form, viewport] of [["desktop", { width: 1280, height: 900 }], ["phone", { width: 390, height: 844 }]] as const) {
  test.describe(`on a ${form}`, () => {
    test.use({ viewport });

    test("the gallery loads without moving anything, and writes its subtitle once", async ({ page, freshDaemon: d }) => {
      // Fails today: the cards land with no placeholder, the New tile and header links arrive late, the cards regroup, the unread summary lands above them, and the subtitle gains "· seen as Alex".
      test.fail();
      await seed(page, d);
      const order = await stagedVisit(page, d, "/", galleryMissing);
      const shifts = await layoutShifts(page);
      const sub = (await textHistory(page))[SUB];
      expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\nreleased in order:\n  ${order.join("\n  ")}`).toBe(0);
      expect.soft(shownTexts(sub), formatTexts(SUB, sub)).toHaveLength(1);
    });

    test("the Tags page loads without moving anything", async ({ page, freshDaemon: d }) => {
      // Fails today: the page's chunk fills an empty main, pushing the footer, and the header's links mount late, moving the theme switch.
      test.fail();
      await seed(page, d);
      const order = await stagedVisit(page, d, "/tags", tagsMissing);
      const shifts = await layoutShifts(page);
      expect(shiftTotal(shifts), `${formatShifts(shifts)}\nreleased in order:\n  ${order.join("\n  ")}`).toBe(0);
    });
  });
}
