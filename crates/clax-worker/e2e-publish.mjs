// A local Clax daemon publishing to the Worker as a hosted Clax, under
// `wrangler dev --local`: the local daemon signs in by device code (the
// Worker's owner approves it in a browser session), pushes an artifact of
// three versions, sees them on the Worker with their authorship, pushes a
// new local version, and is refused when the public copy moved, then pushes
// onto a fork of it.
//
// Usage: CLAX_TEST_BIN=<absolute path to clax> node e2e-publish.mjs [--no-build]
// (the web UI built first with scripts/build-web.sh). Run state lives under
// target/e2e-publish/; the local daemon's home is a fresh temporary
// directory, stopped by its PID and removed at the end.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HERE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";

const ROOT = join(HERE, "target/e2e-publish");
const TOKEN = "e2e-owner-token-" + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
const SECRET = "e2e setup secret " + Math.random().toString(36).slice(2);
const LIMIT = 30_000;
const tick = (ms) => new Promise((r) => setTimeout(r, ms));

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

/** `method` `path` on `base` with `headers`; `{status, body}`. */
async function call(base, method, path, { body, headers = {} } = {}) {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { "content-type": "application/json", ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let v = text;
  try {
    v = JSON.parse(text);
  } catch {
    // not JSON
  }
  return { status: res.status, body: v, headers: res.headers };
}

/** The local daemon: a fresh home, `clax serve` on a free port. */
async function localDaemon(bin) {
  const home = mkdtempSync(join(tmpdir(), "clax-e2e-publish-"));
  writeFileSync(join(home, "config.toml"), '[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n');
  const port = await freePort();
  const child = spawn(bin, ["serve", "--foreground", "--bind", "127.0.0.1", "--port", String(port)], {
    env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_E2E_UNSET_KEY: "" },
    stdio: ["ignore", "ignore", "inherit"],
  });
  const info = join(home, "daemon.json");
  const end = Date.now() + LIMIT;
  while (!existsSync(info)) {
    if (Date.now() > end || child.exitCode !== null) throw new Error("the local daemon did not start");
    await tick(20);
  }
  let d = null;
  while (!d) {
    try {
      d = JSON.parse(readFileSync(info, "utf8"));
    } catch {
      await tick(20);
    }
  }
  const base = `http://127.0.0.1:${port}`;
  while (!(await fetch(`${base}/healthz`).then((r) => r.ok, () => false))) await tick(20);
  const auth = { authorization: `Bearer ${d.token}` };
  return {
    base,
    api: (method, path, body) => call(base, method, path, { body, headers: auth }),
    stop: async () => {
      await fetch(`${base}/api/admin/shutdown`, { method: "POST", headers: auth }).catch(() => {});
      await Promise.race([new Promise((r) => child.once("exit", r)), tick(5000)]);
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      rmSync(home, { recursive: true, force: true });
    },
  };
}

const page = (n) => ({ "index.html": { content: `<title>Tide tables</title><h1>Tide tables</h1><p>Draft ${n}.</p>`, encoding: "utf8" } });

async function main() {
  const bin = process.env.CLAX_TEST_BIN;
  if (!bin || !bin.startsWith("/")) throw new Error("set CLAX_TEST_BIN to an absolute path of a clax binary");
  await build();
  pruneRuns(ROOT, 3);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = writeConfig(dir, { HOSTED: "1", PUBLIC_ORIGIN: origin, SETUP_SECRET: SECRET, CLAX_TOKEN: TOKEN });
  let w;
  let local;
  try {
    w = await start(dir, config, 1, port);
    local = await localDaemon(bin);
    const owner = (method, path, body) => call(origin, method, path, { body, headers: { authorization: `Bearer ${TOKEN}` } });

    // The Worker's owner signs a browser in.
    const setup = await call(origin, "POST", "/api/hosted/setup", { body: { secret: SECRET, name: "Ada" }, headers: { origin } });
    check(setup.status === 200, "the owner signs in on the Worker", setup.body);
    const cookie = setup.headers.get("set-cookie").split(";")[0];
    const browser = (method, path, body) => call(origin, method, path, { body, headers: { cookie, origin } });

    // The local daemon signs in by device code.
    const add = await local.api("POST", "/api/remotes", { url: origin, name: "tides" });
    check(add.status === 201 && add.body.verification_url === `${origin}/device?code=${add.body.user_code}`, "the Worker hands the local daemon a device code", add.body);
    const pageRes = await fetch(`${origin}/device?code=${add.body.user_code}`, { headers: { cookie } });
    check(pageRes.ok && (await pageRes.text()).includes("<div id=\"app\""), "the Worker serves the approval page", pageRes.status);
    const peek = await browser("GET", `/api/hosted/device/${add.body.user_code}`);
    check(peek.status === 200 && peek.body.client_name.startsWith("Clax on "), "the approval names the requesting machine", peek.body);
    const pending = await local.api("POST", `/api/remotes/${add.body.remote.id}/poll`, {});
    check(pending.body.status === "pending", "before approval the sign-in waits", pending.body);
    const approve = await browser("POST", `/api/hosted/device/${add.body.user_code}/approve`, {});
    check(approve.status === 200, "the owner approves it", approve.body);
    let connected;
    const end = Date.now() + LIMIT;
    for (;;) {
      await tick(5200);
      connected = await local.api("POST", "/api/remotes/tides/poll", {});
      if (connected.body.status !== "pending" || Date.now() > end) break;
    }
    check(connected.body.status === "connected" && connected.body.remote.person_name === "Ada", "the local daemon receives the owner's agent token", connected.body);
    const tokens = await browser("GET", "/api/hosted/agent-tokens");
    check(tokens.body.tokens.some((t) => t.id === connected.body.remote.grant_id), "the token is listed on People", tokens.body);
    const again = await browser("POST", `/api/hosted/device/${add.body.user_code}/approve`, {});
    check(again.status === 404, "a code is used once", again.body);

    // An artifact of three versions, pushed.
    const a = await local.api("POST", "/api/artifacts", { title: "Tide tables", files: page(1) });
    const aid = a.body.artifact.id;
    await local.api("POST", `/api/artifacts/${aid}/versions`, { if_version: 1, files: page(2), label: "Second", note: "Evening tides" });
    await local.api("POST", `/api/artifacts/${aid}/versions`, { if_version: 2, files: page(3), label: "Third" });
    const pushed = await local.api("POST", `/api/artifacts/${aid}/push`, {});
    check(pushed.status === 200 && pushed.body.pushed.length === 3, "a push sends all three versions", pushed.body);
    const rid = pushed.body.link.remote_artifact_id;
    check(pushed.body.url === `${origin}/a/${rid}`, "the public link is the Worker's", pushed.body.url);
    const versions = await owner("GET", `/api/artifacts/${rid}/versions`);
    const vs = versions.body.versions;
    check(vs.length === 3 && vs[1].label === "Second" && vs[1].note === "Evening tides", "the Worker has the three versions with their labels and notes", vs);
    const listed = await owner("GET", `/api/artifacts/${rid}`);
    check(listed.body.artifact.title === "Tide tables", "the public artifact keeps its title", listed.body.artifact);
    const file = await owner("GET", `/api/artifacts/${rid}/versions/3/files/index.html`);
    check(String(file.body).includes("Draft 3."), "version 3's file is there", file.body);

    // A new local version follows.
    await local.api("POST", `/api/artifacts/${aid}/versions`, { if_version: 3, files: page(4) });
    const next = await local.api("POST", `/api/artifacts/${aid}/push`, {});
    check(next.status === 200 && JSON.stringify(next.body.pushed) === JSON.stringify([{ n: 4, remote_n: 4 }]), "a new local version is pushed on its own", next.body);

    // Someone publishes on the public copy: the next push is refused.
    const theirs = await owner("POST", `/api/artifacts/${rid}/versions`, { if_version: 4, files: page("theirs") });
    check(theirs.status === 201, "someone publishes on the public copy", theirs.body);
    await local.api("POST", `/api/artifacts/${aid}/versions`, { if_version: 4, files: page(5) });
    const refused = await local.api("POST", `/api/artifacts/${aid}/push`, {});
    check(refused.status === 409 && refused.body.error.code === "remote_moved" && refused.body.error.remote_version === 5, "a push is refused when the public copy moved", refused.body);
    const still = await owner("GET", `/api/artifacts/${rid}`);
    check(still.body.artifact.current_version === 5, "nothing on the public copy was overwritten", still.body.artifact);
    const forked = await local.api("POST", `/api/artifacts/${aid}/push`, { mode: "fork" });
    check(forked.status === 200 && forked.body.link.remote_artifact_id !== rid, "it pushes onto a fork instead", forked.body);
    const fork = await owner("GET", `/api/artifacts/${forked.body.link.remote_artifact_id}`);
    check(fork.body.artifact.forked_from?.artifact_id === rid && fork.body.artifact.current_version === 2, "the fork is of the public copy, with the new version", fork.body.artifact);
    console.log("e2e-publish: ok");
  } finally {
    if (local) await local.stop();
    if (w) await w.stop();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
