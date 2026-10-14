// The Worker (crates/clax-worker) as a hosted Clax under `wrangler dev
// --local`, for the demo runner (demo/local.ts) and the end-to-end test
// (web/e2e/hosted-demo-worker.spec.ts): a wrangler.json for a run
// directory with hosted vars, one `wrangler dev` started on it in a process
// group of its own, and a repository token from the stand-in's dev routes.
//
// The Worker must be built first (`scripts/build-web.sh`, then
// `crates/clax-worker/build.sh`, which `buildWorker` runs).

import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
/** The Worker's crate, where wrangler and its config live. */
export const WORKER_DIR = join(ROOT, "crates/clax-worker");

export function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a !== null ? ok(a.port) : fail(new Error("no port"))));
    });
  });
}

function exited(child: ChildProcess): Promise<void> {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok();
    else child.once("exit", () => ok());
  });
}

const wait = (ms: number) => new Promise<"timeout">((r) => setTimeout(() => r("timeout"), ms));

/** Builds the Worker (`build.sh`: worker-build to build/). The web UI is built separately, first. */
export async function buildWorker(stdio: "inherit" | "ignore" = "inherit"): Promise<void> {
  const b = spawn(join(WORKER_DIR, "build.sh"), [], { cwd: WORKER_DIR, stdio: ["ignore", stdio, "inherit"] });
  await exited(b);
  if (b.exitCode !== 0) throw new Error(`crates/clax-worker/build.sh failed (${b.exitCode ?? b.signalCode})`);
}

export interface WorkerOptions {
  /** The run directory: the config, the log and `state/` (wrangler's --persist-to) go here. */
  dir: string;
  port: number;
  /** The setup secret `/api/hosted/setup` takes once. */
  secret: string;
  /** Live people seats per artifact. */
  seats: string;
  /** Extra vars, over the hosted ones. */
  vars?: Record<string, string>;
  /** Called with wrangler's process as soon as it is spawned, before it is ready. */
  onSpawn?: (child: ChildProcess) => void;
}

export interface RunningWorker {
  /** `http://localhost:<port>`, the public origin. */
  base: string;
  /** `http://127.0.0.1:<port>`, the content origin. */
  content: string;
  log: string;
  child: ChildProcess;
  /** Stops wrangler and its workerd by their process group. */
  stop(): Promise<void>;
}

/** The vars a hosted demo Worker runs with: the public origin
 * `http://localhost:<port>`, artifact content on `http://127.0.0.1:<port>`,
 * the setup secret, `seats` live people seats, versions in the RepoDO
 * stand-in served over git smart HTTP, and its dev routes (the demo's
 * repository tokens, `repoToken`). */
export function demoVars(port: number, secret: string, seats: string): Record<string, string> {
  return {
    HOSTED: "1",
    PUBLIC_ORIGIN: `http://localhost:${port}`,
    CONTENT_ORIGIN: `http://127.0.0.1:${port}`,
    SETUP_SECRET: secret,
    CLAX_LIVE_PEOPLE_SEATS: seats,
    REPO_STORE: "memory",
    GIT_SMART_HTTP: "1",
    REPO_DEV_ROUTES: "1",
  };
}

/** wrangler.jsonc without its custom build (built beforehand), with
 * absolute paths and `vars` set, written to `dir/wrangler.json`. */
function writeConfig(dir: string, vars: Record<string, string>): string {
  const text = readFileSync(join(WORKER_DIR, "wrangler.jsonc"), "utf8")
    .split("\n")
    .filter((l) => !/^\s*\/\//.test(l))
    .join("\n");
  const config = JSON.parse(text) as { $schema?: string; build?: unknown; main: string; assets?: { directory: string }; vars?: Record<string, string> };
  delete config.$schema;
  delete config.build;
  config.main = join(WORKER_DIR, config.main);
  if (config.assets) config.assets.directory = resolve(WORKER_DIR, config.assets.directory);
  config.vars = { ...config.vars, ...vars };
  const path = join(dir, "wrangler.json");
  writeFileSync(path, JSON.stringify(config, null, 2));
  return path;
}

/** Starts `wrangler dev --local` on `o.dir` with the demo's vars; resolves
 * once `/healthz` answers on the public origin. */
export async function startWorker(o: WorkerOptions, readyMs = 90_000): Promise<RunningWorker> {
  mkdirSync(o.dir, { recursive: true });
  const config = writeConfig(o.dir, { ...demoVars(o.port, o.secret, o.seats), ...o.vars });
  const log = join(o.dir, "wrangler.log");
  const out = openSync(log, "a");
  const inspector = await freePort();
  const child = spawn(
    join(WORKER_DIR, "node_modules/.bin/wrangler"),
    [
      "dev", "--local",
      "--config", config,
      "--ip", "127.0.0.1",
      "--port", String(o.port),
      "--inspector-port", String(inspector),
      "--persist-to", join(o.dir, "state"),
      "--show-interactive-dev-session=false",
      "--log-level", "warn",
    ],
    { cwd: o.dir, stdio: ["ignore", out, out], detached: true, env: { ...process.env, WRANGLER_SEND_METRICS: "false" } },
  );
  const pid = child.pid;
  if (pid === undefined) throw new Error("wrangler dev did not start");
  o.onSpawn?.(child);
  let gone = false;
  void exited(child).then(() => (gone = true));
  const stop = async () => {
    if (gone) return;
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      return;
    }
    if ((await Promise.race([exited(child), wait(10_000)])) === "timeout") {
      try {
        process.kill(-pid, "SIGKILL");
      } catch {
        // already gone
      }
      await Promise.race([exited(child), wait(10_000)]);
    }
  };
  const base = `http://localhost:${o.port}`;
  const until = Date.now() + readyMs;
  try {
    for (;;) {
      if (gone) throw new Error(`wrangler dev exited before it was ready; see ${log}:\n${readFileSync(log, "utf8").slice(-2000)}`);
      if (Date.now() > until) throw new Error(`wrangler dev was not ready within ${readyMs / 1000} s; see ${log}`);
      if (await fetch(`${base}/healthz`).then((r) => r.ok, () => false)) break;
      await new Promise((r) => setTimeout(r, 200));
    }
  } catch (e) {
    await stop();
    throw e;
  }
  return { base, content: `http://127.0.0.1:${o.port}`, log, child, stop };
}

/** A repository token the stand-in's ledger hands out (`GitToken`, crates/clax-worker/src/repos/tokens.rs). */
export interface GitToken {
  id: string;
  remote: string;
  token: string;
  expires_at: string;
  clone_command: string;
  jj_clone_command: string;
}

/** A read token for artifact `id`'s repository, held by person `person`
 * (a public ID; none: the owner), from the stand-in's dev route
 * (`POST /_dev/repos/token`, served only with `REPO_DEV_ROUTES=1`). It
 * stands in for the git-token route the server does not serve yet. */
export async function repoToken(base: string, id: string, person?: string): Promise<GitToken> {
  const q = new URLSearchParams({ artifact: id, scope: "read", ...(person === undefined ? {} : { person }) });
  const res = await fetch(`${base}/_dev/repos/token?${q}`, { method: "POST" });
  const text = await res.text();
  if (!res.ok) throw new Error(`POST /_dev/repos/token answered ${res.status}: ${text.slice(0, 200)}`);
  return JSON.parse(text) as GitToken;
}
