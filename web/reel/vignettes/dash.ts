// "From raw data to answers" (an ask-and-deliver vignette of the sizzle
// reel, spec 2026-10-27-sizzle-reel-design §3). On a hosted daemon Alex, the
// owner, has "Northwind Orders" open: a plain grid of raw orders an agent
// published. He turns on comment mode, draws an area over the grid, asks for
// a forecast dashboard and sends the thread to the agent. The agent, scripted
// through the API (no model runs), takes the thread, which shows it working
// in the thread and the top bar, publishes the dashboard as version 2
// (fixtures/dash/dashboard.html: KPI tiles that count up, stacked regional
// areas with a shaded 90-day forecast band, a share donut and a product
// leaderboard) and replies. Alex reloads onto it, the reply lands in the
// thread, and he clicks the West chip: every chart re-animates to West alone.
//
// Each step of the agent's is waited for in Alex's page (the card's working
// line, the Reload button, the new frame, the reply) before the film moves
// on, so the frame it shows on is the same each run.
//
// Beats: `ask` (comment mode on), `working` (the agent took the thread),
// `reveal` (version 2 loads), `touch` (Alex clicks West).
import { readFileSync } from "node:fs";
import type { Frame, Locator, Page } from "@playwright/test";
import { api, contentFrame, publishAs, publishNext, registerSession, setWorking } from "../../e2e/fixtures.ts";
import { openAt, ownerOf } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorPress } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { typingDelays } from "../pace.ts";

const TITLE = "Northwind Orders";
const ASK = "Turn this into a forecast dashboard — trends by region, a 90-day forecast, and let me filter";
const REPLY = "Built a forecast dashboard from the 1,204 orders: trends by region, a 90-day forecast with a confidence band, and filters.";
/** How much faster than the director's own rhythm Alex types the ask. */
const TYPING_SPEEDUP = 3;

// The daemon stamps its own times, while the camera's clock reads the film's
// (REEL_EPOCH, camera.ts): a thread would read "7 d ago" and the agent's
// working clock would show the hours between the two, a figure that differs
// each run. In the shell's documents of the camera's page, every time in an
// API answer or a stream event the shell hears (a key ending in "_at", or
// "since") is restamped as it arrives: a time first heard is given the page's
// now, or a millisecond past the latest time given so far, whichever is
// later, so times keep their order (a version addressed after the thread it
// answers), read "just now", and the working clock counts from 0:00 on the
// film's own clock. A body's new times are given in their own order, and
// what each was given is kept for the tab (sessionStorage), across the reload.
const RESTAMP = `(() => {
  if (location.hostname !== "localhost" || window.reelRestamp) return;
  const KEY = "reel-restamp";
  let given = {};
  try { given = JSON.parse(sessionStorage.getItem(KEY) || "{}"); } catch {}
  const isTime = (k, x) => typeof x === "string" && (k === "since" || /_at$/.test(k)) && !Number.isNaN(Date.parse(x));
  const collect = (v, into) => {
    if (Array.isArray(v)) { for (const x of v) collect(x, into); return; }
    if (!v || typeof v !== "object") return;
    for (const [k, x] of Object.entries(v)) if (isTime(k, x)) into.add(x); else collect(x, into);
  };
  const learn = v => {
    const fresh = new Set();
    collect(v, fresh);
    const order = [...fresh].filter(x => !(x in given)).sort((a, b) => Date.parse(a) - Date.parse(b));
    if (!order.length) return;
    let last = Math.max(0, ...Object.values(given));
    for (const x of order) { last = Math.max(Date.now(), last + 1); given[x] = last; }
    try { sessionStorage.setItem(KEY, JSON.stringify(given)); } catch {}
  };
  const swap = v => {
    if (Array.isArray(v)) return v.map(swap);
    if (!v || typeof v !== "object") return v;
    const out = {};
    for (const [k, x] of Object.entries(v)) out[k] = isTime(k, x) ? new Date(given[x]).toISOString() : swap(x);
    return out;
  };
  const stamp = v => { learn(v); return swap(v); };
  window.reelRestamp = stamp;
  const fetch0 = window.fetch;
  window.fetch = async (...args) => {
    const res = await fetch0(...args);
    const url = new URL(res.url || String(args[0]), location.href);
    if (!url.pathname.startsWith("/api/") || url.pathname === "/api/stream" || !(res.headers.get("content-type") || "").includes("json")) return res;
    const body = stamp(await res.json());
    return new Response(JSON.stringify(body), { status: res.status, statusText: res.statusText, headers: res.headers });
  };
  const wrap = fn => function (e) { return fn.call(this, new MessageEvent("message", { data: stamp(e.data), ports: [...e.ports] })); };
  const on = Object.getOwnPropertyDescriptor(MessagePort.prototype, "onmessage");
  Object.defineProperty(MessagePort.prototype, "onmessage", { configurable: true, get: on.get, set(fn) { on.set.call(this, typeof fn === "function" ? wrap(fn) : fn); } });
  const add = MessagePort.prototype.addEventListener;
  MessagePort.prototype.addEventListener = function (type, fn, opts) { return add.call(this, type, type === "message" && typeof fn === "function" ? wrap(fn) : fn, opts); };
})()`;

// A text field's caret blinks on the browser's own time, which no camera
// steps: in the camera's page it holds still (`caret-animation: manual`,
// inherited into every shadow tree), so a frame with the composer focused is
// the same each run.
const STEADY_CARET = `(() => {
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(":root { caret-animation: manual; }");
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
})()`;

type Cast = { alex: Page; f1: Frame; base: string; token: string; aid: string; sid: string; v2: string };

const fixture = (name: string) => readFileSync(new URL(`../fixtures/dash/${name}`, import.meta.url), "utf8");

/** Moves to `target` and presses it, filmed; then releases it and runs
 * `after` off camera, so what the release sets off shows on the next frame
 * whatever its real time took. */
async function pressThen<T>(d: Director, page: Page, target: Locator, after: () => Promise<T>): Promise<T> {
  await d.move(target);
  await cursorPress(page);
  await page.mouse.down();
  await d.hold(70);
  return d.offstage(async () => {
    const [out] = await Promise.all([after(), page.mouse.up()]);
    return out;
  });
}

/** Drags from where the cursor is to `to` (offset from its centre), the
 * cursor and the real mouse together: from a spot with no text under it, an
 * area. The release, and `after`, are off camera, as in `pressThen`. */
async function dragTo(d: Director, page: Page, to: Locator, offset: { x: number; y: number }, after: () => Promise<void>): Promise<void> {
  await cursorPress(page);
  await page.mouse.down();
  await d.hold(60);
  await d.move(to, { offset });
  await d.hold(60);
  await d.offstage(async () => {
    await page.mouse.up();
    await after();
  });
}

export default {
  id: "dash",

  async setup(stage: Stage): Promise<Cast> {
    const { d, origin } = await stage.hostedDaemon();
    // The agent of the film published the raw grid; registered first, it is plain "claude".
    const agent = await registerSession(d.base, d.token, "claude", "dash-agent");
    const { artifact } = await publishAs(d.base, d.token, agent.id, TITLE, { "index.html": fixture("orders.html") });

    const owner = await ownerOf(stage.browser, origin, "Alex");
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    const alex = await stage.newCamera({ storageState: signedIn });
    await alex.addInitScript(RESTAMP);
    await alex.addInitScript(STEADY_CARET);
    const f1 = await openAt(alex, origin, artifact.id, 1);
    // A second tab of Alex's, never filmed, keeps the shell's shared stream
    // worker alive across the reload onto version 2: the reloaded page joins
    // it rather than fetching its script again, a request the deterministic
    // camera would wait on and never see end.
    const keeper = await alex.context().newPage();
    await keeper.goto(`${origin}/`);
    await f1.locator("#rows tr").first().waitFor({ state: "visible" });
    await alex.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    // The agent's Send button is ready: the shell knows its live agent.
    await alex.locator("button.threads").waitFor({ state: "visible" });
    return { alex, f1, base: d.base, token: d.token, aid: artifact.id, sid: agent.id, v2: fixture("dashboard.html") };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, f1, base, token, aid, sid, v2 } = cast;
    await d.hold(250);

    // Comment mode: the button, then the frame's crosshair.
    d.beat("ask");
    await d.click(alex.getByRole("button", { name: "Comment", exact: true }));
    await d.until(async () => (await quietIn<string>(alex, f1, "document.documentElement.style.cursor")) === "crosshair", "the frame is in comment mode");

    // Alex draws an area over the grid, from the blank end of a Region cell
    // (a press on text would select it) to a Revenue cell rows below.
    const cell = (row: number, col: number) => f1.locator("#rows tr").nth(row).locator("td").nth(col);
    await d.move(cell(0, 3), { offset: { x: 38, y: -6 } });
    // The composer opens once the area's screenshot is taken, on real time:
    // no frame is filmed until it shows with it.
    await dragTo(d, alex, cell(11, 7), { x: 44, y: 8 }, () => alex.locator(".composer img.clip").waitFor({ state: "visible" }));

    // He types the ask, fast, and posts it.
    const delays = typingDelays(ASK, "dash");
    for (const [i, ch] of Array.from(ASK).entries()) {
      await alex.keyboard.type(ch);
      await d.hold(delays[i]! / TYPING_SPEEDUP);
    }
    await d.hold(150);
    // The card comes from the daemon, on its own time: no frame is filmed
    // between the release and its showing.
    const card = alex.locator(".thread-card").filter({ hasText: "forecast dashboard" }).first();
    const send = card.getByRole("button", { name: /^Send to / });
    await pressThen(d, alex, alex.locator(".composer").getByRole("button", { name: "Post comment" }), () => send.waitFor({ state: "visible" }));
    await pressThen(d, alex, send, () => card.locator(".waiting").waitFor({ state: "visible" }));
    const tid = await card.getAttribute("data-thread");
    if (!tid) throw new Error("the thread card has no data-thread");

    // The agent takes it: its feedback, then its working record; the card
    // and the top bar show it working.
    await d.offstage(async () => {
      await api(base, token, `/api/sessions/${sid}/feedback?tier=piggyback`);
      await setWorking(base, token, sid, aid, { thread_ids: [tid], message: "Building the forecast dashboard" });
    });
    await d.until(async () => (await card.locator(".st.ag").count()) > 0, "Alex's thread shows the agent working");
    d.beat("working");
    await d.move(card.locator(".st.ag"), { offset: { x: 40, y: 0 } });
    await d.hold(500);
    // Alex's pointer waits in the top bar, by where Reload will show: he
    // takes version 2 within the shell's second before it marks the thread
    // looked at (LOOK_EVERY_MS), so it opens as "Addressed in v2".
    await d.move(alex.getByRole("button", { name: "Comment", exact: true }), { offset: { x: -90, y: 0 } });
    await d.hold(300);

    // The agent publishes version 2: Reload offers it, and Alex takes it.
    await d.offstage(() => publishNext(base, token, sid, aid, 1, { note: "A forecast dashboard: regional trends, a 90-day forecast, filters", addresses: [tid], files: { "index.html": v2 } }));
    const reload = alex.locator("button.reload");
    await d.until(async () => (await reload.count()) > 0 && (await reload.isVisible()), "Reload offers version 2");
    await d.hold(150);
    // Its release navigates: no frame is filmed until the new page and its
    // frame have loaded (the camera reaches every frame of the page each
    // frame, and the old one is going).
    const f2 = await pressThen(d, alex, reload, async () => {
      await alex.waitForEvent("load");
      return contentFrame(alex, aid, 2);
    });
    // Its script has run (the chips are its own); it draws on the next
    // animation frame, which comes as the camera rolls.
    await d.until(async () => (await f2.locator("#chips button").count()) === 5, "the dashboard's script ran");
    await d.until(async () => (await quietIn<number>(alex, alex.mainFrame(), "window.claxStreamLive ?? 0")) > 0, "the reloaded view hears the daemon");
    d.beat("reveal");
    await d.hold(500);

    // The reply lands in the thread while the dashboard counts up.
    await d.offstage(() => api(base, token, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", session: sid, body: JSON.stringify({ body: REPLY, author_kind: "agent" }) }));
    await d.until(async () => (await alex.locator(".msg.agent").filter({ hasText: "1,204 orders" }).count()) > 0, "the agent's reply reached Alex's thread");
    await d.move(f2.locator("#svg"), { offset: { x: -40, y: 20 } });
    await d.hold(1100);

    // Alex filters to West: the areas, KPIs, donut and leaderboard re-animate.
    d.beat("touch");
    const west = f2.locator('button[data-region="west"]');
    await d.click(west);
    await d.hold(1300);
  },
} satisfies Vignette<Cast>;
