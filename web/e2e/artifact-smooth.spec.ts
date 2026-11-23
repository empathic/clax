// The artifact page loads in its final shape: nothing on it moves, its
// title, by-line, version, tags and counts are written once, the content
// frame's box is where it will stay from the first painted frame and its
// element is never replaced or sent anywhere twice, and the page is never
// blank. Each load is played one response at a time (`load-stages.ts`), as
// the gallery's is (`gallery-smooth.spec.ts`): every request held and
// released in a fixed order with two painted frames between, so each late
// piece (the entry's script, the artifact, its threads, the viewer, the
// lazy chunks, the frame's handshake, the stream) lands as a frame of its
// own on any machine, and the Layout Instability API sees whatever it moves
// (`layout-shift.ts`).
//
// An artifact seeded with every such piece: three versions, two tags, a
// pin, a named viewer whose thread the latest version addressed (unread),
// a thread with the agent's reply that the agent is working on, an agent's
// open question, and the agent and the viewer as participants. It is
// loaded by the local owner, whose navigation carries no bootstrap block,
// and by a hosted Clax's person (its owner, signed in), whose page carries
// it (a local daemon's LAN viewer has no site session, so no page in its
// frame to measure); on a desktop with the
// threads' drawer docked right, at the bottom and floating (as this
// browser remembers it), on a narrower window where the drawer starts
// closed, and on a phone with a fine pointer and with touch; a first visit
// and a second; and a pinned version, a deleted artifact and a missing one.
//
// What decides the top bar's optional controls and the thread list's
// questions and approval cards (Clone's options, Publish's links, ClaxBot's
// runs, the feedback config, the inbox's first read and this viewer's
// approval requests, answered here for the thread the agent works on) is
// released only after the frame's page shows, and ClaxBot's meter never
// answers: the essential controls (Comment, Threads, the version switch,
// More, the theme switch, and the local owner's Publish) must be shown and
// usable when the page first shows, the thread cards, the views' tabs and
// the pins they number must show in the same frame, and nothing may move.
// At the settled page no box of the top bar overlaps another or is cut, nor
// the tags' chips (`overlaps`).
//
// Apart from those: ClaxBot on, with a run on the artifact and the week's
// credits past 80 % (its button with the bar, its usage banner over the
// viewer whenever it comes); an artifact without threads in a narrow window
// on a second visit without the block (the closed drawer's strip held only
// where it shows); the Clax app's view (`?app=1`: no chrome from the first
// frame); and the Permissions panel and the tag editor, each drawn once, in
// its final shape, when opened.
//
// The Layout Instability API sees only nodes that move, and the frame's
// placeholders are replaced, not moved, by what the shell mounts. So each
// stage of a load also records where the page's landmarks are: the top bar,
// its mark and title, each of its controls once shown, the stage and the
// content frame, the drawer, and each thread card's top. A landmark must
// hold its place from the stage it first shows to the settled page; the
// top bar, the mark, the title's line and the stage must hold theirs from
// the first painted frame on.
import type { Browser, BrowserContextOptions, Page } from "@playwright/test";
import { hostChannel, NONCE } from "./app-host";
import { test, expect, type Daemon, api, approve, asStranger, openArtifact, proofHeaders, publish, publishAs, publishNext, publishWith, registerSession, setWorking } from "./fixtures";
import { SECRET, hostedDaemon } from "./hosted-helpers";
import { ask } from "./questions-helpers";
import { type Apart, type Landmark, type Place, formatShifts, formatTexts, landmarks, layoutShifts, overlaps, recordLayoutShifts, recordTexts, shiftTotal, shownTexts, textHistory } from "./layout-shift";
import { type StageRule, stageLoad } from "./load-stages";
import { quietEval } from "./time";

const PAGE = `<!doctype html><meta name="color-scheme" content="light dark"><style>body{background:Canvas;color:CanvasText;font:16px system-ui;margin:24px}</style><main><h2>Quarterly goals</h2><p>Ship the roadmap.</p><figure id="chart">Chart</figure></main>`;

/** The order held requests are released in, when several wait at once: the
 * page's own code and styles, then its data in the order the view reads it,
 * the frame's handshake, what decides the optional controls, then the rest
 * (the event stream last). */
const ORDER: StageRule[] = [
  { name: "css", match: (_u, r) => r.resourceType() === "stylesheet" },
  { name: "js", match: (_u, r) => r.resourceType() === "script" },
  { name: "font", match: (_u, r) => r.resourceType() === "font" },
  // The shell's data files (the haiku list), preloaded with the page.
  { name: "data", match: u => u.pathname.startsWith("/_clax/shell/") && u.pathname.endsWith(".json") },
  { name: "token", match: u => u.pathname === "/api/token" },
  { name: "artifact", match: u => /^\/api\/artifacts\/[^/]+$/.test(u.pathname) },
  { name: "threads", match: u => /^\/api\/artifacts\/[^/]+\/threads$/.test(u.pathname) },
  { name: "viewer", match: u => u.pathname === "/api/viewers/me" },
  { name: "frame", match: (u, r) => r.isNavigationRequest() },
  { name: "site-code", match: u => u.pathname.endsWith("/site-code") },
  // What decides the optional controls (Clone's options, Share's sharing,
  // the feedback config, Inbox and the questions): slower than the frame.
  { name: "controls", match: u => /^\/api\/(artifacts\/[^/]+\/(git-token|remotes)|hosted\/artifacts\/[^/]+\/sharing|inbox(\/summary)?|questions|bot\/runs)$/.test(u.pathname) || u.pathname.startsWith("/_app/feedback/") },
  { name: "api", match: u => u.pathname.startsWith("/api/") && !u.pathname.startsWith("/api/stream") },
  { name: "stream", match: u => u.pathname.startsWith("/api/stream") },
];

/** What never answers: ClaxBot's meter (where ClaxBot is not the case's). */
const NEVER = (u: URL) => u.pathname === "/api/me/bot";

/** The top bar's boxes, and the title's (its tags' chips cut by nothing). */
const APART: Apart[] = [
  { name: "the top bar", of: [".page > .topbar", ".topbar .ttl", ".topbar .ttl .ttags"], items: true, inside: ".topbar" },
  { name: "the tags' chips", of: [".topbar .ttl .ttags .tchips"], items: true, inside: ".tchips" },
];

/** Who loads the page: the local owner (no bootstrap block) or a hosted
 * Clax's person, its owner signed in (the block). */
type Who = "owner" | "member";
const NAME: Record<Who, string> = { owner: "Alex", member: "Ada" };

/** The daemon and the browser page a case loads the artifact in: the
 * worker's daemon and `page` for the local owner; for a member, a hosted
 * daemon of the test's own and its owner's browser, set up as Ada in a
 * context with the test's `use` options. */
const hosted: Daemon[] = [];
test.afterEach(async () => { for (const h of hosted.splice(0)) await h.stop(); });
async function visitor(who: Who, page: Page, d: Daemon, browser: Browser, use: BrowserContextOptions): Promise<{ page: Page; d: Daemon; base: string }> {
  if (who === "owner") return { page, d, base: d.base };
  const h = await hostedDaemon();
  hosted.push(h.d);
  // A browser of the hosted Clax's alone: none of the worker's local owner cookies.
  const ctx = await browser.newContext(use);
  await asStranger(ctx);
  const p = await ctx.newPage();
  await p.goto(`${h.origin}/setup?secret=${encodeURIComponent(SECRET)}`);
  await p.getByLabel("Your name").fill(NAME.member);
  await p.getByRole("button", { name: "Create the first person" }).click();
  await p.waitForURL(`${h.origin}/`);
  await p.goto("about:blank");
  return { page: p, d: h.d, base: h.origin };
}
const TITLE = "Roadmap";

type Seeded = { id: string; addressed: string; replied: string };

/** Seeds `d` with "Roadmap": v1, a thread of the viewer's on its heading and
 * one on its chart, the agent's reply on the second, v2 addressing the
 * first, v3; two tags, pinned, the agent working on the chart's thread and
 * asking a question about it. The viewer (`page`'s context: the local
 * owner, named Alex, or the hosted owner Ada) has seen v1 only. The
 * context's own requests seed it, from a page that runs none of the shell. */
async function seed(page: Page, d: Daemon, who: Who, base = d.base): Promise<Seeded> {
  const s = await registerSession(d.base, d.token, "claude", "smooth-artifact");
  const id = (await publishAs(d.base, d.token, s.id, TITLE, { "index.html": PAGE })).artifact.id;
  await api(d.base, d.token, `/api/artifacts/${id}`, { method: "PATCH", body: JSON.stringify({ pinned: true }) });
  for (const [name, colour] of [["design", "blue"], ["q4", "green"]]) await api(d.base, d.token, "/api/tags", { method: "POST", body: JSON.stringify({ name, colour }) });
  await api(d.base, d.token, `/api/artifacts/${id}/tags`, { method: "PUT", body: JSON.stringify({ tags: ["design", "q4"] }) });
  const side = await page.context().newPage();
  await side.goto(`${base}/healthz`);
  const [addressed, replied] = await side.evaluate(async ([aid, proof, name]) => {
    const h = { ...proof, "content-type": "application/json" };
    await fetch("/api/viewers/me", { headers: proof });
    if (name) await fetch("/api/viewers/me", { method: "PUT", headers: h, body: JSON.stringify({ display_name: name }) });
    const post = async (selector: string, body: string) => {
      const f = new FormData();
      f.set("anchor", JSON.stringify({ kind: "element", selector, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      f.set("body", body); f.set("version", "1");
      return (await (await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", headers: proof, body: f })).json()).thread.id as string;
    };
    const a = await post("body > main > h2", "Two columns, please");
    const b = await post("#chart", "The chart could be wider");
    // Sent to the agent, which replies on it.
    await fetch(`/api/artifacts/${aid}/threads/${b}/send`, { method: "POST", headers: h, body: "{}" });
    await fetch("/api/viewers/me/seen", { method: "PUT", headers: h, body: JSON.stringify({ artifact_id: aid, version: 1 }) });
    return [a, b];
  }, [id, who === "owner" ? proofHeaders(d.base) : {}, who === "owner" ? NAME.owner : ""] as const);
  await side.close();
  await api(d.base, d.token, `/api/artifacts/${id}/threads/${replied}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Widening it now", author_kind: "agent" }) });
  await publishNext(d.base, d.token, s.id, id, 1, { addresses: [addressed], note: "Two columns", files: { "index.html": PAGE } });
  await publishNext(d.base, d.token, s.id, id, 2, { note: "Tighter copy", files: { "index.html": PAGE } });
  await setWorking(d.base, d.token, s.id, id, { thread_ids: [replied], message: "Widening the chart" });
  await ask(d.base, d.token, s.id, { source: "ask", artifact_id: id, questions: [{ question: "Should the chart lead?", header: "Chart", options: [{ label: "Yes" }, { label: "No" }] }] });
  return { id, addressed, replied };
}

/** Answers `page`'s ask for this viewer's approval requests on `sd`'s
 * artifact with one, open, for the thread the agent works on (the inbox's
 * approval item, as `GET /api/inbox?kind=approval` lists it). */
async function stubApproval(page: Page, sd: Seeded): Promise<void> {
  const tid = sd.replied;
  const item = {
    id: "01JA00000000000000000000AA", seq: 7, kind: "approval", read: false, created_at: "2026-10-07T09:12:03.120Z", agent: null,
    artifact: { id: sd.id, title: TITLE, kind: "html", page_url: null },
    thread: { id: tid, summary: "#chart", status: "open" }, reply: null, version: null, published: null, question: null, work: null, gone: false,
    url: `/inbox?approval=01J9R0000000000000000000AA`,
    approval: {
      id: "01J9R0000000000000000000AA", approver: "p1", author: { public_id: "p4", name: "Mia", owner: false }, artifact_id: sd.id, thread_id: tid,
      agent: { handle: "a_1f3a00", name: "clax-multi-person", harness: "codex", live: true }, cause: "routing", count: 1, status: "open", scope: null,
      created_at: "2026-10-07T09:12:03.120Z", decided_at: null,
      content: { thread: { id: tid, status: "open" }, comments: [{ id: "c1", body: "The chart could be wider", created_at: "2026-10-07T09:12:00.000Z", author_name: "Mia", by_agent: false }] },
      via_page: null,
    },
  };
  await page.route(u => u.pathname === "/api/inbox" && u.searchParams.get("kind") === "approval", route => route.fulfill({ json: { items: [item] } }));
}

/** The page's late pieces still missing (none once it has settled). */
const artifactMissing = (opts: { panel: boolean; cards: number; phone: boolean }) => async (page: Page) => [...await pageShown(page), ...await quietEval<string[]>(page, `(() => {
  const q = s => document.querySelector(s);
  const n = s => document.querySelectorAll(s).length;
  const missing = [];
  if (q(".topbar h1")?.textContent !== ${JSON.stringify(TITLE)}) missing.push("the title");
  if (!${opts.phone} && !q(".topbar .by")?.textContent?.trim()) missing.push("the by-line");
  if (!q(".topbar .ttl .ttags")) missing.push("the tags");
  if (!q(".topbar .comment")) missing.push("Comment");
  if (!q(".topbar .version-menu")) missing.push("the version menu");
  if (!q(".topbar .more")) missing.push("the more menu");
  if (!q(".topbar .who")) missing.push("the roster");
  if (!q(".stage > iframe.frame")) missing.push("the content frame");
  if (${opts.panel}) {
    if (!q(".viewer .drawer .drawer-bar")) missing.push("the drawer's controls");
    if (n(".viewer .drawer .thread-card") < ${opts.cards}) missing.push("${opts.cards} thread cards");
    if (!q(".viewer .drawer .strip.ag")) missing.push("the working strip");
  }
  if (!(window.claxStreamLive > 0)) missing.push("the live stream");
  return missing;
})()`)];

/** The path of a frame's address ("" before it has one). */
const pathOf = (url: string): string => { try { return new URL(url).pathname; } catch { return ""; } };

/** Whether the content frame shows the artifact's page (none missing), read through the frame itself. */
async function pageShown(page: Page): Promise<string[]> {
  const f = page.frames().find(x => x !== page.mainFrame() && /^\/_clax\/v\/\d+\//.test(pathOf(x.url())));
  const ok = f ? await f.evaluate(() => (document.querySelector("main h2") ? true : `${document.readyState} ${document.body?.outerHTML.slice(0, 400)}`)).catch((e: unknown) => String(e)) : false;
  return ok === true ? [] : [`the artifact's page in the frame (frames: ${page.frames().map(x => x.url()).join(", ")}: ${String(ok)})`];
}

/** The landmarks of the artifact page. `keys` default to the whole rectangle. */
const TOP_MARKS: Landmark[] = [
  { name: "top bar", skeleton: null, content: ".page > .topbar" },
  { name: "mark", skeleton: null, content: ".topbar > .home" },
  { name: "title", skeleton: null, content: ".topbar .ttl h1", keys: ["x", "y", "h"] },
  { name: "stage", skeleton: null, content: ".viewer > .stage" },
  { name: "frame", skeleton: null, content: ".stage > iframe.frame" },
];
const LATE_MARKS: Landmark[] = [
  { name: "by-line", skeleton: null, content: ".topbar .ttl .by", keys: ["x", "y", "h"] },
  { name: "tags", skeleton: null, content: ".topbar .ttl .ttags", keys: ["y", "h"] },
  // The people pill gives way: it narrows to a control that comes late, and
  // on a full bar shares the title's give by a fraction of a pixel as the
  // title's text settles (the Layout Instability API, above, holds it).
  { name: "roster", skeleton: null, content: ".topbar .who", keys: ["y", "h"] },
  { name: "Comment", skeleton: null, content: ".topbar .comment" },
  { name: "Threads", skeleton: null, content: ".topbar .threads" },
  { name: "version", skeleton: null, content: ".topbar .version-menu" },
  { name: "Fork", skeleton: null, content: ".topbar .fork" },
  { name: "Publish", skeleton: null, content: ".topbar .pub" },
  { name: "ClaxBot", skeleton: null, content: ".topbar .botbtn" },
  { name: "more", skeleton: null, content: ".topbar .more, .topbar .more-slot" },
  { name: "theme", skeleton: null, content: ".topbar .theme-switch" },
  { name: "drawer", skeleton: null, content: ".viewer .drawer" },
  { name: "drawer bar", skeleton: null, content: ".viewer .drawer .drawer-bar" },
  { name: "new comment", skeleton: null, content: ".viewer .drawer .new-comment", keys: ["x", "y", "w"] },
  { name: "working strip", skeleton: null, content: ".viewer .drawer .strip.ag", keys: ["x", "y", "w"] },
  { name: "addressed group", skeleton: null, content: ".viewer .drawer .section-addressed", keys: ["x", "y", "w"] },
  { name: "open group", skeleton: null, content: ".viewer .drawer .section-open", keys: ["x", "y", "w"] },
  { name: "views' tabs", skeleton: null, content: ".viewer .drawer .sb-tabs" },
  { name: "approval card", skeleton: null, content: ".viewer .drawer .thread-approval", keys: ["x", "y", "w"] },
  ...[1, 2, 3].map(n => ({ name: `card ${n}`, skeleton: null, content: `.viewer .drawer .thread-card:nth-of-type(${n})`, keys: ["x", "y", "w"] as (keyof Place)[] })),
  { name: "phone tabs", skeleton: null, content: ".phone-tabs" },
];

/** The landmarks recorded at each stage of a load, and the content frame:
 * whether it shows, and the address its document is at. */
type Stage = { stage: string | null; places: Record<string, Place | null>; status: string | null; frame: { visible: boolean; url: string | null }; essentials: Record<string, boolean>; pins: boolean; cards: boolean; tabs: boolean; approval: boolean };

/** A staged visit's options: `again`, the page was loaded once before,
 * unstaged, so the browser keeps what it keeps (and `kept` answers true
 * before it goes on); `never`, what never answers (`NEVER` by default);
 * `extra`, essential controls beyond the usual wider than 900 px, by name
 * and selector;
 * `each`, run at every stage before its landmarks are read. */
type VisitOpts = { again?: boolean; kept?: (p: Page) => Promise<boolean>; never?: (u: URL) => boolean; extra?: Record<string, string>; each?: (p: Page) => Promise<void> };

/** What the content frame did: its element's insertions, its `src` values,
 * its loads, and its background before its first load. */
type FrameLog = { inserted: number; srcs: string[]; loads: number; bgBefore: string[]; blank: string[] };

/** Records the content frame's life in `page`'s top frame (see `FrameLog`). */
async function recordFrame(page: Page): Promise<void> {
  await page.addInitScript(() => {
    if (window !== window.top) return;
    const log = { inserted: 0, srcs: [] as string[], loads: 0, bgBefore: [] as string[], blank: [] as string[] };
    (window as unknown as { claxFrameLog: typeof log }).claxFrameLog = log;
    const seen = new WeakSet<Element>();
    const watch = (f: HTMLIFrameElement) => {
      if (seen.has(f)) return;
      seen.add(f);
      log.inserted++;
      log.srcs.push(f.getAttribute("src") ?? "");
      log.bgBefore.push(getComputedStyle(f).backgroundColor);
      f.addEventListener("load", () => { log.loads++; });
      new MutationObserver(() => { const s = f.getAttribute("src") ?? ""; if (log.srcs.at(-1) !== s) log.srcs.push(s); }).observe(f, { attributes: true, attributeFilter: ["src"] });
    };
    const scan = () => {
      for (const f of document.querySelectorAll<HTMLIFrameElement>(".stage > iframe.frame")) watch(f);
      // The stage shows nothing at all (no frame, no message): a blank page.
      const st = document.querySelector(".viewer > .stage");
      const frame = document.querySelector(".stage > iframe.frame");
      const said = document.querySelector(".stage p.empty, .stage > .stage-wait");
      if (document.readyState === "loading") return;
      if (!st) log.blank.push("no stage");
      else if (!frame && !said) log.blank.push("a stage with no frame and no message");
    };
    new MutationObserver(scan).observe(document, { childList: true, subtree: true });
    document.addEventListener("DOMContentLoaded", scan);
  });
}

const frameLog = (page: Page) => quietEval<FrameLog>(page, "window.claxFrameLog");

/** Loads `path` staged until `missing` answers nothing; answers the release
 * order and the landmarks at each stage. `again`: the page was loaded once
 * before, unstaged, so the browser keeps what it keeps. */
async function stagedVisit(page: Page, base: string, path: string, missing: (p: Page) => Promise<string[]>, texts: string[], again: boolean | VisitOpts): Promise<{ order: string[]; stages: Stage[]; apart: string[] }> {
  const o: VisitOpts = typeof again === "boolean" ? { again } : again;
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  if (o.again) {
    await page.goto(`${base}${path}`);
    await expect.poll(() => missing(page)).toEqual([]);
    if (o.kept) await expect.poll(() => o.kept!(page), { message: "what the first visit keeps for the next" }).toBe(true);
    await page.goto("about:blank");
  }
  await recordLayoutShifts(page);
  await recordTexts(page, texts);
  await recordFrame(page);
  const load = await stageLoad(page, ORDER, { never: o.never ?? NEVER });
  await page.goto(`${base}${path}`, { waitUntil: "commit" });
  let last: string[] = [];
  const stages: Stage[] = [];
  const each = async (stage: string | null) => {
    await o.each?.(page);
    const places = await landmarks(page, [...TOP_MARKS, ...LATE_MARKS]);
    // The top bar's controls and the title's tags are laid out hidden until the bar shows them: not shown yet.
    if (await quietEval<boolean>(page, `!!document.querySelector(".topbar .tools.deciding")`)) {
      for (const k of ["tags", "roster", "Comment", "Threads", "version", "Fork", "Publish", "ClaxBot", "more", "theme"]) places[k] = null;
    }
    // So is the thread list while it waits for its questions and approval cards.
    if (await quietEval<boolean>(page, `!!document.querySelector(".page.cards-wait")`)) {
      for (const k of ["new comment", "working strip", "addressed group", "open group", "views' tabs", "approval card", "card 1", "card 2", "card 3"]) places[k] = null;
    }
    // Whether each essential control is shown (and Comment usable), and whether pins, thread cards, the views' tabs and an approval card show.
    const seen = await quietEval<{ essentials: Record<string, boolean>; pins: boolean; cards: boolean; tabs: boolean; approval: boolean }>(page, `(() => {
      const shown = sel => { const e = document.querySelector(sel); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility === "visible"; };
      const phone = innerWidth <= 700;
      const essentials = { Comment: shown(".topbar .comment") && !document.querySelector(".topbar .comment").disabled, more: shown(".topbar .more > button") };
      if (!phone) { essentials.Threads = shown(".topbar .threads"); essentials.version = shown(".topbar .vbtn"); }
      if (innerWidth > 900) essentials.theme = shown(".topbar .theme-switch");
      if (phone) essentials.tabs = shown(".phone-tabs [data-shell-control=threads]");
      // Wider than 900 px (the bar hides them narrower, and the more menu lists them).
      for (const [k, sel] of Object.entries(${JSON.stringify(o.extra ?? {})})) if (innerWidth > 900) essentials[k] = shown(sel);
      return { essentials, pins: [...document.querySelectorAll(".thread-pin")].some(p => getComputedStyle(p).visibility === "visible"), cards: shown(".viewer .drawer .thread-card"), tabs: shown(".viewer .drawer .sb-tabs"), approval: shown(".viewer .drawer .thread-approval") };
    })()`);
    const status = await quietEval<string | null>(page, `(document.querySelector("#app .page > p.empty")?.textContent ?? null)`);
    const visible = await quietEval<boolean>(page, `(() => { const f = document.querySelector(".stage > iframe.frame"); return !!f && getComputedStyle(f).visibility === "visible"; })()`);
    const f = page.frames().find(x => x.parentFrame() === page.mainFrame());
    stages.push({ stage, places, status, frame: { visible, url: f ? pathOf(f.url()) : null }, ...seen });
  };
  const order = await load.drive(async () => (last = await missing(page)).length === 0, { each }).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${last.join(", ")}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
  await each("settled");
  return { order, stages, apart: await overlaps(page, APART) };
}

/** Where the page's first frame showing the artifact's page has an
 * essential control not shown (or Comment not usable), where a pin shows
 * before the thread cards it numbers (the drawer open): the cards may show
 * first (the pins come with the page), and where the views' tabs or (with
 * `approval`) an approval card do not show in the cards' first frame. */
function together(stages: Stage[], panel: boolean, approval = false): string[] {
  const out: string[] = [];
  const first = stages.find(s => s.frame.visible);
  if (!first) return ["the page never showed"];
  const off = Object.entries(first.essentials).filter(([, ok]) => !ok).map(([k]) => k);
  if (off.length) out.push(`the page first showed after ${first.stage} without ${off.join(", ")}`);
  if (panel) {
    const pins = stages.findIndex(s => s.pins);
    const cards = stages.findIndex(s => s.cards);
    if (pins < 0 || cards < 0 || pins < cards) out.push(`pins first showed after ${stages[pins]?.stage ?? "never"}, thread cards after ${stages[cards]?.stage ?? "never"}`);
    const tabs = stages.findIndex(s => s.tabs);
    if (tabs !== cards) out.push(`the views' tabs first showed after ${stages[tabs]?.stage ?? "never"}, thread cards after ${stages[cards]?.stage ?? "never"}`);
    const card = stages.findIndex(s => s.approval);
    if (approval && card !== cards) out.push(`the approval card first showed after ${stages[card]?.stage ?? "never"}, thread cards after ${stages[cards]?.stage ?? "never"}`);
  }
  return out;
}

/** Where the top landmarks differ from the first painted frame's, and where
 * each late landmark differs, from the first stage it showed in, from the
 * settled page's. */
function holds(stages: Stage[]): string[] {
  const out: string[] = [];
  const end = stages.at(-1)!.places;
  // Within half a pixel: a bar a 64th of a pixel too full gives that much way.
  const differ = (m: Landmark, a: Place, b: Place) => (m.keys ?? (["x", "y", "w", "h"] as const)).some(k => Math.abs(a[k] - b[k]) >= 0.5);
  for (const m of TOP_MARKS) {
    let from: Place | null = null;
    for (const s of stages) {
      const p = s.places[m.name];
      if (!p) { if (from) { out.push(`after ${s.stage}: ${m.name} gone`); break; } continue; }
      if (!from) { from = p; continue; }
      if (differ(m, from, p)) { out.push(`after ${s.stage}: ${m.name}: ${JSON.stringify(from)} -> ${JSON.stringify(p)}`); break; }
    }
  }
  for (const m of LATE_MARKS) {
    const e = end[m.name];
    for (const s of stages) {
      const p = s.places[m.name];
      if (!p || !e) continue;
      if (differ(m, p, e)) { out.push(`after ${s.stage ?? "(the first frame)"}: ${m.name}: ${JSON.stringify(p)} -> settled ${JSON.stringify(e)}`); break; }
    }
  }
  return out;
}

/** Where the content frame shows anything but the artifact's page (a step
 * of its site's handshake, or a blank document), and where it moves to
 * another document once it showed the page. */
function frameHolds(stages: Stage[]): string[] {
  const out: string[] = [];
  let shown: string | null = null;
  for (const s of stages) {
    if (!s.frame.visible) continue;
    const url = s.frame.url ?? "(none)";
    if (!/^\/_clax\/v\/\d+\//.test(url)) out.push(`after ${s.stage ?? "(the first frame)"}: the frame shows ${url}`);
    else if (shown === null) shown = url;
    else if (url !== shown) out.push(`after ${s.stage}: the frame went from ${shown} to ${url}`);
  }
  return out;
}

/** The texts written once: the title, the by-line, the version button, the
 * tags, the Threads count and the drawer's Open count. */
const TEXTS = [".topbar .ttl h1", ".topbar .ttl .by", ".topbar .vbtn", ".topbar .ttl .tchips", ".topbar .threads .cnt", ".viewer .drawer .section-open .gh .c"];

const hasBlock = (page: Page) => quietEval<boolean>(page, `!!document.getElementById("clax-boot")`);

/** A summary of one load, kept with the test's outputs for the report. */
async function keep(name: string, data: unknown): Promise<void> {
  const dir = process.env.CLAX_SMOOTH_OUT ?? process.env.TEST_UNDECLARED_OUTPUTS_DIR;
  if (!dir) return;
  const fs = await import("node:fs");
  const p = await import("node:path");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(p.join(dir, `${name.replace(/[^a-z0-9]+/gi, "-")}.json`), JSON.stringify(data, null, 1));
}

type Form = { name: string; use: BrowserContextOptions; panel: boolean; phone: boolean; layouts: (string | null)[] };
const FORMS: Form[] = [
  { name: "desktop", use: { viewport: { width: 1280, height: 900 } }, panel: true, phone: false, layouts: [null, "bottom", "float"] },
  { name: "narrow window", use: { viewport: { width: 820, height: 900 } }, panel: false, phone: false, layouts: [null] },
  { name: "phone", use: { viewport: { width: 390, height: 844 } }, panel: false, phone: true, layouts: [null] },
  { name: "phone with touch", use: { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true }, panel: false, phone: true, layouts: [null] },
];

for (const form of FORMS) {
  test.describe(`on a ${form.name}`, () => {
    test.use(form.use);
    for (const layout of form.layouts) {
      for (const [who, again] of [["owner", false], ["owner", true], ["member", false], ["member", true]] as const) {
        const how = `${who === "owner" ? "the owner, no bootstrap block" : "a hosted Clax's member, with the block"}${again ? ", a second visit" : ""}${layout ? `, threads ${layout === "float" ? "floating" : `docked at the ${layout}`}` : ""}`;
        test(`the artifact loads without moving anything, and writes its title, version, tags and counts once (${how})`, async ({ page: own, freshDaemon: fresh, browser }) => {
          const { page, d, base } = await visitor(who, own, fresh, browser, form.use);
          const sd = await seed(page, d, who, base);
          await stubApproval(page, sd);
          if (layout) await page.addInitScript(l => { try { localStorage.setItem("clax.drawer", JSON.stringify({ layout: l, w: 392, h: 280 })); } catch { /* none */ } }, layout);
          // The local owner's shell publishes to a public Clax: its Publish shows with the bar.
          const extra: Record<string, string> = who === "owner" ? { Publish: ".topbar .pub" } : {};
          const { order, stages, apart } = await stagedVisit(page, base, `/a/${sd.id}`, artifactMissing({ panel: form.panel, cards: 2, phone: form.phone }), TEXTS, { again, extra });
          expect(await hasBlock(page), "the page's bootstrap block").toBe(who === "member");
          const shifts = await layoutShifts(page);
          const texts = await textHistory(page);
          const frame = await frameLog(page);
          const released = `released in order:\n  ${order.join("\n  ")}`;
          await keep(`${form.name} ${how}`, { shifts, texts, frame, stages, order });
          expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\n${released}`).toBe(0);
          for (const sel of TEXTS) expect.soft(shownTexts(texts[sel]), formatTexts(sel, texts[sel])).toHaveLength(sel === ".viewer .drawer .section-open .gh .c" && !form.panel ? 0 : 1);
          expect.soft(holds(stages), released).toEqual([]);
          expect.soft(frame.inserted, "content frames inserted").toBe(1);
          expect.soft(frameHolds(stages), "the content frame").toEqual([]);
          expect.soft(frame.blank, "a blank stage").toEqual([]);
          expect.soft(stages.map(s => s.status).filter(Boolean), "a message in place of the viewer").toEqual([]);
          expect.soft(together(stages, form.panel, form.panel), released).toEqual([]);
          expect.soft(apart, "boxes that overlap, or are cut, at the settled page").toEqual([]);
        });
      }
    }
  });
}

test.describe("on a desktop in the dark theme", () => {
  const use: BrowserContextOptions = { viewport: { width: 1280, height: 900 }, colorScheme: "dark" };
  test.use(use);
  for (const who of ["owner", "member"] as const) {
    test(`the content frame never shows white before its page (${who})`, async ({ page: own, freshDaemon: fresh, browser }) => {
      const { page, d, base } = await visitor(who, own, fresh, browser, use);
      const sd = await seed(page, d, who, base);
      const { stages, apart } = await stagedVisit(page, base, `/a/${sd.id}`, artifactMissing({ panel: true, cards: 2, phone: false }), TEXTS, false);
      const frame = await frameLog(page);
      expect.soft(apart, "boxes that overlap, or are cut").toEqual([]);
      await keep(`dark ${who}`, { frame, stages });
      // The frame's own background is white until its page paints: it never shows before its page.
      expect.soft(frameHolds(stages), "the content frame").toEqual([]);
      expect.soft(stages.filter(s => s.frame.visible).length, "stages the frame showed in").toBeGreaterThan(0);
    });
  }
});

test.describe("on a desktop, other addresses", () => {
  const use: BrowserContextOptions = { viewport: { width: 1280, height: 900 } };
  test.use(use);
  for (const who of ["owner", "member"] as const) {
    test(`a pinned version loads without moving anything (${who})`, async ({ page: own, freshDaemon: fresh, browser }) => {
      const { page, d, base } = await visitor(who, own, fresh, browser, use);
      const sd = await seed(page, d, who, base);
      const { order, stages, apart } = await stagedVisit(page, base, `/a/${sd.id}/v/2`, artifactMissing({ panel: true, cards: 2, phone: false }), TEXTS, false);
      expect.soft(apart, "boxes that overlap, or are cut").toEqual([]);
      const shifts = await layoutShifts(page);
      const texts = await textHistory(page);
      await keep(`pinned ${who}`, { shifts, texts, stages, order, frame: await frameLog(page) });
      expect.soft(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
      expect.soft(holds(stages)).toEqual([]);
      for (const sel of TEXTS.slice(0, 3)) expect.soft(shownTexts(texts[sel]), formatTexts(sel, texts[sel])).toHaveLength(1);
    });

    test(`a deleted artifact says so, in place, without moving anything (${who})`, async ({ page: own, freshDaemon: fresh, browser }) => {
      const { page, d, base } = await visitor(who, own, fresh, browser, use);
      const sd = await seed(page, d, who, base);
      await api(d.base, d.token, `/api/artifacts/${sd.id}`, { method: "DELETE" });
      const deleted = (p: Page) => quietEval<string[]>(p, `(() => document.querySelector(".stage p.empty, #app .page > p.empty")?.textContent?.includes("deleted") || /not found|isn.t available/i.test(document.body.textContent) ? [] : ["the deleted message: " + location.href + " " + document.body.textContent.slice(0, 200)])()`);
      const { order, stages, apart } = await stagedVisit(page, base, `/a/${sd.id}`, deleted, TEXTS, false);
      expect.soft(apart, "boxes that overlap, or are cut").toEqual([]);
      const shifts = await layoutShifts(page);
      await keep(`deleted ${who}`, { shifts, stages, order, texts: await textHistory(page) });
      expect.soft(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
      // The stage takes the drawer's place to say why: its width alone may change.
      expect.soft(holds(stages).filter(x => !/ stage: /.test(x))).toEqual([]);
    });

    test(`a missing artifact says so, in place, without moving anything (${who})`, async ({ page: own, freshDaemon: fresh, browser }) => {
      const { page, base } = await visitor(who, own, fresh, browser, use);
      const gone = (p: Page) => quietEval<string[]>(p, `(/not found|isn.t available/i.test(document.body.textContent) ? [] : ["the not-found message: " + location.href + " " + document.body.textContent.slice(0, 200)])`);
      const { order, stages, apart } = await stagedVisit(page, base, "/a/zzzzzzzzzzzz", gone, TEXTS, false);
      expect.soft(apart, "boxes that overlap, or are cut").toEqual([]);
      const shifts = await layoutShifts(page);
      await keep(`missing ${who}`, { shifts, stages, order, texts: await textHistory(page) });
      expect.soft(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
      // The stage takes the drawer's place to say why: its width alone may change.
      expect.soft(holds(stages).filter(x => !/ stage: /.test(x))).toEqual([]);
    });
  }
});

/** ClaxBot on for this person, 85 % of the week's credits spent (its usage
 * banner shows), and a finished run on artifact `aid`. */
const WEEK = { start: "2026-10-19T00:00:00Z", end: "2026-10-26T00:00:00Z", weekly_milli: 2_000_000, lift_milli: 0, spent_milli: 1_700_000, warn: true, exhausted: false };
const ME_BOT = { enabled: true, unenforced: false, available: true, unavailable_reason: null, key: { own: null, use_own: false }, week: WEEK, role: "owner" };
const RUN = (aid: string, tid: string) => ({ id: "r1", artifact_id: aid, thread_ids: [tid], request: "Widen the chart", status: "finished", model: "claude-sonnet-5-5", key_source: "instance", acting_for: { id: "*owner", name: "Alex" }, started_at: "2026-10-21T14:02:00Z", credits_milli: 3000 });

/** Answers ClaxBot's routes for `page` (this person's ClaxBot, the
 * artifact's runs and one run), and, for a page served with the block,
 * puts the same answers in the block's `deciders`, as a daemon with
 * ClaxBot on writes them. */
async function stubBot(page: Page, sd: Seeded): Promise<void> {
  const runs = { runs: [RUN(sd.id, sd.replied)] };
  await page.route(u => u.pathname === "/api/me/bot" || u.pathname.startsWith("/api/bot/runs"), route => {
    const u = new URL(route.request().url());
    if (u.pathname === "/api/me/bot") return route.fulfill({ json: ME_BOT });
    if (u.pathname === "/api/bot/runs") return route.fulfill({ json: runs });
    if (u.pathname === "/api/bot/runs/r1") return route.fulfill({ json: { ...runs.runs[0], steps: [] } });
    return route.fulfill({ status: 404, json: { error: { code: "not_found", message: "not found" } } });
  });
  // The block's answers are written as the parser inserts the block, before
  // any script reads it (a navigation the test fulfilled itself would lose
  // the page's local address, and with it its frame).
  await page.addInitScript((r: unknown) => {
    if (window !== window.top) return;
    const me = { enabled: true, unenforced: false, available: true, unavailable_reason: null, key: { own: null, use_own: false }, role: "owner",
      week: { start: "2026-10-19T00:00:00Z", end: "2026-10-26T00:00:00Z", weekly_milli: 2_000_000, lift_milli: 0, spent_milli: 1_700_000, warn: true, exhausted: false } };
    const o = new MutationObserver(() => {
      const el = document.getElementById("clax-boot");
      if (!el) return;
      let b: { deciders?: { status: Record<string, number>; [k: string]: unknown } };
      try { b = JSON.parse(el.textContent ?? ""); } catch { return; }
      o.disconnect();
      if (!b.deciders) return;
      b.deciders.status.bot = 200; b.deciders.bot = me;
      b.deciders.status.runs = 200; b.deciders.runs = r;
      el.textContent = JSON.stringify(b);
    });
    o.observe(document, { childList: true, subtree: true, characterData: true });
  }, runs);
}

test.describe("on a desktop, with ClaxBot", () => {
  const use: BrowserContextOptions = { viewport: { width: 1280, height: 900 } };
  test.use(use);
  for (const who of ["owner", "member"] as const) {
    test(`its runs button shows with the bar and its usage banner moves nothing (${who === "owner" ? "the owner, no bootstrap block" : "a hosted Clax's member, with the block"})`, async ({ page: own, freshDaemon: fresh, browser }) => {
      const { page, d, base } = await visitor(who, own, fresh, browser, use);
      const sd = await seed(page, d, who, base);
      await stubBot(page, sd);
      const settled = artifactMissing({ panel: true, cards: 2, phone: false });
      const missing = async (p: Page) => [...await settled(p), ...await quietEval<string[]>(p, `(document.querySelector(".bot-banner-at .bbanner") ? [] : ["the usage banner"])`)];
      const extra: Record<string, string> = { ClaxBot: ".topbar .botbtn", ...(who === "owner" ? { Publish: ".topbar .pub" } : {}) };
      const { order, stages, apart } = await stagedVisit(page, base, `/a/${sd.id}`, missing, TEXTS, { never: () => false, extra });
      const shifts = await layoutShifts(page);
      const released = `released in order:\n  ${order.join("\n  ")}`;
      await keep(`claxbot ${who}`, { shifts, stages, order, texts: await textHistory(page) });
      expect.soft(shiftTotal(shifts), `${formatShifts(shifts)}\n${released}`).toBe(0);
      expect.soft(holds(stages), released).toEqual([]);
      expect.soft(together(stages, true), released).toEqual([]);
      expect.soft(apart, "boxes that overlap, or are cut, at the settled page").toEqual([]);
      // The banner lies over the viewer, under the top bar, and Dismiss hides it.
      const bar = (await page.locator(".page > .topbar").boundingBox())!;
      const ban = (await page.locator(".bot-banner-at .bbanner").boundingBox())!;
      expect.soft(Math.abs(ban.y - (bar.y + bar.height))).toBeLessThan(1);
      await page.locator(".bot-banner-at").getByRole("button", { name: "Dismiss" }).click();
      await expect(page.locator(".bot-banner-at")).toHaveCount(0);
    });
  }
});

test.describe("on a narrow window, an artifact without threads", () => {
  const use: BrowserContextOptions = { viewport: { width: 820, height: 900 } };
  test.use(use);
  test("a second visit without the block holds no strip, and the stage its whole width", async ({ page, freshDaemon: d }) => {
    const id = (await publish(d.base, d.token, "Quiet page", { "index.html": PAGE })).artifact.id as string;
    const quiet = async (p: Page) => [...await pageShown(p), ...await quietEval<string[]>(p, `(() => {
      const m = [];
      if (!document.querySelector(".topbar .comment")) m.push("Comment");
      if (!(window.claxStreamLive > 0)) m.push("the live stream");
      return m;
    })()`)];
    const kept = (p: Page) => quietEval<boolean>(p, `(() => { try { return JSON.parse(localStorage.getItem("clax.shape"))[${JSON.stringify(`/a/${id}`)}].t === 0; } catch { return false; } })()`);
    const { order, stages } = await stagedVisit(page, d.base, `/a/${id}`, quiet, TEXTS.slice(0, 3), { again: true, kept });
    expect(await hasBlock(page), "the page's bootstrap block").toBe(false);
    const shifts = await layoutShifts(page);
    await keep("narrow no threads", { shifts, stages, order });
    expect.soft(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
    expect.soft(holds(stages)).toEqual([]);
    expect.soft(stages.at(-1)!.places.stage?.w, "the stage's width").toBe(820);
    expect.soft(await page.locator(".viewer > :is(.rail, .rail-held)").count(), "a strip").toBe(0);
  });
});

test.describe("in the Clax app's view", () => {
  for (const [form, use] of [["phone", { viewport: { width: 390, height: 800 }, hasTouch: true, isMobile: true }], ["Mac", { viewport: { width: 1280, height: 900 } }]] as const) {
    test.describe(`on a ${form}`, () => {
      test.use(use);
      test("the page loads without chrome from its first frame, and moves nothing", async ({ page, freshDaemon: d }) => {
        const sd = await seed(page, d, "owner");
        await hostChannel(page);
        // The app inits its host once the host module is in, as its WebView does.
        const init = async (p: Page) => {
          if (await quietEval<boolean>(p, `typeof window.claxHost === "object" && !window.claxInited`)) await quietEval(p, `(window.claxInited = true, window.claxHost.init(${JSON.stringify({ nonce: NONCE })}))`);
        };
        const app = async (p: Page) => [...await pageShown(p), ...await quietEval<string[]>(p, `(() => {
          const m = [];
          if (!document.documentElement.classList.contains("app")) m.push("app mode");
          if (!window.claxInited) m.push("the app's init");
          if (!(window.claxStreamLive > 0)) m.push("the live stream");
          return m;
        })()`)];
        const { order, stages } = await stagedVisit(page, d.base, `/a/${sd.id}?app=1`, app, [], { each: init });
        const shifts = await layoutShifts(page);
        const frame = await frameLog(page);
        await keep(`app ${form}`, { shifts, stages, order, frame });
        expect.soft(shiftTotal(shifts), formatShifts(shifts)).toBe(0);
        expect.soft(holds(stages)).toEqual([]);
        // From the first painted frame the stage fills the window: no top bar, no drawer, no tabs.
        const size = use.viewport;
        for (const s of stages) {
          const st = s.places.stage;
          if (!st) continue;
          expect.soft({ at: s.stage, ...st }, "the stage").toEqual({ at: s.stage, x: 0, y: 0, w: size.width, h: size.height });
          for (const k of ["top bar", "drawer", "phone tabs"]) expect.soft(s.places[k], `${k} after ${s.stage}`).toBeNull();
        }
        expect.soft(frame.inserted, "content frames inserted").toBe(1);
        expect.soft(frameHolds(stages), "the content frame").toEqual([]);
      });
    });
  }
});

test.describe("on a desktop, the panels the page opens", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("the Permissions panel and the tag editor open on the press, and write what they read once", async ({ page, freshDaemon: d }) => {
    const sd = await seed(page, d, "owner");
    const { artifact } = await publishWith(d.base, d.token, "Forecast", PAGE, { net: { hosts: ["api.clax.invalid"] } });
    await approve(d.base, d.token, artifact.id);
    await recordTexts(page, [".perm-panel", ".tag-pop"]);
    await openArtifact(page, d.base, artifact.id, 1);
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Permissions…" }).click();
    await expect(page.locator(".perm-panel li[data-permission]").first()).toBeVisible();
    // Open at once, its rows' places a skeleton (no text), then its rows:
    // never a loading line, nor "asks for nothing" before the rows.
    const perm = (await textHistory(page))[".perm-panel"];
    expect.soft(shownTexts(perm).length, formatTexts(".perm-panel", perm)).toBeLessThanOrEqual(2);
    expect.soft(shownTexts(perm).filter(t => /Loading|asks for nothing/.test(t)), formatTexts(".perm-panel", perm)).toEqual([]);
    expect.soft(shownTexts(perm).at(-1), formatTexts(".perm-panel", perm)).toContain("The internet (api.clax.invalid)");
    await page.locator(".perm-panel").getByRole("button", { name: "Close" }).click();
    await page.goto("about:blank");
    // The tag editor, on the seeded artifact with its tags.
    await openArtifact(page, d.base, sd.id, 3);
    await page.locator(".topbar .ttl .tedit").click();
    await expect(page.locator(".tag-pop").getByText("design")).toBeVisible();
    // Open at once with the artifact's own tags, then every tag there is.
    const tags = (await textHistory(page))[".tag-pop"];
    expect.soft(shownTexts(tags)[0], formatTexts(".tag-pop", tags)).toContain("designq4");
    expect.soft(shownTexts(tags).filter(t => /Loading/.test(t)), formatTexts(".tag-pop", tags)).toEqual([]);
  });
});
