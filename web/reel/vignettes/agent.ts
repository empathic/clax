// "Ask your agent" (spec 2026-10-27-sizzle-reel-design §3):
// a person and an agent collaborating on a real application, not a page of
// text. On a hosted daemon, so a teammate can watch (a local daemon's
// browsers are all its owner, web/e2e/hosted-helpers.ts), an agent session
// (`claude`, scripted through the API: no model runs) has published
// "Northwind Ops", a launch-ops console whose board of tasks lives in the
// page's documents (seed/ops-v1.html). Alex, the owner, turns on comment
// mode, draws an area over the board and asks the agent for a timeline with
// dependencies and a burndown. The agent takes the thread (the shell shows
// it working), publishes version 2 (seed/ops-v2.html: the same tasks as a
// timeline with their dependencies drawn, a burndown and a new theme) and
// replies in the thread; Alex loads it in place. Mia, a member watching the
// same page all along, loads it too, points at the timeline and cheers in
// the thread. Only Alex's browser is filmed.
//
// Mia is teal every run: she is minted until her ID hashes to that live
// colour (people.ts) and joins the page before Alex, so it is free. Every
// step of the agent's and of Mia's is waited for in Alex's page before the
// film moves on, so the frame it shows on is the same each run.
//
// Beats: `ask` (Alex starts his comment), `working` (the agent takes the
// thread, publishes, and Alex clicks Reload), `reveal` (the first frame of
// version 2, its arrival playing), `touch` (Mia points at the timeline and
// replies).
import { readFileSync } from "node:fs";
import type { Frame, Page, Response } from "@playwright/test";
import { api, contentFrame, registerSession } from "../../e2e/fixtures.ts";
import { openAt, ownerOf, rightsOn } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorTo } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { CURVES, ease } from "../pace.ts";
import { LIVE_PALETTE, memberIn, welcomedColor } from "../people.ts";

/** The launch's tasks: a board in version 1, a timeline in version 2. Days count from November 3. */
const TASKS = [
  { id: "date", title: "Pick the launch date", owner: "Alex", status: "done", start: 0, days: 1, points: 2, depends_on: [] },
  { id: "copy", title: "Pricing page copy", owner: "Mia", status: "done", start: 1, days: 3, points: 3, depends_on: ["date"] },
  { id: "api", title: "Billing API cutover", owner: "Sam", status: "doing", start: 1, days: 7, points: 8, depends_on: [] },
  { id: "design", title: "Checkout redesign", owner: "Priya", status: "doing", start: 4, days: 4, points: 5, depends_on: ["copy"] },
  { id: "docs", title: "Migration guide", owner: "Alex", status: "review", start: 5, days: 3, points: 3, depends_on: ["copy"] },
  { id: "load", title: "Load test checkout", owner: "Sam", status: "todo", start: 8, days: 3, points: 5, depends_on: ["api", "design"] },
  { id: "email", title: "Launch email", owner: "Mia", status: "todo", start: 8, days: 3, points: 2, depends_on: ["docs"] },
  { id: "oncall", title: "On-call runbook", owner: "Priya", status: "todo", start: 11, days: 2, points: 3, depends_on: ["load"] },
  { id: "flags", title: "Flip the feature flags", owner: "Alex", status: "todo", start: 13, days: 2, points: 1, depends_on: ["oncall", "email"] },
];
/** Points remaining at the end of each day so far. */
const BURNDOWN = [32, 31, 29, 28, 26, 25, 23, 22];

const ASK = "@claude Timeline with dependencies + burndown";
const WORKING = "Drawing the timeline";
/** How fast Alex types the ask: twice the reel's usual rate, as the ask is the reel's quickest beat. */
const ASK_CPS = 28;
const NOTE = "Timeline with dependencies, and a burndown";
const REPLY = "Swapped the board for a timeline of the same tasks, with their dependencies drawn, and added a burndown and the critical path.";
const CHEER = "Whoa, the critical path is right there. 🙌";
/** Mia's live colour (LIVE_PALETTE): teal. */
const MIA = 2;

type Point = { x: number; y: number };
type Cast = { alex: Page; af: Frame; mia: Page; mf: Frame; aid: string; origin: string; base: string; token: string; sid: string };

// What Alex's content frame hears from the shell, kept where the recorder
// can read it: each peer's latest place (`clax:peers`, by colour).
const HEARING = `(() => {
  if (globalThis.reelHeard) return;
  const heard = globalThis.reelHeard = { peers: {} };
  addEventListener("message", e => {
    const m = e.data;
    if (m && m.type === "clax:peers") for (const p of m.peers) heard.peers[p.color] = JSON.stringify([p.anchor, p.selection]);
  });
})()`;

// Marks a document of Mia's (`data-reel-here` on its root) once her shell
// tells it someone else is on the page (`clax:peers` with `here`).
const HERE = `addEventListener("message", e => {
  if (e.data && e.data.type === "clax:peers" && e.data.here) document.documentElement.setAttribute("data-reel-here", "");
})`;

/** What Alex's frame `af` last heard of Mia's pointer. */
const miaHeard = (alex: Page, af: Frame) => quietIn<string | null>(alex, af, `globalThis.reelHeard.peers[${JSON.stringify(LIVE_PALETTE[MIA])}] ?? null`);

/** A frame's element `selector`, as a whole point in its page's viewport, at fractions `fx`, `fy` of its box. */
async function pointIn(page: Page, frame: Frame, selector: string, fx = 0.5, fy = 0.5): Promise<Point> {
  const box = await frame.locator(selector).first().boundingBox();
  if (!box) throw new Error(`${selector} has no box in ${page.url()}`);
  return { x: Math.round(box.x + box.width * fx), y: Math.round(box.y + box.height * fy) };
}

/** How long what Alex's view heard of Mia must stay the same before her
 * glide starts, in real time: longer than LIVE_CURSOR_MS (the shell's
 * least gap between two reports of a pointer) many times over. */
const SETTLED_MS = 500;

/** The least distance, in pixels, between two of Mia's steps: the shell
 * may pass on no smaller move. */
const MIN_STEP_PX = 8;

/** Mia's pointer glides from `from` to `to` in at most `steps`, a step per
 * shot of `stepMs`, each step waited for until Alex's frame `af` has heard
 * it. A step too close to the last is left out, but never the last. */
async function glide(d: Director, alex: Page, af: Frame, mia: Page, from: Point, to: Point, steps: number, stepMs: number): Promise<void> {
  const far = (a: Point, b: Point) => Math.hypot(a.x - b.x, a.y - b.y) >= MIN_STEP_PX;
  const path: Point[] = [];
  for (let i = 1; i < steps; i++) {
    const e = ease(i / steps, CURVES.standard);
    const p = { x: Math.round(from.x + (to.x - from.x) * e), y: Math.round(from.y + (to.y - from.y) * e) };
    if (far(p, path.at(-1) ?? from) && far(p, to)) path.push(p);
  }
  path.push(to);
  for (const p of path) {
    const before = await miaHeard(alex, af);
    await d.offstage(() => mia.mouse.move(p.x, p.y));
    await d.until(async () => {
      const now = await miaHeard(alex, af);
      return now !== null && now !== before;
    }, "Alex's view heard Mia's pointer move");
    await d.hold(stepMs);
  }
}

/** Text of the shell's element `selector` in Alex's page, or "" when there is none. */
const shellText = (alex: Page, selector: string) => quietIn<string>(alex, alex.mainFrame(), `document.querySelector(${JSON.stringify(selector)})?.textContent ?? ""`);

export default {
  id: "agent",

  async setup(stage: Stage): Promise<Cast> {
    const { d, origin } = await stage.hostedDaemon();
    // The agent's session publishes version 1, so it owns and watches the page.
    const session = await registerSession(d.base, d.token, "claude", "reel-agent");
    const v1 = readFileSync(new URL("../seed/ops-v1.html", import.meta.url), "utf8");
    const res = await fetch(`${d.base}/api/artifacts`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${d.token}`, "x-clax-session": session.id },
      body: JSON.stringify({ title: "Northwind Ops", capabilities: { db: {} }, files: { "index.html": { content: v1, encoding: "utf8" } } }),
    });
    if (!res.ok) throw new Error(`publish: ${res.status} ${await res.text()}`);
    const aid = ((await res.json()) as { artifact: { id: string } }).artifact.id;

    // Alex sets the daemon up, seeds the tasks and invites Mia, in a browser
    // of the setup's own, then leaves the page, so he holds no live colour
    // when she arrives; the camera signs in as him.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    const seed = await openAt(owner, origin, aid, 1);
    await seed.evaluate(async ([tasks, remaining]) => {
      const db = await (window as unknown as { claude: { use(n: string): Promise<{ doc(p: string): { set(v: unknown): Promise<unknown> } }> } }).claude.use("db");
      for (const [order, { id, ...task }] of tasks.entries()) await db.doc(`tasks/${id}`).set({ ...task, order });
      await db.doc("metrics/burndown").set({ remaining });
    }, [TASKS, BURNDOWN] as const);
    await owner.goto(`${origin}/`);
    const mia = await memberIn(stage.browser, owner, origin, "Mia", rightsOn(aid), MIA);
    // Alex always allows what Mia writes on this page through to his agent
    // (People's "Always allow"), so her reply reaches it with no approval.
    await owner.evaluate(async id => {
      const { people } = (await (await fetch("/api/people")).json()) as { people: { display_name: string; public_id: string }[] };
      const author = people.find(p => p.display_name === "Mia")?.public_id;
      if (!author) throw new Error("no Mia in Alex's People");
      const r = await fetch("/api/approvals", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ author, scope: "artifact", artifact_id: id }) });
      if (!r.ok) throw new Error(`allow Mia: ${r.status} ${await r.text()}`);
    }, aid);
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    // Mia joins the page, then Alex, each with the threads open. Each of her
    // frames marks its document once her shell says someone else is here,
    // which is when her page starts reporting her pointer.
    await mia.setViewportSize({ width: 1280, height: 800 });
    await mia.addInitScript(HERE);
    const welcomed = welcomedColor(mia);
    const mf = await openAt(mia, origin, aid, 1);
    const got = await welcomed;
    if (got !== MIA) throw new Error(`Mia joined in live colour ${got}, not ${MIA}`);
    const alex = await stage.newCamera({ storageState: signedIn });
    const af = await openAt(alex, origin, aid, 1);
    for (const [page, frame] of [[alex, af], [mia, mf]] as const) {
      await frame.locator(".card[data-task]").first().waitFor({ state: "visible" });
      await page.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
      const threads = page.locator("button.threads");
      if ((await threads.getAttribute("aria-pressed")) !== "true") await threads.click();
    }
    await alex.locator(`.who .tok.p.here[title="Mia"]`).waitFor({ state: "visible" });
    await mf.locator("html[data-reel-here]").waitFor({ state: "attached" });
    await quietIn(alex, af, HEARING);
    return { alex, af, mia, mf, aid, origin, base: d.base, token: d.token, sid: session.id };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, mia, mf, aid, origin, base, token, sid } = cast;
    let af = cast.af;
    // Mia rests her pointer on the board, where Alex sees it.
    const miaAt = await pointIn(mia, mf, `.card[data-task="load"]`, 0.75, 0.3);
    const before = await miaHeard(alex, af);
    await d.offstage(() => mia.mouse.move(miaAt.x, miaAt.y));
    await d.until(async () => (await miaHeard(alex, af)) !== before, "Alex's view heard Mia's pointer");
    await d.hold(200);

    // Whether Alex's view has marked his thread looked at (PUT /api/viewers/me/looked).
    let looked = false;
    const onLooked = (r: Response) => {
      if (r.request().method() === "PUT" && new URL(r.url()).pathname === "/api/viewers/me/looked") looked = true;
    };
    alex.on("response", onLooked);

    // Alex turns on comment mode and draws an area over the board.
    d.beat("ask");
    await d.click(alex.getByRole("button", { name: /^Comment/ }).first());
    await d.until(async () => (await quietIn<string>(alex, af, "document.documentElement.style.cursor")) === "crosshair", "Alex's page is in comment mode");
    // The composer opens, with its screenshot of the area, before the next frame.
    const textarea = alex.locator(".stage .composer textarea");
    await d.drag(alex.frameLocator("iframe.frame").locator("#board"), { x: 0.01, y: 0.0 }, { x: 0.99, y: 0.7 }, {
      landed: () => d.until(async () => (await textarea.count()) > 0 && (await alex.locator(".stage .composer img.clip").count()) > 0 && (await quietIn<boolean>(alex, alex.mainFrame(), `document.activeElement?.matches(".stage .composer textarea") ?? false`)), "Alex's composer opened with its screenshot, its field focused"),
    });
    // The composer opened with its field focused: Alex types straight in.
    await d.keys(ASK, { cps: ASK_CPS });
    await d.hold(150);
    const card = alex.locator(".thread-card").first();
    await d.click(alex.locator(".stage .composer").getByRole("button", { name: /^(Post comment|Send)$/ }), {
      // The composer closes and the new thread shows, selected, with comment
      // mode still on: every change the post makes, before the next frame.
      landed: () => d.until(async () => (await alex.locator(".stage .composer").count()) === 0 && (await alex.locator(".thread-card.selected[data-thread]").count()) === 1 && (await alex.locator('button.comment[aria-pressed="true"]').count()) === 1, "Alex's thread shows, selected, in his threads"),
    });
    const tid = (await card.getAttribute("data-thread"))!;
    await d.hold(150);

    // The agent takes the thread (its next tool result carries it), and
    // says what it is doing.
    d.beat("working");
    await d.offstage(() => api(base, token, `/api/sessions/${sid}/feedback?tier=piggyback`));
    await d.until(async () => (await shellText(alex, ".who .sum b.l1")).includes("working") && (await alex.locator(".strip.ag").count()) > 0 && (await card.locator(".st.ag").count()) > 0, "Alex's view shows the agent working");
    await d.offstage(() => api(base, token, `/api/sessions/${sid}/working/${aid}`, { method: "PUT", body: JSON.stringify({ thread_ids: [tid], message: WORKING }) }));
    await d.until(async () => (await shellText(alex, ".who .sum b.l1")).includes(WORKING) && (await shellText(alex, ".strip.ag b")).includes(WORKING), "Alex's view shows what the agent is doing");
    // Alex looks over at the agent's strip in his threads (comment mode
    // stays on, which keeps the strip to its message alone).
    await d.move(alex.locator(".strip.ag"), { offset: { x: 40, y: 0 } });
    // The shell marks the thread looked at a second after Alex saw it (on
    // the page's clock); the agent publishes after that, so version 2's
    // change is news to him and his threads gather it as addressed.
    for (let i = 0; ; i++) {
      if (looked) break;
      if (i === 40) throw new Error("Alex's view never marked his thread looked at");
      await d.hold(100);
    }
    alex.off("response", onLooked);
    await d.hold(200);

    // Version 2 lands: offered in Alex's top bar, he loads it in place (the
    // reveal: the timeline sweeps in, its dependencies draw, the burndown
    // draws), and the agent's reply arrives in his thread.
    const v2 = readFileSync(new URL("../seed/ops-v2.html", import.meta.url), "utf8");
    await d.offstage(() => api(base, token, `/api/artifacts/${aid}/versions`, {
      method: "POST",
      session: sid,
      body: JSON.stringify({ if_version: 1, note: NOTE, addresses: [tid], files: { "index.html": { content: v2, encoding: "utf8" } } }),
    }));
    const reload = alex.getByRole("button", { name: "Reload", exact: true });
    await d.until(async () => (await reload.count()) > 0, "Alex's view offers version 2");
    await d.hold(150);
    const at = await reload.boundingBox();
    if (!at) throw new Error("Reload has no box");
    // Reload loads the shell anew, on version 2: the next frame shows it
    // drawn, with the camera's cursor put back where it was.
    await d.click(reload, {
      landed: async () => {
        af = await contentFrame(alex, aid, 2);
        await cursorTo(alex, Math.round((at.x + at.width / 2) * 10) / 10, Math.round((at.y + at.height / 2) * 10) / 10);
        await d.until(async () => (await quietIn<number>(alex, af, `document.querySelectorAll("#gantt .bar").length`)) === TASKS.length && (await quietIn<number>(alex, af, `document.querySelectorAll("#burndown polyline").length`)) === 1, "version 2 drew its timeline and burndown");
        d.beat("reveal");
      },
    });
    await quietIn(alex, af, HEARING);
    // The arrival plays; then Alex's eye goes to the version button, which
    // reads v2 now, and the agent's reply arrives. The critical path keeps
    // flowing and today's point keeps pulsing throughout.
    await d.hold(700);
    await d.move(alex.locator("button.vbtn"), { offset: { x: 6, y: 16 } });
    await d.offstage(() => api(base, token, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", session: sid, body: JSON.stringify({ body: REPLY, author_kind: "agent" }) }));
    await d.until(async () => (await card.locator(".msg.agent").count()) > 0, "the agent's reply reached Alex's thread");
    await d.hold(600);

    // Mia loads version 2 too, points at the timeline and cheers.
    d.beat("touch");
    // She leaves the page and opens it again, until the daemon welcomes her
    // back in her own colour: her old socket may still hold it when her new
    // one joins.
    const mf2 = await d.offstage(async () => {
      for (let attempt = 0; attempt < 5; attempt++) {
        await mia.goto(`${origin}/`);
        const welcomed = welcomedColor(mia);
        const frame = await openAt(mia, origin, aid, 2);
        if ((await welcomed) !== MIA) continue;
        await frame.locator("html[data-reel-here]").waitFor({ state: "attached" });
        return frame;
      }
      throw new Error(`Mia never rejoined in live colour ${MIA}`);
    });
    // Her version 2 finishes arriving (on her own, real clock) before she
    // points: a place on a bar still sweeping in would be told to Alex as a
    // share of a box that differs from run to run.
    await d.offstage(async () => {
      await mf2.locator("#gantt .bar").nth(TASKS.length - 1).waitFor({ state: "attached" });
      await mf2.waitForFunction(() => document.getAnimations().every(a => a.effect?.getComputedTiming().iterations === Infinity || a.playState === "finished"));
    });
    const from = await pointIn(mia, mf2, "#gantt", 0.95, 0.95);
    const to = await pointIn(mia, mf2, "#gantt", 0.62, 0.62);
    // Her pointer starts at `from`: her new socket may report a place of its
    // own as it opens, so Alex's view is waited on until what it heard of
    // her has stopped changing (SETTLED_MS with no change), and only then
    // does each step of the glide make the one change it waits for.
    await d.offstage(async () => {
      await mia.mouse.move(from.x, from.y);
      let last = await miaHeard(alex, af);
      let since = Date.now();
      const deadline = since + 10_000;
      while (last === null || Date.now() - since < SETTLED_MS) {
        if (Date.now() > deadline) throw new Error("Alex's view never settled on Mia's pointer");
        await new Promise(r => setTimeout(r, 25));
        const now = await miaHeard(alex, af);
        if (now !== last) { last = now; since = Date.now(); }
      }
    });
    await glide(d, alex, af, mia, from, to, 12, 45);
    const replies = await card.locator(".msg").count();
    await d.offstage(() => mia.evaluate(async ([a, t, body]) => {
      const r = await fetch(`/api/artifacts/${a}/threads/${t}/comments`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ body }) });
      if (!r.ok) throw new Error(`reply: ${r.status} ${await r.text()}`);
    }, [aid, tid, CHEER] as const));
    // Her reply goes on to the agent, which reads it with its next tool
    // call, and is done: Alex's card says the agent saw it.
    await d.offstage(async () => {
      await api(base, token, `/api/sessions/${sid}/feedback?tier=piggyback`);
      await api(base, token, `/api/sessions/${sid}/working/${aid}`, { method: "DELETE" });
    });
    await d.until(async () => (await card.locator(".msg").count()) > replies && (await card.locator(".waiting").allTextContents()).join() === "seen by the agent" && (await card.locator(".st.ag").count()) === 0, "Mia's reply reached Alex's thread, seen by the agent");
    await d.hold(1000);
  },
} satisfies Vignette<Cast>;
