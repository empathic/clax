// Runs the whole local demo from one command (`just hosted-demo`):
//
//   node demo/local.ts [--native] [--port <free port>] [--seats 1] [--agents 3] [--pace 8000] [--no-build]
//
// By default the demo runs on the Worker: it installs the Worker's
// packages from their lockfile (scripts/pnpm-install-stamped.sh, skipped
// when node_modules already matches it), builds the web UI (scripts/run.sh
// web: Bazel when it is installed) and the Worker (crates/clax-worker), and
// starts `wrangler dev --local` on a free port with fresh state under
// target/demo-worker/ (wiped on every run) and the hosted vars
// (lib/worker.ts): the public origin
// `http://localhost:<port>`, artifact content on `http://127.0.0.1:<port>`,
// a random setup secret, CLAX_LIVE_PEOPLE_SEATS (1 by default, so the
// spectator mode shows with one guest: the owner and the people an artifact
// names hold reserved seats and stay live), and versions in git in the
// RepoDO stand-in served at /git/; the demo takes Rae's read token from the
// server's git-token route (what the Clone dialog asks) to clone the brief
// with git or jj. `--native` runs the native
// daemon in hosted mode instead (its home under target/demo/, the same
// seats and secret), with no git remote.
//
// Either way it seeds the instance (lib/seed.ts) and prints the links and
// the steps to try. Enter starts the demo agents on the brief (agents.ts,
// over the MCP endpoint: `/mcp` on the Worker, `/mcp/json` on the daemon) at
// a pace a person can watch; Ctrl-C stops everything. The server and the
// agents are stopped by the process groups they were started in.

import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { NATIVE_MCP_PATH } from "./lib/http.ts";
import { BuildLog } from "./lib/build.ts";
import { describe, seed, type Seeded } from "./lib/seed.ts";
import { freePort, gitToken, startWorker } from "./lib/worker.ts";

const DEMO = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(DEMO, "..");

const { values: opts } = parseArgs({
  options: {
    port: { type: "string" },
    seats: { type: "string", default: "1" },
    agents: { type: "string", default: "3" },
    pace: { type: "string", default: "8000" },
    "no-build": { type: "boolean", default: false },
    native: { type: "boolean", default: false },
    worker: { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});
if (opts.help === true) {
  console.log("usage: just hosted-demo [--native] [--port <free port>] [--seats 1] [--agents 3] [--pace 8000] [--no-build]\n  (the Worker under wrangler dev by default; --native for the native daemon)");
  process.exit(0);
}

/** A target the demo runs on: its run directory and log, how to start it,
 * its MCP endpoint's path, and what it adds to the seeded instance's
 * description (the brief's git clone commands on the Worker). */
interface Target {
  name: string;
  dir: string;
  log: string;
  mcpPath: string;
  start(port: number, secret: string, seats: string): Promise<ChildProcess>;
  extra(base: string, s: Seeded): Promise<string>;
}

/** Where the builds' output goes (lib/build.ts), opened in the run directory once it is made. */
let builds: BuildLog;

/** The native daemon in hosted mode, on a home of its own. */
const native: Target = {
  name: "the native daemon",
  dir: join(ROOT, "target/demo"),
  log: join(ROOT, "target/demo/server.log"),
  mcpPath: NATIVE_MCP_PATH,
  async start(port, secret, seats) {
    const log = this.log;
    if (!opts["no-build"] || !existsSync(join(ROOT, "web/dist/index.html"))) await builds.web();
    const bin = await builds.output("the daemon", join(ROOT, "scripts/run.sh"), ["bin"]);
    const home = join(this.dir, "home");
    mkdirSync(home, { recursive: true });
    writeFileSync(
      join(home, "config.toml"),
      `[sample]\napi_key_env = "CLAX_DEMO_UNSET_KEY"\n\n[hosted]\npublic_origin = "http://localhost:${port}"\nsetup_secret_env = "CLAX_DEMO_SETUP_SECRET"\n`,
    );
    console.log(`Starting the daemon in hosted mode on :${port} (home ${home}, log ${log})…`);
    return spawn(bin, ["serve", "--foreground", "--bind", "127.0.0.1", "--port", String(port), "--web-dist", join(ROOT, "web/dist")], {
      cwd: ROOT,
      stdio: ["ignore", openSync(log, "a"), openSync(log, "a")],
      detached: true,
      env: { ...process.env, CLAX_HOME: home, CLAX_DEMO_SETUP_SECRET: secret, CLAX_DEMO_UNSET_KEY: "", CLAX_LIVE_PEOPLE_SEATS: seats, CLAX_CODEX_BIN: "", CLAX_NO_OPEN: "1" },
    });
  },
  extra: () => Promise.resolve(""),
};

/** The Worker (crates/clax-worker) under `wrangler dev --local`, hosted (lib/worker.ts). */
const worker: Target = {
  name: "the Worker",
  dir: join(ROOT, "target/demo-worker"),
  log: join(ROOT, "target/demo-worker/wrangler.log"),
  mcpPath: "/mcp",
  async start(port, secret, seats) {
    await builds.workerPackages();
    if (!opts["no-build"]) {
      await builds.web();
      await builds.worker();
    }
    console.log(`Starting the Worker (wrangler dev --local) in hosted mode on :${port} (state ${this.dir}/state, log ${this.log})…`);
    return (await startWorker({ dir: this.dir, port, secret, seats, onSpawn: (c) => children.push(c) })).child;
  },
  async extra(base, s) {
    const t = await gitToken(s.owner.session, s.brief.id, "read");
    return `

Clone the Brief's repository once the agents have published: one commit
per version, each naming its publisher and agent in Clax-Publisher and
Clax-Agent trailers. The read token is ${s.owner.name}'s, from the brief's
Clone dialog (POST /api/artifacts/${s.brief.id}/git-token), and is good
until ${t.expires_at}; for a push, take a Write token in ${s.owner.name}'s
Clone dialog:
  ${t.clone_command} brief
  git -C brief log --format='%h %s%n    %(trailers:key=Clax-Publisher,key=Clax-Agent,separator=%x2C )'
or with jj:
  ${t.jj_clone_command} brief-jj`;
  },
};

function portTaken(port: number): Promise<boolean> {
  return new Promise((ok) => {
    const s = createServer();
    s.once("error", () => ok(true));
    s.listen(port, "127.0.0.1", () => s.close(() => ok(false)));
  });
}

function exited(child: ChildProcess): Promise<number | null> {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok(child.exitCode);
    else child.once("exit", (code) => ok(code));
  });
}

/** Stops a child started with `detached: true` by its process group. */
async function stopGroup(child: ChildProcess): Promise<void> {
  const pid = child.pid;
  if (pid === undefined || child.exitCode !== null || child.signalCode !== null) return;
  try {
    process.kill(-pid, "SIGTERM");
  } catch {
    return;
  }
  const gone = await Promise.race([exited(child).then(() => true), new Promise<boolean>((r) => setTimeout(() => r(false), 10_000))]);
  if (!gone) {
    try {
      process.kill(-pid, "SIGKILL");
    } catch {
      // already gone
    }
  }
}

const children: ChildProcess[] = [];
let stopping = false;
/** Stops the children newest first (the agents end their sessions before the server goes), then exits. */
async function shutdown(code: number): Promise<never> {
  if (stopping) return new Promise<never>(() => {});
  stopping = true;
  for (const c of [...children].reverse()) await stopGroup(c);
  process.exit(code);
}
process.on("SIGINT", () => void shutdown(0));
process.on("SIGTERM", () => void shutdown(0));

if (opts.native && opts.worker) {
  console.error("Pass --native or --worker, not both.");
  process.exit(2);
}
const target = opts.native ? native : worker;
const port = opts.port === undefined ? await freePort() : Number(opts.port);
if (await portTaken(port)) {
  console.error(`Port ${port} is in use. Stop what holds it, or pass --port <free port>.`);
  process.exit(1);
}

rmSync(target.dir, { recursive: true, force: true });
mkdirSync(target.dir, { recursive: true });
builds = new BuildLog(target.dir);
console.log(`Build output goes to ${builds.path}.`);
const log = target.log;
const secret = randomBytes(18).toString("base64url");
let server: ChildProcess;
try {
  server = await target.start(port, secret, opts.seats);
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(2);
}
if (!children.includes(server)) children.push(server);
void exited(server).then(() => {
  if (stopping) return;
  console.error(`${target.name} exited; see ${log}:\n${readFileSync(log, "utf8").slice(-2000)}`);
  void shutdown(1);
});

const base = `http://localhost:${port}`;
const until = Date.now() + 90_000;
while (!(await fetch(`${base}/healthz`).then((r) => r.ok, () => false))) {
  if (Date.now() > until) {
    console.error(`${target.name} was not ready after 90 s; see ${log}`);
    await shutdown(1);
  }
  await new Promise((r) => setTimeout(r, 200));
}

let seeded;
try {
  seeded = await seed({ url: base, secret, agents: Number(opts.agents), mcpPath: target.mcpPath });
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  await shutdown(1);
}
const s = seeded!;
let extra = "";
try {
  extra = await target.extra(base, s);
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  await shutdown(1);
}
const agentsCommand = `node demo/agents.ts --url ${base} --mcp-path ${target.mcpPath} --token ${s.token.token} --id ${s.brief.id} --agents ${s.agents} --pace ${opts.pace}`;
const links = join(target.dir, "links.txt");
const text = `
Running on ${target.name}.

${describe(s)}${extra}

Run the demo agents again with:
  ${agentsCommand}

Try it (live seats for people: ${opts.seats}; the owner and the people an artifact names always get one):
  1. Open ${s.owner.name}'s and ${s.editor.name}'s sign-in links in two browser profiles, click
     Sign in as ${s.owner.name} (or ${s.editor.name}), and open the Brief in both: each shows the other here in its
     people panel, and a pointer moved or text selected in one is drawn over
     the page in the other, in that person's colour.
  2. Press Enter here to start ${s.agents} demo agent(s) on the Brief: watch their
     chips in the top bar, the bar above the page while they draft (Show draft
     shows one, its changes tinted) and the versions landing, a conflict merged
     each round.
  3. Comment on the Brief and send the thread to one of the agents: it revises
     its section, replies and resolves the thread. ${s.editor.name}'s comment sent to
     ${s.owner.name}'s agent is held until ${s.owner.name} approves ${s.editor.name}.
  4. Spectator mode: in ${s.owner.name}'s window, share the Brief by link for comments
     (People, Share by link). Accept ${s.invite.name}'s invite in a third profile. In a
     private window open the Brief and comment as a guest (it takes the free
     seat); ${s.invite.name} then opens the Brief and gets the read-only view. Close the
     private window, and ${s.invite.name} sees Join live.
  5. Open the Fork in ${s.editor.name}'s window: under its title, "Forked from
     ${s.checklist.title} v2" links back to the version it was copied from.

Ctrl-C stops everything; the next run starts fresh.`;
writeFileSync(links, `${text.trimStart()}\n`);
console.log(`${text}\n\nThe links and steps above are also in ${links}.`);

const rl = createInterface({ input: process.stdin });
let started = false;
rl.on("line", () => {
  if (started) return;
  started = true;
  console.log("Starting the demo agents…");
  const agents = spawn(
    process.execPath,
    [join(DEMO, "agents.ts"), "--url", base, "--mcp-path", target.mcpPath, "--token", s.token.token, "--id", s.brief.id, "--agents", String(s.agents), "--pace", opts.pace],
    { cwd: ROOT, stdio: ["ignore", "inherit", "inherit"], detached: true },
  );
  children.push(agents);
});
