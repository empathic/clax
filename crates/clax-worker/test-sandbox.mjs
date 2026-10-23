// Runs ClaxBot's sandbox conformance suite (clax_bot_sandbox::conformance)
// against the Worker Loader sandbox (src/sandbox/) under `wrangler dev
// --local`: builds the Worker (./build.sh), starts it on free ports with a
// fresh `--persist-to` under target/sandbox-test/ and the test routes on,
// lists the cases (GET /_worker/sandbox on the instance) and runs each
// (POST /_worker/sandbox?case=…), one at a time. A case of mode `run` must
// pass as on every sandbox; one of mode `differs` must end with the error
// kind src/sandbox/mod.rs documents in PLATFORM_DIFFERENCES; one of mode
// `platform` needs a CPU or memory limit only Cloudflare enforces
// (PLATFORM_ONLY) and is listed, not run: under workerd it would hold the
// runtime. A listener of its own on 127.0.0.1 (a free port, passed to
// every case as `port`) is what the network probe tries to reach; the run
// fails if anything reaches it. Prints one line per case and a JSON
// summary; exits non-zero if any case failed. wrangler is stopped by the
// process group it was started in, the listener closed with the run.
//
// `node test-sandbox.mjs --no-build` skips the build; `--case <name>` runs
// one case.

import { mkdirSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";
import { HERE, build, pruneRuns, start, writeConfig } from "./harness.mjs";

const ROOT = join(HERE, "target/sandbox-test");
const KEEP_RUNS = 3;
const TOKEN = "sandbox-test-token-0123456789abcdef0123456789";
// One case's request: well over any case's own limits (the slowest runs
// for about two seconds), so a case that holds the runtime fails.
const CASE_LIMIT_MS = 60_000;

/** A listener on 127.0.0.1 on a free port, for the network probe to
 * try: it notes every connection, request and upgrade it sees. */
function listen() {
  const reached = [];
  const server = createServer((req, res) => {
    reached.push(`${req.method} ${req.url}`);
    res.end("reached");
  });
  server.on("connection", (s) => reached.push(`connection from ${s.remoteAddress}`));
  server.on("upgrade", (req, socket) => {
    reached.push(`upgrade ${req.url}`);
    socket.destroy();
  });
  return new Promise((ok, fail) => {
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      ok({
        port: typeof address === "object" && address !== null ? address.port : 0,
        reached,
        close: () => new Promise((done) => server.close(() => done())),
      });
    });
  });
}

async function main() {
  await build();
  pruneRuns(ROOT, KEEP_RUNS);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const config = writeConfig(dir, { CLAX_TOKEN: TOKEN, CLAX_TEST_ROUTES: "1" });
  const only = process.argv.includes("--case") ? process.argv[process.argv.indexOf("--case") + 1] : undefined;
  let w;
  const listener = await listen();
  const stop = () => (w ? w.stop() : Promise.resolve());
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void stop().then(() => process.exit(130)));
  const headers = { authorization: `Bearer ${TOKEN}` };
  const summary = { ok: true, run: 0, passed: 0, differs: 0, platform: [], failed: [] };
  try {
    w = await start(dir, config, 1);
    const list = await fetch(`${w.base}/_worker/sandbox`, { headers });
    if (list.status !== 200) throw new Error(`GET /_worker/sandbox: ${list.status} ${await list.text()}`);
    const cases = (await list.json()).filter((c) => only === undefined || c.name === only);
    if (cases.length === 0) throw new Error(`no case ${only}`);
    for (const c of cases) {
      if (c.mode === "platform") {
        summary.platform.push(c.name);
        console.log(`skip ${c.name} (Cloudflare only: ${c.why})`);
        continue;
      }
      summary.run++;
      const t = Date.now();
      let r;
      try {
        const res = await fetch(`${w.base}/_worker/sandbox?case=${c.name}&port=${listener.port}`, {
          method: "POST",
          headers,
          signal: AbortSignal.timeout(CASE_LIMIT_MS),
        });
        const text = await res.text();
        try {
          r = JSON.parse(text);
        } catch {
          r = { ok: false, problems: `${res.status} ${text.slice(0, 500)}` };
        }
      } catch (e) {
        r = { ok: false, problems: String(e) };
      }
      const ms = Date.now() - t;
      if (r.ok) {
        if (c.mode === "differs") summary.differs++;
        else summary.passed++;
        console.log(`ok   ${c.name}${c.mode === "differs" ? ` (documented difference: ${c.why})` : ""} [${ms} ms]`);
      } else {
        summary.ok = false;
        summary.failed.push(c.name);
        console.log(`FAIL ${c.name}: ${r.problems}${r.outcome ? ` ${JSON.stringify(r.outcome).slice(0, 800)}` : ""}`);
      }
    }
    if (listener.reached.length > 0) {
      summary.ok = false;
      summary.failed.push("the local listener");
      console.log(`FAIL the local listener was reached: ${listener.reached.join("; ")}`);
    } else {
      console.log("ok   no script reached the local listener");
    }
  } finally {
    await stop();
    await listener.close();
  }
  console.log(JSON.stringify({ ...summary, dir }));
  if (!summary.ok) process.exit(1);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
