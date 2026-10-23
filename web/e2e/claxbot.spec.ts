// ClaxBot's screens (docs/superpowers/specs/2026-10-21-claxbot-design.md
// §11) on a local daemon, with the states a real run reaches only slowly
// or not at all here answered at the network layer by the routes below:
// the New artifact tile and its "ClaxBot builds it now" switch landing on
// a run panel replaying a scripted run, a week at its limit, and the
// settings, You and audit pages at phone width without a sideways scroll.
// The daemon makes the artifacts, threads and pages. A Send to ClaxBot
// that runs on the daemon, and the admin pages over the daemon's own
// routes, are claxbot-daemon.spec.ts's, with nothing stubbed.
import { type Page, type Route } from "@playwright/test";
import { expect, openArtifact, postThread, publish, test } from "./fixtures";

const WEEK = { start: "2026-10-19T00:00:00Z", end: "2026-10-26T00:00:00Z", weekly_milli: 2_000_000, lift_milli: 0, spent_milli: 310_000, warn: false, exhausted: false };
const ME = { enabled: true, unenforced: false, available: true, unavailable_reason: null, key: { own: null, use_own: false }, week: WEEK, role: "owner" };
const RUN = (aid: string, o: Record<string, unknown> = {}) => ({
  id: "r1", artifact_id: aid, thread_ids: ["t1"], request: "brief", status: "running", model: "claude-sonnet-5-5", key_source: "instance",
  acting_for: { id: "*owner", name: "Viewer" }, started_at: "2026-10-21T14:02:00Z", credits_milli: 0, steps: [], ...o,
});
const sse = (events: [string, unknown][]) => events.map(([e, d]) => `event: ${e}\ndata: ${JSON.stringify(d)}\n\n`).join("");
/** A run that narrates, runs one script with two calls, and finishes. */
const EVENTS: [string, unknown][] = [
  ["step_started", { step: { n: 1, kind: "turn", status: "running", started_at: "2026-10-21T14:02:01Z", credits_milli: 0 } }],
  ["turn_delta", { step: 1, text: "Reading the page and its data first." }],
  ["step_done", { step: { n: 1, kind: "turn", status: "done", started_at: "2026-10-21T14:02:01Z", credits_milli: 0, text: "Reading the page and its data first." }, run: { credits_milli: 2000 } }],
  ["step_started", { step: { n: 2, kind: "script", status: "running", started_at: "2026-10-21T14:02:03Z", credits_milli: 0 } }],
  ["script", { step: 2, title: "Read the page and its data", code: "const html = await clax.files.read(\"index.html\");\nreturn html.length;" }],
  ["call", { step: 2, call: { op: "files.read", target: "index.html", ok: true, ms: 12 } }],
  ["call", { step: 2, call: { op: "db.list", target: "items", ok: true, ms: 22 } }],
  ["step_done", { step: { n: 2, kind: "script", status: "done", started_at: "2026-10-21T14:02:03Z", credits_milli: 1000, title: "Read the page and its data", calls: [{ op: "files.read", target: "index.html", ok: true, ms: 12 }, { op: "db.list", target: "items", ok: true, ms: 22 }], value: 8312 }, run: { credits_milli: 3000 } }],
  ["step_done", { step: { n: 3, kind: "finish", status: "done", started_at: "2026-10-21T14:02:05Z", credits_milli: 0, summary: "Built the splitter." } }],
  ["run_end", { run: { status: "finished", credits_milli: 3000 } }],
];

type Stub = { me?: unknown; runs?: Record<string, unknown>; started?: unknown[]; json?: Record<string, unknown> };
/** Answers ClaxBot's routes: `GET /api/me/bot`, a run, its events and its
 * artifact's runs, `POST /api/bot/runs` (kept in `started`), and any other
 * `METHOD /path` in `json`. */
async function stubBot(page: Page, s: Stub) {
  const ours = (u: URL) => u.pathname === "/api/me/bot" || u.pathname.startsWith("/api/me/") || u.pathname.startsWith("/api/bot/") || u.pathname.startsWith("/api/admin/") || u.pathname.startsWith("/api/audit");
  await page.route(url => ours(url), async (route: Route) => {
    const req = route.request();
    const u = new URL(req.url());
    const key = `${req.method()} ${u.pathname}`;
    if (key === "GET /api/me/bot") return route.fulfill({ json: s.me ?? ME });
    const run = /^\/api\/bot\/runs\/([^/]+)(\/events)?$/.exec(u.pathname);
    if (req.method() === "GET" && run && s.runs?.[run[1]]) {
      if (run[2]) return route.fulfill({ status: 200, headers: { "content-type": "text/event-stream", "cache-control": "no-store" }, body: sse(EVENTS) });
      return route.fulfill({ json: s.runs[run[1]] });
    }
    if (key === "GET /api/bot/runs") return route.fulfill({ json: { runs: Object.values(s.runs ?? {}) } });
    if (key === "POST /api/bot/runs") { s.started?.push(req.postDataJSON()); return route.fulfill({ json: { run: Object.values(s.runs ?? {})[0] } }); }
    const v = s.json?.[key];
    if (v !== undefined) return route.fulfill({ json: typeof v === "function" ? (v as (b: unknown) => unknown)(req.postDataJSON()) : v });
    return route.fulfill({ status: 404, json: { error: { code: "not_found", message: "not found" } } });
  });
}

/** What reaches past the page's right edge, so that it would scroll sideways: none. */
const sideways = (page: Page) => page.evaluate(() => {
  const w = document.documentElement.clientWidth;
  if (document.documentElement.scrollWidth <= w) return [];
  // Each element past the edge that is not inside a box that scrolls on its own.
  const scrolls = (el: Element | null): boolean => !!el && (getComputedStyle(el).overflowX !== "visible" || scrolls(el.parentElement));
  return [...document.body.querySelectorAll("*")].filter(el => el.getBoundingClientRect().right > w + 0.5 && !scrolls(el.parentElement)).map(el => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")}`).slice(0, 8);
});

test("the New artifact tile leads the gallery, and ClaxBot builds what it makes with the run panel open", async ({ page, daemon: d }) => {
  await publish(d.base, d.token, "Tile neighbour", { "index.html": "<h1>Hi</h1>" });
  const s: Stub = { runs: {} };
  await stubBot(page, s);
  // The daemon makes the artifact as it does today; the run is the stub's.
  let asked: Record<string, unknown> | null = null;
  await page.route("**/api/artifacts:new", async route => {
    asked = route.request().postDataJSON();
    const { build: _build, ...rest } = asked!;
    const res = await route.fetch({ postData: JSON.stringify(rest) });
    const made = await res.json();
    s.runs!.r1 = RUN(made.artifact.id);
    await route.fulfill({ status: res.status(), json: { ...made, run: { id: "r1" } } });
  });
  await page.goto(d.base);
  await expect(page.locator(".gbar .bot-meter")).toHaveText("ClaxBot · 310 of 2,000 credits");
  const tile = page.locator(".cards > .new-tile").first();
  await expect(tile).toBeVisible();
  // The tile leads the first grid: "Artifacts", or "Needs your eyes" once
  // the attention summary, read after the first paint, lists one there.
  await expect(page.locator(".cards").first().locator(":scope > *").first()).toHaveClass(/(^|\s)new-tile(\s|$)/);
  await expect(tile).toContainText("Name it; ClaxBot can build it");
  await tile.getByRole("button", { name: "New artifact" }).click();
  const dialog = page.getByRole("dialog", { name: "New artifact" });
  await dialog.getByLabel("Name").fill("Grocery splitter");
  await dialog.getByLabel("Description").fill("Split a grocery receipt between housemates.");
  await expect(dialog.getByRole("switch", { name: "ClaxBot builds it now" })).toHaveAttribute("aria-checked", "true");
  await expect(dialog).toContainText("You have 1,690 credits left this week.");
  await dialog.getByRole("button", { name: "Create and build" }).click();
  await expect(page).toHaveURL(/\/a\/[a-z0-9]{12}#run=r1$/);
  expect(asked).toMatchObject({ title: "Grocery splitter", build: "claxbot" });
  const run = page.getByRole("complementary", { name: "ClaxBot" });
  await expect(run).toBeVisible();
  await expect(run.locator(".st")).toHaveText("Finished");
  await expect(run).toContainText("Reading the page and its data first.");
  const card = run.locator("li.script details");
  await expect(card.locator("summary")).toContainText("2 calls · 34 ms");
  await card.locator("summary").click();
  await expect(card.getByLabel("Calls")).toContainText("db.list items");
  await expect(run.locator(".finish")).toContainText("Built the splitter.");
  await expect(run.locator(".cost")).toHaveText("This run: 3 credits");
  await expect(run.getByRole("link", { name: "Audit ↗" })).toHaveAttribute("href", /\/audit\?artifact=[a-z0-9]{12}&q=r1$/);
  // The top bar's ClaxBot button closes and opens it.
  await page.getByRole("button", { name: /^ClaxBot/ }).click();
  await expect(run).toHaveCount(0);
  await page.getByRole("button", { name: /^ClaxBot/ }).click();
  await expect(run).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(run).toHaveCount(0);
});

test("at its limit, the gallery says ClaxBot is paused and the Send menu lists it disabled with the reset", async ({ page, daemon: d }) => {
  const me = { ...ME, available: false, unavailable_reason: "no_credits", week: { ...WEEK, spent_milli: 2_000_000, exhausted: true } };
  const { artifact } = await publish(d.base, d.token, "Over the limit", { "index.html": "<main><h2>Totals</h2></main>" });
  const t = await postThread(d.base, artifact.id, "Fix the arrows.");
  await stubBot(page, { me });
  await page.goto(d.base);
  const banner = page.getByRole("alert").filter({ hasText: "This week's credits are used up." });
  await expect(banner).toContainText("ClaxBot is paused until Monday, Oct 26.");
  await banner.getByRole("button", { name: "Ask an admin for a lift" }).click();
  await expect(banner).toContainText("Admin › People & flags");
  await openArtifact(page, d.base, artifact.id, 1);
  // The artifact view says so too, under its top bar.
  await expect(page.locator(".bot-banner-at").getByRole("alert")).toContainText("This week's credits are used up.");
  if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
  const card = page.locator(`.thread-card[data-thread="${t.id}"]`);
  await card.getByRole("button", { name: "Choose the agent" }).click();
  const bot = card.getByRole("group", { name: "Clax" }).getByRole("menuitemradio", { name: /ClaxBot/ });
  await expect(bot).toHaveAttribute("aria-disabled", "true");
  await expect(bot).toContainText("Weekly credits used · resets Mon Oct 26");
});

test("Settings reads the daemon's own settings route and writes a change to config.toml, after restart", async ({ page, freshDaemon: d }) => {
  await stubBot(page, { json: { "GET /api/admin/provider/models": { models: [{ id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5" }, { id: "claude-opus-5-5", display_name: "Claude Opus 5.5" }], fetched_at: "x" } } });
  await page.goto(`${d.base}/settings`);
  // The port in effect is the one it listens on, with what chose it.
  const port = page.locator("#set-serve-port");
  await expect(port).toHaveValue(String(d.port));
  await expect(page.locator(".field", { has: port })).toContainText("the daemon listens on");
  // A model field is the provider's model selector.
  await expect(page.locator("select#set-bot-models-default")).toBeEnabled();
  await page.locator("#set-bot-max-steps").fill("9");
  await page.getByRole("button", { name: "Save to config.toml" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved" })).toHaveText("Saved 1 to config.toml. Restart Clax for them to take effect.");
  await expect(page.locator(".field", { has: page.locator("#set-bot-max-steps") }).locator(".pend")).toHaveText("after restart: 9");
  const back = await page.evaluate(async () => (await (await fetch("/api/settings", { headers: { authorization: `Bearer ${(await (await fetch("/api/token")).json()).token}` } })).json()) as { fields: { key: string; pending?: unknown }[] });
  expect(back.fields.find(f => f.key === "bot.max_steps")?.pending).toBe(9);
});

test("Settings, You and Audit show at phone width without a sideways scroll", async ({ page, daemon: d }) => {
  await stubBot(page, { json: {
    "GET /api/me/usage": { week: { start: WEEK.start, end: WEEK.end }, totals: [], runs: [] },
    "GET /api/audit": { events: [{ seq: 48212, at: "2026-10-21T14:05:58Z", actor: { id: "agent:claxbot", name: "ClaxBot" }, on_behalf_of: { id: "p_owner", name: "Alex" }, kind: "bot.script", summary: "\"Fix rounding and arrow direction\" · 3 calls · 2 credits", artifact_id: null, run: "r1" }], next_cursor: null },
    "GET /api/audit/48212": { seq: 48212, at: "2026-10-21T14:05:58Z", actor: { id: "agent:claxbot", name: "ClaxBot" }, on_behalf_of: { id: "p_owner", name: "Alex" }, kind: "bot.script", summary: "x", artifact_id: null, run: "r1", body: { v: 1, via: "bot" } },
  } });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${d.base}/settings`);
  await expect(page.locator("#set-serve-port")).toHaveValue(String(d.port));
  expect(await sideways(page)).toEqual([]);
  await page.goto(`${d.base}/me`);
  await expect(page.getByRole("meter", { name: "Your credits this week" })).toHaveAttribute("aria-valuetext", "310 of 2,000 credits");
  expect(await sideways(page)).toEqual([]);
  await page.goto(`${d.base}/audit`);
  await page.getByRole("button", { name: /ClaxBot for Alex/ }).click();
  await expect(page.getByLabel("Stored body")).toContainText("\"via\": \"bot\"");
  expect(await sideways(page)).toEqual([]);
});
