// "Realtime collaboration" (spec 2026-10-27-sizzle-reel-design §3): three
// people on one page at once. On a hosted daemon (a local daemon's browsers
// are all its owner, web/e2e/hosted-helpers.ts), Alex, the owner, watches
// Northwind's launch checklist while Mia and Sam, members with rights on it,
// work on it in browsers of their own: their pointers move over the list
// (Alex sees the shell's arrows and name tags), Mia starts a comment and her
// draft shows in Alex's threads as she types it, and Sam ticks "Book the
// venue", which ticks in Alex's list. Only Alex's browser is filmed.
//
// Beats: `pointers` (Mia's and Sam's pointers arrive), `draft` (Mia starts
// her comment), `tick` (Sam ticks the venue).
import { readFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { publishWith } from "../../e2e/fixtures.ts";
import { memberOf, openAt, ownerOf, rightsOn } from "../../e2e/hosted-helpers.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { CURVES, ease, typingDelays } from "../pace.ts";

/** The checklist's items, in order. Northwind and its people are invented. */
const ITEMS = [
  { id: "venue", title: "Book the venue", owner: "Sam" },
  { id: "invites", title: "Send the invitations", owner: "Mia" },
  { id: "badges", title: "Print the badges", owner: "Priya" },
  { id: "caterer", title: "Confirm the caterer", owner: "Sam" },
  { id: "keynote", title: "Rehearse the keynote", owner: "Alex" },
];
const VENUE = "Book the venue";
const DRAFT = "Deposit is due Friday";

type Cast = { alex: Page; af: Frame; mia: Page; mf: Frame; sam: Page; sf: Frame };

/** A frame's element `selector`, as a point in its page's viewport, `dx` in from its left. */
async function pointIn(page: Page, frame: Frame, selector: string, dx?: number): Promise<{ x: number; y: number }> {
  const box = await frame.locator(selector).first().boundingBox();
  if (!box) throw new Error(`${selector} has no box in ${page.url()}`);
  return { x: box.x + (dx ?? box.width / 2), y: box.y + box.height / 2 };
}

/** A person's pointer glides from `from` to `to` in `steps` eased moves, a
 * shot of `stepMs` each, off camera: what Alex sees is the shell drawing it. */
async function glide(d: Director, page: Page, from: { x: number; y: number }, to: { x: number; y: number }, steps: number, stepMs: number) {
  for (let i = 1; i <= steps; i++) {
    const e = ease(i / steps, CURVES.standard);
    await d.offstage(() => page.mouse.move(from.x + (to.x - from.x) * e, from.y + (to.y - from.y) * e));
    await d.hold(stepMs);
  }
}

/** Two people's pointers glide at once, a step of each per shot. */
async function glideBoth(d: Director, a: [Page, { x: number; y: number }, { x: number; y: number }], b: [Page, { x: number; y: number }, { x: number; y: number }], steps: number, stepMs: number) {
  for (let i = 1; i <= steps; i++) {
    const e = ease(i / steps, CURVES.standard);
    for (const [page, from, to] of [a, b]) {
      await d.offstage(() => page.mouse.move(from.x + (to.x - from.x) * e, from.y + (to.y - from.y) * e));
    }
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
    // browser of the setup's own; the camera then signs in as him.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    const seed = await openAt(owner, origin, aid, 1);
    await seed.evaluate(async items => {
      const db = await (window as unknown as { claude: { use(n: string): Promise<{ doc(p: string): { set(v: unknown): Promise<unknown> } }> } }).claude.use("db");
      for (const [order, item] of items.entries()) await db.doc(`items/${item.id}`).set({ title: item.title, owner: item.owner, order, done: false });
    }, ITEMS);
    const mia = await memberOf(stage.browser, owner, origin, "Mia", rightsOn(aid));
    const sam = await memberOf(stage.browser, owner, origin, "Sam", rightsOn(aid));
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    const alex = await stage.newCamera({ storageState: signedIn });
    const af = await openAt(alex, origin, aid, 1);
    for (const p of [mia, sam]) await p.setViewportSize({ width: 1280, height: 800 });
    const mf = await openAt(mia, origin, aid, 1);
    const sf = await openAt(sam, origin, aid, 1);
    // Each view shows the seeded list and hears the daemon's events as they happen.
    for (const [page, frame] of [[alex, af], [mia, mf], [sam, sf]] as const) {
      await frame.getByRole("checkbox", { name: VENUE }).waitFor({ state: "visible" });
      await page.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    }
    return { alex, af, mia, mf, sam, sf };
  },

  async play(d: Director, { alex, af, mia, mf, sam, sf }: Cast): Promise<void> {
    // Alex rests his pointer by his own item.
    await d.hold(300);
    await d.move(af.getByText("Rehearse the keynote"), { offset: { x: 110, y: 6 } });
    await d.hold(300);

    // Mia's and Sam's pointers come onto the list from its right.
    d.beat("pointers");
    const miaTo = await pointIn(mia, mf, `input[data-item="invites"]`, 260);
    const samTo = await pointIn(sam, sf, `input[data-item="caterer"]`, 180);
    const enter = (p: { x: number; y: number }) => ({ x: p.x + 420, y: p.y + 120 });
    await d.offstage(async () => { await mia.mouse.move(enter(miaTo).x, enter(miaTo).y); await sam.mouse.move(enter(samTo).x, enter(samTo).y); });
    await glideBoth(d, [mia, enter(miaTo), miaTo], [sam, enter(samTo), samTo], 18, 55);
    await d.until(async () => (await alex.locator(".peer-tag").count()) === 2, "Mia's and Sam's pointers showed in Alex's view");
    await d.hold(500);

    // Alex opens the threads, where drafts show.
    const threads = alex.locator("button.threads");
    if ((await threads.getAttribute("aria-pressed")) !== "true") await d.click(threads);
    await d.hold(300);

    // Mia starts a comment on the venue: her draft shows in Alex's threads as she types.
    d.beat("draft");
    await d.offstage(async () => {
      await mia.getByRole("button", { name: /^Comment/ }).first().click();
      await mf.waitForFunction(() => document.documentElement.style.cursor === "crosshair");
    });
    const venueRow = await pointIn(mia, mf, `input[data-item="venue"]`, 200);
    await glide(d, mia, miaTo, venueRow, 10, 45);
    await d.offstage(async () => {
      await mia.mouse.click(venueRow.x, venueRow.y);
      await mia.locator(".stage .composer textarea").waitFor({ state: "visible" });
    });
    const delays = typingDelays(DRAFT, "realtime");
    for (const [i, ch] of Array.from(DRAFT).entries()) {
      await d.offstage(() => mia.keyboard.type(ch));
      await d.hold(delays[i]!);
    }
    await d.until(async () => ((await alex.locator(".new-writers .live-writers").textContent()) ?? "").includes(DRAFT), "Mia's draft reached Alex's threads");
    await d.hold(500);

    // Sam ticks the venue: the tick appears in Alex's list.
    d.beat("tick");
    const box = await pointIn(sam, sf, `input[data-item="venue"]`);
    await glide(d, sam, samTo, box, 14, 45);
    await d.offstage(() => sam.mouse.click(box.x, box.y));
    await d.until(() => af.getByRole("checkbox", { name: VENUE }).isChecked(), "Sam's tick reached Alex's list");
    await d.hold(600);
    // Alex looks up at the count.
    await d.move(af.getByText("1 of 5 done"), { offset: { x: 70, y: 4 } });
    await d.hold(1000);
  },
} satisfies Vignette<Cast>;
