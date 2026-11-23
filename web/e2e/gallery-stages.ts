// What the staged-load specs of the gallery's pages share
// (`gallery-smooth.spec.ts`, `pages-smooth.spec.ts`): the order held
// requests are released in, the seeded gallery, the staged visit that
// records each stage's landmarks, and the checks over them.
//
// Each stage records, beside the landmarks (`layout-shift.ts`
// `landmarks`), where the bottom of what `main` shows is (the skeleton's,
// or the page's) and every loading line it shows (`LOADING_TEXT` in a box
// that is on screen): a page beside the gallery is revealed once it has
// its first data, so after that nothing in it may grow, and no stage may
// show a line saying it is loading.
import type { Page } from "@playwright/test";
import { expect, type Daemon, api, asLocalViewer, proofHeaders, publishAs, publishNext, registerSession } from "./fixtures";
import { ask } from "./questions-helpers";
import { CONTENT_MARKS, HEADER_MARKS, type Landmark, type Place, landmarks, placesDiffer, recordLayoutShifts, recordTexts } from "./layout-shift";
import { type StageRule, stageLoad } from "./load-stages";
import { quietEval } from "./time";

export const PAGE = "<main><h2>Quarterly goals</h2></main>";
export const SUB = ".gbar .sub";

/** The order held requests are released in, when several wait at once: the
 * page's own code and styles, then its data in the order the gallery reads
 * it, then the rest (the event stream last). */
export const ORDER: StageRule[] = [
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
export type Who = "owner" | "viewer";
export const NAME: Record<Who, string> = { owner: "Alex", viewer: "Bea" };

/** Seeds `d` with a gallery of five artifacts by an agent: "Roadmap" needs
 * the viewer (a thread of theirs addressed in a version they have not
 * seen), "Budget" is pinned, "Design notes" and "Launch checklist" carry
 * tags, and the agent asks the owner a question about the roadmap (the
 * inbox's unread summary). The viewer is named (Alex for the owner, Bea for
 * a LAN viewer, which the context becomes), so the subtitle reads "… · seen
 * as <name>". The context's own requests seed it, from a page that runs
 * none of the shell. Answers the artifacts' IDs. */
export async function seed(page: Page, d: Daemon, who: Who = "owner"): Promise<string[]> {
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

/** What a line saying a page is loading reads. */
export const LOADING_TEXT = /Loading|Counting|Starting…/;

/** The landmarks recorded at each stage of a load: whether the skeleton
 * shows, how many cards and skeleton cards, the landmarks' places, the
 * bottom of what `main` shows below its top, and the loading lines shown. */
export type Stage = { stage: string | null; skeleton: boolean; cards: number; skeletonCards: number; places: Record<string, Place | null>; bottom: number; loading: string[] };

/** Where the bottom of what `main` shows is, from `main`'s top, and the
 * loading lines it shows: text matching `LOADING_TEXT` in a box on screen,
 * outside the visually hidden status lines. */
export const probeMain = (page: Page) => quietEval<{ bottom: number; loading: string[] }>(page, `(() => {
  const main = document.querySelector("#app > main.gal");
  if (!main) return { bottom: 0, loading: [] };
  const top = main.getBoundingClientRect().top;
  let bottom = 0;
  for (const c of main.children) {
    const r = c.getBoundingClientRect();
    if (r.height > 0 && !c.matches(".vh")) bottom = Math.max(bottom, Math.round((r.bottom - top) * 64) / 64);
  }
  const loading = [];
  const walk = document.createTreeWalker(main, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const t = (n.textContent ?? "").trim();
    if (!t || !${LOADING_TEXT}.test(t)) continue;
    const e = n.parentElement;
    if (!e || e.closest(".vh, .sr, [hidden], .gpage.pending")) continue;
    const r = e.getBoundingClientRect();
    if (r.width < 2 || r.height < 2 || getComputedStyle(e).visibility !== "visible") continue;
    loading.push(t.slice(0, 60));
  }
  return { bottom, loading };
})()`);

/** Whether any skeleton card shows. */
const skeletonCards = (page: Page) => quietEval<number>(page, `[...document.querySelectorAll("main.gal .gskel .sk-c")].filter(e => e.getBoundingClientRect().height > 0).length`);

/** The key a page keeps its shape under (`gallery-shape.ts` `shapeKeyOf`). */
export const shapeKey = (path: string) => path.replace(/[?#].*$/, "").replace(/^\/queues\/[^/]+\/[^/]+$/, "/queues/*/*").replace(/^\/queues\/[^/]+$/, "/queues/*");

/** Loads `path` staged, until `missing` answers nothing; answers the release
 * order and the landmarks at each stage. Before the first release, the
 * page must show the header and the skeleton: the document alone, its
 * scripts held. `again`: the page was loaded once before, unstaged, so the
 * browser keeps its shape. */
export async function stagedVisit(page: Page, d: Daemon, path: string, missing: (p: Page) => Promise<string[]>, texts: string[] = [], again = false): Promise<{ order: string[]; stages: Stage[] }> {
  // The page holds the event stream itself, as where shared workers are
  // missing (shell/src/stream.ts): a shared worker's requests reach no event
  // of the page's, so the stream's would land at no fixed stage.
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  if (again) {
    await page.goto(`${d.base}${path}`);
    await expect.poll(() => missing(page)).toEqual([]);
    // Its shape kept once its first render is painted (view/gallery-shape.ts).
    const top = shapeKey(new URL(page.url()).pathname);
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
    const { bottom, loading } = await probeMain(page);
    stages.push({ stage, skeleton, cards, skeletonCards: await skeletonCards(page), places, bottom, loading });
  };
  const order = await load.drive(async () => (last = await missing(page)).length === 0, { first, each }).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${last.join(", ")}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
  await each("settled");
  return { order, stages };
}

/** The header's landmarks at every stage against the first frame's; the
 * links slot by its right edge unless its width was known (`slot`). */
export function headerHolds(stages: Stage[], slot: boolean): string[] {
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

/** The loading lines any stage showed, each with its stage. */
export const loadingShown = (stages: Stage[]): string[] => stages.flatMap(s => s.loading.map(t => `after ${s.stage ?? "(the first frame)"}: "${t}"`));

/** What `main` showed at a different height from the settled page's: at
 * every stage after the skeleton went (the page shows its first data, and
 * nothing in it grows after), and with `skeleton` at the skeleton's stages
 * too (a kept shape or the block predicts the page). */
export function heightHolds(stages: Stage[], skeleton: boolean): string[] {
  const end = stages.at(-1)!.bottom;
  return stages.filter(s => skeleton || !s.skeleton).filter(s => s.bottom !== end)
    .map(s => `${s.skeleton ? "skeleton" : "page"} after ${s.stage ?? "(the first frame)"}: main's content ends at ${s.bottom}, settled at ${end}`);
}

/** Whether the page holds the daemon's bootstrap block. */
export const hasBlock = (page: Page) => quietEval<boolean>(page, `!!document.getElementById("clax-boot")`);

export const FORMS = [
  ["desktop", { viewport: { width: 1280, height: 900 } }],
  ["phone", { viewport: { width: 390, height: 844 } }],
  ["phone with touch", { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true }],
] as const;
