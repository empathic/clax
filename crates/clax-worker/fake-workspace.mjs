// What test-bot.mjs talks to in place of the real thing: a workspace service
// speaking crates/clax-bot-remote/src/wire.rs, and a Zulip site's message
// route. Each keeps what it was asked; neither reaches anything.

import { createServer } from "node:http";

/** An offer, in the wire's form: every field, none unknown. */
const offer = (o) => ({
  shell: false, change: false, confidential: false, reviewed_previews: false, wake_on_reply: false,
  no_follow_ups: false, answers_only: false, unsure: false, guidance: null, preview_gate: null, ...o,
});
const SHELL = offer({ shell: true });
const CHANGE = offer({ shell: true, change: true });
/** How long a preview may yet be told of, as the service's PREVIEW_GRACE_MS. */
const GRACE_MS = 120_000;

/** The service's rule for a run's or a thread's ID, and for a job's key. */
const wellFormed = (id, limit, extra = "") =>
  typeof id === "string" && id.length <= limit && new RegExp(`^[A-Za-z0-9][A-Za-z0-9_\\-${extra}]*$`).test(id);
const isId = (id) => wellFormed(id, 128);
const isKey = (key) => wellFormed(key, 192, ":");

function listening(server) {
  return new Promise((done, fail) => {
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const a = server.address();
      done(`http://127.0.0.1:${a.port}`);
    });
  });
}

const closing = (server) => new Promise((d) => { server.closeAllConnections(); server.close(() => d()); });

function readBody(req) {
  return new Promise((done) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => done(raw));
  });
}

/** A workspace service holding `secret`, behind what asks for `front`, the
 * `{ id, secret }` of the two extra headers, when given: a request without
 * them never reaches the service, and answers 403 and a page of its own, which
 * `stopped` counts.
 *
 * A job whose command starts with `hold` runs until `release(command)`; any
 * other answers at once, as does a proposal, with `proposal`. A job's poll
 * waits as the real one does, for its end or `wait` seconds, so one a tick
 * gives up shows as `abandoned`. `down` drops every connection unanswered,
 * and `blips` that many polls of a job.
 * `queues` and `others` are what it offers; `tell` adds a preview's event,
 * which stays until acknowledged, and `waiting` says a wait is going on. */
export async function fakeWorkspace(secret, instance, front) {
  const s = {
    secret,
    seq: 0,
    jobs: new Map(),
    posts: [],
    polls: 0,
    abandoned: 0,
    reaps: [],
    ended: [],
    events: [],
    acked: [],
    badIds: [],
    refused: 0,
    stopped: 0,
    fronted: 0,
    served: 0,
    previewPolls: 0,
    down: false,
    dropped: 0,
    blips: 0,
    waiting: false,
    queues: {},
    others: SHELL,
    runs: {},
    proposal: (n) => ({ pr: 7, url: "https://forge.example/o/r/pull/7", commit: `${String(n).repeat(40)}`.slice(0, 40), verify_url: null }),
    nextEvent: 0,
    proposed: 0,
  };

  const snapshot = () => ({
    seq: ++s.seq,
    runs: s.runs,
    queues: s.queues,
    others: s.others,
    private_runs: [],
    private_queues: [],
    previews_until_ms: s.waiting || s.events.length > 0 ? Date.now() + GRACE_MS : null,
  });
  const json = (res, status, v) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(v));
  };
  const answer = (job) => {
    const state = job.done ? { state: "done", value: job.done } : { state: "running" };
    return { ...state, snapshot: snapshot() };
  };

  const server = createServer(async (req, res) => {
    const blip = s.blips > 0 && req.method === "GET" && req.url.startsWith("/v1/jobs/");
    if (s.down || blip) {
      if (blip) s.blips--;
      s.dropped++;
      req.socket.destroy();
      return;
    }
    if (front) {
      if (req.headers["cf-access-client-id"] !== front.id || req.headers["cf-access-client-secret"] !== front.secret) {
        s.stopped++;
        res.writeHead(403, { "content-type": "text/html" });
        return res.end("<html>Forbidden</html>");
      }
      s.fronted++;
    }
    s.served++;
    if (req.headers.authorization !== `Bearer ${secret}`) {
      s.refused++;
      return json(res, 401, { code: "unauthorized", message: "no" });
    }
    if (req.headers["clax-workspace-wire"] !== "1") {
      return json(res, 426, { code: "wire_skew", message: "another version", wire: 1 });
    }
    if (req.headers["clax-workspace-instance"] !== instance) {
      s.refused++;
      return json(res, 403, { code: "wrong_instance", message: "another instance" });
    }
    const url = new URL(req.url, "http://service");
    const raw = await readBody(req);
    const body = raw ? JSON.parse(raw) : null;
    const jobKey = url.pathname.match(/^\/v1\/jobs\/([^/]+)$/)?.[1];
    const runId = url.pathname.match(/^\/v1\/runs\/([^/]+)$/)?.[1];
    if (req.method === "POST" && url.pathname === "/v1/jobs") {
      for (const id of [body.run.id, ...body.run.thread_ids]) if (!isId(id)) s.badIds.push(id);
      if (!isKey(body.key)) s.badIds.push(body.key);
      s.posts.push(body);
      let job = s.jobs.get(body.key);
      if (!job) {
        job = { key: body.key, kind: body.kind, command: body.command, run: body.run.id, done: null, waiters: [] };
        s.jobs.set(body.key, job);
        s.runs[body.run.id] = (body.queue && s.queues[body.queue]) || s.others;
        if (body.kind === "propose") job.done = s.proposal(++s.proposed);
        else if (!body.command.startsWith("hold")) job.done = { exit_code: 0, output: `ran ${body.command}` };
      }
      return json(res, 200, answer(job));
    }
    if (req.method === "GET" && jobKey) {
      s.polls++;
      const job = s.jobs.get(decodeURIComponent(jobKey));
      if (!job) return json(res, 404, { code: "no_such_job", message: "no such job" });
      if (!job.done) {
        const wait = Number(url.searchParams.get("wait") ?? 0) * 1000;
        await new Promise((woken) => {
          const timer = setTimeout(woken, wait);
          job.waiters.push(() => { clearTimeout(timer); woken(); });
          res.once("close", () => { clearTimeout(timer); woken(); });
        });
        if (res.destroyed || res.writableEnded) {
          s.abandoned++;
          return;
        }
      }
      return json(res, 200, answer(job));
    }
    if (req.method === "DELETE" && runId) {
      const id = decodeURIComponent(runId);
      s.ended.push(id);
      delete s.runs[id];
      return json(res, 200, snapshot());
    }
    if (req.method === "POST" && url.pathname === "/v1/reap") {
      s.reaps.push(body.active);
      for (const id of Object.keys(s.runs)) if (!body.active.includes(id)) delete s.runs[id];
      return json(res, 200, snapshot());
    }
    if (req.method === "GET" && url.pathname === "/v1/previews") {
      s.previewPolls++;
      return json(res, 200, { instance, events: s.events, snapshot: snapshot() });
    }
    if (req.method === "POST" && url.pathname === "/v1/previews/ack") {
      s.acked.push(...body.ids);
      s.events = s.events.filter((e) => !body.ids.includes(e.id));
      res.writeHead(204).end();
      return;
    }
    json(res, 404, { code: "no_route", message: `${req.method} ${url.pathname}` });
  });
  const base = await listening(server);
  return Object.assign(s, {
    base,
    CHANGE,
    SHELL,
    /** The job of `command` answers. */
    release(command, output = `ran ${command}`) {
      for (const job of s.jobs.values()) {
        if (job.command !== command) continue;
        job.done = { exit_code: 0, output };
        for (const w of job.waiters.splice(0)) w();
      }
    },
    /** Tells of `run`'s preview: `{verify_url, reviewed}`, or nothing for a miss. */
    tell(run, ready) {
      const id = `p${s.nextEvent++}`;
      s.events.push(ready ? { id, run, told: "ready", ...ready } : { id, run, told: "missed" });
      return id;
    },
    close: () => closing(server),
  });
}

/** A Zulip site: `POST /api/v1/messages`, each kept as `{authorization, form}`. */
export async function fakeZulip() {
  const got = [];
  const server = createServer(async (req, res) => {
    const raw = await readBody(req);
    if (req.method !== "POST" || req.url !== "/api/v1/messages") {
      res.writeHead(404).end();
      return;
    }
    got.push({ authorization: req.headers.authorization, form: Object.fromEntries(new URLSearchParams(raw)) });
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ result: "success", msg: "", id: got.length }));
  });
  const base = await listening(server);
  return { base, got, close: () => closing(server) };
}
