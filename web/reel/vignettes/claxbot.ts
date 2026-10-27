// "No agent? Use ClaxBot" (spec 2026-10-27-sizzle-reel-design §3): a person
// with no agent of their own starts an artifact from a brief and ClaxBot
// builds it. The owner presses New in the gallery, names the artifact,
// writes the brief and creates it with "ClaxBot builds it now" on; the
// artifact opens with ClaxBot's run panel, which follows the run: its
// narration, the script it writes, the check, the publish; then the view
// loads the version ClaxBot published, a launch tracker with a burndown,
// beside the finished run.
//
// On a local daemon, not a hosted one: a hosted daemon reads no `[bot]`
// configuration and takes only a public https base URL for ClaxBot's
// provider (clax_store::bot_settings::BaseUrlPolicy::Hosted), so it cannot
// be pointed at a scripted model on loopback without a model running.
//
// ClaxBot runs on the daemon for real; only the model is scripted. The
// daemon's `[bot] base_url` names a stand-in for the Messages API on
// loopback, as web/e2e/claxbot-daemon.spec.ts runs ClaxBot (there with
// web/e2e/fake-messages.ts): here each scripted answer streams in parts
// and stops at gates the film opens, so every state the panel shows is
// waited for before the camera rolls on it. The page ClaxBot writes is
// fixtures/claxbot/tracker.html, inside the script's code.
//
// The film's page clock is set before the daemon's (`PAGE_TIME`): a running
// turn's activity line counts from the daemon's start of the step to the
// page's now (web/shell/src/bot/model.ts turnActivity), and the reel's
// epoch is days after the daemon's real time, so it would read thousands of
// minutes, a different number each run. Before the daemon's time, it reads
// 0:00, which each turn here is.
//
// Beats: `brief` (the New dialog opens), `building` (the run panel shows
// ClaxBot at work), `done` (the tracker is on screen beside the finished run).
import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import type { Locator, Page } from "@playwright/test";
import { NO_KEY_CONFIG, publishWith } from "../../e2e/fixtures.ts";
import { ownerCookie } from "../../e2e/questions-helpers.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { typingDelays } from "../pace.ts";

const NAME = "Launch tracker";
const BRIEF = "A tracker for our product launch with owners, dates and a burndown";
/** The camera page's wall clock while it films: before any daemon's. */
const PAGE_TIME = new Date("2026-10-20T09:41:00Z");
/** Typing here runs at twice pace.ts's rhythm: a brisk typist, to fit the cut. */
const TYPING = 0.5;

const TRACKER = readFileSync(new URL("../fixtures/claxbot/tracker.html", import.meta.url), "utf8");
/** The page's text as a JS template literal's body. */
const literal = (s: string) => s.replace(/\\/g, "\\\\").replace(/`/g, "\\`").replace(/\$\{/g, "\\${");

const BUILD_CODE = `const html = \`${literal(TRACKER)}\`;
await clax.files.write("index.html", html);
const report = await clax.draft.check();
return report.ok ? "index.html passes the page contract" : report.problems;
`;
const PUBLISH_CODE = `const v = await clax.draft.publish({ note: "Launch tracker: burndown, owners and twelve tasks" });
return "Published version " + v.version;
`;

// ---- the scripted Messages API ----

type Sse = { name: string; data: unknown };
/** One part of a scripted answer: its events, sent once its gate (if any) opens. */
type Part = { gate?: string; events: Sse[] };

const start = (): Sse => ({ name: "message_start", data: { type: "message_start", message: { id: "msg_reel", model: "claude-sonnet-5-5", usage: { input_tokens: 1800, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, output_tokens: 1 } } } });
const text = (i: number, t: string): Sse[] => [
  { name: "content_block_start", data: { type: "content_block_start", index: i, content_block: { type: "text", text: "" } } },
  { name: "content_block_delta", data: { type: "content_block_delta", index: i, delta: { type: "text_delta", text: t } } },
  { name: "content_block_stop", data: { type: "content_block_stop", index: i } },
];
const toolStart = (i: number, id: string, name: string): Sse => ({ name: "content_block_start", data: { type: "content_block_start", index: i, content_block: { type: "tool_use", id, name, input: {} } } });
const toolInput = (i: number, json: string): Sse => ({ name: "content_block_delta", data: { type: "content_block_delta", index: i, delta: { type: "input_json_delta", partial_json: json } } });
const end = (i: number, outputTokens: number): Sse[] => [
  { name: "content_block_stop", data: { type: "content_block_stop", index: i } },
  { name: "message_delta", data: { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: outputTokens } } },
  { name: "message_stop", data: { type: "message_stop" } },
];

/** ClaxBot's three turns: it says what it will build and writes it, says
 * the check passed and publishes it, and finishes. Each turn's narration
 * and its tool call wait at gates of their own. */
function answers(): Part[][] {
  const call = (title: string, code: string) => JSON.stringify({ title, code });
  return [
    [
      { events: [start()] },
      { gate: "plan", events: text(0, "I'll build Northwind's launch tracker: tasks by workstream with owners and due dates, a burndown of open points to launch day, and who has what left.") },
      { gate: "build", events: [toolStart(1, "tu_build", "run_script"), toolInput(1, call("Build the tracker", BUILD_CODE)), ...end(1, 5200)] },
    ],
    [
      { events: [start()] },
      { gate: "check", events: text(0, "The page passes its checks. Publishing it.") },
      { gate: "publish", events: [toolStart(1, "tu_publish", "run_script"), toolInput(1, call("Publish it", PUBLISH_CODE)), ...end(1, 160)] },
    ],
    [
      { events: [start()] },
      { gate: "finish", events: [toolStart(0, "tu_finish", "finish"), toolInput(0, JSON.stringify({ summary: "Built the launch tracker: 12 tasks across 4 workstreams with owners and due dates, a burndown from kickoff to Nov 18 with a forecast, and points left by owner." })), ...end(0, 90)] },
    ],
  ];
}

const MODELS = { data: [{ type: "model", id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5", created_at: "2026-09-01T00:00:00Z" }], has_more: false, first_id: "claude-sonnet-5-5", last_id: "claude-sonnet-5-5" };

/** The Messages API on loopback, answering each request with the next of
 * `answers()`, a part at a time: a part with a gate waits there until the
 * film opens it. `held()` names the gate an answer waits at. */
async function gatedMessages() {
  const queue = answers();
  let waiting: { gate: string; go: () => void } | null = null;
  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    req.resume();
    req.on("end", () => {
      void (async () => {
        if (req.method === "GET" && req.url?.startsWith("/v1/models")) {
          res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(MODELS));
          return;
        }
        const parts = req.method === "POST" && req.url?.startsWith("/v1/messages") ? queue.shift() : undefined;
        if (!parts) {
          res.writeHead(400, { "content-type": "application/json" }).end(JSON.stringify({ type: "error", error: { type: "invalid_request_error", message: "no scripted answer left" } }));
          return;
        }
        res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
        for (const p of parts) {
          if (p.gate) await new Promise<void>(go => { waiting = { gate: p.gate!, go }; });
          // A ping closes each part, so the part's last event is heard before the next arrives.
          res.write([...p.events, { name: "ping", data: { type: "ping" } }].map(e => `event: ${e.name}\ndata: ${JSON.stringify(e.data)}\n\n`).join(""));
        }
        res.end();
      })();
    });
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", r));
  const { port } = server.address() as AddressInfo;
  return {
    base: `http://127.0.0.1:${port}`,
    held: () => waiting?.gate ?? null,
    open(gate: string) {
      if (waiting?.gate !== gate) throw new Error(`the scripted model waits at ${waiting?.gate ?? "no gate"}, not ${gate}`);
      const { go } = waiting;
      waiting = null;
      go();
    },
    left: () => queue.length,
    stop: () => new Promise<void>(r => { server.closeAllConnections(); server.close(() => r()); }),
  };
}
type Gated = Awaited<ReturnType<typeof gatedMessages>>;

// ---- the film ----

type Cast = { page: Page; fake: Gated };

/** Clicks `target` and types `typed` into it, at TYPING times pace.ts's rhythm. */
async function typeInto(d: Director, page: Page, target: Locator, typed: string): Promise<void> {
  await d.click(target);
  const delays = typingDelays(typed, "claxbot");
  for (const [i, ch] of Array.from(typed).entries()) {
    await d.offstage(() => page.keyboard.type(ch));
    await d.hold(Math.round(delays[i]! * TYPING));
  }
}

/** Waits until the scripted model waits at `gate` and `cond` holds in the view (`page`). */
async function at(d: Director, page: Page, fake: Gated, gate: string, cond: () => Promise<boolean>, what: string): Promise<void> {
  try {
    await d.until(async () => fake.held() === gate && (await cond()), `the model waits at ${gate} and ${what}`);
  } catch (e) {
    throw new Error(`${e instanceof Error ? e.message : String(e)}; the model waits at ${fake.held() ?? "no gate"}, the run panel reads ${JSON.stringify(await d.offstage(() => panelText(page)))}`, { cause: e });
  }
}

/** The run panel's text, or "". */
const panelText = (page: Page) => page.locator("aside.runp").innerText().catch(() => "");

export default {
  id: "claxbot",

  async setup(stage: Stage): Promise<Cast> {
    const fake = await gatedMessages();
    stage.onStop(() => fake.stop());
    const d = await stage.startDaemon({
      config: `${NO_KEY_CONFIG}[bot]\nbase_url = "${fake.base}"\napi_key_env = "CLAX_REEL_BOT_KEY"\n`,
      env: { CLAX_REEL_BOT_KEY: "sk-ant-reel-not-a-real-key-0000" },
    });

    // The owner is Alex, and the gallery has some of Northwind's pages in it already.
    const named = await fetch(`${d.base}/api/viewers/me`, { method: "PUT", headers: { cookie: ownerCookie(d.base, d.token), origin: d.base, "content-type": "application/json" }, body: JSON.stringify({ display_name: "Alex" }) });
    if (!named.ok) throw new Error(`naming the owner: ${named.status} ${await named.text()}`);
    const seeded = readFileSync(new URL("../seed/checklist.html", import.meta.url), "utf8");
    for (const title of ["Launch checklist", "Q4 roadmap", "Press kit"]) await publishWith(d.base, d.token, title, seeded.replace("<title>Launch checklist</title>", `<title>${title}</title>`), {});

    const page = await stage.newCamera();
    // The gallery's haiku is picked with Math.random each visit
    // (web/shell/src/view/haiku.ts): seeded here, it is the same each run.
    await page.context().addInitScript(() => {
      let s = 0x2f6b9a1d;
      Math.random = () => {
        s = (s + 0x6d2b79f5) | 0;
        let t = Math.imul(s ^ (s >>> 15), 1 | s);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      };
    });
    // A second tab of the gallery, off camera, keeps the shell's shared
    // stream worker alive across the camera's navigation to the new
    // artifact, so that page joins it rather than loading the worker's
    // script again: a deterministic frame waits on the page's requests,
    // and a shared worker's script load is never heard to finish there.
    const keeper = await page.context().newPage();
    await keeper.goto(`${d.base}/`);
    await keeper.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    await page.goto(`${d.base}/`);
    await page.getByRole("button", { name: "New artifact" }).waitFor({ state: "visible" });
    await page.getByText("Name it; ClaxBot can build it").waitFor({ state: "visible" });
    await page.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    return { page, fake };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { page, fake } = cast;
    await d.offstage(() => page.clock.setSystemTime(PAGE_TIME));
    await d.hold(200);

    // Alex presses New, names the artifact and writes the brief.
    d.beat("brief");
    await d.click(page.getByRole("button", { name: "New artifact" }));
    const dialog = page.getByRole("dialog", { name: "New artifact" });
    await d.until(() => dialog.locator("#na-bot-l").isVisible(), "the New dialog offers ClaxBot");
    if ((await dialog.getByRole("switch", { name: "ClaxBot builds it now" }).getAttribute("aria-checked")) !== "true") throw new Error(`ClaxBot is not available: ${await dialog.locator(".bot").innerText()}`);
    await typeInto(d, page, dialog.getByLabel("Name"), NAME);
    await typeInto(d, page, dialog.getByLabel("Description"), BRIEF);
    await d.hold(150);
    await d.move(dialog.getByRole("switch", { name: "ClaxBot builds it now" }));
    await d.hold(200);

    // Create and build: the artifact opens with ClaxBot's run panel, and
    // ClaxBot says what it will build and starts writing it.
    await d.click(dialog.getByRole("button", { name: "Create and build" }));
    const panel = page.locator("aside.runp");
    const steps = panel.locator("ol.steps > li");
    await at(d, page, fake, "plan", async () => (await panelText(page)).includes("Building from the brief"), "the run panel shows the run");
    d.beat("building");
    await d.hold(250);
    fake.open("plan");
    await at(d, page, fake, "build", async () => (await panelText(page)).includes("I'll build Northwind's launch tracker"), "the panel shows ClaxBot's plan");
    await d.hold(600);

    // Its script writes the page and checks it; it says so, and publishes.
    fake.open("build");
    await at(d, page, fake, "check", async () => (await panel.locator("li.script summary").first().innerText().catch(() => "")).includes("Build the tracker") && (await steps.count()) === 3, "the script ran and the next turn began");
    await d.hold(350);
    fake.open("check");
    await at(d, page, fake, "publish", async () => (await panelText(page)).includes("The page passes its checks"), "the panel shows ClaxBot's next words");
    await d.hold(350);
    fake.open("publish");
    await at(d, page, fake, "finish", async () => (await panel.locator("li.script").count()) === 2 && (await steps.count()) === 5, "the publish ran");
    fake.open("finish");

    // The run finishes and the view loads the version ClaxBot published,
    // on the page's own timers: the film rolls while it does.
    const shown = async () => {
      const frame = page.frames().find(f => /\/_clax\/v\/2\//.test(f.url()));
      return !!frame && (await frame.evaluate(() => document.querySelectorAll("#tasks tr").length).catch(() => 0)) === 12 && (await panel.locator(".st").innerText().catch(() => "")) === "Finished";
    };
    for (let i = 0; !(await d.offstage(shown)); i++) {
      if (i === 100) throw new Error("the view did not show version 2 and the finished run within 10 s of film");
      await d.hold(100);
    }
    d.beat("done");
    const tracker = page.frames().find(f => /\/_clax\/v\/2\//.test(f.url()))!;
    await d.move(tracker.locator("#burn"), { offset: { x: 30, y: 10 } });
    await d.hold(950);
    if (fake.left() !== 0) throw new Error(`ClaxBot left ${fake.left()} scripted answers unasked`);
  },
} satisfies Vignette<Cast>;
