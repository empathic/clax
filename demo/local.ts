// Runs the whole local demo from one command (`just hosted-demo`):
//
//   node demo/local.ts [--port <free port>] [--seats 1] [--agents 3] [--pace 8000] [--no-build] [--worker]
//
// Builds the daemon (and the web UI when it is not built), starts a fresh
// native daemon in hosted mode (its home under target/demo/, wiped on every
// run; `[hosted] public_origin = "http://localhost:<port>"`; a random setup
// secret in its environment; CLAX_LIVE_PEOPLE_SEATS, 1 by default, so the
// spectator mode shows with one guest: the owner and the people an artifact
// names hold reserved seats and stay live), seeds it (lib/seed.ts) and prints
// the links and the steps to try. Enter starts the demo agents on the brief
// (agents.ts) at a pace a person can watch; Ctrl-C stops everything. The
// daemon and the agents are stopped by the process groups they were started
// in.
//
// `--worker` is to run the same demo on the Worker (crates/clax-worker under
// `wrangler dev --local`, whose MCP endpoint is `/mcp`); it is not wired yet.

import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { NATIVE_MCP_PATH } from "./lib/http.ts";
import { describe, seed } from "./lib/seed.ts";

const DEMO = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(DEMO, "..");
const DIR = join(ROOT, "target/demo");

const { values: opts } = parseArgs({
  options: {
    port: { type: "string" },
    seats: { type: "string", default: "1" },
    agents: { type: "string", default: "3" },
    pace: { type: "string", default: "8000" },
    "no-build": { type: "boolean", default: false },
    worker: { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});
if (opts.help === true) {
  console.log("usage: just hosted-demo [--port <free port>] [--seats 1] [--agents 3] [--pace 8000] [--no-build] [--worker]");
  process.exit(0);
}

/** A target the demo runs on: how to start it, and its MCP endpoint's path. */
interface Target {
  name: string;
  mcpPath: string;
  start(port: number, secret: string, seats: string, log: string): Promise<ChildProcess>;
}

/** The native daemon in hosted mode, on a home of its own. */
const native: Target = {
  name: "the native daemon",
  mcpPath: NATIVE_MCP_PATH,
  async start(port, secret, seats, log) {
    if (!opts["no-build"]) {
      console.log("Building the daemon…");
      if ((await exited(spawn("cargo", ["build", "-p", "clax-cli", "--bin", "clax"], { cwd: ROOT, stdio: ["ignore", "ignore", "inherit"] }))) !== 0) await shutdown(1);
      if (!existsSync(join(ROOT, "web/dist/index.html"))) {
        console.log("Building the web UI…");
        if ((await exited(spawn("just", ["web"], { cwd: ROOT, stdio: ["ignore", "ignore", "inherit"] }))) !== 0) await shutdown(1);
      }
    }
    const home = join(DIR, "home");
    mkdirSync(home, { recursive: true });
    writeFileSync(
      join(home, "config.toml"),
      `[sample]\napi_key_env = "CLAX_DEMO_UNSET_KEY"\n\n[hosted]\npublic_origin = "http://localhost:${port}"\nsetup_secret_env = "CLAX_DEMO_SETUP_SECRET"\n`,
    );
    const bin = join(ROOT, "target/debug/clax");
    console.log(`Starting the daemon in hosted mode on :${port} (home ${home}, log ${log})…`);
    return spawn(bin, ["serve", "--foreground", "--bind", "127.0.0.1", "--port", String(port), "--web-dist", join(ROOT, "web/dist")], {
      cwd: ROOT,
      stdio: ["ignore", openSync(log, "a"), openSync(log, "a")],
      detached: true,
      env: { ...process.env, CLAX_HOME: home, CLAX_DEMO_SETUP_SECRET: secret, CLAX_DEMO_UNSET_KEY: "", CLAX_LIVE_PEOPLE_SEATS: seats, CLAX_CODEX_BIN: "", CLAX_NO_OPEN: "1" },
    });
  },
};

/** The Worker (crates/clax-worker) under `wrangler dev --local`: not wired yet. */
const worker: Target = {
  name: "the Worker",
  mcpPath: "/mcp",
  start() {
    // To do: build the UI, run `wrangler dev --local --port <port>` in
    // crates/clax-worker with the setup secret, the public origin and the
    // seats as vars, persisting under target/demo/worker.
    return Promise.reject(new Error("--worker is not wired yet: run the demo on the native daemon (without --worker)."));
  },
};

function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a !== null ? ok(a.port) : fail(new Error("no port"))));
    });
  });
}

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
async function shutdown(code: number): Promise<never> {
  stopping = true;
  await Promise.all(children.map(stopGroup));
  process.exit(code);
}
process.on("SIGINT", () => void shutdown(0));
process.on("SIGTERM", () => void shutdown(0));

const target = opts.worker ? worker : native;
const port = opts.port === undefined ? await freePort() : Number(opts.port);
if (await portTaken(port)) {
  console.error(`Port ${port} is in use. Stop what holds it, or pass --port <free port>.`);
  process.exit(1);
}

rmSync(DIR, { recursive: true, force: true });
mkdirSync(DIR, { recursive: true });
const log = join(DIR, "server.log");
const secret = randomBytes(18).toString("base64url");
let server: ChildProcess;
try {
  server = await target.start(port, secret, opts.seats, log);
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(2);
}
children.push(server);
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
  seeded = await seed({ url: base, secret, agents: Number(opts.agents) });
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  await shutdown(1);
}
const s = seeded!;
const agentsCommand = `node demo/agents.ts --url ${base} --mcp-path ${target.mcpPath} --token ${s.token.token} --id ${s.brief.id} --agents ${s.agents} --pace ${opts.pace}`;
console.log(`
${describe(s)}

Run the demo agents again with:
  ${agentsCommand}

Try it (live seats for people: ${opts.seats}; the owner and the people an artifact names always get one):
  1. Open ${s.owner.name}'s and ${s.editor.name}'s sign-in links in two browser profiles, click
     Sign in, and open the Brief in both: each shows the other here in its
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

Ctrl-C stops everything; the next run starts fresh.`);

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
