// "Ask for anything" (spec 2026-10-27-sizzle-reel-design §3): a person asks
// an agent for something ambitious on the page itself, and it delivers. On
// a hosted daemon, an agent session, scripted through the API, has
// published "Northwind Customers", a plain table of forty accounts with
// their cities, coordinates and ARR. Alex, the owner, turns comment mode
// on, draws an area over the table and asks for a spinning 3D globe with
// arcs from HQ sized by revenue, and sends the thread to the agent. The
// agent takes it (the shell shows it working, on the thread and in the top
// bar) and publishes version 2 (web/reel/fixtures/globe/globe.html: a globe
// the page draws on a canvas, with arcs from Portland, glowing points sized
// by ARR, a legend and a top 10); Alex reloads to it, the agent's reply
// arrives in the thread, now in the Addressed group, and Alex drags the
// globe a little and hovers a city for its tooltip.
//
// The globe's motion reads the page's clock, which the deterministic camera
// steps, in whole frames, so it turns the same each run. The agent's steps
// are each waited for in Alex's page (the top bar's working line, the
// Reload button, the reply) before the film moves on, and so is what each
// of Alex's clicks starts in another process or over the network.
//
// Beats: `ask` (comment mode, the area, the request), `working` (the agent
// takes the thread), `reveal` (version 2 loads), `touch` (Alex drags the
// globe and hovers a city).
import { readFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { api, contentFrame, publishAs, publishNext, registerSession, setWorking } from "../../e2e/fixtures.ts";
import { openAt, ownerOf } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorPress } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { typingDelays } from "../pace.ts";

const TITLE = "Northwind Customers";
const ASK = "Show these on a spinning 3D globe, with arcs from HQ sized by revenue";
const REPLY = "Built a 3D globe of all 40 accounts with revenue arcs from Portland; drag to spin.";
const WORKING = "Building a 3D globe";
/** The city Alex hovers on version 2. */
const HOVERED = "Hudson Yard Capital";
/** The ask is typed this much faster than the director's own rhythm. */
const TYPE_PACE = 0.25;

// The daemon stamps times on the wall clock, and the camera's page reads
// the film's (REEL_EPOCH, camera.ts): a comment would read "7 d ago" and the
// agent's working clock count from a week back, by a margin that differs
// each run. In the camera's page, every time the daemon sends about the
// artifact (its answers, and its event stream) is restamped to the page's
// now as it arrives, so the film reads "just now" and the working clock
// counts from 0:00, the same each run. For that the page holds its own
// stream (no shared worker, whose requests no page script sees).
const FILM_TIMES = `(() => {
  if (window !== window.top) return;
  try { Object.defineProperty(window, "SharedWorker", { value: undefined, configurable: true }); } catch {}
  const ISO = /^\\d{4}-\\d\\d-\\d\\dT\\d\\d:\\d\\d:\\d\\d(\\.\\d+)?(Z|[+-]\\d\\d:\\d\\d)$/;
  const restamp = (v, key) => {
    if (typeof v === "string") return ISO.test(v) && !/expire/i.test(key || "") ? new Date().toISOString() : v;
    if (Array.isArray(v)) return v.map(x => restamp(x, key));
    if (v && typeof v === "object") { const o = {}; for (const k of Object.keys(v)) o[k] = restamp(v[k], k); return o; }
    return v;
  };
  const ours = p => p === "/api/stream" || p.startsWith("/api/artifacts") || p === "/api/viewers/me/looked";
  const real = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const res = await real(input, init);
    let path;
    try { path = new URL(typeof input === "string" ? input : input.url, location.href).pathname; } catch { return res; }
    if (!ours(path) || !res.ok || !res.body) return res;
    const head = { status: res.status, statusText: res.statusText, headers: res.headers };
    if (path === "/api/stream") {
      const dec = new TextDecoder(), enc = new TextEncoder();
      let rest = "";
      const line = l => {
        if (!l.startsWith("data:")) return l;
        try { return "data: " + JSON.stringify(restamp(JSON.parse(l.slice(5)))); } catch { return l; }
      };
      const body = res.body.pipeThrough(new TransformStream({
        transform(chunk, out) {
          const lines = (rest + dec.decode(chunk, { stream: true })).split("\\n");
          rest = lines.pop();
          if (lines.length) out.enqueue(enc.encode(lines.map(line).join("\\n") + "\\n"));
        },
        flush(out) { if (rest) out.enqueue(enc.encode(line(rest))); },
      }));
      return new Response(body, head);
    }
    if (!(res.headers.get("content-type") || "").includes("json")) return res;
    const json = await res.json();
    return new Response(JSON.stringify(restamp(json)), head);
  };
})()`;

const STILL = `(() => {
  if (window !== window.top) return;
  const style = () => { const s = document.createElement("style"); s.textContent = "textarea, input { caret-color: transparent !important; } .thread-card { transition: none !important; }"; document.head.appendChild(s); };
  if (document.head) style(); else addEventListener("DOMContentLoaded", style, { once: true });
})()`;

type Cast = { alex: Page; af: Frame; aid: string; base: string; token: string; sid: string; globe: string };

/** Drags the camera's pointer from `from` to `to`, (x, y) offsets from the
 * centre of `target`, as a press, a travel and a release; `released`, when
 * given, is waited for before the film rolls on, so what the release starts
 * in another process starts on the same frame each run. */
async function drag(d: Director, page: Page, target: ReturnType<Frame["locator"]>, from: { x: number; y: number }, to: { x: number; y: number }, released?: [() => Promise<boolean>, string]): Promise<void> {
  await d.move(target, { offset: from });
  await page.mouse.down();
  await d.hold(90);
  await d.move(target, { offset: to });
  await d.hold(60);
  await page.mouse.up();
  if (released) await d.until(...released);
  await d.hold(120);
}

/** Clicks `target` as the director does, but holds the film still from the
 * press until `settled` holds: what the click starts asynchronously (an
 * answer, a navigation) lands on the same frame each run. */
async function press(d: Director, page: Page, target: ReturnType<Page["locator"]>, settled?: [() => Promise<boolean>, string]): Promise<void> {
  await d.move(target);
  await cursorPress(page);
  await d.offstage(async () => {
    await page.mouse.down();
    await page.mouse.up();
  });
  if (settled) await d.until(...settled);
  await d.hold(150);
}

export default {
  id: "globe",

  async setup(stage: Stage): Promise<Cast> {
    const { d, origin } = await stage.hostedDaemon();
    const table = readFileSync(new URL("../fixtures/globe/customers.html", import.meta.url), "utf8");
    const globe = readFileSync(new URL("../fixtures/globe/globe.html", import.meta.url), "utf8");
    // The agent of the film, registered first: plain "claude".
    const agent = await registerSession(d.base, d.token, "claude", "globe-agent");
    const { artifact } = await publishAs(d.base, d.token, agent.id, TITLE, { "index.html": table });
    const aid = artifact.id;

    // Alex sets the daemon up in a browser of the setup's own; the camera signs in as him.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    const signedIn = await owner.context().storageState();
    await owner.context().close();
    const alex = await stage.newCamera({ storageState: signedIn });
    await alex.addInitScript(FILM_TIMES);
    // A text field's caret blinks on the browser's own time, not the page's
    // clock the camera steps: shown or not on a frame differs each run. The
    // camera's page draws none; the typed text shows as it is typed. A new
    // thread's card is selected as its post is answered, and the stream may
    // bring the card first, unselected, or not: the card takes its selected
    // border at once rather than in a transition only one of those runs has.
    await alex.addInitScript(STILL);
    const af = await openAt(alex, origin, aid, 1);
    await af.getByRole("cell", { name: "Hudson Yard Capital" }).waitFor({ state: "visible" });
    await alex.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    return { alex, af, aid, base: d.base, token: d.token, sid: agent.id, globe };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, af, aid, base, token, sid, globe } = cast;
    let looks = 0;
    alex.on("requestfinished", r => {
      if (new URL(r.url()).pathname === "/api/viewers/me/looked") looks += 1;
    });
    await d.hold(250);

    // Alex turns comment mode on and draws an area over the table.
    d.beat("ask");
    await d.click(alex.getByRole("button", { name: "Comment", exact: true }));
    await d.until(async () => (await quietIn<string>(alex, af, "document.documentElement.style.cursor")) === "crosshair", "comment mode reached the page");
    const table = af.locator("table");
    const box = await table.boundingBox();
    if (!box) throw new Error("the table has no box");
    const tl = { x: -box.width / 2 + 4, y: -box.height / 2 + 2 };
    // The composer takes the focus, and the shell draws the area's clip in
    // a frame of its own (about:srcdoc) that goes once it is drawn; the
    // camera films a frame of every frame the page has, so the film waits
    // for the clip, and the frame gone, before it rolls on.
    const clipped = async () =>
      alex.frames().every(f => f.url() !== "about:srcdoc") &&
      (await quietIn<boolean>(alex, alex.mainFrame(), `document.activeElement?.matches(".composer textarea") === true && document.querySelector(".composer img.clip")?.complete === true`));
    await drag(d, alex, table, tl, { x: box.width / 2 - 6, y: tl.y + 330 }, [clipped, "the composer opened on the area, focused, with its clip"]);

    // He asks, quickly, and sends it to the agent.
    const delays = typingDelays(ASK, "globe");
    for (const [i, ch] of Array.from(ASK).entries()) {
      await alex.keyboard.type(ch);
      await d.hold(Math.round(delays[i]! * TYPE_PACE));
    }
    await d.hold(150);
    const card = alex.locator(".thread-card").filter({ hasText: ASK });
    await press(d, alex, alex.locator(".composer").getByRole("button", { name: "Post comment" }), [async () => (await card.and(alex.locator(".selected")).count()) === 1, "the ask posted as a thread, selected"]);
    await press(d, alex, card.getByRole("button", { name: /^Send to / }), [async () => (await card.locator(".waiting").count()) === 1, "the ask was sent to the agent"]);
    const tid = (await card.getAttribute("data-thread"))!;

    // The agent takes it: the shell shows it working.
    d.beat("working");
    await d.offstage(async () => {
      await api(base, token, `/api/sessions/${sid}/feedback?tier=piggyback`);
      await setWorking(base, token, sid, aid, { message: WORKING, thread_ids: [tid] });
    });
    await d.until(async () => (await alex.locator(".who .sum b.l1").textContent())?.includes(WORKING) === true, "Alex's top bar shows the agent working");
    await d.hold(1100);
    // Alex's view marks his thread looked at once it has been on screen a
    // while, on the shell's clock, which only the film moves: within this
    // hold, on the same frame each run, with frames to spare for its answer
    // however slow the network. The agent publishes after that mark, so
    // version 2 counts as news to him and its load shows the thread in the
    // Addressed group.
    await d.hold(600);
    await d.until(async () => looks > 0, "Alex's view marked his thread looked at");

    // It publishes version 2: the top bar offers it.
    await d.offstage(() => publishNext(base, token, sid, aid, 1, { files: { "index.html": globe }, note: "A 3D globe of the accounts", addresses: [tid] }));
    const reload = alex.locator(".topbar button.reload");
    await d.until(async () => (await reload.count()) === 1, "Alex's top bar offers version 2");
    await d.hold(250);

    // Alex reloads to it: the globe. The press is made off camera and the
    // film waits out the navigation it starts: no frame is filmed while the
    // old content frame goes, as a deterministic camera reaches every frame
    // of the page to film one.
    d.beat("reveal");
    const v2 = () => alex.frames().find(f => f.url().includes(`${aid}.localhost`) && f.url().includes("/_clax/v/2/"));
    await press(d, alex, reload, [async () => {
      const f = v2();
      if (!f || alex.frames().some(g => g.url().includes("/_clax/v/1/") || g.url() === "about:srcdoc")) return false;
      return quietIn<number>(alex, f, `document.querySelectorAll("#top li").length`).then(n => n === 10, () => false);
    }, "version 2 loaded and drew its globe"]);
    const gf = await d.offstage(() => contentFrame(alex, aid, 2));
    // The shell lays its threads out on its own clock, which only the film
    // moves: the thread shows in the Addressed group, and the agent's reply
    // arrives in it live.
    await d.hold(700);
    const addressed = alex.locator(".section-addressed .thread-card");
    if ((await addressed.count()) !== 1) throw new Error("the ask is not in v2's Addressed group");
    await d.offstage(() => api(base, token, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", session: sid, body: JSON.stringify({ body: REPLY, author_kind: "agent" }) }));
    await d.until(async () => (await addressed.locator(".msg.agent").count()) > 0, "the agent's reply reached Alex's threads");
    await d.hold(1000);

    // He drags it a little, then hovers a city.
    d.beat("touch");
    const canvas = gf.locator("canvas#globe");
    await drag(d, alex, canvas, { x: 40, y: 30 }, { x: -60, y: 10 });
    const spots = await quietIn<string>(alex, gf, `document.querySelector("canvas#globe").dataset.spots ?? "[]"`);
    const spot = (JSON.parse(spots) as [string, number, number][]).find(s => s[0] === HOVERED);
    if (!spot) throw new Error(`${HOVERED} is not on the globe's face: ${spots}`);
    const cbox = await canvas.boundingBox();
    if (!cbox) throw new Error("the globe has no box");
    await d.move(canvas, { offset: { x: spot[1] - cbox.width / 2, y: spot[2] - cbox.height / 2 } });
    await d.until(async () => (await quietIn<boolean>(alex, gf, `document.getElementById("tip").classList.contains("on")`)), "the tooltip shows");
    await d.hold(1000);
  },
} satisfies Vignette<Cast>;
