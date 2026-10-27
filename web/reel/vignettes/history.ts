// "Every version, kept" (spec 2026-10-27-sizzle-reel-design §3, `history`):
// Northwind's launch plan has five versions an agent published over a few
// days, from a plain list (v1) to a planning board with its numbers and a
// timeline (v5). On a local daemon, Alex opens the version menu, its
// changelog of who published what and why, and steps back through it: v4,
// v3, v2, the page changing under him each time. The agent then restores v2
// as a new version (`versions:restore`, the API an agent's restore uses:
// the shell has no restore control of its own), Alex reloads onto it, and
// the menu shows v6 "Restored v2" on top of the five it kept.
//
// The versions were published seconds before the film, and the camera's
// clock stands at REEL_EPOCH: so that the menu's "when" reads as a history
// (and the same each run), the camera's copies of the artifact's data name
// each version's publish time as days, hours and minutes before the roll
// (`staged`); nothing else of them changes.
//
// Every version choice is a navigation of the shell: the press is filmed,
// and the film waits off camera until the chosen version's page has loaded
// and the version button names it, so the frame it first shows on is the
// same each run.
//
// Beats: `menu` (the version menu opens), `scrub` (the first step back),
// `restore` (the restored version arrives).
import { readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { api, contentFrame, registerSession, streamLive } from "../../e2e/fixtures.ts";
import { quietIn, REEL_EPOCH, ROLL_AT_MS } from "../camera.ts";
import { cursorPress, cursorTo } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";

const TITLE = "Northwind Launch Plan";
/** The five versions, oldest first: label, note, and how long before the roll each was published. */
const VERSIONS = [
  { label: "First draft", note: "The launch tasks as a list", ago: 3 * 86_400_000 },
  { label: "By week", note: "Grouped by week, with owners and ticks", ago: 2 * 86_400_000 },
  { label: "Owners and status", note: "A table with due dates, status and progress", ago: 26 * 3_600_000 },
  { label: "Board", note: "To do, In progress and Done as a board", ago: 5 * 3_600_000 },
  { label: "Planning board", note: "Launch countdown, numbers and a timeline", ago: 40 * 60_000 },
];
const RESTORED = { label: "Restored v2", note: "Back to the week-by-week checklist, as Alex asked" };
/** The camera's time while it films. */
const ROLL = REEL_EPOCH.getTime() + ROLL_AT_MS;

type Cast = { page: Page; base: string; token: string; sid: string; aid: string; staged: Map<string, string> };

/**
 * From the camera's next document on, the shell's first-load data (its
 * `clax-boot` block, which carries the versions) names each version's staged
 * publish time for its real one: rewritten when the document has been
 * parsed and before its deferred scripts, the shell's, run. Each call's map
 * replaces the last one's. The document itself is the daemon's, untouched
 * in transit: one the recorder answered would lose its local address, and
 * Chromium's local network checks would refuse it the artifact's frame.
 */
async function stageTimes(page: Page, base: string, staged: Map<string, string>): Promise<void> {
  await page.addInitScript(
    ([origin, pairs]) => {
      if (window !== window.top || location.origin !== origin) return;
      const w = window as unknown as { reelStaged?: [string, string][] };
      const first = !w.reelStaged;
      w.reelStaged = pairs;
      if (!first) return;
      document.addEventListener("readystatechange", () => {
        const boot = document.getElementById("clax-boot");
        if (document.readyState !== "interactive" || !boot?.textContent) return;
        let text = boot.textContent;
        for (const [real, shown] of w.reelStaged!) text = text.replaceAll(real, shown);
        boot.textContent = text;
      });
    },
    [base, [...staged]] as [string, [string, string][]],
  );
}

/** Version `n`'s page, from the vignette's fixtures. */
const fixture = (n: number) => readFileSync(new URL(`../fixtures/history/v${n}.html`, import.meta.url), "utf8");

/** The version button, which names the version shown ("v4of 5 ▾"). */
const versionButton = (page: Page) => page.locator("button.vbtn");

/** The version menu's row for version `n`. */
const rowOf = (page: Page, n: number) => page.locator(".vmenu .vrow a").filter({ has: page.locator(".g", { hasText: new RegExp(`^v${n}$`) }) });

/** What the shell's version button says, read without a gesture. */
const shownText = (page: Page) => quietIn<string>(page, page.mainFrame(), `document.querySelector("button.vbtn")?.textContent ?? ""`);

/** Waits off camera until the shell shows version `n` of `latest`, its page loaded in the frame. */
async function landed(d: Director, cast: Cast, n: number, latest: number): Promise<void> {
  await d.offstage(() => contentFrame(cast.page, cast.aid, n));
  await d.until(async () => (await shownText(cast.page)).startsWith(`v${n}of ${latest}`), `the shell shows v${n} of ${latest}`);
  await closeThreads(d, cast.page);
}

/**
 * Closes the threads panel, which every load of the shell opens at this
 * width and which is empty here, so the page keeps the whole width: as the
 * panel's own toggle, pressed off camera with no pointer moved.
 */
async function closeThreads(d: Director, page: Page): Promise<void> {
  const open = () => quietIn<boolean>(page, page.mainFrame(), `document.querySelector("button.threads")?.getAttribute("aria-pressed") === "true"`);
  if (!(await open())) return;
  await d.offstage(() => quietIn(page, page.mainFrame(), `document.querySelector("button.threads").click()`));
  await d.until(async () => !(await open()), "the threads panel closed");
}

/**
 * The cursor moves to `target` and presses it; the press starts a load of
 * the shell, which the film waits out off camera (until version `n` of
 * `latest` shows), and the cursor is put back where it was in the new
 * document.
 */
async function pressAndLand(d: Director, cast: Cast, target: Locator, n: number, latest: number): Promise<void> {
  const { page } = cast;
  await d.move(target);
  const box = (await target.boundingBox())!;
  const at = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await cursorPress(page);
  await page.mouse.down();
  await d.hold(70);
  await d.offstage(() => page.mouse.up());
  await landed(d, cast, n, latest);
  await d.offstage(() => cursorTo(page, Math.round(at.x * 10) / 10, Math.round(at.y * 10) / 10));
}

/** Opens the version menu and waits until its rows show. */
async function openMenu(d: Director, page: Page): Promise<void> {
  await d.click(versionButton(page));
  await d.until(async () => (await quietIn<number>(page, page.mainFrame(), `document.querySelectorAll(".vmenu .vrow").length`)) > 0, "the version menu shows its rows");
}

export default {
  id: "history",

  async setup(stage: Stage): Promise<Cast> {
    const d = await stage.startDaemon();
    const { base, token } = d;
    // An agent session publishes every version, so the menu names it.
    const sid = (await registerSession(base, token, "claude", "reel-history")).id;
    const staged = new Map<string, string>();
    let aid = "";
    for (const [i, v] of VERSIONS.entries()) {
      const n = i + 1;
      const body = { title: TITLE, label: v.label, note: v.note, files: { "index.html": { content: fixture(n), encoding: "utf8" } }, ...(aid ? { if_version: n - 1 } : {}) };
      const r = (await api(base, token, aid ? `/api/artifacts/${aid}/versions` : "/api/artifacts", { method: "POST", session: sid, body: JSON.stringify(body) })) as { artifact: { id: string }; version: { created_at: string } };
      aid = r.artifact.id;
      staged.set(r.version.created_at, new Date(ROLL - v.ago).toISOString());
    }

    const page = await stage.newCamera();
    // The camera's copies of the artifact's data name the staged publish times.
    await stageTimes(page, base, staged);
    // A gallery tab of Alex's, never filmed, keeps the shell's shared
    // stream worker running while the camera's shell loads again for each
    // version: else each load starts the worker anew, mid film, and its
    // script's load never ends as far as the camera's page can tell.
    const keeper = await page.context().newPage();
    await keeper.goto(`${base}/`);
    await streamLive(keeper);
    await page.goto(`${base}/a/${aid}`);
    await contentFrame(page, aid, VERSIONS.length);
    await streamLive(page);
    await versionButton(page).waitFor({ state: "visible" });
    await quietIn(page, page.mainFrame(), `document.querySelector('button.threads[aria-pressed="true"]')?.click()`);
    await page.locator('button.threads[aria-pressed="false"]').waitFor();
    return { page, base, token, sid, aid, staged };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { page, base, token, sid, aid, staged } = cast;
    const latest = VERSIONS.length;
    await d.hold(300);

    // Alex opens the version menu: five versions, each with who, when and why.
    d.beat("menu");
    await openMenu(d, page);
    const menu = await quietIn<string>(page, page.mainFrame(), `document.querySelector(".vmenu")?.textContent ?? ""`);
    if (!menu.includes(" min ago") || !menu.includes("3 d ago")) throw new Error(`the version menu shows no staged times: ${menu}`);
    await d.hold(600);

    // He steps back through them, the page changing under him each time.
    d.beat("scrub");
    for (const n of [4, 3, 2]) {
      if (n !== 4) await openMenu(d, page);
      await d.hold(n === 4 ? 0 : 150);
      await pressAndLand(d, cast, rowOf(page, n), n, latest);
      await d.hold(n === 2 ? 600 : 350);
    }

    // The agent restores v2 as a new version; Alex reloads onto it.
    d.beat("restore");
    await d.offstage(async () => {
      const r = (await api(base, token, `/api/artifacts/${aid}/versions:restore`, { method: "POST", session: sid, body: JSON.stringify({ version: 2, if_version: latest, ...RESTORED }) })) as { version: { created_at: string } };
      staged.set(r.version.created_at, new Date(ROLL).toISOString());
      await stageTimes(page, base, staged);
    });
    const reload = page.locator("button.reload");
    await d.until(() => reload.isVisible(), "the Reload button offers the restored version");
    await d.hold(300);
    await pressAndLand(d, cast, reload, latest + 1, latest + 1);
    await d.hold(300);
    // The menu shows the restored version on top, and every one before it.
    await openMenu(d, page);
    await d.move(rowOf(page, latest + 1), { offset: { x: 130, y: -10 } });
    await d.hold(1000);
  },
} satisfies Vignette<Cast>;
