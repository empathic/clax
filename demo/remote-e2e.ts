// An agent of a local Clax connects to clax.dev, reads a private page there
// and is told of a comment on it, end to end on this machine
// (`just remote-e2e`):
//
//   node demo/remote-e2e.ts [--no-build] [--keep]
//
// It starts two servers on free ports: the Worker (crates/clax-worker)
// under `wrangler dev --local` with clax-dev-local's settings
// (demo/clax-dev-local.ts: hosted, a CLAX_KEK, the bundled Terms; GitHub
// sign-in is configured but not used), its owner made through setup; and
// the debug `clax` daemon on a scratch home (CLAX_HOME under
// target/remote-e2e/). Each step prints one line:
//
//  1. The owner of the Worker publishes a private page there.
//  2. An agent of the local Clax (`clax mcp --agent claude`, its session
//     registered by Claude Code's SessionStart hook) reads its URL: the
//     answer is remote_sign_in_required, naming connect_instance.
//  3. The agent calls connect_instance. The local owner answers its card
//     with Connect from a browser session of theirs (made as `clax open`
//     makes one), the decision an agent's liveness asks a browser to
//     confirm (held for that session's approval when passkeys are
//     required); the daemon token alone is refused. The daemon starts the
//     Worker's device sign-in, and the Worker's owner, signed in there,
//     approves the code. connect_instance answers connected.
//  4. The agent reads the private page through the connection.
//  5. The agent watches it; the Worker's owner comments on the page and
//     sends the thread to the agent; the `wake.follow_command` the watch
//     answered, run with --once (`clax feedback follow --once --agent
//     claude --harness-session <ID>`), prints a line for it, and
//     comments_read shows the comment.
//
// It exits 0 once every step passed, and 1 at the first that failed, with
// the end of the logs. Everything it starts is stopped by its PID (the
// Worker by its process group); the run directory
// (target/remote-e2e/run-<time>/: the build log, wrangler's log, the
// daemon's log and home) is removed on success unless --keep is given.
// --no-build uses the web UI, Worker and daemon built before.

import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, openSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { BuildLog } from "./lib/build.ts";
import { setupFirstPerson, type PersonSession } from "./lib/http.ts";
import { devLocalVars, freePort, startWorker, type RunningWorker } from "./lib/worker.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const RUNS = join(ROOT, "target/remote-e2e");

const { values: opts } = parseArgs({
  options: {
    "no-build": { type: "boolean", default: false },
    keep: { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});
if (opts.help === true) {
  console.log("usage: just remote-e2e [--no-build] [--keep]\n  (an agent connects, reads and follows a private page on the dev Worker; runs in target/remote-e2e/)");
  process.exit(0);
}

const PAGE_TEXT = "Only the owner sees this plan.";
const PAGE = `<!doctype html><title>Private plan</title><main><h1>Private plan</h1><p>${PAGE_TEXT}</p></main>`;
const COMMENT = "Add the launch date to the plan.";
const HARNESS_SESSION = `remote-e2e-${randomBytes(4).toString("hex")}`;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Polls `get` every 100 ms until it answers non-null, for at most `ms`. */
async function until<T>(what: string, get: () => Promise<T | null | undefined>, ms = 30_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await get();
    if (v !== null && v !== undefined) return v;
    if (Date.now() > end) throw new Error(`${what}: not within ${ms / 1000} s`);
    await sleep(100);
  }
}

function exited(child: ChildProcess): Promise<void> {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok();
    else child.once("exit", () => ok());
  });
}

/** Stops `child` by its PID: SIGTERM, then SIGKILL after 10 s. */
async function stopChild(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  if (await Promise.race([exited(child).then(() => true), sleep(10_000).then(() => false)])) return;
  child.kill("SIGKILL");
  await Promise.race([exited(child), sleep(10_000)]);
}

/** This process's environment without what names an agent, a harness or a
 * Clax home, so the children run outside the session this runs in. */
function cleanEnv(extra: Record<string, string>): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (/^(CLAX_|CLAUDE|CODEX_|GROK_|PI_)/.test(k)) continue;
    env[k] = v;
  }
  return { ...env, ...extra };
}

/** Every server and CLI child this run starts, registered as it is
 * spawned so that any exit, a signal's included, stops it by its PID;
 * `group` for one started in a process group of its own (wrangler),
 * stopped by that group. */
const spawned: { child: ChildProcess; group: boolean }[] = [];

/** Stops a child started detached, by its process group: SIGTERM, then SIGKILL after 10 s. */
async function stopGroup(child: ChildProcess): Promise<void> {
  const pid = child.pid;
  if (pid === undefined || child.exitCode !== null || child.signalCode !== null) return;
  const signal = (sig: NodeJS.Signals) => {
    try {
      process.kill(-pid, sig);
    } catch {
      // already gone
    }
  };
  signal("SIGTERM");
  if (await Promise.race([exited(child).then(() => true), sleep(10_000).then(() => false)])) return;
  signal("SIGKILL");
  await Promise.race([exited(child), sleep(10_000)]);
}

let step = 0;
function say(line: string): void {
  step += 1;
  console.log(`${step}. ${line}`);
}

// ── The local daemon ────────────────────────────────────────────────────────

interface Daemon {
  base: string;
  token: string;
  child: ChildProcess;
  log: string;
}

/** `clax serve` on the scratch home `home`, port `port`; resolves once
 * `daemon.json` names it and `/healthz` answers. */
async function startDaemon(bin: string, home: string, port: number, log: string): Promise<Daemon> {
  mkdirSync(home, { recursive: true, mode: 0o700 });
  writeFileSync(join(home, "config.toml"), '[sample]\napi_key_env = "CLAX_REMOTE_E2E_UNSET_KEY"\n');
  const out = openSync(log, "a");
  const child = spawn(bin, ["serve", "--foreground", "--bind", "127.0.0.1", "--port", String(port)], {
    cwd: ROOT,
    stdio: ["ignore", out, out],
    env: cleanEnv({ CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_NO_OPEN: "1", CLAX_GALLERY__QUICKSTART: "false" }),
  });
  spawned.push({ child, group: false });
  try {
    const info = join(home, "daemon.json");
    const { token } = await until(
    "the daemon's daemon.json",
    async () => {
      if (child.exitCode !== null) throw new Error(`the daemon exited (${child.exitCode}); see ${log}`);
      try {
        return JSON.parse(readFileSync(info, "utf8")) as { token: string };
      } catch {
        return null;
      }
    },
      60_000,
    );
    const base = `http://127.0.0.1:${port}`;
    await until("the daemon's /healthz", () => fetch(`${base}/healthz`).then((r) => (r.ok ? true : null), () => null), 60_000);
    return { base, token, child, log };
  } catch (e) {
    await stopChild(child);
    throw e;
  }
}

/** A JSON call to the local daemon; `auth` is its token (bearer) or an
 * owner browser session's cookies, with the daemon's Origin. */
async function local(d: Daemon, method: string, path: string, o: { body?: unknown; cookie?: string; headers?: Record<string, string> } = {}): Promise<{ status: number; headers: Headers; json: Record<string, unknown> }> {
  const headers: Record<string, string> = { accept: "application/json", ...o.headers };
  if (o.cookie !== undefined) {
    headers.cookie = o.cookie;
    headers.origin = d.base;
  } else headers.authorization = `Bearer ${d.token}`;
  if (o.body !== undefined) headers["content-type"] = "application/json";
  const res = await fetch(`${d.base}${path}`, { method, headers, body: o.body === undefined ? undefined : JSON.stringify(o.body) });
  const text = await res.text();
  let json: Record<string, unknown> = {};
  try {
    json = text === "" ? {} : (JSON.parse(text) as Record<string, unknown>);
  } catch {
    json = { text };
  }
  return { status: res.status, headers: res.headers, json };
}

/** A browser session of the owner's on the daemon, as `clax open` makes
 * one: a login code made with the token (`POST /api/browser-login`),
 * redeemed at `GET /login`; the cookies it set, as a `Cookie` header. */
async function ownerBrowser(d: Daemon): Promise<string> {
  const made = await local(d, "POST", "/api/browser-login", { body: { next: "/" } });
  if (made.status !== 200 && made.status !== 201) throw new Error(`browser-login answered ${made.status}: ${JSON.stringify(made.json)}`);
  const res = await fetch(`${d.base}${made.json.path as string}`, { redirect: "manual" });
  const cookies = res.headers.getSetCookie().map((c) => c.split(";")[0]!);
  if (res.status !== 303 || cookies.length === 0) throw new Error(`the login answered ${res.status} with no cookie`);
  return cookies.join("; ");
}

// ── The agent: `clax mcp` over stdio ────────────────────────────────────────

interface ToolResult {
  text: string;
  data: Record<string, unknown>;
  isError: boolean;
}

/** An MCP client of `clax mcp --agent claude` under the harness session
 * HARNESS_SESSION, as Claude Code runs it. */
class Agent {
  private readonly child: ChildProcess;
  private next = 1;
  private readonly waiting = new Map<number, (m: Record<string, unknown>) => void>();
  stderr = "";

  constructor(bin: string, env: NodeJS.ProcessEnv, cwd: string) {
    this.child = spawn(bin, ["mcp", "--agent", "claude"], { cwd, env, stdio: ["pipe", "pipe", "pipe"] });
    spawned.push({ child: this.child, group: false });
    this.child.stderr!.on("data", (b: Buffer) => (this.stderr = (this.stderr + b.toString()).slice(-4000)));
    createInterface({ input: this.child.stdout! }).on("line", (line) => {
      let m: Record<string, unknown>;
      try {
        m = JSON.parse(line) as Record<string, unknown>;
      } catch {
        return;
      }
      if (typeof m.id === "number") {
        this.waiting.get(m.id)?.(m);
        this.waiting.delete(m.id);
      }
    });
  }

  private send(m: Record<string, unknown>): void {
    this.child.stdin!.write(JSON.stringify(m) + "\n");
  }

  private request(method: string, params: Record<string, unknown>, ms: number): Promise<Record<string, unknown>> {
    const id = this.next++;
    return new Promise((ok, fail) => {
      const t = setTimeout(() => fail(new Error(`${method}: no answer within ${ms / 1000} s; clax mcp's stderr: ${this.stderr}`)), ms);
      this.waiting.set(id, (m) => {
        clearTimeout(t);
        ok(m);
      });
      this.send({ jsonrpc: "2.0", id, method, params });
    });
  }

  async connect(): Promise<void> {
    await this.request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "remote-e2e", version: "1.0.0" } }, 30_000);
    this.send({ jsonrpc: "2.0", method: "notifications/initialized" });
  }

  async call(tool: string, args: Record<string, unknown>, ms = 60_000): Promise<ToolResult> {
    const m = await this.request("tools/call", { name: tool, arguments: args }, ms);
    if (m.error !== undefined) throw new Error(`${tool}: ${JSON.stringify(m.error)}`);
    const r = m.result as { content?: { type: string; text?: string }[]; structuredContent?: Record<string, unknown>; isError?: boolean };
    const text = (r.content ?? []).filter((c) => c.type === "text").map((c) => c.text ?? "").join("\n");
    let data = r.structuredContent;
    if (data === undefined) {
      try {
        data = JSON.parse(r.content?.[0]?.text ?? "{}") as Record<string, unknown>;
      } catch {
        data = {};
      }
    }
    return { text, data, isError: r.isError === true };
  }

  async ok(tool: string, args: Record<string, unknown>, ms?: number): Promise<ToolResult> {
    const r = await this.call(tool, args, ms);
    if (r.isError) throw new Error(`${tool} failed: ${r.text}`);
    return r;
  }

  async stop(): Promise<void> {
    this.child.stdin!.end();
    if (await Promise.race([exited(this.child).then(() => true), sleep(5_000).then(() => false)])) return;
    await stopChild(this.child);
  }
}

/** Runs the CLI with `env`; answers its exit status and output, killing it after `ms`. */
function cli(bin: string, args: string[], env: NodeJS.ProcessEnv, cwd: string, input: string | null, ms: number): Promise<{ status: number | null; stdout: string; stderr: string }> {
  return new Promise((ok) => {
    const child = spawn(bin, args, { cwd, env, stdio: [input === null ? "ignore" : "pipe", "pipe", "pipe"] });
    spawned.push({ child, group: false });
    let stdout = "";
    let stderr = "";
    child.stdout!.on("data", (b: Buffer) => (stdout += b.toString()));
    child.stderr!.on("data", (b: Buffer) => (stderr += b.toString()));
    if (input !== null) child.stdin!.end(input);
    const t = setTimeout(() => void stopChild(child), ms);
    child.once("exit", (status) => {
      clearTimeout(t);
      ok({ status, stdout, stderr });
    });
  });
}

/** Removes all but the newest two runs' directories. */
function pruneRuns(): void {
  if (!existsSync(RUNS)) return;
  const runs = readdirSync(RUNS).filter((n) => /^run-\d+$/.test(n)).sort().reverse();
  for (const n of runs.slice(2)) rmSync(join(RUNS, n), { recursive: true, force: true });
}

// ── The run ─────────────────────────────────────────────────────────────────

function expect(cond: unknown, what: string, got?: unknown): void {
  if (!cond) throw new Error(`${what}${got === undefined ? "" : `; got ${typeof got === "string" ? got : JSON.stringify(got)}`}`);
}

pruneRuns();
const dir = join(RUNS, `run-${Date.now()}`);
mkdirSync(dir, { recursive: true });
const builds = new BuildLog(dir);
let worker: RunningWorker | undefined;
let daemon: Daemon | undefined;
let agent: Agent | undefined;
let stopping: Promise<void> | undefined;

/** Stops everything this run started, once however often it is called:
 * the agent first (it ends its session), the daemon after asking it to
 * shut down, then every other registered child by its PID or group. */
function stopAll(): Promise<void> {
  stopping ??= (async () => {
    await agent?.stop().catch(() => undefined);
    if (daemon) await local(daemon, "POST", "/api/admin/shutdown").catch(() => undefined);
    await Promise.all(spawned.map(({ child, group }) => (group ? stopGroup(child) : stopChild(child)).catch(() => undefined)));
  })();
  return stopping;
}

process.on("SIGINT", () => void stopAll().finally(() => process.exit(130)));
process.on("SIGTERM", () => void stopAll().finally(() => process.exit(143)));

try {
  console.log(`Run directory ${dir} (build log ${builds.path}).`);
  await builds.workerPackages();
  if (!opts["no-build"]) {
    await builds.web();
    await builds.worker();
  }
  const bin = await builds.output("the daemon", join(ROOT, "scripts/run.sh"), ["bin"]);
  expect(existsSync(bin), `scripts/run.sh bin printed no binary`, bin);

  // The Worker, with clax-dev-local's settings and an owner made by setup.
  const secret = randomBytes(18).toString("base64url");
  const wport = await freePort();
  worker = await startWorker({
    dir: join(dir, "worker"),
    port: wport,
    secret,
    seats: "",
    onSpawn: (child) => spawned.push({ child, group: true }),
    vars: devLocalVars({ githubClientId: "remote-e2e-unused", githubClientSecret: "remote-e2e-unused", kek: randomBytes(32).toString("hex") }),
  });
  const owner: PersonSession = await setupFirstPerson(worker.base, secret, "Owner");
  const discovery = (await fetch(`${worker.base}/.well-known/clax`).then((r) => r.json())) as { kind: string; device?: unknown };
  expect(discovery.kind === "hosted" && discovery.device !== undefined, "the Worker's /.well-known/clax names a hosted Clax with a device sign-in", discovery);
  console.log(`The Worker is up at ${worker.base} (log ${worker.log}); its owner is made.`);

  // The local daemon on a scratch home.
  const home = join(dir, "home");
  daemon = await startDaemon(bin, home, await freePort(), join(dir, "daemon.log"));
  console.log(`The local daemon is up at ${daemon.base} (home ${home}, log ${daemon.log}).`);

  // 1. A private page on the Worker.
  const made = await owner.call<{ artifact: { id: string } }>("POST", "/api/artifacts", {
    title: "Private plan",
    files: { "index.html": { content: PAGE, encoding: "utf8" } },
  });
  const raid = made.artifact.id;
  const pageUrl = `${worker.base}/a/${raid}`;
  const anon = await fetch(`${worker.base}/api/artifacts/${raid}`, { headers: { accept: "application/json" } });
  expect(anon.status === 401 || anon.status === 403 || anon.status === 404, "the page is private: an anonymous read is refused", anon.status);
  say(`The Worker's owner published a private page, ${pageUrl} (an anonymous read answers ${anon.status}).`);

  // 2. The agent, as Claude Code starts it, reads the URL before any connection.
  const project = join(dir, "work", "remote-e2e");
  mkdirSync(join(project, ".git"), { recursive: true });
  writeFileSync(join(project, ".git", "HEAD"), "ref: refs/heads/main\n");
  const agentEnv = cleanEnv({ CLAX_HOME: home, CLAUDE_CODE_SESSION_ID: HARNESS_SESSION, CLAX_CHANNEL: "agent" });
  const hook = await cli(
    bin,
    ["hook", "--agent", "claude", "session-start"],
    agentEnv,
    project,
    JSON.stringify({ session_id: HARNESS_SESSION, cwd: project, hook_event_name: "SessionStart", source: "startup" }),
    30_000,
  );
  expect(hook.status === 0, "clax hook session-start exits 0", hook.stderr);
  agent = new Agent(bin, agentEnv, project);
  await agent.connect();
  const before = await agent.call("read", { url_or_id: pageUrl });
  expect(before.isError && before.text.includes("connect_instance"), "read before connecting answers sign_in_required, naming connect_instance", before.text);
  say(`The agent's read of the page answers sign_in_required: ${(before.data.error as { code?: string } | undefined)?.code ?? before.text.slice(0, 80)}, naming connect_instance.`);

  // 3. connect_instance: the owner answers here, confirms, and approves there.
  const connecting = agent.call("connect_instance", { url_or_origin: pageUrl, reason: "to read the private plan", timeout_s: 120 }, 150_000);
  // Awaited below; handled now so a rejection while the owner acts is not unhandled.
  connecting.catch(() => undefined);
  const card = await until("the connection request's card in the owner's inbox", async () => {
    const r = await local(daemon!, "GET", "/api/inbox?kind=remote_request&read=all");
    const items = (r.json.items ?? []) as { remote_request?: { id: string; origin: string; status: string } }[];
    return items.find((i) => i.remote_request?.status === "open")?.remote_request ?? null;
  });
  expect(card.origin === worker.base, "the card names the Worker's origin", card);
  const cookie = await ownerBrowser(daemon);
  const answerPath = `/api/remotes/requests/${card.id}/answer`;
  // The answer is a decision the daemon confirms in a browser while an
  // agent is live (docs/contract.md, "Browser confirmations"): an owner
  // browser session is that browser and decides at once, unless passkeys
  // are required, when it answers 202 and the same session confirms.
  let answered = await local(daemon, "POST", answerPath, { cookie, body: { answer: "connect" } });
  let how = "decided at once, the browser session being the confirmation an agent's liveness asks for";
  if (answered.status === 202 && answered.json.status === "confirmation_pending") {
    const confirmation = answered.json.confirmation_id as string;
    const confirmed = await local(daemon, "POST", `/api/confirmations/${confirmation}`, { cookie, body: { approve: true } });
    expect(confirmed.status === 200, "the owner's browser session confirms", { status: confirmed.status, ...confirmed.json });
    answered = await local(daemon, "POST", answerPath, { cookie, body: { answer: "connect" }, headers: { "x-clax-confirmation": confirmation } });
    how = `held for confirmation ${confirmation}, which the same browser session approved`;
  }
  expect(answered.status === 200 && typeof answered.json.verification_url === "string", "Connect starts the Worker's device sign-in", { status: answered.status, ...answered.json });
  // The token alone may not answer at all.
  const byToken = await local(daemon, "POST", answerPath, { body: { answer: "connect" } });
  expect(byToken.status === 403, "the daemon token alone may not answer a connection request", { status: byToken.status, ...byToken.json });
  const userCode = new URL(answered.json.verification_url as string).searchParams.get("code")!;
  say(`The owner answered the card with Connect from a browser session of theirs (${how}; the token alone is refused ${byToken.status}); the Worker's sign-in code is ${userCode}.`);
  const peek = await owner.call<{ client_name: string }>("GET", `/api/hosted/device/${userCode}`);
  await owner.call("POST", `/api/hosted/device/${userCode}/approve`, {});
  const connected = await connecting;
  expect(!connected.isError && connected.data.status === "connected", "connect_instance answers connected", connected.text);
  say(`The Worker's owner approved "${peek.client_name}" there; connect_instance answered connected (remote ${connected.data.remote as string}).`);

  // 4. The private page, read through the connection.
  const read = await agent.ok("read", { url_or_id: pageUrl });
  expect(read.text.includes(PAGE_TEXT), "read answers the private page's content", read.text.slice(0, 400));
  say(`The agent read the private page through the connection: "${PAGE_TEXT}"`);

  // 5. Watch, a comment there, and the follower's line.
  const watched = await agent.ok("watch", { url_or_id: pageUrl });
  const wake = watched.data.wake as { follow_command?: string } | undefined;
  expect(typeof wake?.follow_command === "string", "watch answers a follow command", watched.data);
  const status = await agent.ok("status", {});
  const name = (status.data.session as { name?: string } | undefined)?.name;
  expect(typeof name === "string" && name !== "", "status names the agent's session", status.data);
  await until("the agent's session on the Worker", async () => {
    const r = await owner.call<{ agents?: { name?: string }[] }>("GET", "/api/agents").catch(() => ({ agents: [] }));
    return (r.agents ?? []).some((a) => a.name === name) ? true : null;
  });
  say(`The agent ${name} watches the page; its follow command is: ${wake!.follow_command}`);
  // The follow command, with --once: one line, then exit (as under Claude
  // Code's Bash run_in_background).
  expect(wake!.follow_command!.includes(`feedback follow --agent claude --harness-session '${HARNESS_SESSION}'`), "the follow command follows this harness session", wake!.follow_command);
  const followArgs = ["feedback", "follow", "--once", "--agent", "claude", "--harness-session", HARNESS_SESSION];
  const follow = cli(bin, followArgs, agentEnv, project, null, 90_000);
  const thread = await owner.form<{ thread: { id: string } }>(`/api/artifacts/${raid}/threads`, {
    anchor: JSON.stringify({ kind: "artifact" }),
    body: COMMENT,
    version: "1",
  });
  await owner.call("POST", `/api/artifacts/${raid}/threads/${thread.thread.id}/send`, { to: name });
  say(`The Worker's owner commented "${COMMENT}" on the page and sent the thread to ${name}.`);
  const out = await follow;
  expect(out.status === 0 && out.stdout.trim() !== "", "clax feedback follow --once prints a line and exits 0", out);
  const notice = out.stdout.trim().split("\n")[0]!;
  say(`clax feedback follow --once printed: ${notice}`);
  const comments = await agent.ok("comments_read", { url_or_id: pageUrl });
  expect(comments.text.includes(COMMENT), "comments_read shows the comment", comments.text.slice(0, 400));
  say(`comments_read shows the comment: "${COMMENT}"`);

  await stopAll();
  if (!opts.keep) rmSync(dir, { recursive: true, force: true });
  console.log("Every step passed.");
  process.exit(0);
} catch (e) {
  console.error(`\nFailed at step ${step + 1}: ${e instanceof Error ? e.message : e}`);
  for (const [what, path] of [
    ["the daemon's log", daemon?.log],
    ["wrangler's log", worker?.log],
  ] as const) {
    if (path && existsSync(path)) console.error(`\nThe end of ${what} (${path}):\n${readFileSync(path, "utf8").slice(-3000)}`);
  }
  if (agent?.stderr) console.error(`\nclax mcp's stderr:\n${agent.stderr}`);
  await stopAll();
  console.error(`\nThe run directory is kept: ${dir}`);
  process.exit(1);
}
