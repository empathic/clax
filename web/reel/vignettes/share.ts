// "Share it" (spec 2026-10-27-sizzle-reel-design §3): a page goes from one
// person to two. On a hosted daemon (standing in for clax.dev), Alex, the
// owner, opens Share on Northwind's Q3 board deck, finds Priya among the
// people of his Clax, gives her Can comment, adds her and copies the link.
// Priya, signed in in a browser of her own, opens the deck: her token
// lights up in Alex's top bar (his threads open by now), her named pointer
// comes up the ARR chart to the ARR figure, and she comments on the figure:
// her draft shows in his threads as she types it and lands there as a
// thread. Only Alex's browser is filmed.
//
// Priya is magenta every run: she is minted until her ID hashes to that
// live colour (people.ts), and the daemon is set up again in the rare run
// where Alex's own ID hashes to it, so that Alex, on the page first, never
// holds her colour when she arrives. Every step of hers is waited for in
// Alex's page before the film moves on (her token in his top bar, her
// pointer's new place, the next letter of her draft, her thread), so the
// frame it shows on is the same each run, however long the network took.
//
// Beats: `share` (Alex opens Share and adds Priya), `arrive` (Priya opens
// the deck), `comment` (Priya comments on the chart).
import { readFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { publishWith } from "../../e2e/fixtures.ts";
import { openAt, ownerOf } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorPress } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { CURVES, ease, typingDelays } from "../pace.ts";
import { LIVE_PALETTE, memberIn, preferredColor, welcomedColor } from "../people.ts";

const TITLE = "Northwind Board Deck";
const COMMENT = "Split this by region?";
/** Priya's live colour (LIVE_PALETTE): magenta. */
const PRIYA = 3;
/** How many daemons setup starts, at most, to find Alex a colour other than Priya's. */
const MAX_DAEMONS = 6;

type Point = { x: number; y: number };
type Cast = { alex: Page; af: Frame; priya: Page; origin: string; aid: string };

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

/** What Alex's frame last heard of Priya's place. */
const priyaHeard = (cast: Cast) => quietIn<string | null>(cast.alex, cast.af, `globalThis.reelHeard.peers[${JSON.stringify(LIVE_PALETTE[PRIYA])}] ?? null`);

/** Evaluates `expr` in Alex's shell, granting no gesture. */
const inShell = <T>(cast: Cast, expr: string) => quietIn<T>(cast.alex, cast.alex.mainFrame(), expr);

/** A frame's element `selector`, as a whole point in its page's viewport, at (`fx`, `fy`) of its box. */
async function pointIn(page: Page, frame: Frame, selector: string, fx = 0.5, fy = 0.5): Promise<Point> {
  const box = await frame.locator(selector).first().boundingBox();
  if (!box) throw new Error(`${selector} has no box in ${page.url()}`);
  return { x: Math.round(box.x + box.width * fx), y: Math.round(box.y + box.height * fy) };
}

/** Priya's pointer moves to `to`, off camera, and the film waits until Alex's frame has heard its new place. */
async function moveTo(d: Director, cast: Cast, to: Point): Promise<void> {
  const before = await priyaHeard(cast);
  await d.offstage(() => cast.priya.mouse.move(to.x, to.y));
  await d.until(async () => {
    const h = await priyaHeard(cast);
    return h !== null && h !== before;
  }, "Alex's view heard Priya's pointer move");
}

/** Priya's pointer glides from `from` to `to` on the standard curve, a step per shot of `stepMs`. */
async function glide(d: Director, cast: Cast, from: Point, to: Point, steps: number, stepMs: number): Promise<void> {
  let last: Point | null = null;
  for (let i = 1; i <= steps; i++) {
    const e = ease(i / steps, CURVES.standard);
    const p = { x: Math.round(from.x + (to.x - from.x) * e), y: Math.round(from.y + (to.y - from.y) * e) };
    if (last && last.x === p.x && last.y === p.y) continue;
    last = p;
    await moveTo(d, cast, p);
    await d.hold(stepMs);
  }
}

export default {
  id: "share",

  async setup(stage: Stage): Promise<Cast> {
    const html = readFileSync(new URL("../fixtures/share/deck.html", import.meta.url), "utf8");
    // Alex sets the daemon up in a browser of the setup's own; a daemon
    // where his ID hashes to Priya's colour is left for another.
    for (let tries = 1; ; tries++) {
      const { d, origin } = await stage.hostedDaemon();
      const owner = await ownerOf(stage.browser, origin, "Alex");
      const me = await owner.evaluate(async () => (await (await fetch("/api/viewers/me")).json()).viewer.public_id as string);
      if (preferredColor(me) === PRIYA) {
        await owner.context().close();
        if (tries >= MAX_DAEMONS) throw new Error(`Alex hashed to Priya's live colour on ${tries} daemons`);
        continue;
      }
      const { artifact } = await publishWith(d.base, d.token, TITLE, html, {});
      const aid = artifact.id;

      // Priya is one of the people of Alex's Clax, with her own space and
      // no rights on the deck: Share is what lets her in.
      const priya = await memberIn(stage.browser, owner, origin, "Priya", [], PRIYA);
      await priya.setViewportSize({ width: 1280, height: 800 });
      const signedIn = await owner.context().storageState();
      await owner.context().close();

      const alex = await stage.newCamera({ storageState: signedIn });
      await alex.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin });
      // The Share dialog's Site line names the deck's site at this
      // daemon's address, a random port on localhost: its sharing answer
      // reaches Alex's page without the site, so the dialog draws no Site
      // line, as for a daemon that names none, and the film never shows
      // localhost nor differs between runs.
      await alex.route(`${origin}/api/hosted/artifacts/*/sharing`, async route => {
        const response = await route.fetch();
        const { site_access: _a, site_url: _u, ...json } = (await response.json()) as Record<string, unknown>;
        await route.fulfill({ response, json });
      });
      // The text caret blinks on the browser's own clock, not the page's:
      // the film draws none, so a frame with the add field focused is the
      // same each run.
      await alex.addInitScript(() => {
        addEventListener("DOMContentLoaded", () => {
          const style = document.createElement("style");
          style.textContent = "* { caret-color: transparent !important; }";
          document.head.append(style);
        });
      });
      const welcomed = welcomedColor(alex);
      const af = await openAt(alex, origin, aid, 1);
      const mine = await welcomed;
      if (mine !== preferredColor(me)) throw new Error(`Alex joined in live colour ${mine}, not his own ${preferredColor(me)}`);
      await af.locator("#arr").waitFor({ state: "visible" });
      await alex.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
      // His threads are closed: he opens them once he has shared the deck.
      const threads = alex.locator("button.threads");
      if ((await threads.getAttribute("aria-pressed")) === "true") await threads.click();
      await alex.getByRole("button", { name: "Share", exact: true }).waitFor({ state: "visible" });
      await quietIn(alex, af, HEARING);
      // Priya waits at her gallery, out of the deck.
      if ((await priya.request.get(`${origin}/api/artifacts/${aid}`)).ok()) throw new Error("Priya could read the deck before it was shared with her");
      return { alex, af, priya, origin, aid };
    }
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, priya, origin, aid } = cast;
    await d.hold(100);

    // Alex opens Share, finds Priya among his people, gives her Can
    // comment, adds her and copies the link.
    d.beat("share");
    // The press and the dialog's opening happen between two frames: the
    // browser moves hover off the button on a clock of its own once the
    // dialog covers it, so a pointer move, with no frame between, settles it.
    const share = alex.getByRole("button", { name: "Share", exact: true });
    await d.move(share);
    await cursorPress(alex);
    await d.offstage(async () => {
      const box = (await share.boundingBox())!;
      await alex.mouse.down();
      await alex.mouse.up();
      await alex.getByRole("dialog").waitFor({ state: "visible" });
      await alex.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 1);
    });
    await d.hold(220);
    const dialog = alex.getByRole("dialog", { name: `Share ${TITLE}` });
    // The dialog opens with its add field focused: Alex types straight in.
    const who = dialog.getByRole("textbox", { name: "GitHub login" });
    await d.until(async () => inShell<boolean>(cast, `document.activeElement?.getAttribute("name") === "who"`), "the add field has the focus");
    for (const [i, ch] of Array.from("Pri").entries()) {
      await alex.keyboard.type(ch);
      await d.hold(typingDelays("Pri", "share")[i]!);
    }
    if ((await who.inputValue()) !== "Pri") throw new Error("the add field did not take Alex's typing");
    await d.hold(120);
    await d.click(dialog.locator(".hints button", { hasText: "Priya" }));
    const access = dialog.getByRole("combobox", { name: "Access for the person added" });
    await d.move(access);
    await cursorPress(alex);
    await d.offstage(() => access.selectOption("commenter"));
    await d.hold(150);
    await d.click(dialog.getByRole("button", { name: "Add", exact: true }));
    await d.until(async () => inShell<boolean>(cast, `[...document.querySelectorAll(".share-panel .shared li")].some(li => li.textContent.includes("Priya"))`), "Priya is among the people with access");
    await d.hold(250);
    await d.click(dialog.getByRole("button", { name: "Copy link" }));
    await d.until(async () => inShell<boolean>(cast, `[...document.querySelectorAll(".share-panel footer .copy")].some(b => b.textContent === "Copied")`), "the link was copied");
    await d.hold(300);
    await d.press("Escape");
    // Alex opens his threads, where Priya's comment will show.
    await d.click(alex.locator("button.threads"));

    // Priya opens the link: her token lights up in Alex's top bar.
    d.beat("arrive");
    const link = await d.offstage(() => alex.evaluate(() => navigator.clipboard.readText()));
    if (link !== `${origin}/a/${aid}`) throw new Error(`the link copied was ${link}, not the deck's`);
    const pf = await d.offstage(async () => {
      const welcomed = welcomedColor(priya);
      const frame = await openAt(priya, origin, aid, 1);
      const got = await welcomed;
      if (got !== PRIYA) throw new Error(`Priya joined in live colour ${got}, not ${PRIYA}`);
      await frame.locator("#arr").waitFor({ state: "visible" });
      await priya.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
      return frame;
    });
    await d.until(async () => inShell<boolean>(cast, `!!document.querySelector('.who .tok.p.here[title="Priya"]')`), "Priya's token is in Alex's top bar");
    await d.hold(200);

    // Her pointer comes up the ARR chart onto the ARR figure.
    const arr = await pointIn(priya, pf, ".kpi .v", 0.62, 0.55);
    const enter = await pointIn(priya, pf, `#arr rect[data-quarter="Q3 ’26"]`, 0.5, 0.6);
    await moveTo(d, cast, enter);
    await glide(d, cast, enter, arr, 14, 45);
    await d.hold(150);
    const tags = await inShell<string[]>(cast, `[...document.querySelectorAll(".peer-tag")].map(t => t.textContent)`);
    if (tags.join() !== "Priya") throw new Error(`Alex's view tags ${JSON.stringify(tags)}, not Priya`);

    // She comments on the figure: her draft shows in Alex's threads as she
    // types it, and lands there as a thread.
    d.beat("comment");
    await d.offstage(async () => {
      await priya.getByRole("button", { name: /^Comment/ }).first().click();
      await pf.waitForFunction(() => document.documentElement.style.cursor === "crosshair");
      await priya.mouse.click(arr.x, arr.y);
      await priya.locator(".stage .composer textarea").waitFor({ state: "visible" });
    });
    const draft = () => inShell<string>(cast, `document.querySelector(".new-writers .live-writers")?.textContent ?? ""`);
    const delays = typingDelays(COMMENT, "share");
    for (const [i, ch] of Array.from(COMMENT).entries()) {
      const typed = Array.from(COMMENT).slice(0, i + 1).join("");
      await d.offstage(() => priya.keyboard.type(ch));
      // A trailing space shows as nothing: its draft is the one before.
      await d.until(async () => (await draft()).includes(typed.trimEnd()), `Priya's draft "${typed}" reached Alex's threads`);
      await d.hold(delays[i]!);
    }
    await d.hold(150);
    await d.offstage(() => priya.locator(".stage .composer button[type=submit]").click());
    await d.until(async () => inShell<boolean>(cast, `[...document.querySelectorAll(".thread-card")].some(c => c.textContent.includes(${JSON.stringify(COMMENT)}))`), "Priya's thread is in Alex's threads");
    await d.hold(660);
  },
} satisfies Vignette<Cast>;
