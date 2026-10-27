// "Your agents build pages. Clax gives them a home." (spec
// 2026-10-27-sizzle-reel-design §3, the cold open): ask, and it delivers.
// On a hosted daemon, Alex's gallery holds a few of Northwind's pages. He
// presses New artifact, names it "Q4 Revenue", writes what he wants and
// sends the brief to his live `claude` agent, all in the gallery's own New
// dialog; he lands on the placeholder page with the brief as its first
// thread. The agent, scripted through the API, says it is working on the
// thread, then publishes v2 addressing it: a dashboard with KPI tiles, a
// three-region chart and a filterable accounts table
// (fixtures/coldopen/revenue.html). Alex's view offers Reload; he takes it
// and the dashboard replaces the placeholder; his pointer runs along the
// chart, its crosshair and tooltip following.
//
// Times: the daemon stamps real ones. The camera's page runs LAG_MS behind
// real time, so everything made during the film reads "just now" and the
// agent's elapsed clock 0:00 (both clamp at zero), and the seeded pages'
// times are restamped in the camera's API answers (`restamp`) to fixed ages
// before the page's clock.
//
// Beats: `ask` (the gallery, then New), `working` (the agent takes the
// brief), `reveal` (v2 shows, after Reload), `touch` (the pointer on the chart).
import { readFileSync } from "node:fs";
import type { Frame, Locator, Page, Route } from "@playwright/test";
import { NO_KEY_CONFIG, contentFrame, publishAs, publishNext, registerSession, setWorking } from "../../e2e/fixtures.ts";
import { SECRET, freePort, ownerOf } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorPress } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";

const NAME = "Q4 Revenue";
const BRIEF = "Q4 revenue by region, with our top accounts";
/** The hosted daemon's port: the gallery's header shows it. */
const PORT = 47391;
/** How far the camera's page clock runs behind real time. */
const LAG_MS = 120_000;
/** How long the director holds after each typed key: under a frame. */
const KEY_MS = 25;

/** The opening of the one haiku the shell shows (web/shell/src/view/haiku.json). */
const HAIKU = "A page, freshly made";

/** A style in each of the camera's documents that stops the caret's blink. */
const STILL_CARET = `(() => {
  const add = () => { const s = document.createElement("style"); s.textContent = "* { caret-animation: manual !important; }"; document.head.append(s); };
  if (document.head) add(); else document.addEventListener("DOMContentLoaded", add, { once: true });
})()`;

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

/** Rewrites the times of a seeded page in an API answer (an object whose
 * `id` or `artifact_id` names one, and everything inside it) to its age
 * before `now`; every other time is left as the daemon stamped it. */
function restamp(v: unknown, ages: Map<string, number>, now: number, own?: number): unknown {
  if (Array.isArray(v)) return v.map(x => restamp(x, ages, now, own));
  if (!v || typeof v !== "object") return v;
  const o = v as Record<string, unknown>;
  const key = [o.id, o.artifact_id].find(k => typeof k === "string" && ages.has(k)) as string | undefined;
  const age = key ? ages.get(key) : own;
  const out: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(o)) {
    if (age !== undefined && typeof x === "string" && /_at$/.test(k) && !Number.isNaN(Date.parse(x))) out[k] = new Date(now - age * 60_000).toISOString();
    else out[k] = restamp(x, ages, now, age);
  }
  return out;
}

/** A hosted daemon whose public origin is localhost:`port`; one that took
 * another port (`port` was taken) is not usable as it stands. */
function hosted(stage: Stage, port: number) {
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "http://localhost:${port}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  return stage.startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
}

/** `target`'s centre in the viewport, as the director moves to it. */
async function centre(target: Locator): Promise<{ x: number; y: number }> {
  await target.waitFor({ state: "visible" });
  const box = await target.boundingBox();
  if (!box) throw new Error(`${String(target)} has no box`);
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

type Cast = { alex: Page; base: string; token: string; sid: string; html: string };

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

    // Alex's agent, live and offered by New; registered first, it is plain
    // "claude".
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
    // Alex has read his inbox, and has ClaxBot off for himself (his flag,
    // as Admin › People sets it), so New hands the brief to an agent.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    await owner.evaluate(async () => {
      for (const [url, method, body] of [["/api/inbox/read", "POST", { all: true }], [`/api/admin/people/${encodeURIComponent("*owner")}/flags`, "PUT", { claxbot: false }]] as const) {
        const r = await fetch(url, { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
        if (!r.ok) throw new Error(`${method} ${url}: ${r.status} ${await r.text()}`);
      }
    });
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    const alex = await stage.newCamera({ storageState: signedIn });
    // The page's clock behind real time; the seeded pages' ages count back
    // from where it stands now (a few seconds of setup later is still the
    // same whole minute of each age).
    const now = Date.now() - LAG_MS;
    await alex.clock.setSystemTime(now);
    await alex.context().route(/\/api\/(artifacts(\/[^/?]+)?|inbox(\/summary)?)(\?.*)?$/, async (route: Route) => {
      if (route.request().method() !== "GET") return route.continue();
      const res = await route.fetch();
      const type = res.headers()["content-type"] ?? "";
      if (!res.ok() || !type.includes("json")) return route.fulfill({ response: res });
      return route.fulfill({ response: res, json: restamp(await res.json(), ages, now) });
    });
    // A text caret blinks on the browser's own time, not the camera's: it
    // holds still here (caret-animation: manual), shown while it is in a field.
    await alex.addInitScript(STILL_CARET);
    // The shell's haiku (the gallery's footer, the working strip's line)
    // picks from its list by Math.random or by the working record's key, a
    // fresh ULID each run: the camera's list holds one of them, the same
    // one each run.
    await alex.context().route(/\/haiku[^/]*\.json$/, async (route: Route) => {
      const res = await route.fetch();
      const list = (await res.json()) as string[];
      const one = list.find(h => h.startsWith(HAIKU));
      if (!one) throw new Error(`the shell's haiku list has none starting "${HAIKU}"`);
      return route.fulfill({ response: res, json: [one] });
    });
    await alex.goto(`${origin}/`);
    // A second tab of Alex's on the gallery, never filmed, keeps the shell's
    // shared stream worker alive across the move into the artifact: the
    // artifact page joins it rather than fetching its script again, a
    // request the deterministic camera would wait on and never see end.
    const keeper = await alex.context().newPage();
    await keeper.goto(`${origin}/`);
    for (const s of SEEDS) await alex.locator(".card-wrap", { hasText: s.title }).waitFor({ state: "visible" });
    await alex.getByRole("button", { name: "New artifact" }).waitFor({ state: "visible" });
    await alex.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    return { alex, base: d.base, token: d.token, sid: agent.id, html };
  },

  async play(d: Director, cast: Cast) {
    const { alex, base, token, sid, html } = cast;
    // Where the pointer is: the page under a still pointer can change
    // (a dialog's next step, a page loaded), and the browser restyles its
    // hover on its own time; a real move to the same place restyles it at
    // once, before the next frame.
    let at = { x: 0, y: 0 };
    const click = async (target: Locator) => {
      at = await centre(target);
      await d.click(target);
    };
    // A click that navigates: the press shows, and no frame is filmed until
    // the next page has its content frame (the director's click rolls on
    // while the old page's frames go, which the camera cannot freeze).
    const navClick = async (target: Locator) => {
      at = await centre(target);
      await d.move(target);
      await cursorPress(alex);
      await d.offstage(async () => {
        await alex.mouse.down();
        await alex.mouse.up();
      });
    };
    const settleHover = () => d.offstage(() => alex.mouse.move(at.x, at.y));
    const typeFast = async (text: string) => {
      for (const ch of Array.from(text)) {
        await alex.keyboard.type(ch);
        await d.hold(KEY_MS);
      }
    };

    // The ask: New, a name, what he wants, and Create.
    d.beat("ask");
    await d.hold(200);
    await click(alex.getByRole("button", { name: "New artifact" }));
    const form = alex.getByRole("dialog", { name: "New artifact" });
    const name = form.getByLabel("Name");
    await d.until(async () => (await name.count()) > 0 && (await name.evaluate(e => e === document.activeElement)), "New's dialog is open, its name focused");
    await settleHover();
    await typeFast(NAME);
    await click(form.getByLabel("Description"));
    await typeFast(BRIEF);
    await d.hold(100);
    await click(form.getByRole("button", { name: "Create" }));
    // Who builds it: his live agent, sent the brief.
    const who = alex.getByRole("dialog", { name: "Who builds it?" });
    const claude = who.locator("button.agent", { hasText: "claude" });
    await d.until(async () => (await claude.count()) > 0, "New offers the claude agent");
    await settleHover();
    await d.hold(200);
    await navClick(claude);
    await d.until(async () => /\/a\/[a-z0-9]+$/.test(new URL(alex.url()).pathname), "New landed on the artifact");
    const aid = /\/a\/([a-z0-9]+)$/.exec(new URL(alex.url()).pathname)![1]!;
    const placeholder = await d.offstage(() => contentFrame(alex, aid, 1));
    await d.until(async () => (await placeholder.locator("h1").count()) > 0, "the placeholder page shows");
    await settleHover();
    await d.hold(150);

    // The agent takes the brief: it says it is working on the thread.
    const threads = await d.offstage(async () => {
      const r = await fetch(`${base}/api/artifacts/${aid}/threads`, { headers: { authorization: `Bearer ${token}` } });
      return ((await r.json()) as { threads: { id: string }[] }).threads;
    });
    const tid = threads[0]!.id;
    d.beat("working");
    await d.offstage(() => setWorking(base, token, sid, aid, { thread_ids: [tid] }));
    await d.until(async () => (await alex.locator(".topbar.working").count()) > 0, "Alex's view shows the agent working");
    await d.hold(600);

    // It publishes v2, addressing the brief: Alex's view offers Reload, and
    // he takes it, onto the dashboard.
    await d.offstage(() => publishNext(base, token, sid, aid, 1, { files: { "index.html": html }, addresses: [tid], note: "Q4 revenue by region, KPIs and top accounts" }));
    const reload = alex.locator(".topbar button.reload");
    await d.until(async () => (await reload.count()) > 0, "Alex's view offers v2");
    await d.hold(250);
    await navClick(reload);
    let frame: Frame | null = null;
    await d.until(async () => {
      frame = alex.frames().find(f => /\/_clax\/v\/2\//.test(f.url())) ?? null;
      return !!frame && (await frame.locator("#chart svg").count()) > 0;
    }, "v2 drew its chart in Alex's view");
    await settleHover();
    d.beat("reveal");
    await d.hold(700);

    // His pointer runs along the chart: the crosshair and tooltip follow.
    d.beat("touch");
    const chart = (frame as unknown as Frame).locator("#chart svg");
    await d.move(chart, { offset: { x: -110, y: 0 } });
    await d.move(chart, { offset: { x: 130, y: -20 } });
    await d.hold(600);
    if (!(await quietIn<boolean>(alex, frame as unknown as Frame, `getComputedStyle(document.getElementById("tip")).display !== "none"`))) throw new Error("the chart's tooltip is not showing");
  },
} satisfies Vignette<Cast>;
