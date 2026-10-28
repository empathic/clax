// "Realtime collaboration" (spec 2026-10-27-sizzle-reel-design §3): three
// people on one page at once. On a hosted daemon (a local daemon's browsers
// are all its owner, web/e2e/hosted-helpers.ts), Alex, the owner, watches
// Northwind's launch checklist while Mia and Sam, members with rights on it,
// work on it in browsers of their own: their pointers move over the list
// (Alex sees the shell's arrows and name tags), Mia starts a comment on her
// own item, "Send the invitations", and her draft shows in Alex's threads as
// she types it, and Sam ticks "Book the venue", which ticks in Alex's list.
// Only Alex's browser is filmed.
//
// Mia is teal and Sam blue every run: each is minted until their ID hashes
// to that live colour (people.ts), and both join the page before Alex, so
// neither colour is held when they arrive. Every step of theirs is waited
// for in Alex's page before the film moves on (their pointer's new place,
// the next letter of the draft, the venue's change), so the frame it shows
// on is the same each run, however long the network took.
//
// Beats: `pointers` (Mia's and Sam's pointers arrive), `draft` (Mia starts
// her comment), `tick` (Sam ticks the venue).
import { readFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { publishWith } from "../../e2e/fixtures.ts";
import { openAt, ownerOf, rightsOn } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { CURVES, ease, typingDelays } from "../pace.ts";
import { LIVE_PALETTE, memberIn, welcomedColor } from "../people.ts";

/** The checklist's items, in order. Northwind and its people are invented. */
const ITEMS = [
  { id: "venue", title: "Book the venue", owner: "Sam" },
  { id: "invites", title: "Send the invitations", owner: "Mia" },
  { id: "badges", title: "Print the badges", owner: "Priya" },
  { id: "caterer", title: "Confirm the caterer", owner: "Sam" },
  { id: "keynote", title: "Rehearse the keynote", owner: "Alex" },
];
const VENUE = "Book the venue";
const DRAFT = "Sending these Thursday";
/** Mia's and Sam's live colours (LIVE_PALETTE): teal and blue. */
const MIA = 2;
const SAM = 0;

type Point = { x: number; y: number };
type Cast = { alex: Page; af: Frame; mia: Page; mf: Frame; sam: Page; sf: Frame };

// What Alex's content frame hears from the shell, kept where the recorder
// can read it: each peer's latest place (`clax:peers`, by colour) and how
// many of the page's documents changed (`clax:event` db/changed).
const HEARING = `(() => {
  if (globalThis.reelHeard) return;
  const heard = globalThis.reelHeard = { peers: {}, changed: 0 };
  addEventListener("message", e => {
    const m = e.data;
    if (m && m.type === "clax:peers") for (const p of m.peers) heard.peers[p.color] = JSON.stringify([p.anchor, p.selection]);
    if (m && m.type === "clax:event" && m.ns === "db" && m.topic === "changed") heard.changed += 1;
  });
})()`;

/** What Alex's frame last heard of the peer in live colour `color`. */
const peerHeard = (alex: Page, af: Frame, color: number) => quietIn<string | null>(alex, af, `globalThis.reelHeard.peers[${JSON.stringify(LIVE_PALETTE[color])}] ?? null`);

/** How many document changes Alex's frame has heard. */
const changesHeard = (alex: Page, af: Frame) => quietIn<number>(alex, af, "globalThis.reelHeard.changed");

/** A frame's element `selector`, as a whole point in its page's viewport, `dx` in from its left. */
async function pointIn(page: Page, frame: Frame, selector: string, dx?: number): Promise<Point> {
  const box = await frame.locator(selector).first().boundingBox();
  if (!box) throw new Error(`${selector} has no box in ${page.url()}`);
  return { x: Math.round(box.x + (dx ?? box.width / 2)), y: Math.round(box.y + box.height / 2) };
}

/** People's pointers move to their points, off camera, and the film waits
 * until Alex's frame has heard each one's new place. */
async function moveTo(d: Director, cast: Cast, moves: [Page, number, Point][]): Promise<void> {
  const before = await Promise.all(moves.map(([, color]) => peerHeard(cast.alex, cast.af, color)));
  await d.offstage(async () => {
    for (const [page, , to] of moves) await page.mouse.move(to.x, to.y);
  });
  await d.until(async () => (await Promise.all(moves.map(([, color]) => peerHeard(cast.alex, cast.af, color)))).every((h, i) => h !== null && h !== before[i]), `Alex's view heard ${moves.length === 1 ? "a pointer" : "the pointers"} move`);
}

/** The whole points of an eased glide from `from` to `to` in `steps`, each one apart from the last. */
function path(from: Point, to: Point, steps: number): Point[] {
  const out: Point[] = [];
  for (let i = 1; i <= steps; i++) {
    const e = ease(i / steps, CURVES.standard);
    const p = { x: Math.round(from.x + (to.x - from.x) * e), y: Math.round(from.y + (to.y - from.y) * e) };
    const last = out.at(-1);
    if (!last || last.x !== p.x || last.y !== p.y) out.push(p);
  }
  return out;
}

/** People's pointers glide at once, a step of each per shot of `stepMs`. */
async function glide(d: Director, cast: Cast, moves: [Page, number, Point, Point][], steps: number, stepMs: number): Promise<void> {
  const paths = moves.map(([, , from, to]) => path(from, to, steps));
  for (let i = 0; i < Math.max(...paths.map(p => p.length)); i++) {
    const step: [Page, number, Point][] = [];
    for (const [k, [page, color]] of moves.entries()) if (paths[k]![i]) step.push([page, color, paths[k]![i]!]);
    await moveTo(d, cast, step);
    await d.hold(stepMs);
  }
}

export default {
  id: "realtime",

  async setup(stage: Stage): Promise<Cast> {
    const { d, origin } = await stage.hostedDaemon();
    const html = readFileSync(new URL("../seed/checklist.html", import.meta.url), "utf8");
    const { artifact } = await publishWith(d.base, d.token, "Launch checklist", html, { db: {} });
    const aid = artifact.id;

    // Alex sets the daemon up, seeds the list and invites Mia and Sam, in a
    // browser of the setup's own, then leaves the page, so he holds no live
    // colour when they arrive; the camera signs in as him.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    const seed = await openAt(owner, origin, aid, 1);
    await seed.evaluate(async items => {
      const db = await (window as unknown as { claude: { use(n: string): Promise<{ doc(p: string): { set(v: unknown): Promise<unknown> } }> } }).claude.use("db");
      for (const [order, item] of items.entries()) await db.doc(`items/${item.id}`).set({ title: item.title, owner: item.owner, order, done: false });
    }, ITEMS);
    await owner.goto(`${origin}/`);
    const mia = await memberIn(stage.browser, owner, origin, "Mia", rightsOn(aid), MIA);
    const sam = await memberIn(stage.browser, owner, origin, "Sam", rightsOn(aid), SAM);
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    // Mia, then Sam, then Alex join the page, each in their own colour.
    const frames: Frame[] = [];
    for (const [page, name, color] of [[mia, "Mia", MIA], [sam, "Sam", SAM]] as const) {
      await page.setViewportSize({ width: 1280, height: 800 });
      const welcomed = welcomedColor(page);
      frames.push(await openAt(page, origin, aid, 1));
      const got = await welcomed;
      if (got !== color) throw new Error(`${name} joined in live colour ${got}, not ${color}`);
    }
    const [mf, sf] = frames as [Frame, Frame];
    const alex = await stage.newCamera({ storageState: signedIn });
    const af = await openAt(alex, origin, aid, 1);
    // Each view shows the seeded list and hears the daemon's events as they
    // happen; Alex sees Mia and Sam here, and his frame keeps what it hears.
    for (const [page, frame] of [[alex, af], [mia, mf], [sam, sf]] as const) {
      await frame.getByRole("checkbox", { name: VENUE }).waitFor({ state: "visible" });
      await page.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    }
    for (const name of ["Mia", "Sam"]) await alex.locator(`.who .tok.p.here[title="${name}"]`).waitFor({ state: "visible" });
    await quietIn(alex, af, HEARING);
    return { alex, af, mia, mf, sam, sf };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, af, mia, mf, sam, sf } = cast;
    // Alex rests his pointer by his own item.
    await d.hold(300);
    await d.move(af.getByText("Rehearse the keynote"), { offset: { x: 110, y: 6 } });
    await d.hold(300);

    // Mia's and Sam's pointers come onto the list from its right.
    d.beat("pointers");
    const miaTo = await pointIn(mia, mf, `input[data-item="invites"]`, 260);
    const samTo = await pointIn(sam, sf, `input[data-item="caterer"]`, 180);
    const enter = (p: Point) => ({ x: p.x + 420, y: p.y + 120 });
    await moveTo(d, cast, [[mia, MIA, enter(miaTo)], [sam, SAM, enter(samTo)]]);
    await glide(d, cast, [[mia, MIA, enter(miaTo), miaTo], [sam, SAM, enter(samTo), samTo]], 18, 55);
    await d.hold(300);
    const tags = await quietIn<string[]>(alex, alex.mainFrame(), `[...document.querySelectorAll(".peer-tag")].map(t => t.textContent).sort()`);
    if (tags.join() !== "Mia,Sam") throw new Error(`Alex's view tags ${JSON.stringify(tags)}, not Mia and Sam`);

    // Alex opens the threads, where drafts show.
    const threads = alex.locator("button.threads");
    if ((await threads.getAttribute("aria-pressed")) !== "true") await d.click(threads);
    await d.hold(300);

    // Mia starts a comment on her own item: her draft shows in Alex's
    // threads as she types it, a letter at a time.
    d.beat("draft");
    await d.offstage(async () => {
      await mia.getByRole("button", { name: /^Comment/ }).first().click();
      await mf.waitForFunction(() => document.documentElement.style.cursor === "crosshair");
    });
    const invitesRow = await pointIn(mia, mf, `input[data-item="invites"]`, 120);
    await glide(d, cast, [[mia, MIA, miaTo, invitesRow]], 10, 45);
    await d.offstage(async () => {
      await mia.mouse.click(invitesRow.x, invitesRow.y);
      await mia.locator(".stage .composer textarea").waitFor({ state: "visible" });
    });
    const draft = () => quietIn<string>(alex, alex.mainFrame(), `document.querySelector(".new-writers .live-writers")?.textContent ?? ""`);
    const delays = typingDelays(DRAFT, "realtime");
    for (const [i, ch] of Array.from(DRAFT).entries()) {
      const typed = Array.from(DRAFT).slice(0, i + 1).join("");
      await d.offstage(() => mia.keyboard.type(ch));
      // A trailing space shows as nothing: its draft is the one before.
      await d.until(async () => (await draft()).includes(typed.trimEnd()), `Mia's draft "${typed}" reached Alex's threads`);
      await d.hold(delays[i]!);
    }
    await d.hold(300);

    // Sam ticks the venue: the tick appears in Alex's list.
    d.beat("tick");
    const box = await pointIn(sam, sf, `input[data-item="venue"]`);
    await glide(d, cast, [[sam, SAM, samTo, box]], 14, 45);
    const changes = await changesHeard(alex, af);
    await d.offstage(() => sam.mouse.click(box.x, box.y));
    await d.until(async () => (await changesHeard(alex, af)) > changes, "Sam's tick reached Alex's page");
    await d.hold(450);
    if (!(await quietIn<boolean>(alex, af, `document.querySelector('input[data-item="venue"]')?.checked === true`))) throw new Error("Sam's tick did not show in Alex's list");
    // Alex looks up at the count.
    await d.move(af.getByText("1 of 5 done"), { offset: { x: 70, y: 4 } });
    await d.hold(650);
  },
} satisfies Vignette<Cast>;
