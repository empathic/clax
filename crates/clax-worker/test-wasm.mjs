// Checks the Worker's wasm build end to end: builds it (./build.sh), starts
// `wrangler dev --local` on free ports with a fresh `--persist-to` under
// target/wasm-test/, writes notes and blobs through the routes, checks that
// a rollback, a refused journal mode and a transaction past the VFS's cap
// leave the database as it was, then restarts wrangler on the same state
// and checks that a fresh isolate sees the same rows, blob digest and page
// count, and `PRAGMA integrity_check` says ok. Each wrangler is stopped by
// the process group it was started in. Prints one line per check and a JSON
// summary; exits non-zero on the first failure.
//
// `node test-wasm.mjs --no-build` skips the build.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, openSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "target/wasm-test");
const KEEP_RUNS = 3;
const START_LIMIT_MS = 60_000;
const STOP_LIMIT_MS = 10_000;

function freePort() {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a !== null ? ok(a.port) : fail(new Error("no port"))));
    });
  });
}

function exited(child) {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok();
    else child.once("exit", () => ok());
  });
}

const timeout = (ms) => new Promise((r) => setTimeout(() => r("timeout"), ms));

/** Removes all but the newest KEEP_RUNS `run-<ms>-<pid>` directories. */
function pruneRuns() {
  if (!existsSync(ROOT)) return;
  const runs = readdirSync(ROOT)
    .filter((n) => /^run-\d+-\d+$/.test(n))
    .sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]));
  for (const n of runs.slice(KEEP_RUNS - 1)) rmSync(join(ROOT, n), { recursive: true, force: true });
}

/** wrangler.jsonc without its custom build (this script builds once) and with an absolute `main`. */
function writeConfig(dir) {
  const text = readFileSync(join(HERE, "wrangler.jsonc"), "utf8")
    .split("\n")
    .filter((l) => !/^\s*\/\//.test(l))
    .join("\n");
  const config = JSON.parse(text);
  delete config.$schema;
  delete config.build;
  config.main = join(HERE, config.main);
  const path = join(dir, "wrangler.json");
  writeFileSync(path, JSON.stringify(config, null, 2));
  return path;
}

/** Starts one wrangler dev on `state`; resolves once it answers HTTP. */
async function start(dir, config, n) {
  const [port, inspector] = [await freePort(), await freePort()];
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
  return { base, stop };
}

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

/** One route call: the status and the parsed JSON body (or text). */
async function call(base, method, path) {
  const res = await fetch(`${base}/do/wasm-test${path}`, { method });
  const text = await res.text();
  let body = text;
  try {
    body = JSON.parse(text);
  } catch {
    // an error's text
  }
  return { status: res.status, body };
}

async function ok(base, method, path) {
  const r = await call(base, method, path);
  if (r.status !== 200) throw new Error(`FAIL ${method} ${path}: ${r.status} ${JSON.stringify(r.body)}`);
  return r.body;
}

async function main() {
  const t0 = Date.now();
  if (!process.argv.includes("--no-build")) {
    const build = spawn(join(HERE, "build.sh"), [], { cwd: HERE, stdio: ["ignore", "inherit", "inherit"] });
    await exited(build);
    if (build.exitCode !== 0) throw new Error(`./build.sh failed (${build.exitCode ?? build.signalCode})`);
  }
  const built = Date.now();
  pruneRuns();
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const config = writeConfig(dir);
  const running = new Set();
  const cleanup = () => Promise.all([...running].map((w) => w.stop()));
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void cleanup().then(() => process.exit(130)));
  try {
    let w = await start(dir, config, 1);
    running.add(w);
    await ok(w.base, "POST", "/init");
    for (const body of ["one", "two", "three"]) await ok(w.base, "POST", `/notes?body=${body}`);
    const abort = await ok(w.base, "POST", "/abort?body=never");
    check(abort.count_inside === 4 && abort.count_after === 3, "a rolled-back note is gone", abort);
    const fill = await ok(w.base, "POST", "/fill?mb=2");
    check(fill.store_commits === 1, "2 MiB of blobs in one store commit", fill);
    const wal = await call(w.base, "POST", "/journal?mode=WAL");
    check(wal.status === 500 && String(wal.body).includes("not authorized"), "journal_mode=WAL is refused", wal);
    const memory = await ok(w.base, "POST", "/journal?mode=memory");
    check(memory.journal_mode === "memory", "journal_mode=MEMORY is kept", memory);
    const big = await call(w.base, "POST", "/fill?mb=17");
    check(big.status === 500 && /full/i.test(String(big.body)), "a 17 MiB transaction fails at the 16 MiB cap", big);
    const before = await ok(w.base, "GET", "/stats?check=1");
    check(
      before.integrity_check === "ok" && before.notes === 3 && before.blobs === 32 && before.journal_mode === "memory",
      "the database holds the notes and the 2 MiB, and is intact",
      before,
    );
    await w.stop();
    running.delete(w);

    w = await start(dir, config, 2);
    running.add(w);
    const after = await ok(w.base, "GET", "/stats?check=1");
    check(after.construction === 1, "the restart runs a fresh isolate", after);
    check(after.integrity_check === "ok", "integrity_check after the restart", after);
    const same = ["notes", "blobs", "page_count", "page_size", "blob_bytes", "blob_fnv1a", "store_chunk_rows"];
    const diff = same.filter((k) => after[k] !== before[k]);
    check(diff.length === 0, `the restart sees the same ${same.join(", ")}`, { diff, before, after });
    const note = await ok(w.base, "POST", "/notes?body=four");
    check(note.count === 4, "writes go on after the restart", note);
    await w.stop();
    running.delete(w);
    const summary = {
      ok: true,
      build_s: (built - t0) / 1000,
      run_s: (Date.now() - built) / 1000,
      page_size: after.page_size,
      page_count: after.page_count,
      store_chunk_rows: after.store_chunk_rows,
      blob_fnv1a: after.blob_fnv1a,
      dir,
    };
    console.log(JSON.stringify(summary));
  } finally {
    await cleanup();
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
