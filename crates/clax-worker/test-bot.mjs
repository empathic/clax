// Runs ClaxBot end to end on the Worker under `wrangler dev --local`
// (src/bot.rs): the instance's alarm drives the runs, the wasm Messages
// client (`fetch`) talks to a scripted Messages API on 127.0.0.1, and the
// scripts run in the Worker Loader sandbox. In order:
//
// 1. without CLAX_KEK: storing the instance key is 503 no_kek, a send to
//    ClaxBot is 403 not_configured, the instance serves on, and the
//    provider's URL is refused for a loopback, private, metadata or
//    internal host (without the test escape the Worker sends the key to
//    public https hosts only);
// 2. restarted with CLAX_KEK and the escape (CLAX_TEST_LOOPBACK_PROVIDER),
//    on the same state: the owner sets the provider's URL and key and sends
//    a thread to ClaxBot; the first turn's script reads the page, edits the
//    draft, publishes version 2 and replies on the thread; the second turn
//    is held open by the API, while the alarm drives, kept a resume
//    interval ahead and renewed; a second run started meanwhile leaves it
//    driving;
// 3. wrangler is killed by its process group while that turn streams and
//    started again on the same state, and nothing asks the instance
//    anything: its alarm alone resumes the runs in the new incarnation, the
//    cut turn is asked again, the run finishes, nothing is published or
//    replied twice, and its tokens equal what the API served, the cut
//    stream's included, each once;
// 4. the run's events, read with the Last-Event-ID the reader had before
//    the restart, replay from the committed steps (the live hub went with
//    the old incarnation) up to run_end, with no ID at or before it;
//    the second run, which waited for the person's one slot, is cancelled
//    while its turn is held: it ends stopped and the API sees its connection
//    closed (the wasm transport's abort);
// 5. ClaxBot on the web, through the scripted web source the test routes
//    select (CLAX_TEST_WEB, POST /_worker/web-fixtures, loaded before the
//    restart and kept across it): a run whose script searches, fetches the
//    first result and finishes is charged one search and one fetch, and
//    its reply ends with the Sources it fetched; a fetch of a page that
//    redirects to itself ends web_unavailable after five redirects, one
//    source call, charged once;
// 6. with the free week cut to nothing, a send is 403 credits_exhausted;
// 7. with every run ended, no alarm is held for ClaxBot.
// (deploy.mjs's CLAX_KEK plan is deploy-lib.test.mjs's.)
//
// Prints one line per check and a JSON summary; exits non-zero on the first
// failure. `node test-bot.mjs --no-build` skips the build.

import { mkdirSync } from "node:fs";
import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { join } from "node:path";
import { HERE, STATE, build, pruneRuns, start, writeConfig } from "./harness.mjs";

const ROOT = join(STATE, "bot-test");
const KEEP_RUNS = 3;
const TOKEN = "bot-test-token-0123456789abcdef0123456789abcdef";
const KEK = randomBytes(32).toString("hex");
const KEY = "sk-ant-api03-WORKERTESTKEY-wxyz";
// The longest any one condition is waited for: a run's steps here take well
// under a second each; this only bounds a hang.
const LIMIT_MS = 60_000;
// How far ahead the instance keeps its alarm while ClaxBot drives
// (CLAX_TEST_RESUME_AFTER_MS; a minute but in tests), so a resume from the
// alarm alone is waited for in seconds.
const RESUME_MS = 3_000;

const summary = { ok: true, checks: 0 };
function check(cond, what, detail) {
  summary.checks++;
  if (!cond) {
    summary.ok = false;
    throw new Error(`FAIL ${what}${detail === undefined ? "" : `: ${JSON.stringify(detail).slice(0, 1500)}`}`);
  }
  console.log(`ok   ${what}`);
}

/** Polls `f` until it returns a value other than undefined. */
async function until(what, f) {
  const end = Date.now() + LIMIT_MS;
  let last;
  for (;;) {
    try {
      const v = await f();
      if (v !== undefined) return v;
    } catch (e) {
      last = e instanceof Error ? e.message : e;
    }
    if (Date.now() > end) throw new Error(`FAIL ${what} within ${LIMIT_MS / 1000} s${last ? `: ${last}` : ""}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

async function call(base, method, path, body) {
  const headers = { authorization: `Bearer ${TOKEN}` };
  let payload;
  if (body instanceof FormData) payload = body;
  else if (body !== undefined) {
    headers["content-type"] = "application/json";
    payload = JSON.stringify(body);
  }
  const res = await fetch(`${base}${path}`, { method, headers, body: payload });
  const text = await res.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = { text };
  }
  return { status: res.status, json };
}

async function ok(base, method, path, body) {
  const r = await call(base, method, path, body);
  if (r.status >= 300) throw new Error(`${method} ${path}: ${r.status} ${JSON.stringify(r.json)}`);
  return r.json;
}

// ---------------------------------------------------------------------------
// The scripted Messages API
// ---------------------------------------------------------------------------

/** `POST /v1/messages` answered by `plan(body)`: `{blocks, stop, usage}` streams
 * a whole turn; `{hold, usage}` sends `message_start` and holds the response
 * open until the client closes it. Every request's usage as sent is kept by
 * run marker, with each held request's close. */
function fakeMessages(plan) {
  const seen = [];
  const server = createServer((req, res) => {
    if (req.method !== "POST" || req.url !== "/v1/messages") {
      res.writeHead(404).end();
      return;
    }
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const body = JSON.parse(raw);
      const p = plan(body);
      const rec = { marker: p.marker, key: req.headers["x-api-key"], input: p.usage.input, output: 1, held: !!p.hold, closed: false };
      seen.push(rec);
      res.writeHead(200, { "content-type": "text/event-stream" });
      const ev = (name, data) => res.write(`event: ${name}\ndata: ${JSON.stringify({ type: name, ...data })}\n\n`);
      ev("message_start", {
        message: { id: `msg_${seen.length}`, type: "message", role: "assistant", model: body.model, content: [],
          usage: { input_tokens: p.usage.input, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, output_tokens: 1 } },
      });
      if (p.hold) {
        res.on("close", () => (rec.closed = true));
        return;
      }
      p.blocks.forEach((b, i) => {
        if (b.type === "text") {
          ev("content_block_start", { index: i, content_block: { type: "text", text: "" } });
          ev("content_block_delta", { index: i, delta: { type: "text_delta", text: b.text } });
        } else {
          ev("content_block_start", { index: i, content_block: { type: "tool_use", id: b.id, name: b.name, input: {} } });
          ev("content_block_delta", { index: i, delta: { type: "input_json_delta", partial_json: JSON.stringify(b.input) } });
        }
        ev("content_block_stop", { index: i });
      });
      rec.output = p.usage.output;
      ev("message_delta", { delta: { stop_reason: p.stop, stop_sequence: null }, usage: { output_tokens: p.usage.output } });
      ev("message_stop", {});
      res.end();
    });
  });
  return new Promise((done, fail) => {
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const a = server.address();
      done({
        base: `http://127.0.0.1:${a.port}`,
        seen,
        close: () => new Promise((d) => { server.closeAllConnections(); server.close(() => d()); }),
      });
    });
  });
}

const SCRIPT = `
const page = await clax.files.read("index.html");
await clax.files.replace("index.html", "06:12", "06:15");
const after = await clax.files.read("index.html");
const out = { read: page.includes("06:12"), edited: after.includes("06:15") };
try { out.published = (await clax.draft.publish({ note: "06:15" })).version; } catch (e) { out.publish_error = e.code; }
const [t] = await clax.threads.list();
try { out.replied = !!(await clax.threads.reply(t.id, "Changed it to 06:15.")).comment_id; } catch (e) { out.reply_error = e.code; }
return out;
`;

/** The web run's script: a search, a fetch of its first result. */
const WEB_SCRIPT = `
const s = await clax.web.search("tide tables today");
const p = await clax.web.fetch(s.results[0].url);
return { results: s.results.length, first: s.results[0].url, status: p.status, text: p.text, left: p.left };
`;

/** The redirect loop's script: a fetch of a page that redirects to itself. */
const LOOP_SCRIPT = `
try { await clax.web.fetch("https://loop.example/"); return { fetched: true }; }
catch (e) { return { code: e.code }; }
`;

/** What the scripted web source answers (src/web_source.rs FixtureWeb). */
const WEB_FIXTURES = {
  searches: {
    "tide tables today": [
      { url: "https://tides.example/today", title: "Tides today", description: "High water at 06:15." },
      { url: "https://tides.example/week", title: "This week's tides" },
    ],
  },
  pages: {
    "https://tides.example/today": { status: 200, content_type: "text/markdown", text: "# Tides today\n\nHigh water at 06:15.\n", links: [] },
    "https://loop.example/": { redirect: "https://loop.example/" },
  },
};

/** A run that runs `code` once, then finishes with `summary`. */
function scriptThenFinish(marker, turns, code, summary) {
  if (turns === 0) {
    return {
      marker,
      blocks: [{ type: "tool_use", id: `tu_${marker}_1`, name: "run_script", input: { title: marker, code } }],
      stop: "tool_use",
      usage: { input: 700, output: 30 },
    };
  }
  return {
    marker,
    blocks: [{ type: "tool_use", id: `tu_${marker}_2`, name: "finish", input: { summary } }],
    stop: "tool_use",
    usage: { input: 800, output: 20 },
  };
}

/** The scripted runs, told apart by their thread's text. */
let held = 0;
function plan(body) {
  const text = JSON.stringify(body.messages);
  const turns = body.messages.filter((m) => m.role === "assistant").length;
  if (text.includes("Hold this one")) return { marker: "cancel", hold: true, usage: { input: 900 } };
  // The loop's thread is on the web run's artifact, after it: asked first.
  if (text.includes("Check the looping page")) return scriptThenFinish("loop", turns, LOOP_SCRIPT, "The page redirects to itself.");
  if (text.includes("Look up today's tides")) return scriptThenFinish("web", turns, WEB_SCRIPT, "High water is at 06:15 today.");
  if (turns === 0) {
    return {
      marker: "main",
      blocks: [
        { type: "text", text: "Reading the page." },
        { type: "tool_use", id: "tu_1", name: "run_script", input: { title: "Fix the time", code: SCRIPT } },
      ],
      stop: "tool_use",
      usage: { input: 1200, output: 80 },
    };
  }
  if (held++ === 0) return { marker: "main", hold: true, usage: { input: 1500 } };
  return {
    marker: "main",
    blocks: [{ type: "tool_use", id: "tu_2", name: "finish", input: { summary: "High water is now 06:15." } }],
    stop: "tool_use",
    usage: { input: 1500, output: 40 },
  };
}

// ---------------------------------------------------------------------------
// The run's events
// ---------------------------------------------------------------------------

/** Reads run `rid`'s SSE stream (after `last`), calling `on` with each
 * `{id, event, data}`; resolves with every event once `run_end` arrives or
 * `stop()` says so. */
function readEvents(base, rid, last, on = () => false) {
  const ctl = new AbortController();
  const got = [];
  const done = (async () => {
    const headers = { authorization: `Bearer ${TOKEN}` };
    if (last) headers["last-event-id"] = last;
    const res = await fetch(`${base}/api/bot/runs/${rid}/events`, { headers, signal: ctl.signal });
    if (res.status !== 200) throw new Error(`events: ${res.status} ${await res.text()}`);
    if (!String(res.headers.get("content-type")).startsWith("text/event-stream")) throw new Error(`events: ${res.headers.get("content-type")}`);
    const dec = new TextDecoder();
    let buf = "";
    try {
      for await (const chunk of res.body) {
        buf += dec.decode(chunk, { stream: true });
        let end;
        while ((end = buf.indexOf("\n\n")) >= 0) {
          const frame = buf.slice(0, end);
          buf = buf.slice(end + 2);
          const e = {};
          for (const line of frame.split("\n")) {
            if (line.startsWith("id: ")) e.id = line.slice(4);
            else if (line.startsWith("event: ")) e.event = line.slice(7);
            else if (line.startsWith("data: ")) e.data = JSON.parse(line.slice(6));
          }
          if (!e.event) continue;
          got.push(e);
          if (e.event === "run_end" || on(e)) {
            ctl.abort();
            return got;
          }
        }
      }
    } catch (e) {
      if (!ctl.signal.aborted) throw e;
    }
    return got;
  })();
  return { done, got, abort: () => ctl.abort() };
}

/** `(step, attempt, seq)` of an event ID, comparable. */
function idKey(id) {
  const [sa, seq] = id.split(":");
  const [s, a] = sa.split(".");
  return [Number(s), Number(a), Number(seq)];
}
const after = (a, b) => {
  const [x, y] = [idKey(a), idKey(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i];
  return false;
};

// ---------------------------------------------------------------------------

async function thread(base, aid, text) {
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > p", quote: "High water", prefix: "", suffix: "", html_hash: "sha256:00", rect: null, custom_name: null }));
  form.set("body", text);
  form.set("version", "1");
  return (await ok(base, "POST", `/api/artifacts/${aid}/threads`, form)).thread.id;
}

async function run(base, rid) {
  return ok(base, "GET", `/api/bot/runs/${rid}`);
}

/** The `body` of every comment in a threads answer (each object with
 * a string `body` and an `id`, wherever it is nested), each once. */
function commentBodies(v, seen = new Set()) {
  if (Array.isArray(v)) return v.flatMap((x) => commentBodies(x, seen));
  if (!v || typeof v !== "object") return [];
  const own = typeof v.body === "string" && typeof v.id === "string" && !seen.has(v.id) ? (seen.add(v.id), [v.body]) : [];
  return [...own, ...Object.values(v).flatMap((x) => commentBodies(x, seen))];
}

/** How far ahead the object's alarm is set, in ms; `null` with none. */
async function alarmIn(base) {
  const { alarm, now } = await ok(base, "GET", "/_worker/alarm");
  return alarm === null ? null : alarm - now;
}

const ENDED = ["finished", "failed", "gave_up", "stopped"];

async function main() {
  await build();
  pruneRuns(ROOT, KEEP_RUNS);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const fake = await fakeMessages(plan);
  let w;
  const stop = () => (w ? w.stop() : Promise.resolve());
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void stop().then(() => process.exit(130)));
  const vars = { CLAX_TOKEN: TOKEN, CLAX_TEST_ROUTES: "1", CLAX_TEST_WEB: "1", CLAX_TEST_RESUME_AFTER_MS: String(RESUME_MS) };
  try {
    // 1. No CLAX_KEK, and no test escape for the provider's URL.
    w = await start(dir, writeConfig(dir, vars), 1);
    let base = w.base;
    const page = "<title>Tide tables</title><main><h2>Tide tables</h2><p>High water at 06:12.</p></main>";
    const { artifact } = await ok(base, "POST", "/api/artifacts", {
      title: "Tide tables", files: { "index.html": { content: page, encoding: "utf8" } },
    });
    const aid = artifact.id;
    const tid = await thread(base, aid, "Make the time 06:15");
    let r = await call(base, "PUT", "/api/admin/provider/key", { key: KEY });
    check(r.status === 503 && r.json?.error?.code === "no_kek", "without CLAX_KEK the instance key is not stored (503 no_kek)", r);
    r = await call(base, "POST", "/api/bot/runs", { thread_ids: [tid], artifact_id: aid });
    check(r.status === 403 && r.json?.error?.code === "not_configured", "without CLAX_KEK a send to ClaxBot is 403 not_configured", r);
    check((await call(base, "GET", "/healthz")).status === 200, "the instance serves on without CLAX_KEK");
    for (const url of [fake.base, "https://10.0.0.5", "https://169.254.169.254", "https://metadata.google.internal", "https://localhost"]) {
      r = await call(base, "PUT", "/api/admin/provider", { base_url: url });
      check(r.status === 400 && r.json?.error?.code === "invalid_setting", `the Worker refuses ${url} as the provider's URL`, r);
    }
    await w.stop();

    // 2. With CLAX_KEK and the test escape, the same state.
    const config = writeConfig(dir, { ...vars, CLAX_KEK: KEK, CLAX_TEST_LOOPBACK_PROVIDER: "1" });
    w = await start(dir, config, 2);
    base = w.base;
    await ok(base, "PUT", "/api/admin/provider", { base_url: fake.base });
    const key = await ok(base, "PUT", "/api/admin/provider/key", { key: KEY });
    check(JSON.stringify(key).includes("wxyz") && !JSON.stringify(key).includes(KEY), "the instance key is stored, write-only", key);
    // The web source the test routes select, scripted before the restart.
    r = await call(base, "POST", "/_worker/web-fixtures", { pages: [] });
    check(r.status === 400, "fixtures that are not fixtures are refused", r);
    await ok(base, "POST", "/_worker/web-fixtures", WEB_FIXTURES);
    const web = await ok(base, "PUT", "/api/admin/web", { search: { enabled: true }, fetch: { enabled: true } });
    check(web.configured === true && web.search.enabled === true && web.fetch.enabled === true, "with the test web source the host's web use is configured, and the admin turns it on", web);
    await ok(base, "PUT", "/api/me/bot", { web: true });
    r = await call(base, "POST", "/api/bot/runs", { thread_ids: [tid], artifact_id: aid });
    check(r.status === 201, "a thread is sent to ClaxBot", r);
    const rid = r.json.run.id;
    const live = readEvents(base, rid, null, () => fake.seen.some((s) => s.marker === "main" && s.held));
    await until("the second turn is held", () => (fake.seen.some((s) => s.marker === "main" && s.held) ? true : undefined));
    const mid = await run(base, rid);
    check(mid.status === "running", "the run is running when its second turn streams", mid.status);
    const scriptStep = mid.steps?.find((s) => s.kind === "script");
    check(scriptStep?.value?.read === true && scriptStep?.value?.edited === true, "the script read the page and edited the draft", scriptStep);
    check(scriptStep.value.published === 2 && scriptStep.value.replied === true, "the script published version 2 and replied", scriptStep.value);
    const art = await ok(base, "GET", `/api/artifacts/${aid}`);
    check(art.artifact.current_version === 2, "the artifact is at version 2 on the Worker", art.artifact.current_version);
    const threads = JSON.stringify(await ok(base, "GET", `/api/artifacts/${aid}/threads`));
    check(threads.includes("Changed it to 06:15."), "ClaxBot's reply is on the thread", threads.slice(0, 500));
    check(fake.seen.every((s) => s.key === KEY), "every model request carried the instance key");
    // The alarm drives; it is kept RESUME_MS ahead, and renewed.
    const a1 = await alarmIn(base);
    check(a1 !== null && a1 > 0 && a1 <= RESUME_MS + 1_000, "while the alarm drives, it is kept a resume interval ahead", a1);
    const firstAt = (await ok(base, "GET", "/_worker/alarm")).alarm;
    const renewed = await until("the alarm is renewed while the turn is held", async () => {
      const { alarm, now } = await ok(base, "GET", "/_worker/alarm");
      return alarm !== null && alarm !== firstAt && alarm > now ? alarm - now : undefined;
    });
    check(renewed <= RESUME_MS + 1_000, "the renewed alarm is again a resume interval ahead", renewed);
    // A run started while the alarm drives (one person's runs go one at a
    // time: it waits for the first, in this alarm or the next).
    const tid2 = await thread(base, aid, "Hold this one");
    const r2 = await ok(base, "POST", "/api/bot/runs", { thread_ids: [tid2], artifact_id: aid });
    const rid2 = r2.run.id;
    const a2 = await alarmIn(base);
    check(a2 !== null && a2 > 0 && a2 <= RESUME_MS + 1_000, "a run started while the alarm drives leaves it driving, not set again for now", a2);
    // The live stream's events up to here; its last ID is where it resumes.
    await until("the stream has the script step's end", () => (live.got.some((e) => e.event === "step_done" && e.data?.step?.kind === "script") ? true : undefined));
    live.abort();
    const lastId = live.got.at(-1).id;

    // 3. Kill wrangler mid-turn; start it again on the same state. Nothing
    // asks the instance anything: the alarm alone resumes the runs.
    await w.stop();
    const cut = fake.seen.find((s) => s.marker === "main" && s.held);
    await until("the cut turn's connection closes", () => (cut.closed ? true : undefined));
    const before = fake.seen.length;
    w = await start(dir, config, 3);
    base = w.base;
    await until("the alarm alone resumes the run (the cut turn is asked again)", () =>
      fake.seen.length > before && fake.seen.slice(before).some((s) => s.marker === "main") ? true : undefined);
    await until("the second run's turn is held after the first ends", () => (fake.seen.some((s) => s.marker === "cancel") ? true : undefined));
    // The second run's turn holds the person's one slot: it is cancelled
    // while its turn is held, and the first run takes its last step.
    await ok(base, "POST", `/api/bot/runs/${rid2}/cancel`);
    const stopped = await until("the cancelled run ends", async () => {
      const x = await run(base, rid2);
      return ENDED.includes(x.status) ? x : undefined;
    });
    check(stopped.status === "stopped", "a cancelled run ends stopped", { status: stopped.status, reason: stopped.reason });
    const held2 = fake.seen.find((s) => s.marker === "cancel");
    await until("the cancelled turn's request is aborted", () => (held2.closed ? true : undefined));
    check(held2.closed, "the cancel aborts the model request (the wasm transport's AbortController)");

    const done = await until("the first run finishes", async () => {
      const x = await run(base, rid);
      return ENDED.includes(x.status) ? x : undefined;
    });
    check(done.status === "finished", "the run resumed in the new incarnation from its alarm and finished", { status: done.status, reason: done.reason });
    const kinds = done.steps.map((s) => s.kind);
    check(JSON.stringify(kinds) === JSON.stringify(["turn", "script", "turn", "finish"]), "its steps are each committed once", kinds);
    const served = fake.seen.filter((s) => s.marker === "main");
    check(served.length === 3, "the model was asked three times: the cut turn again", served.length);
    const input = served.reduce((n, s) => n + s.input, 0);
    const output = served.reduce((n, s) => n + s.output, 0);
    check(done.tokens.input === input && done.tokens.output === output,
      "the run's tokens equal what the API served, the cut stream's included, each once", { tokens: done.tokens, served: { input, output } });
    const after2 = await ok(base, "GET", `/api/artifacts/${aid}`);
    check(after2.artifact.current_version === 2, "the resumed run published nothing twice", after2.artifact.current_version);
    const replies = (JSON.stringify(await ok(base, "GET", `/api/artifacts/${aid}/threads`)).match(/Changed it to 06:15\./g) ?? []).length;
    check(replies === 1, "and replied once", replies);
    const usage = await ok(base, "GET", "/api/me/usage");
    const line = (l) => usage.totals.find((t) => t.line === l);
    check(line("do.requests")?.qty > 0 && line("toolpath.storage")?.qty > 0, "the hosted lines (DO requests, Toolpath storage) are charged", usage.totals.map((t) => t.line));

    // 4. SSE replay across the restart.
    const replay = await readEvents(base, rid, lastId).done;
    check(replay.length > 0 && replay.at(-1).event === "run_end", "the events after the restart replay to run_end", replay.map((e) => e.event));
    check(replay.every((e) => after(e.id, lastId)), "every replayed event is after the reader's Last-Event-ID", { lastId, ids: replay.map((e) => e.id) });
    const doneKinds = replay.filter((e) => e.event === "step_done").map((e) => e.data?.step?.kind);
    check(JSON.stringify(doneKinds) === JSON.stringify(["turn", "finish"]), "the replay rebuilds the committed steps the live hub lost", doneKinds);

    // 5. ClaxBot on the web, after the restart: the fixtures were kept.
    const wid = (await ok(base, "POST", "/api/artifacts", {
      title: "Tide sources", files: { "index.html": { content: page, encoding: "utf8" } },
    })).artifact.id;
    const webOf = (u) => u.totals.filter((t) => t.line.startsWith("web."));
    const before5 = webOf(await ok(base, "GET", "/api/me/usage"));
    check(before5.length === 0, "no web use is charged before a run uses the web", before5);
    const wt = await thread(base, wid, "Look up today's tides");
    const wr = (await ok(base, "POST", "/api/bot/runs", { thread_ids: [wt], artifact_id: wid })).run.id;
    const wdone = await until("the web run ends", async () => {
      const x = await run(base, wr);
      return ENDED.includes(x.status) ? x : undefined;
    });
    check(wdone.status === "finished", "the web run finished", { status: wdone.status, reason: wdone.reason });
    const wstep = wdone.steps.find((x) => x.kind === "script");
    check(wstep?.value?.results === 2 && wstep.value.first === "https://tides.example/today" && wstep.value.status === 200
      && wstep.value.text.includes("High water at 06:15."), "its script searched the fixture and fetched the first result", wstep);
    check(JSON.stringify(wstep.calls?.map((x) => [x.op, x.ok])) === JSON.stringify([["web.search", true], ["web.fetch", true]]),
      "the script's calls are one search and one fetch, both answered", wstep.calls);
    const rows = webOf(await ok(base, "GET", "/api/me/usage"));
    check(rows.length === 2 && rows.some((t) => t.line.startsWith("web.search.") && t.qty === 1) && rows.some((t) => t.line === "web.fetch" && t.qty === 1),
      "the run is charged exactly one search and one fetch", rows);
    const wthreads = await ok(base, "GET", `/api/artifacts/${wid}/threads`);
    const webReplies = commentBodies(wthreads).filter((t) => t.startsWith("High water is at 06:15 today."));
    const sources = webReplies[0]?.split("\n").reduce((at, l, i, all) => (/Sources/.test(l) ? all.slice(i) : at), null);
    check(webReplies.length === 1 && sources?.some((l) => l.includes("https://tides.example/today"))
      && !webReplies[0].includes("tides.example/week"), "the reply ends with the Sources it fetched (not those it only found)", webReplies);
    // a_redirect_loop_ends_after_five: a page that redirects to itself.
    const lt = await thread(base, wid, "Check the looping page https://loop.example/");
    const lr = (await ok(base, "POST", "/api/bot/runs", { thread_ids: [lt], artifact_id: wid })).run.id;
    const ldone = await until("the loop run ends", async () => {
      const x = await run(base, lr);
      return ENDED.includes(x.status) ? x : undefined;
    });
    const lstep = ldone.steps.find((x) => x.kind === "script");
    check(lstep?.value?.code === "web_unavailable", "a fetch of a page that redirects to itself ends web_unavailable (it never hangs)", lstep);
    const { calls } = await ok(base, "GET", "/_worker/web-fixtures");
    check(calls.filter(([op, u]) => op === "fetch" && u === "https://loop.example/").length === 1, "the loop is one source call", calls);
    const fetches = webOf(await ok(base, "GET", "/api/me/usage")).find((t) => t.line === "web.fetch");
    check(fetches?.qty === 2, "and is charged once: it went out", fetches);

    // 6. Credits exhausted.
    const pricing = await ok(base, "GET", "/api/admin/pricing");
    await ok(base, "PUT", "/api/admin/pricing", { ...pricing, free_weekly_milli: 1 });
    const tid3 = await thread(base, aid, "One more");
    r = await call(base, "POST", "/api/bot/runs", { thread_ids: [tid3], artifact_id: aid });
    check(r.status === 403 && r.json?.error?.code === "credits_exhausted", "with the week spent a send is 403 credits_exhausted", r);

    // 7. With every run ended, ClaxBot holds no alarm: what is left is the
    // instance's own timed work, at least its sweep period ahead.
    const runs = await ok(base, "GET", `/api/bot/runs?artifact=${aid}`);
    const list = runs.runs ?? runs;
    check(Array.isArray(list) && list.length === 2 && list.every((x) => ENDED.includes(x.status)), "every run has ended", list.map?.((x) => x.status));
    const last = await ok(base, "GET", "/_worker/alarm");
    check(last.bot === null, "with no run left, ClaxBot's driver wants no alarm", last);
    check(last.alarm === null || last.alarm > last.now, "and the alarm left is only the instance's own work, ahead", last);
  } finally {
    await stop();
    await fake.close();
  }
  console.log(JSON.stringify({ ...summary, dir }));
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  console.log(JSON.stringify({ ...summary, ok: false }));
  process.exit(1);
});
