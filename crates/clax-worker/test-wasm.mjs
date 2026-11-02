// Checks the Worker's wasm build end to end: builds it (./build.sh), starts
// `wrangler dev --local` on free ports with a fresh `--persist-to` under
// target/clax-worker/wasm-test/, writes notes and blobs through the routes, checks that
// a rollback, a refused journal mode and a transaction past the VFS's cap
// leave the database as it was, then restarts wrangler on the same state
// and checks that a fresh isolate sees the same rows, blob digest and page
// count, and `PRAGMA integrity_check` says ok. The spike's routes are test
// routes (`CLAX_TEST_ROUTES` = `1`); a last start with the checked-in
// `wrangler.jsonc`'s variables checks that they are absent without it, as
// are the instance's test routes. Each wrangler is stopped by
// the process group it was started in. Prints one line per check and a JSON
// summary; exits non-zero on the first failure.
//
// `node test-wasm.mjs --no-build` skips the build.

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { HERE, STATE, build, pruneRuns, start, writeConfig } from "./harness.mjs";

const ROOT = join(STATE, "wasm-test");
const KEEP_RUNS = 3;

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
  await build();
  const built = Date.now();
  pruneRuns(ROOT, KEEP_RUNS);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const config = writeConfig(dir, { CLAX_TEST_ROUTES: "1" });
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

    // The checked-in configuration: no test routes.
    const plainDir = join(dir, "plain");
    mkdirSync(plainDir, { recursive: true });
    w = await start(dir, writeConfig(plainDir), 3);
    running.add(w);
    for (const [method, path] of [["GET", "/stats"], ["POST", "/notes?body=five"], ["POST", "/init"]]) {
      const r = await call(w.base, method, path);
      // Clax's own router answers: a request with no credential is refused
      // before any route (401 no_credential), and one with a credential
      // finds no such route.
      const refused = r.status === 401 && r.body?.error?.code === "no_credential";
      check(refused || r.status === 404 || r.status === 405, `without test routes ${method} /do/wasm-test${path} is not the spike's`, r);
    }
    for (const [method, path] of [["GET", "/_worker/alarm"], ["POST", "/_worker/hibernate"], ["GET", "/_worker/sandbox"]]) {
      const r = await fetch(`${w.base}${path}`, { method, headers: { authorization: "Bearer any" } });
      // Clax's own router answers: the made-up token is refused before any
      // route (401), as a real one finds no such route (404 or 405).
      check(r.status === 401 || r.status === 404 || r.status === 405, `without test routes ${method} ${path} is absent`, r.status);
    }
    await w.stop();
    running.delete(w);
    w = await start(dir, config, 4);
    running.add(w);
    const kept = await ok(w.base, "GET", "/stats?check=1");
    check(kept.notes === 4 && kept.integrity_check === "ok", "the plain start wrote nothing to the spike", kept);
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
