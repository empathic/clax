// The Clax server in its Durable Object, end to end under `wrangler dev
// --local`, in two browsers: builds the Worker (./build.sh; the web UI must
// be built, scripts/build-web.sh), starts wrangler with fresh state under
// target/clax-worker/e2e/ and an owner token, publishes an artifact through the API,
// and checks that the gallery shows it, that it opens with its content on
// its own origin, that a comment one browser posts reaches the other live
// over the stream's WebSocket, that a rebuilt server (as after hibernation:
// POST /_worker/hibernate) reopens the sockets' streams and still delivers,
// and that after a wrangler restart on the same state everything is still
// there. The object's alarm is checked through GET /_worker/alarm: none
// while nothing is due, set for what is due (not a fixed short period)
// once something is. Prints one line per check and a JSON summary with the cold and
// warm request times; exits non-zero on the first failure.
//
// `node e2e.mjs --no-build` skips the build; CLAX_E2E_SHOTS=<dir> saves
// screenshots there.

import { mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { HERE, STATE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";

const require = createRequire(join(HERE, "../../web/package.json"));
const { chromium } = require("@playwright/test");

const ROOT = join(STATE, "e2e");
const TOKEN = "e2e-owner-token-" + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
const LIMIT = 15_000;
const SHOTS = process.env.CLAX_E2E_SHOTS ?? "";

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

/** Polls `f` until it returns something truthy, or fails after `ms`. */
async function until(what, f, ms = LIMIT) {
  const end = Date.now() + ms;
  let last;
  for (;;) {
    try {
      last = await f();
      if (last) return last;
    } catch (e) {
      last = e instanceof Error ? e.message : e;
    }
    if (Date.now() > end) throw new Error(`FAIL ${what}: ${JSON.stringify(last)}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

async function api(base, path, init = {}) {
  const res = await fetch(`${base}${path}`, {
    ...init,
    headers: { "content-type": "application/json", authorization: `Bearer ${TOKEN}`, ...(init.headers ?? {}) },
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${init.method ?? "GET"} ${path}: ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

/** How far ahead the object's alarm is set, in ms; `null` with none. */
async function alarmIn(base) {
  const { alarm, now } = await api(base, "/_worker/alarm");
  return alarm === null ? null : alarm - now;
}

/** A request's time in ms. */
async function timed(f) {
  const t = performance.now();
  await f();
  return Math.round(performance.now() - t);
}

/** Records the frames of every `/api/stream` WebSocket `page` opens. */
function watchSockets(page) {
  const seen = { opened: 0, frames: [] };
  page.on("websocket", (ws) => {
    if (!new URL(ws.url()).pathname.endsWith("/api/stream")) return;
    seen.opened += 1;
    ws.on("framereceived", (f) => seen.frames.push(String(f.payload)));
  });
  return seen;
}

/** The threads panel, open. */
async function threads(page) {
  const toggle = page.locator("button.threads");
  if ((await toggle.getAttribute("aria-pressed")) !== "true") await toggle.click();
  await page.locator(".new-comment-open").waitFor({ timeout: LIMIT });
}

async function setName(page, name) {
  await page.getByRole("button", { name: "People and agents" }).click();
  const field = page.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
  await field.fill(name);
  await field.press("Enter");
  await page.keyboard.press("Escape");
}

/** Posts `text` from the New comment box. */
async function comment(page, text) {
  await threads(page);
  await page.locator(".new-comment-open").click();
  const composer = page.locator(".new-comment .composer");
  await composer.locator("textarea").fill(text);
  await composer.getByRole("button", { name: "Post comment" }).click();
  await composer.waitFor({ state: "detached", timeout: LIMIT });
}

const cardWith = (page, text) => page.locator(".thread-card", { hasText: text });

async function frameText(page, id, n, selector) {
  const url = new RegExp(`${id}\\.localhost:\\d+/_clax/v/${n}/$`);
  const frame = await until(`the content frame of v${n}`, () => page.frame({ url }));
  return until(`${selector} in the content frame`, () => frame.locator(selector).textContent({ timeout: 1000 }));
}

async function shot(page, name) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${name}.png`) });
}

async function main() {
  await build();
  pruneRuns(ROOT, 3);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const config = writeConfig(dir, { CLAX_TOKEN: TOKEN, CLAX_TEST_ROUTES: "1" });
  const port = await freePort();
  // `localhost`, so artifacts get their `<id>.localhost` origins as on the daemon.
  const base = `http://localhost:${port}`;
  const running = new Set();
  const browser = await chromium.launch();
  const cleanup = async () => {
    await browser.close().catch(() => {});
    await Promise.all([...running].map((w) => w.stop()));
  };
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void cleanup().then(() => process.exit(130)));
  const summary = { ok: false, dir };
  const pages = [];
  try {
    let w = await start(dir, config, 1, port);
    running.add(w);
    summary.cold_ms = await timed(() => api(base, "/api/artifacts"));
    summary.warm_ms = await timed(() => api(base, "/api/artifacts"));
    check(true, `the instance answers (cold ${summary.cold_ms} ms, warm ${summary.warm_ms} ms)`);
    check((await alarmIn(base)) === null, "with nothing due the object sets no alarm", await alarmIn(base));

    const page = "<title>Tide tables</title><main><h2 id=h>Tide tables</h2><p>High water at 06:12.</p></main>";
    const { artifact } = await api(base, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({ title: "Tide tables", files: { "index.html": { content: page, encoding: "utf8" } } }),
    });
    const id = artifact.id;
    check(artifact.current_version === 1, "an artifact is published with the owner token", artifact);
    const optimize = await alarmIn(base);
    check(
      optimize !== null && optimize > 50 * 60_000 && optimize <= 60 * 60_000,
      "after a write the alarm is set for the planner refresh, an hour on",
      optimize,
    );
    const anon = await fetch(`${base}/api/artifacts`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    check(anon.status === 401, "publishing without the token is refused", anon.status);
    const tok = await fetch(`${base}/api/token`);
    // No caller of a Worker holds an owner browser session (only a login on
    // the daemon's own machine opens one), and one with no credential is
    // refused before any route.
    check(tok.status === 401, "GET /api/token serves no one", tok.status);

    // A Worker that is not hosted signs no browser in, and a private
    // artifact's site answers only a site session, which the shell asks for
    // with the owner's credential: each browser carries the owner's token.
    const owner = { viewport: { width: 1280, height: 800 }, extraHTTPHeaders: { authorization: `Bearer ${TOKEN}` } };
    const a = await browser.newContext(owner);
    const b = await browser.newContext(owner);
    const pa = await a.newPage();
    const pb = await b.newPage();
    // The second browser's stream hub runs in its tab, not a shared worker,
    // so the test sees its WebSocket's frames.
    await pb.addInitScript(() => { delete window.SharedWorker; });
    watchSockets(pa);
    const sb = watchSockets(pb);
    for (const [name, p] of [["a", pa], ["b", pb]]) {
      p.on("console", (m) => { if (m.type() === "error" && !m.text().includes("403")) console.log(`     [${name} console] ${m.text()}`); });
      p.on("pageerror", (e) => console.log(`     [${name} error] ${e.message}`));
      p.on("response", (r) => { if (r.status() >= 400 && !r.url().endsWith("/api/token")) console.log(`     [${name}] ${r.status()} ${r.request().method()} ${r.url()}`); });
    }
    pages.push(pa, pb);

    await pa.goto(`${base}/`);
    await pa.locator("a.card", { hasText: "Tide tables" }).waitFor({ timeout: LIMIT });
    check(true, "the gallery shows the artifact");
    await shot(pa, "gallery");

    await pa.goto(`${base}/a/${id}`);
    check((await frameText(pa, id, 1, "#h")) === "Tide tables", "the artifact opens with its content");
    const src = (await pa.locator("iframe.frame").getAttribute("src")) ?? "";
    console.log(`     frame: ${src}`);
    await setName(pa, "Ada");

    await pb.goto(`${base}/a/${id}`);
    await frameText(pb, id, 1, "#h");
    await setName(pb, "Brook");
    await threads(pb);
    await until("the second browser's stream is live", () => pb.evaluate(() => (window.claxStreamLive ?? 0) > 0));
    check(sb.opened >= 1, "the second browser's stream is a WebSocket", sb);
    const live = (summary.alarm_live_ms = await alarmIn(base));
    check(
      live !== null && live > 10_000 && live < 50 * 60_000,
      "with browsers open the alarm is set for the first presence or seat that lapses, not every few seconds",
      live,
    );

    await comment(pa, "Add the low water times too.");
    await cardWith(pb, "Add the low water times too.").waitFor({ timeout: LIMIT });
    check(
      sb.frames.some((f) => f.startsWith("event: thread") && f.includes(id)),
      "the comment reaches the second browser live, over its WebSocket",
      sb.frames.map((f) => f.slice(0, 60)),
    );
    await shot(pb, "live-comment");

    // As after hibernation: a new server on the same storage, the sockets kept.
    const before = sb.opened;
    const hib = await fetch(`${base}/_worker/hibernate`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` } });
    check(hib.status === 200, "the server is rebuilt on the same storage", hib.status);
    const readies = sb.frames.filter((f) => f.startsWith("event: ready")).length;
    await api(base, `/api/artifacts/${id}`);
    await until("a new ready on the kept socket", () => sb.frames.filter((f) => f.startsWith("event: ready")).length > readies);
    check(sb.opened === before, "the woken server opens the kept socket a new stream, on the same socket", { before, after: sb.opened });
    await until("the stream is live again", () => pb.evaluate(() => (window.claxStreamLive ?? 0) > 0));
    await comment(pa, "And the moon phase.");
    await cardWith(pb, "And the moon phase.").waitFor({ timeout: LIMIT });
    check(true, "a comment after the rebuild still arrives live");

    // A restart on the same state.
    await w.stop();
    running.delete(w);
    w = await start(dir, config, 2, port);
    running.add(w);
    summary.restart_cold_ms = await timed(() => api(base, "/api/artifacts"));
    const list = await api(base, "/api/artifacts");
    check(list.artifacts.some((x) => x.id === id), "after the restart the API lists the artifact", list.artifacts.map((x) => x.id));
    await pb.goto(`${base}/`);
    await pb.locator("a.card", { hasText: "Tide tables" }).waitFor({ timeout: LIMIT });
    check(true, "after the restart the gallery shows the artifact");
    await pb.goto(`${base}/a/${id}`);
    check((await frameText(pb, id, 1, "#h")) === "Tide tables", "after the restart the content is served");
    await threads(pb);
    for (const text of ["Add the low water times too.", "And the moon phase."]) {
      await cardWith(pb, text).waitFor({ timeout: LIMIT });
    }
    check(true, "after the restart both comments are there");
    await pa.goto(`${base}/a/${id}`);
    await frameText(pa, id, 1, "#h");
    await until("the second browser's stream is live after the restart", () => pb.evaluate(() => (window.claxStreamLive ?? 0) > 0));
    await comment(pa, "Thanks!");
    await cardWith(pb, "Thanks!").waitFor({ timeout: LIMIT });
    check(true, "after the restart a comment still arrives live");
    await shot(pb, "after-restart");

    const health = await fetch(`${base}/healthz`).then((r) => r.json());
    summary.ok = true;
    summary.version = health.version;
    console.log(JSON.stringify(summary));
  } catch (e) {
    for (const [i, p] of pages.entries()) await p.screenshot({ path: join(dir, `failure-${i}.png`) }).catch(() => {});
    throw e;
  } finally {
    await cleanup();
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
