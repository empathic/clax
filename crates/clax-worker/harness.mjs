// What the Worker's checks share: free ports, a wrangler.json for a run
// directory, and one `wrangler dev --local` started on it and stopped by
// the process group it was started in.

import { spawn } from "node:child_process";
import { existsSync, openSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const HERE = dirname(fileURLToPath(import.meta.url));
const START_LIMIT_MS = 60_000;
const STOP_LIMIT_MS = 10_000;

export function freePort() {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a !== null ? ok(a.port) : fail(new Error("no port"))));
    });
  });
}

export function exited(child) {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok();
    else child.once("exit", () => ok());
  });
}

export const timeout = (ms) => new Promise((r) => setTimeout(() => r("timeout"), ms));

/** Removes all but the newest `keep` `run-<ms>-<pid>` directories under `root`. */
export function pruneRuns(root, keep) {
  if (!existsSync(root)) return;
  const runs = readdirSync(root)
    .filter((n) => /^run-\d+-\d+$/.test(n))
    .sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]));
  for (const n of runs.slice(keep - 1)) rmSync(join(root, n), { recursive: true, force: true });
}

/** wrangler.jsonc as an object (its comments are whole lines). */
export function baseConfig() {
  const text = readFileSync(join(HERE, "wrangler.jsonc"), "utf8")
    .split("\n")
    .filter((l) => !/^\s*\/\//.test(l))
    .join("\n");
  return JSON.parse(text);
}

/** wrangler.jsonc without its custom build (the caller builds once), with
 * absolute paths, and with `vars` added, written to `dir`. */
export function writeConfig(dir, vars = {}) {
  const config = baseConfig();
  delete config.$schema;
  delete config.build;
  config.main = join(BUILD, "index.js");
  if (config.assets) config.assets.directory = resolve(HERE, config.assets.directory);
  config.vars = { ...config.vars, ...vars };
  const path = join(dir, "wrangler.json");
  writeFileSync(path, JSON.stringify(config, null, 2));
  return path;
}

/** Starts one wrangler dev on `dir`'s state with `config`, on `port` when
 * given; resolves once it answers HTTP, with its base URL, its port and
 * `stop`. */
export async function start(dir, config, n, port) {
  port ??= await freePort();
  const inspector = await freePort();
  const log = join(dir, `wrangler-${n}.log`);
  const out = openSync(log, "a");
  const child = spawn(
    join(HERE, "node_modules/.bin/wrangler"),
    [
      "dev",
      "--local",
      "--config", config,
      "--ip", "127.0.0.1",
      "--port", String(port),
      "--inspector-port", String(inspector),
      "--persist-to", join(dir, "state"),
      "--show-interactive-dev-session=false",
      "--log-level", "warn",
    ],
    { cwd: dir, stdio: ["ignore", out, out], detached: true, env: { ...process.env, WRANGLER_SEND_METRICS: "false" } },
  );
  if (child.pid === undefined) throw new Error("wrangler dev did not start");
  const pid = child.pid;
  let gone = false;
  void exited(child).then(() => (gone = true));
  const stop = async () => {
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      return;
    }
    if ((await Promise.race([exited(child), timeout(STOP_LIMIT_MS)])) === "timeout") {
      try {
        process.kill(-pid, "SIGKILL");
      } catch {
        // already gone
      }
      await Promise.race([exited(child), timeout(STOP_LIMIT_MS)]);
    }
  };
  const base = `http://127.0.0.1:${port}`;
  const until = Date.now() + START_LIMIT_MS;
  try {
    for (;;) {
      if (gone) throw new Error(`wrangler dev exited before it was ready; see ${log}:\n${readFileSync(log, "utf8").slice(-2000)}`);
      if (Date.now() > until) throw new Error(`wrangler dev was not ready within ${START_LIMIT_MS / 1000} s; see ${log}`);
      if (await fetch(`${base}/`).then(() => true, () => false)) break;
      await new Promise((r) => setTimeout(r, 200));
    }
  } catch (e) {
    await stop();
    throw e;
  }
  return { base, port, stop, log };
}

/** Where the tests and e2e runs keep their state: the test runner's
 * TEST_TMPDIR (Bazel's), else target/clax-worker at the repository root,
 * never this source directory. */
export const STATE = process.env.TEST_TMPDIR || join(HERE, "../../target/clax-worker");

/** The built Worker: CLAX_WORKER_BUNDLE when set (a Bazel test's
 * //crates/clax-worker:bundle), else the directory ./build.sh builds it
 * into, wrangler.jsonc's `main`'s: target/clax-worker/build at the
 * repository root. */
export const BUILD = process.env.CLAX_WORKER_BUNDLE
  ? resolve(process.env.CLAX_WORKER_BUNDLE)
  : dirname(join(HERE, baseConfig().main));

/** Whether the built Worker carries web/dist's current index.html. */
export function embedsWebUi() {
  try {
    const wasm = readFileSync(join(BUILD, "index_bg.wasm"));
    return wasm.indexOf(readFileSync(join(HERE, "../../web/dist/index.html"))) >= 0;
  } catch {
    return false;
  }
}

/** Runs ./build.sh unless `--no-build` was given, then fails unless the
 * Worker embeds web/dist's current index.html: a Worker built before the
 * web UI, or from an older one, serves pages the checkout no longer has. */
export async function build() {
  const skip = process.argv.includes("--no-build");
  if (!skip) {
    const b = spawn(join(HERE, "build.sh"), [], { cwd: HERE, stdio: ["ignore", "inherit", "inherit"] });
    await exited(b);
    if (b.exitCode !== 0) throw new Error(`./build.sh failed (${b.exitCode ?? b.signalCode})`);
  }
  if (!embedsWebUi()) {
    const fix = skip ? "build the web UI (scripts/build-web.sh), then rebuild the Worker (drop --no-build)" : "build the web UI (scripts/build-web.sh)";
    throw new Error(`${join(BUILD, "index_bg.wasm")} does not embed web/dist/index.html: ${fix}`);
  }
}
