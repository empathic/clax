// The hosted demo (demo/) end to end on the Worker: crates/clax-worker under
// `wrangler dev --local` in hosted mode with one live people seat, versions
// in git in the RepoDO stand-in (demo/lib/worker.ts), as `just hosted-demo`
// runs it. It covers the demo's shots: Rae and Mia live on the brief, each
// drawing the other's pointer; three scripted agents (demo/lib/agents.ts)
// drafting over `/mcp` and publishing; a comment Rae sends to an agent
// answered with a revision, and Mia's to Rae's agent held; the spectator
// path with one seat ending in Join live; Mia's fork of the checklist
// naming its source; and a real `git clone` of the brief, with the read
// token Rae's Clone dialog makes, whose commits name each version's
// publisher and agent in `Clax-Publisher` and `Clax-Agent` trailers (the
// demo's printed token comes from the same route, `gitToken`).
//
// It needs the web UI and the Worker built, in that order
// (scripts/build-web.sh, then crates/clax-worker/build.sh: the Worker embeds
// web/dist as it is built, crates/clax-server/src/routes/shell.rs, and its
// entry pages name the chunks they load), and runs only with
// CLAX_E2E_WORKER=1, so `npm run e2e` skips it:
//
//   CLAX_E2E_WORKER=1 npx playwright test hosted-demo-worker
//
// CLAX_E2E_SHOTS=1 writes screenshots to target/hosted-shots/ (for looking
// at, not compared).
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Browser, BrowserContext, Frame, FrameLocator, Page } from "@playwright/test";
import { runDemo } from "../../demo/lib/agents";
import { acceptInvite, type PersonSession } from "../../demo/lib/http";
import { HEADING_ANCHOR, seed } from "../../demo/lib/seed";
import { WORKER_DIR, freePort, gitToken, startWorker, type RunningWorker } from "../../demo/lib/worker";
import { expect, setName, test } from "./fixtures";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const SECRET = "e2e worker demo secret";
const BANNER = "Lots of people are here, so you're viewing: comments and live activity are paused.";
const SHOTS = process.env.CLAX_E2E_SHOTS ? join(ROOT, "target/hosted-shots") : "";

/** A browser signed in as `who`, by the session cookie seed made for them. */
async function signedIn(ctx: BrowserContext, origin: string, who: PersonSession): Promise<Page> {
  await ctx.addCookies([{ ...who.cookiePair(), url: origin }]);
  return ctx.newPage();
}

/** Shows the newest version on `p`: the top bar offers an agent's new version as Reload. */
async function showLatest(p: Page) {
  const reload = p.getByRole("button", { name: "Reload" });
  await expect(reload).toBeVisible({ timeout: 20_000 });
  await reload.click();
  await expect(reload).toHaveCount(0);
}

async function shot(p: Page, name: string) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await p.screenshot({ path: join(SHOTS, `demo-worker-${name}.png`) });
}

/** `url` as `who` at phone width in both schemes, photographed (only with CLAX_E2E_SHOTS). */
async function phoneShots(browser: Browser, origin: string, who: PersonSession, url: string, name: string, ready: (p: Page) => Promise<void>) {
  if (!SHOTS) return;
  for (const colorScheme of ["light", "dark"] as const) {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, colorScheme });
    const p = await signedIn(ctx, origin, who);
    await p.goto(url);
    await ready(p);
    // The people pill stays in the top bar, beside Comment and More.
    await expect(p.locator("button.who .ros")).toBeVisible();
    await expect.poll(async () => (await p.locator("button.who").boundingBox())?.width ?? 0).toBeGreaterThan(40);
    await shot(p, `${name}-390-${colorScheme}`);
    expect(await p.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await ctx.close();
  }
}

/** The thread card on `p` whose text has `text`, with the threads panel open. */
async function card(p: Page, text: string) {
  if (!(await p.locator("aside.sidebar").isVisible())) await p.getByRole("button", { name: /Threads/ }).first().click();
  return p.locator(".thread-card").filter({ hasText: text });
}

/** The content frame of `p` (the shell's `iframe.frame`, served on the content origin). */
function contentFrame(p: Page, content: string): Frame | null {
  return p.frames().find((f) => f.url().startsWith(`${content}/c/`)) ?? null;
}

/** Moves `p`'s pointer over the brief's heading in its content frame. */
async function wave(p: Page, frame: FrameLocator) {
  // An element's box in a frame is in the page's coordinates.
  const h = (await frame.locator("h1").boundingBox())!;
  await p.mouse.move(h.x + 20, h.y + h.height / 2);
  await p.mouse.move(h.x + 26, h.y + h.height / 2);
}

/** Runs git with no user or system configuration and no prompts; answers its stdout. */
function git(cwd: string, args: string[]): string {
  const r = spawnSync("git", args, {
    cwd,
    encoding: "utf8",
    timeout: 60_000,
    env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0" },
  });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.status ?? r.signal} ${r.stderr}`);
  return r.stdout;
}

/** Removes all but the newest two runs' directories under `root`. */
function pruneRuns(root: string) {
  if (!existsSync(root)) return;
  const runs = readdirSync(root).filter((n) => /^run-\d+-\d+$/.test(n)).sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]));
  for (const n of runs.slice(2)) rmSync(join(root, n), { recursive: true, force: true });
}

const BUILT = existsSync(join(WORKER_DIR, "build/index.js")) && existsSync(join(ROOT, "web/dist/index.html"));

test("the demo on the Worker: two people live, agents drafting and publishing, comments answered and held, a spectator, a fork and a git clone", async ({ browser }) => {
  test.skip(process.env.CLAX_E2E_WORKER !== "1", "runs with CLAX_E2E_WORKER=1, once the Worker is built");
  test.skip(!BUILT, "build the web UI and the Worker first: scripts/build-web.sh, then crates/clax-worker/build.sh");
  test.setTimeout(240_000);
  const port = await freePort();
  pruneRuns(join(ROOT, "target/e2e-demo-worker"));
  const dir = join(ROOT, `target/e2e-demo-worker/run-${Date.now()}-${process.pid}`);
  let w: RunningWorker | undefined;
  try {
    w = await startWorker({ dir, port, secret: SECRET, seats: "1" });
    const origin = w.base;
    const s = await seed({ url: origin, secret: SECRET, agents: 3, mcpPath: "/mcp" });
    const brief = s.brief.id;
    const rae = await signedIn(await browser.newContext({ viewport: { width: 1280, height: 800 } }), origin, s.owner.session);
    const mia = await signedIn(await browser.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: "dark" }), origin, s.editor.session);
    for (const p of [rae, mia]) {
      await p.goto(`${origin}/a/${brief}`);
      await expect(p.frameLocator("iframe.frame").locator("h1")).toHaveText("Product brief: Trailhead");
      // Content is on the content origin, not the app's.
      await expect.poll(() => contentFrame(p, w!.content) !== null).toBe(true);
    }

    // Two people live: each one's pointer is drawn over the page in the other's browser.
    const arrows = (p: Page) => p.frameLocator("iframe.frame").locator("clax-live .c");
    for (const [from, to] of [[rae, mia], [mia, rae]] as const) {
      await expect.poll(async () => {
        await wave(from, from.frameLocator("iframe.frame"));
        return arrows(to).count();
      }, { timeout: 20_000 }).toBeGreaterThan(0);
    }
    await shot(rae, "two-live-1280-light");
    await shot(mia, "two-live-1280-dark");

    let listening!: () => void;
    const ready = new Promise<void>((r) => { listening = r; });
    const log: string[] = [];
    const demo = runDemo({
      url: origin, mcpPath: "/mcp", token: s.token.token, artifactId: brief, agents: 3, paceMs: 3000, listenS: 90, stopAfter: 1,
      log: (l) => log.push(l), onListening: () => listening(),
    });
    // The agents draft live: the bar above the page names one drafting.
    const drafting = rae.locator(".banner.draft");
    await expect(drafting).toContainText(/-writer.* drafting/, { timeout: 30_000 });
    await shot(rae, "drafting-1280-light");
    // Their last drafts land in both browsers, each round's conflicts merged.
    await ready;
    for (const p of [rae, mia]) await showLatest(p);
    for (const p of [rae, mia]) {
      const who = p.locator(".who .sum b");
      await expect(who).toContainText("summary-writer");
      const frame = p.frameLocator("iframe.frame");
      await expect(frame.locator("#summary")).toContainText("Ask: two engineers and one designer for one quarter.");
      await expect(frame.locator("#users")).toContainText("often with no signal for hours");
      await expect(frame.locator("#proposal")).toContainText("shades each grid cell");
    }
    await shot(rae, "agents-1280-light");
    await phoneShots(browser, origin, s.editor.session, `${origin}/a/${brief}`, "brief", async (p) => {
      await expect(p.frameLocator("iframe.frame").locator("#proposal")).toContainText("shades each grid cell");
    });

    // A comment Rae sends to an agent is answered; Mia's to Rae's agent is held.
    const { working } = await s.owner.session.call<{ working: { agent: string; message: string }[] }>("GET", `/api/artifacts/${brief}/working`);
    const handle = (heading: string) => working.find((x) => x.message === `waiting for comments on ${heading}`)!.agent;
    const head = (await s.owner.session.call<{ artifact: { current_version: number } }>("GET", `/api/artifacts/${brief}`)).artifact.current_version;
    const held = await s.editor.session.form<{ thread: { id: string } }>(`/api/artifacts/${brief}/threads`, { anchor: HEADING_ANCHOR, body: "Add a section on pricing.", version: String(head) });
    const asked = await s.owner.session.form<{ thread: { id: string } }>(`/api/artifacts/${brief}/threads`, { anchor: HEADING_ANCHOR, body: "Say how many searchers a typical callout has.", version: String(head) });
    for (const p of [rae, mia]) {
      await expect(await card(p, "Say how many searchers")).toBeVisible();
      await expect(await card(p, "Add a section on pricing.")).toBeVisible();
    }
    await s.editor.session.call("POST", `/api/artifacts/${brief}/threads/${held.thread.id}/send`, { to: handle("summary") });
    await s.owner.session.call("POST", `/api/artifacts/${brief}/threads/${asked.thread.id}/send`, { to: handle("who it is for") });
    const r = await demo;
    expect(r.answered.map((a) => a.agent)).toEqual(["users-writer"]);
    expect(r.held).toContainEqual(expect.objectContaining({ agent: "summary-writer", thread_id: held.thread.id, authors: ["Mia"] }));
    expect(r.conflicts.length).toBeGreaterThan(0);
    expect([...new Set(r.drafts.map((x) => x.agent))].sort()).toEqual(["proposal-writer", "summary-writer", "users-writer"]);
    expect(log.filter((l) => /no draft tool|draft not shown/.test(l))).toEqual([]);
    for (const p of [rae, mia]) {
      await showLatest(p);
      await expect(p.frameLocator("iframe.frame").locator("#users .revised")).toContainText("Revised for Rae");
      await expect(await card(p, "Say how many searchers")).toContainText("Revised the Who it is for section in version");
    }
    await shot(rae, "answered-1280-light");
    await shot(mia, "answered-1280-dark");

    // A real git clone of the brief, with the token Rae's Clone dialog
    // makes: one commit per version, each agent's naming it and the person
    // it works for.
    await rae.locator("button.clone").click();
    const dialog = rae.getByRole("dialog", { name: "Clone with git or jj" });
    await expect(dialog.locator("input[type=radio]")).toHaveCount(2);
    await dialog.getByRole("button", { name: "Make a token" }).click();
    const command = (await dialog.locator("pre[aria-label=git]").textContent({ timeout: 20_000 })) ?? "";
    await shot(rae, "clone-1280-light");
    await rae.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    const shown = /^git -c http\.extraHeader="Authorization: Bearer (cxm_read_[^"]+)" clone (\S+)$/.exec(command);
    expect(shown, command).not.toBeNull();
    const [, token, remote] = shown!;
    expect(remote).toBe(`${origin}/git/a-${brief}.git`);
    const clone = join(dir, "brief");
    git(dir, ["-c", `http.extraHeader=Authorization: Bearer ${token}`, "clone", "-q", remote, clone]);
    // The demo's printed token: the same route, as Rae.
    const printed = await gitToken(s.owner.session, brief, "read");
    expect(printed.scope).toBe("read");
    git(dir, ["-c", `http.extraHeader=Authorization: Bearer ${printed.token}`, "clone", "-q", printed.remote, join(dir, "brief-printed")]);
    const commits = git(clone, ["log", "--format=%(trailers:key=Clax-Version,valueonly,separator=)|%(trailers:key=Clax-Publisher,valueonly,separator=)|%(trailers:key=Clax-Agent,valueonly,separator=)"])
      .trim().split("\n").map((l) => l.split("|"));
    const current = (await s.owner.session.call<{ artifact: { current_version: number } }>("GET", `/api/artifacts/${brief}`)).artifact.current_version;
    expect(commits.map((c) => Number(c[0]))).toEqual(Array.from({ length: current }, (_, i) => current - i));
    const byVersion = new Map(commits.map(([n, publisher, agent]) => [Number(n), { publisher, agent }]));
    for (const v of [...r.versions, ...r.answered.map((a) => ({ n: a.version, agent: a.agent }))]) {
      expect(byVersion.get(v.n), `v${v.n}'s commit`).toEqual({ publisher: s.owner.public_id, agent: v.agent });
    }
    expect(new Set(r.versions.map((v) => byVersion.get(v.n)!.agent))).toEqual(new Set(["summary-writer", "users-writer", "proposal-writer"]));

    // The spectator path: the brief shared by link; a guest takes the one people seat.
    await s.owner.session.call("PUT", `/api/hosted/artifacts/${brief}/link`, { access: "commenter" });
    const guest = await (await browser.newContext()).newPage();
    await guest.goto(`${origin}/a/${brief}`);
    await setName(guest, "Sam");
    await expect(guest.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    await expect.poll(async () => (await (await guest.request.get(`${origin}/api/artifacts/${brief}/snapshot`)).json()).seats_free, { timeout: 20_000 }).toBe(0);
    // Jo, an editor of the checklist only, holds no reserved seat here: a spectator.
    const jo = await signedIn(await browser.newContext({ viewport: { width: 1280, height: 800 } }), origin, await acceptInvite(origin, s.invite.url));
    await jo.goto(`${origin}/a/${brief}`);
    const banner = jo.locator(".banner.spectate");
    await expect(banner).toContainText(BANNER, { timeout: 20_000 });
    await expect(jo.getByRole("button", { name: /^Comment/ })).toBeDisabled();
    await shot(jo, "spectator-1280-light");
    await expect(rae.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    await expect(mia.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    // The guest leaves: Jo is offered the seat (or takes it at once, when her
    // stream asks after the seat is free) and is live.
    await guest.context().close();
    const joinLive = banner.getByRole("button", { name: "Join live" });
    await expect.poll(async () => (await joinLive.isVisible()) || (await banner.count()) === 0, { timeout: 30_000 }).toBe(true);
    if (await banner.count()) {
      await shot(jo, "join-live-1280-light");
      await joinLive.click();
    }
    await expect(banner).toHaveCount(0);
    await expect(jo.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    const now = (await s.owner.session.call<{ artifact: { current_version: number } }>("GET", `/api/artifacts/${brief}`)).artifact.current_version;
    await s.owner.session.form(`/api/artifacts/${brief}/threads`, { anchor: HEADING_ANCHOR, body: "Jo, the pricing section is yours.", version: String(now) });
    await expect(await card(jo, "Jo, the pricing section is yours.")).toBeVisible();

    // Mia's fork of the checklist's v2 names its source, and links back to it.
    // Her fork is hers: she reads its comments.
    await s.editor.session.call("GET", `/api/artifacts/${s.fork.id}/threads`);
    await mia.goto(s.fork.url);
    const lineage = mia.locator(".ttl .by a.lineage");
    await expect(lineage).toHaveText("Forked from Launch checklist v2");
    await expect(lineage).toHaveAttribute("href", `/a/${s.checklist.id}/v/2`);
    await expect(mia.frameLocator("iframe.frame").locator("main")).toContainText("Submit the app build for review");
    await expect(mia.getByText("Could not load comments")).toHaveCount(0);
    await shot(mia, "fork-1280-dark");
    await phoneShots(browser, origin, s.editor.session, s.fork.url, "fork", async (p) => {
      await expect(p.locator(".ttl .by a.lineage")).toHaveText("Forked from Launch checklist v2");
    });
  } finally {
    await w?.stop();
  }
});
