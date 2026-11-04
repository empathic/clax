// What the Worker's checks share: free ports, a wrangler.json for a run
// directory, one `wrangler dev --local` started on it and stopped by the
// process group it was started in (also when the check's process ends
// without stopping it), the hang guard every wait is bounded by, and the
// waits themselves.

import { spawn } from "node:child_process";
import { existsSync, openSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const HERE = dirname(fileURLToPath(import.meta.url));

/** The hang guard of every wait in the Worker's checks: a bound that only
 * ends a check that is stuck, never one a loaded machine is still working
 * through, so no check's result depends on it. */
export const HANG_MS = 120_000;

/** How long a wrangler sent SIGTERM is given to end before SIGKILL. */
const STOP_GRACE_MS = 10_000;

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

/** Resolves "timeout" after `ms`: the bound of a race with a child
 * process's exit or output. Its timer holds no process open (the child
 * does, for as long as the race lasts), so a check that is done ends at
 * once rather than when the bound would have passed. */
// oxlint-disable-next-line clax-timing/timer-delay -- a bound on a real process's exit (a stop's grace, or HANG_MS): it ends a stuck run, and decides no result
export const timeout = (ms) => new Promise((r) => setTimeout(() => r("timeout"), ms).unref());

/** Polls `f` every `every` ms until it answers something truthy, and
 * answers that; fails, naming `what` and the last answer or error, only
 * once the hang guard (HANG_MS) has passed. */
export async function until(what, f, every = 100) {
  const end = Date.now() + HANG_MS;
  let last;
  for (;;) {
    try {
      last = await f();
      if (last) return last;
    } catch (e) {
      last = e instanceof Error ? e.message : e;
    }
    if (Date.now() > end) throw new Error(`FAIL ${what} (stuck past the ${HANG_MS / 1000} s hang guard): ${JSON.stringify(last)}`);
    // oxlint-disable-next-line clax-timing/timer-delay -- until's pause between polls: the condition decides, not the delay
    await new Promise((r) => setTimeout(r, every));
  }
}

/** The frame of Playwright page `page` whose URL matches `url`: one there
 * now, or the first to navigate there, heard as its `framenavigated` event
 * (no polling). It fails, naming `url`, when the page closes first or the
 * hang guard (HANG_MS) passes. */
export function frameAt(page, url) {
  return new Promise((resolve, reject) => {
    const navigated = (f) => {
      if (url.test(f.url())) {
        done();
        resolve(f);
      }
    };
    const closed = () => {
      done();
      reject(new Error(`the page closed before a frame showed ${url}`));
    };
    const stuck = setTimeout(() => {
      done();
      reject(new Error(`no frame showed ${url} within the ${HANG_MS / 1000} s hang guard`));
    }, HANG_MS);
    const done = () => {
      clearTimeout(stuck);
      page.off("framenavigated", navigated);
      page.off("close", closed);
    };
    page.on("framenavigated", navigated);
    page.on("close", closed);
    const now = page.frame({ url });
    if (now) navigated(now);
    else if (page.isClosed()) closed();
  });
}

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
 * absolute paths, and with `vars` added, written to `dir` as `name`. */
export function writeConfig(dir, vars = {}, name = "wrangler.json") {
  const config = baseConfig();
  delete config.$schema;
  delete config.build;
  config.main = join(BUILD, "index.js");
  if (config.assets) config.assets.directory = resolve(HERE, config.assets.directory);
  config.vars = { ...config.vars, ...vars };
  const path = join(dir, name);
  writeFileSync(path, JSON.stringify(config, null, 2));
  return path;
}

/** Ends process group `pid` (a wrangler and its workerd) once its standard
 * input closes, as it does when the process holding the pipe's other end
 * ends, however that ends: SIGTERM, then SIGKILL past `grace` ms. It runs
 * as `node -e GUARD <pid> <grace>`. */
const GUARD = `
const [pid, grace] = process.argv.slice(1).map(Number);
const alive = () => { try { process.kill(-pid, 0); return true; } catch { return false; } };
process.stdin.on("data", () => {});
process.stdin.on("end", () => {
  if (!alive()) process.exit(0);
  try { process.kill(-pid, "SIGTERM"); } catch {}
  const end = Date.now() + grace;
  setInterval(() => {
    if (!alive()) process.exit(0);
    if (Date.now() > end) { try { process.kill(-pid, "SIGKILL"); } catch {} process.exit(0); }
  }, 100);
});
`;

/** Starts one wrangler dev on `dir`'s state with `config`, on `port` when
 * given; resolves once it answers HTTP, with its base URL, its port,
 * `stop` and its log: `wrangler-<n>.log` in the test runner's undeclared
 * outputs (TEST_UNDECLARED_OUTPUTS_DIR, so a failed Bazel run keeps it),
 * else in `dir`, at CLAX_WORKER_LOG_LEVEL (default `warn`; `debug` adds
 * workerd's verbose output). wrangler runs in a process group of its own,
 * which `stop` ends by wrangler's PID; should this process end without
 * calling `stop` (it is killed, or exits past every `finally`), a guard
 * (GUARD) ends that group, so no wrangler or workerd outlives the check
 * that started it.
 *
 * wrangler runs without its local observability (X_LOCAL_OBSERVABILITY),
 * which attaches a trace collector to the Worker as a streaming tail: the
 * collector fails its own batches ("Cannot perform I/O on behalf of a
 * different request"), and while it is on, now and then a request
 * answers 500 "Network connection lost" from miniflare's entry worker
 * whatever the Worker did, more often the busier the machine. No check
 * reads those traces. */
export async function start(dir, config, n, port) {
  port ??= await freePort();
  const inspector = await freePort();
  const log = join(process.env.TEST_UNDECLARED_OUTPUTS_DIR || dir, `wrangler-${n}.log`);
  const out = openSync(log, "a");
  // wrangler's own entry, run by this Node: pnpm's node_modules/.bin has
  // it, Bazel's linked packages (crates/clax-worker/BUILD.bazel) have no .bin.
  const child = spawn(
    process.execPath,
    [
      join(HERE, "node_modules/wrangler/bin/wrangler.js"),
      "dev",
      "--local",
      "--config", config,
      "--ip", "127.0.0.1",
      "--port", String(port),
      "--inspector-port", String(inspector),
      "--persist-to", join(dir, "state"),
      "--show-interactive-dev-session=false",
      "--log-level", process.env.CLAX_WORKER_LOG_LEVEL || "warn",
    ],
    { cwd: dir, stdio: ["ignore", out, out], detached: true, env: { ...process.env, WRANGLER_SEND_METRICS: "false", X_LOCAL_OBSERVABILITY: "false" } },
  );
  if (child.pid === undefined) throw new Error("wrangler dev did not start");
  const pid = child.pid;
  // In a process group of its own too, so whatever ends this process's
  // group (a test runner's timeout) leaves the guard to end wrangler's.
  const guard = spawn(process.execPath, ["-e", GUARD, String(pid), String(STOP_GRACE_MS)], {
    stdio: ["pipe", "ignore", "ignore"],
    detached: true,
  });
  guard.unref();
  guard.stdin.unref();
  let gone = false;
  void exited(child).then(() => (gone = true));
  const stop = async () => {
    guard.kill();
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      return;
    }
    if ((await Promise.race([exited(child), timeout(STOP_GRACE_MS)])) === "timeout") {
      try {
        process.kill(-pid, "SIGKILL");
      } catch {
        // already gone
      }
      await Promise.race([exited(child), timeout(STOP_GRACE_MS)]);
    }
  };
  const base = `http://127.0.0.1:${port}`;
  const end = Date.now() + HANG_MS;
  try {
    for (;;) {
      if (gone) throw new Error(`wrangler dev exited before it was ready; see ${log}:\n${readFileSync(log, "utf8").slice(-2000)}`);
      if (Date.now() > end) throw new Error(`wrangler dev did not answer within the ${HANG_MS / 1000} s hang guard; see ${log}`);
      if (await fetch(`${base}/`).then(() => true, () => false)) break;
      // oxlint-disable-next-line clax-timing/timer-delay -- the pause between readiness checks of wrangler dev, a real process: its answer decides, not the delay
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

/** What build.sh records of the build in BUILD when given
 * CLAX_BUILD_COMMIT, beside it: the commit and index_bg.wasm's sha256
 * (deploy-lib.mjs builtFromRefusal). */
export const BUILT_FROM = join(dirname(BUILD), "built-from.json");

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
