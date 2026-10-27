// "Build things together" (spec 2026-10-27-sizzle-reel-design §3, the
// ask-and-deliver shape): Alex asks the agent for more than a page, and the
// next version is a game his team plays at once. On a hosted daemon (a
// local daemon's browsers are all its owner, web/e2e/hosted-helpers.ts),
// Alex, the owner, has Northwind's "Team Sketch" open, a single-player
// drawing pad, while Mia, a member with rights on it, has it open in a
// browser of her own. Alex turns on comment mode, picks the pad and asks
// "Make this a multiplayer game we can all play at once", then sends it to
// the agent. The agent (a session scripted through the API; no model runs)
// takes it, shows as working, publishes the star-chase
// (seed/starchase.html, declaring `room`) and replies. Both views load it:
// Mia's orb, steered by her pointer in her browser, crosses Alex's arena
// through the room and takes a star; Alex steers his own to the next one;
// the scores tick in both. Only Alex's browser is filmed.
//
// Mia is teal every run: she is minted until her ID hashes to that live
// colour (people.ts) and joins the page before Alex. Every step of hers is
// waited for in Alex's page (her orb's place and score as his page's room
// holds them, and her shell pointer) before the film moves on, so the
// frame it shows on is the same each run, however long the network took.
//
// Beats: `ask` (comment mode on), `working` (the agent took the thread),
// `reveal` (the star-chase lands), `touch` (Alex steers for a star).
import { readFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { api, contentFrame, registerSession } from "../../e2e/fixtures.ts";
import { openAt, ownerOf, rightsOn } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorPress, cursorTo } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { CURVES, ease } from "../pace.ts";
import { LIVE_PALETTE, memberIn, welcomedColor } from "../people.ts";

const ASK = "Make this a multiplayer game we can all play at once";
const REPLY = "Made it a multiplayer star-chase. Everyone on the page plays; scores sync live.";
/** Mia's live colour (LIVE_PALETTE): teal. */
const MIA = 2;
/** The star-chase's stars, as seed/starchase.html places them, in turn. */
const STARS: [number, number][] = [[0.66, 0.42], [0.32, 0.36], [0.80, 0.70]];
/** How near a pointer must come to a star to take it, as a share of the
 * arena's width (seed/starchase.html's CATCH), and the arena's height over
 * its width. */
const CATCH = 0.05;
const ASPECT = 9 / 16;
/** The least a step of Mia's moves her pointer, in pixels. */
const MIN_STEP_PX = 3;
/** A typed key's pause, in ms: the ask is typed fast. */
const KEY_MS = 34;

type Point = { x: number; y: number };
type Heard = { name: string; x: number; y: number; score: number };
type Cast = { alex: Page; af: Frame; mia: Page; mf: Frame; d: { base: string; token: string }; aid: string; sid: string; miaAt?: Point };

// What Alex's content frame hears from the shell of other people's
// pointers, kept where the recorder can read it (as realtime.ts keeps it).
const HEARING = `(() => {
  if (globalThis.reelHeard) return;
  const heard = globalThis.reelHeard = { peers: {}, here: false };
  addEventListener("message", e => {
    const m = e.data;
    if (m && m.type === "clax:peers") heard.here = m.here === true;
    if (m && m.type === "clax:peers") for (const p of m.peers) heard.peers[p.color] = JSON.stringify([p.anchor, p.selection]);
  });
})()`;

// Two things in the camera's shell run on real time, not its page clock:
// a text field's blinking caret (Chromium's own timer; `caret-animation:
// manual` keeps it on), and the elapsed clocks beside "working", counted
// from the daemon's real start time to the page's fixed one (hours off, and
// different each run), in the top bar, on the thread's marker and in its
// "sent, waiting" line. The caret stays drawn; the elapsed clocks and the
// waiting line are not drawn in the film. The shell's CSS transitions (a
// thread card's hover, the Comment button turning off) are dropped too: they
// were filmed at a different age from run to run.
const CAMERA_STYLE = `(() => {
  if (window !== window.top) return;
  const add = () => {
    const s = document.createElement("style");
    s.textContent = "textarea, input { caret-animation: manual; } *, *::before, *::after { transition: none !important; } .thread-card .st.ag > small, small.el, .thread-card .waiting { display: none !important; }";
    document.head.append(s);
  };
  if (document.head) add(); else addEventListener("DOMContentLoaded", add);
})()`;

/** What Alex's frame last heard of Mia's shell pointer. */
const pointerHeard = (alex: Page, af: Frame) => quietIn<string | null>(alex, af, `globalThis.reelHeard?.peers[${JSON.stringify(LIVE_PALETTE[MIA])}] ?? null`);

type State = { me: Heard; heard: Heard[]; total: number };

/** A star-chase frame's player and the players its room holds now, as the
 * page answers a "starchase:read" event (seed/starchase.html); null before
 * the page listens. */
const stateIn = (page: Page, frame: Frame) => quietIn<State | null>(page, frame, `(() => {
  delete document.documentElement.dataset.state;
  document.dispatchEvent(new CustomEvent("starchase:read"));
  const s = document.documentElement.dataset.state;
  return s ? JSON.parse(s) : null;
})()`);

/** The players a star-chase frame's room holds now. */
const heardIn = async (page: Page, frame: Frame) => (await stateIn(page, frame))?.heard ?? [];

/** The point at fractions (`fx`, `fy`) of the arena in `frame`, in its page's viewport. */
async function arenaAt(frame: Frame, fx: number, fy: number): Promise<Point> {
  const box = await frame.locator("#arena").boundingBox();
  if (!box) throw new Error("the arena has no box");
  return { x: Math.round(box.x + box.width * fx), y: Math.round(box.y + box.height * fy) };
}

/** Eased fractions of the arena from `from` to `to` in `steps`. */
function path(from: [number, number], to: [number, number], steps: number): [number, number][] {
  return Array.from({ length: steps }, (_, i) => {
    const e = ease((i + 1) / steps, CURVES.standard);
    return [from[0] + (to[0] - from[0]) * e, from[1] + (to[1] - from[1]) * e] as [number, number];
  });
}

/** Mia's score along `steps` toward `star`, from `base`: one more from the
 * first step that comes within CATCH of it. */
function scores(steps: [number, number][], star: [number, number], base: number): number[] {
  let caught = false;
  return steps.map(([x, y]) => {
    caught ||= Math.hypot(x - star[0], (y - star[1]) * ASPECT) < CATCH;
    return caught ? base + 1 : base;
  });
}

/**
 * One frame of play: Mia's pointer (when given) moves to fractions `m` of
 * her arena, off camera, and the film waits until Alex's room holds her
 * there with `score` and his shell has heard her pointer move; Alex's
 * pointer (when given) moves to `a` of his; then a frame rolls.
 */
async function step(d: Director, cast: Cast, m: { at: [number, number]; score: number } | null, a: [number, number] | null): Promise<void> {
  const { alex, af, mia, mf } = cast;
  const to = m && await arenaAt(mf, m.at[0], m.at[1]);
  // A step within a few pixels of where Mia is moves her shell pointer too
  // little for its anchor to change: it is left out.
  if (m && to && (!cast.miaAt || Math.abs(to.x - cast.miaAt.x) + Math.abs(to.y - cast.miaAt.y) >= MIN_STEP_PX)) {
    cast.miaAt = to;
    const before = await pointerHeard(alex, af);
    await d.offstage(() => mia.mouse.move(to.x, to.y));
    let last = "";
    await d.until(async () => {
      const her = (await heardIn(alex, af)).find(p => p.name === "Mia");
      const now = await pointerHeard(alex, af);
      last = `${JSON.stringify(her)}, pointer ${now === before ? "unmoved" : "moved"}`;
      if (!her || her.score !== m.score || Math.abs(her.x - m.at[0]) > 0.01 || Math.abs(her.y - m.at[1]) > 0.01) return false;
      return now !== null && now !== before;
    }, `Alex's arena heard Mia at ${m.at.map(v => v.toFixed(3)).join(", ")} with ${m.score}`).catch(e => {
      throw new Error(`${e instanceof Error ? e.message : String(e)}; last heard ${last}`, { cause: e });
    });
  }
  if (a) {
    const aim = await arenaAt(af, a[0], a[1]);
    await cursorTo(alex, aim.x, aim.y);
    await alex.mouse.move(aim.x, aim.y);
  }
  await d.hold(1000 / 30);
}

export default {
  id: "game",

  async setup(stage: Stage): Promise<Cast> {
    const { d, origin } = await stage.hostedDaemon();
    // The agent: a live session, whose publish makes the pad its own.
    const session = await registerSession(d.base, d.token, "claude", "reel-game");
    const sketch = readFileSync(new URL("../seed/sketch.html", import.meta.url), "utf8");
    const made = await api(d.base, d.token, "/api/artifacts", { method: "POST", session: session.id, body: JSON.stringify({ title: "Team Sketch", files: { "index.html": { content: sketch, encoding: "utf8" } } }) }) as { artifact: { id: string } };
    const aid = made.artifact.id;

    // Alex sets the daemon up and invites Mia, in a browser of the setup's
    // own, so he holds no live colour when she arrives; the camera signs in
    // as him.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    const mia = await memberIn(stage.browser, owner, origin, "Mia", rightsOn(aid), MIA);
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    await mia.setViewportSize({ width: 1280, height: 800 });
    const welcomed = welcomedColor(mia);
    const mf = await openAt(mia, origin, aid, 1);
    const got = await welcomed;
    if (got !== MIA) throw new Error(`Mia joined in live colour ${got}, not ${MIA}`);
    const alex = await stage.newCamera({ storageState: signedIn });
    // The shell reads its event stream without a shared worker in the
    // camera's browser (its fallback, stream.ts bestLink): a shared worker's
    // script, fetched again when Reload loads the shell anew, is a request
    // the deterministic camera never sees finish.
    await alex.context().addInitScript("delete globalThis.SharedWorker");
    await alex.context().addInitScript(CAMERA_STYLE);
    const af = await openAt(alex, origin, aid, 1);
    for (const [page, frame] of [[alex, af], [mia, mf]] as const) {
      await frame.locator("#pad").waitFor({ state: "visible" });
      await page.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    }
    await alex.locator(`.who .tok.p.here[title="Mia"]`).waitFor({ state: "visible" });
    return { alex, af, mia, mf, d: { base: d.base, token: d.token }, aid, sid: session.id };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, mia, aid, sid } = cast;
    const { base, token } = cast.d;
    await d.hold(300);

    // Alex turns comment mode on, picks the pad, and asks.
    d.beat("ask");
    await d.click(alex.getByRole("button", { name: "Comment", exact: true }));
    await d.until(() => quietIn<boolean>(alex, cast.af, `document.documentElement.style.cursor === "crosshair"`), "comment mode is on in Alex's page");
    await d.click(cast.af.locator("#pad"));
    const composer = alex.locator(".composer textarea");
    await composer.waitFor({ state: "visible" });
    await d.hold(150);
    for (const ch of Array.from(ASK)) {
      await d.offstage(() => alex.keyboard.type(ch));
      await d.hold(KEY_MS);
    }
    await d.hold(200);
    await d.click(alex.locator(".composer").getByRole("button", { name: "Post comment" }));
    const card = alex.locator(".section-open .thread-card").first();
    const send = card.getByRole("button", { name: /^Send to / });
    await d.click(send);
    const tid = await card.getAttribute("data-thread");
    if (!tid) throw new Error("the ask's thread card names no thread");

    // The agent takes the thread and shows as working on it.
    d.beat("working");
    await d.offstage(async () => {
      await api(base, token, `/api/sessions/${sid}/feedback?tier=piggyback`);
      await api(base, token, `/api/sessions/${sid}/working/${aid}`, { method: "PUT", body: JSON.stringify({ thread_ids: [tid], message: "Building a multiplayer star-chase" }) });
    });
    await d.until(async () => (await alex.locator(".topbar.working").count()) > 0 && (await card.locator(".st.ag").count()) > 0, "Alex's view shows the agent working");
    await d.move(alex.locator(".topbar"), { offset: { x: 120, y: 0 } });
    await d.hold(1100);

    // The agent publishes the star-chase and replies; both views load it.
    d.beat("reveal");
    const game = readFileSync(new URL("../seed/starchase.html", import.meta.url), "utf8");
    await d.offstage(() => api(base, token, `/api/artifacts/${aid}/versions`, {
      method: "POST", session: sid,
      body: JSON.stringify({ if_version: 1, files: { "index.html": { content: game, encoding: "utf8" } }, capabilities: { room: {}, user: { scopes: ["profile"] } }, note: "A multiplayer star-chase", addresses: [tid] }),
    }));
    await d.offstage(() => api(base, token, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", session: sid, body: JSON.stringify({ body: REPLY, author_kind: "agent" }) }));
    await d.until(async () => (await card.locator(".msg.agent").count()) > 0, "the agent's reply reached Alex's thread");
    await d.hold(500);
    // An agent's publish offers Reload in the top bar of every open view.
    await d.offstage(() => mia.locator(".topbar button.reload").click());
    const mf = await d.offstage(() => contentFrame(mia, aid, 2));
    // Mia's threads are open too, so her arena is the size of Alex's and
    // her shell pointer lands on her orb in his view.
    await d.offstage(async () => {
      const threads = mia.locator("button.threads");
      if ((await threads.getAttribute("aria-pressed")) !== "true") await threads.click();
    });
    await d.until(async () => (await alex.locator(".topbar button.reload").count()) > 0, "Alex's view offers the new version");
    // Alex's Reload loads the shell anew: pressed with the camera still, and
    // filmed again once the new document shows the game, so no frame is
    // taken of a document on its way out. The new document draws the
    // cursor again where it was.
    const reload = alex.locator(".topbar button.reload");
    await d.move(reload);
    const box = (await reload.boundingBox())!;
    const pressed = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await cursorPress(alex);
    await d.offstage(async () => {
      await alex.mouse.down();
      await alex.mouse.up();
    });
    const af = await d.offstage(() => contentFrame(alex, aid, 2));
    await d.until(async () => (await stateIn(alex, af)) !== null, "Alex's star-chase answers");
    await d.offstage(() => cursorTo(alex, pressed.x, pressed.y));
    cast.af = af;
    cast.mf = mf;
    await d.offstage(() => quietIn(alex, af, HEARING));
    // The shell's live layers load once the new document has painted, on
    // the page's clock: two frames, then the film waits until they tell
    // Alex's frame that Mia is here.
    await d.hold(2000 / 30);
    await d.until(() => quietIn<boolean>(alex, af, "globalThis.reelHeard.here"), "Alex's live layers hear Mia");
    // Each view's room holds the other player.
    await d.until(async () => (await heardIn(alex, af)).some(p => p.name === "Mia"), "Alex's arena holds Mia");
    await d.until(async () => (await heardIn(mia, mf)).some(p => p.name === "Alex"), "Mia's arena holds Alex");
    // The arena shows a moment before anyone moves.
    await d.hold(450);

    // Mia's orb sweeps in from her corner and takes the first star.
    const miaFrom = (await stateIn(mia, mf))!.me;
    const miaPath = path([miaFrom.x, miaFrom.y], STARS[0]!, 16);
    const miaScores = scores(miaPath, STARS[0]!, 0);
    if (miaScores.at(-1) !== 1) throw new Error("Mia's path misses the first star");
    for (const [i, at] of miaPath.entries()) await step(d, cast, { at, score: miaScores[i]! }, null);
    await d.hold(250);

    // Alex steers into the arena, then for the next star while Mia makes
    // for the one after.
    d.beat("touch");
    await d.move(af.locator("#arena"), { offset: { x: 0, y: 0 } });
    const alexFrom = (await stateIn(alex, af))!.me;
    const alexPath = path([alexFrom.x, alexFrom.y], STARS[1]!, 18);
    const miaNext = path(STARS[0]!, [0.74, 0.6], 18);
    for (let i = 0; i < 18; i++) await step(d, cast, { at: miaNext[i]!, score: 1 }, alexPath[i]!);
    await d.until(async () => (await stateIn(alex, af))?.me.score === 1, "Alex took the second star");
    // Mia's view hears Alex's score before she takes the third star.
    await d.until(async () => (await stateIn(mia, mf))?.total === 2, "Mia's arena counts two stars");
    const miaLast = path([0.74, 0.6], STARS[2]!, 8);
    const lastScores = scores(miaLast, STARS[2]!, 1);
    for (const [i, at] of miaLast.entries()) await step(d, cast, { at, score: lastScores[i]! }, null);
    await d.hold(1100);
  },
} satisfies Vignette<Cast>;
