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
// questions (Clone's options, the feedback config, ClaxBot's meter, the
// inbox's first read) is released only after the frame's page shows, and
// ClaxBot's meter and Publish's links never answer: the essential controls
// (Comment, Threads, the version switch, More, the theme switch) must be
// shown and usable when the page first shows, the thread cards and the
// pins they number must show in the same frame, and nothing may move. At
// the settled page no box of the top bar overlaps another or is cut, nor
// the tags' chips (`overlaps`).
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
import { test, expect, type Daemon, api, asStranger, proofHeaders, publishAs, publishNext, registerSession, setWorking } from "./fixtures";
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
  { name: "controls", match: u => /^\/api\/(artifacts\/[^/]+\/git-token|hosted\/artifacts\/[^/]+\/sharing|inbox(\/summary)?|questions)$/.test(u.pathname) || u.pathname.startsWith("/_app/feedback/") },
  { name: "api", match: u => u.pathname.startsWith("/api/") && !u.pathname.startsWith("/api/stream") },
  { name: "stream", match: u => u.pathname.startsWith("/api/stream") },
];

/** Deciders that never answer: ClaxBot's meter and Publish's links. */
const NEVER = (u: URL) => u.pathname === "/api/me/bot" || /^\/api\/artifacts\/[^/]+\/remotes$/.test(u.pathname);

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
  { name: "more", skeleton: null, content: ".topbar .more, .topbar .more-slot" },
  { name: "theme", skeleton: null, content: ".topbar .theme-switch" },
  { name: "drawer", skeleton: null, content: ".viewer .drawer" },
  { name: "drawer bar", skeleton: null, content: ".viewer .drawer .drawer-bar" },
  { name: "new comment", skeleton: null, content: ".viewer .drawer .new-comment", keys: ["x", "y", "w"] },
  { name: "working strip", skeleton: null, content: ".viewer .drawer .strip.ag", keys: ["x", "y", "w"] },
  { name: "addressed group", skeleton: null, content: ".viewer .drawer .section-addressed", keys: ["x", "y", "w"] },
  { name: "open group", skeleton: null, content: ".viewer .drawer .section-open", keys: ["x", "y", "w"] },
  ...[1, 2, 3].map(n => ({ name: `card ${n}`, skeleton: null, content: `.viewer .drawer .thread-card:nth-of-type(${n})`, keys: ["x", "y", "w"] as (keyof Place)[] })),
  { name: "phone tabs", skeleton: null, content: ".phone-tabs" },
];

/** The landmarks recorded at each stage of a load, and the content frame:
 * whether it shows, and the address its document is at. */
type Stage = { stage: string | null; places: Record<string, Place | null>; status: string | null; frame: { visible: boolean; url: string | null }; essentials: Record<string, boolean>; pins: boolean; cards: boolean };

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
async function stagedVisit(page: Page, base: string, path: string, missing: (p: Page) => Promise<string[]>, texts: string[], again: boolean): Promise<{ order: string[]; stages: Stage[]; apart: string[] }> {
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  if (again) {
    await page.goto(`${base}${path}`);
    await expect.poll(() => missing(page)).toEqual([]);
    await page.goto("about:blank");
  }
  await recordLayoutShifts(page);
  await recordTexts(page, texts);
  await recordFrame(page);
  const load = await stageLoad(page, ORDER, { never: NEVER });
  await page.goto(`${base}${path}`, { waitUntil: "commit" });
  let last: string[] = [];
  const stages: Stage[] = [];
  const each = async (stage: string | null) => {
    const places = await landmarks(page, [...TOP_MARKS, ...LATE_MARKS]);
    // The top bar's controls and the title's tags are laid out hidden until the bar shows them: not shown yet.
    if (await quietEval<boolean>(page, `!!document.querySelector(".topbar .tools.deciding")`)) {
      for (const k of ["tags", "roster", "Comment", "Threads", "version", "Fork", "more", "theme"]) places[k] = null;
    }
    // So is the thread list while it waits for its questions.
    if (await quietEval<boolean>(page, `!!document.querySelector(".page.cards-wait")`)) {
      for (const k of ["new comment", "working strip", "addressed group", "open group", "card 1", "card 2", "card 3"]) places[k] = null;
    }
    // Whether each essential control is shown (and Comment usable), and whether pins and thread cards show.
    const seen = await quietEval<{ essentials: Record<string, boolean>; pins: boolean; cards: boolean }>(page, `(() => {
      const shown = sel => { const e = document.querySelector(sel); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility === "visible"; };
      const phone = innerWidth <= 700;
      const essentials = { Comment: shown(".topbar .comment") && !document.querySelector(".topbar .comment").disabled, more: shown(".topbar .more > button") };
      if (!phone) { essentials.Threads = shown(".topbar .threads"); essentials.version = shown(".topbar .vbtn"); }
      if (innerWidth > 900) essentials.theme = shown(".topbar .theme-switch");
      if (phone) essentials.tabs = shown(".phone-tabs [data-shell-control=threads]");
      return { essentials, pins: [...document.querySelectorAll(".thread-pin")].some(p => getComputedStyle(p).visibility === "visible"), cards: shown(".viewer .drawer .thread-card") };
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
 * essential control not shown (or Comment not usable), and where a pin
 * shows before the thread cards it numbers (the drawer open): the cards
 * may show first (the pins come with the page). */
function together(stages: Stage[], panel: boolean): string[] {
  const out: string[] = [];
  const first = stages.find(s => s.frame.visible);
  if (!first) return ["the page never showed"];
  const off = Object.entries(first.essentials).filter(([, ok]) => !ok).map(([k]) => k);
  if (off.length) out.push(`the page first showed after ${first.stage} without ${off.join(", ")}`);
  if (panel) {
    const pins = stages.findIndex(s => s.pins);
    const cards = stages.findIndex(s => s.cards);
    if (pins < 0 || cards < 0 || pins < cards) out.push(`pins first showed after ${stages[pins]?.stage ?? "never"}, thread cards after ${stages[cards]?.stage ?? "never"}`);
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
          if (layout) await page.addInitScript(l => { try { localStorage.setItem("clax.drawer", JSON.stringify({ layout: l, w: 392, h: 280 })); } catch { /* none */ } }, layout);
          const { order, stages, apart } = await stagedVisit(page, base, `/a/${sd.id}`, artifactMissing({ panel: form.panel, cards: 2, phone: form.phone }), TEXTS, again);
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
          expect.soft(together(stages, form.panel), released).toEqual([]);
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
