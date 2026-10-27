// "Your agents build pages. Clax gives them a home." (spec
// 2026-10-27-sizzle-reel-design §3, the cold open). On a hosted daemon,
// Alex's gallery holds a few of Northwind's pages; an agent session,
// scripted through the API, publishes "Northwind Q4 Revenue", a dashboard
// with KPI tiles, a three-region chart and a filterable accounts table; its
// card lands first in Alex's open gallery from the gallery's stream, without
// a reload; Alex opens it and the app fills the screen, and his pointer
// runs along the chart, its crosshair and tooltip following.
//
// The daemon stamps its own times, and the camera's clock reads REEL_EPOCH:
// the camera's API answers are restamped (`restamp`), each seeded page to an
// age of its own before the film's now and anything else to the film's
// now, so the cards read "2 h ago" and "just now" each run, whatever the
// day.
//
// Beats: `gallery` (the gallery, before the agent publishes), `lands` (the
// new card shows), `open` (Alex clicks it).
import { readFileSync } from "node:fs";
import type { Page, Route } from "@playwright/test";
import { NO_KEY_CONFIG, contentFrame, publishAs, publishNext, registerSession } from "../../e2e/fixtures.ts";
import { SECRET, freePort, ownerOf } from "../../e2e/hosted-helpers.ts";
import { REEL_EPOCH, ROLL_AT_MS, quietIn } from "../camera.ts";
import type { Director, Stage, Vignette } from "../director.ts";

const TITLE = "Northwind Q4 Revenue";
/** The hosted daemon's port: the gallery's header shows it. */
const PORT = 47391;

/** Northwind's earlier pages: title, how many versions, how long ago (min)
 * the gallery says each was last published, and its one line of copy. All
 * invented. */
const SEEDS = [
  { title: "Launch plan", versions: 4, ago: 38, line: "Northwind's November 18 launch, week by week." },
  { title: "Pricing page draft", versions: 3, ago: 140, line: "Three tiers, annual and monthly." },
  { title: "Onboarding funnel", versions: 2, ago: 26 * 60, line: "Sign-up to first project, step by step." },
  { title: "Support triage board", versions: 5, ago: 2 * 24 * 60, line: "Open tickets by queue and age." },
  { title: "Brand colour study", versions: 1, ago: 3 * 24 * 60, line: "Six palettes for the new mark." },
];

const stub = (title: string, line: string) => `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${title}</title></head><body><main><h1>${title}</h1><p>${line}</p></main></body></html>`;

/** The film's now on the camera's clock (the deterministic camera's first frame). */
const FILM_NOW = REEL_EPOCH.getTime() + ROLL_AT_MS;

/** Rewrites the daemon's times in an API answer: an object whose `id` is a
 * seeded page's takes that page's age; every other time, the film's now. */
function restamp(v: unknown, ages: Map<string, number>, own?: number): unknown {
  if (Array.isArray(v)) return v.map(x => restamp(x, ages, own));
  if (!v || typeof v !== "object") return v;
  const o = v as Record<string, unknown>;
  const key = [o.id, o.artifact_id].find(k => typeof k === "string" && ages.has(k)) as string | undefined;
  const age = key ? ages.get(key) : own;
  const out: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(o)) {
    if (typeof x === "string" && /_at$/.test(k) && !Number.isNaN(Date.parse(x))) out[k] = new Date(FILM_NOW - (age ?? 0) * 60_000).toISOString();
    else out[k] = restamp(x, ages, age);
  }
  return out;
}

/** Math.random as mulberry32 from a fixed seed, in every document of the camera's page. */
const SEEDED_RANDOM = `(() => {
  let a = 0x5eed;
  Math.random = () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
})()`;

/** A hosted daemon whose public origin is localhost:`port`; one that took
 * another port (`port` was taken) is not usable as it stands. */
function hosted(stage: Stage, port: number) {
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "http://localhost:${port}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  return stage.startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
}

type Cast = { alex: Page; base: string; token: string; origin: string; sid: string; html: string };

export default {
  id: "coldopen",

  async setup(stage: Stage): Promise<Cast> {
    // The gallery's header names its host, port and all: on PORT it reads
    // the same each run. When PORT is taken (another recording of this
    // vignette, as the fast and deterministic ones built at once), the
    // daemon goes on a free port, and only that header differs.
    let d = await hosted(stage, PORT);
    if (d.port !== PORT) {
      await d.stop();
      const port = await freePort();
      d = await hosted(stage, port);
      if (d.port !== port) throw new Error(`the hosted daemon took port ${d.port}, not ${port}: its public origin would not match`);
    }
    const origin = `http://localhost:${d.port}`;
    const html = readFileSync(new URL("../fixtures/coldopen/revenue.html", import.meta.url), "utf8");

    // The agent of the film, live, with nothing published yet; registered
    // first, it is plain "claude".
    const agent = await registerSession(d.base, d.token, "claude", "coldopen-agent");
    // Another agent published Northwind's earlier pages, oldest first.
    const ages = new Map<string, number>();
    const earlier = await registerSession(d.base, d.token, "codex", "coldopen-earlier");
    for (const s of [...SEEDS].reverse()) {
      const page = { "index.html": stub(s.title, s.line) };
      const { artifact } = await publishAs(d.base, d.token, earlier.id, s.title, page);
      for (let v = 1; v < s.versions; v++) await publishNext(d.base, d.token, earlier.id, artifact.id, v, { files: page });
      ages.set(artifact.id, s.ago);
    }
    // Alex has read his inbox: the new page is its one unread item.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    await owner.evaluate(async () => {
      const r = await fetch("/api/inbox/read", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ all: true }) });
      if (!r.ok) throw new Error(`inbox read: ${r.status} ${await r.text()}`);
    });
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    const alex = await stage.newCamera({ storageState: signedIn });
    // The gallery's footer picks a haiku with Math.random: seeded, it picks the same one each run.
    await alex.addInitScript(SEEDED_RANDOM);
    await alex.context().route(/\/api\/(artifacts(\/[^/?]+)?|inbox(\/summary)?)(\?.*)?$/, async (route: Route) => {
      if (route.request().method() !== "GET") return route.continue();
      const res = await route.fetch();
      const type = res.headers()["content-type"] ?? "";
      if (!res.ok() || !type.includes("json")) return route.fulfill({ response: res });
      return route.fulfill({ response: res, json: restamp(await res.json(), ages) });
    });
    await alex.goto(`${origin}/`);
    // A second tab of Alex's on the gallery, never filmed, keeps the shell's
    // shared stream worker alive across the click into the artifact: the
    // artifact page joins it rather than fetching its script again, a
    // request the deterministic camera would wait on and never see end.
    const keeper = await alex.context().newPage();
    await keeper.goto(`${origin}/`);
    for (const s of SEEDS) await alex.locator(".card-wrap", { hasText: s.title }).waitFor({ state: "visible" });
    await alex.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    return { alex, base: d.base, token: d.token, origin, sid: agent.id, html };
  },

  async play(d: Director, cast: Cast) {
    const { alex, base, token, sid, html } = cast;
    const card = alex.locator(".card-wrap", { hasText: TITLE });

    // Alex's gallery: Northwind's pages, his pointer resting by them.
    d.beat("gallery");
    await d.hold(300);
    // It rests in the gallery's empty right side, where nothing lands: a
    // card arriving under a still pointer takes its hover style when the
    // browser next checks, on its own time, not the camera's.
    const gal = alex.locator("main.gal");
    const box = await gal.boundingBox();
    if (!box) throw new Error("the gallery has no box");
    await d.move(gal, { offset: { x: 1010 - (box.x + box.width / 2), y: 590 - (box.y + box.height / 2) } });
    await d.hold(500);

    // The agent publishes: its card lands first in the gallery, live.
    // The page hears the publish as two stream events (the gallery's
    // version, the inbox's item); once both are in, the card is fetched on
    // the next frame's clock, whatever the network took.
    const heard = () => quietIn<number>(alex, alex.mainFrame(), "window.claxStreamEvents ?? 0");
    const before = await heard();
    const { artifact } = await d.offstage(() => publishAs(base, token, sid, TITLE, { "index.html": html }));
    await d.until(async () => (await heard()) - before >= 2, "Alex's gallery heard the publish");
    d.beat("lands");
    await d.hold(800);
    if ((await card.count()) === 0) throw new Error("the new card did not reach Alex's gallery");
    const order = await quietIn<string[]>(alex, alex.mainFrame(), `[...document.querySelectorAll(".card-wrap h3")].map(h => h.textContent)`);
    if (order[0] !== TITLE) throw new Error(`the new card did not land first: ${JSON.stringify(order)}`);
    await d.move(card.locator("h3"));
    await d.hold(450);

    // Alex opens it: the app fills the screen.
    d.beat("open");
    await d.click(card.locator("a.card"));
    const frame = await d.offstage(() => contentFrame(alex, artifact.id, 1));
    await d.until(async () => (await frame.locator("#chart svg").count()) > 0, "the dashboard drew its chart");
    await d.hold(500);
    // He hides the threads, and the app takes the whole width.
    await d.click(alex.getByRole("button", { name: "Hide threads" }));
    await d.hold(400);
    // His pointer runs along the chart: the crosshair and tooltip follow.
    await d.move(frame.locator("#chart svg"), { offset: { x: -120, y: 0 } });
    await d.move(frame.locator("#chart svg"), { offset: { x: 150, y: -20 } });
    await d.hold(900);
  },
} satisfies Vignette<Cast>;
