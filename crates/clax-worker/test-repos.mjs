// Checks the repository stand-in under `wrangler dev --local`: builds the
// Worker (./build.sh), starts wrangler on free ports with a fresh
// `--persist-to` under target/repos-test/ and REPO_DEV_ROUTES=1, writes two
// versions of a new artifact through the content store (`content_store`,
// GitContent over RepoDO), mints a read token and clones the artifact from
// the Worker's /git/ URL with real `git` and with `jj git clone`, checks
// that a clone without a token or with a revoked one is refused, that a
// write token pushes to main and not a version tag, that RepoDO's internal
// API is unreachable from outside, that push bodies are bounded and read
// only for a live token, that commits pushed with `git push` and with
// `jj git push` to an artifact published through the API become versions
// the API lists (a publish that did not see them conflicts, one that did
// lands), that the server's git token routes hand out tokens whose
// answered commands clone and push and that an ended session's token clones
// nothing, that assets are kept in
// the ASSETS R2 bucket, then restarts wrangler on the same state and clones
// again and reads the asset back, and once more with REPO_STORE=artifacts,
// where (without a Cloudflare account) the binding's calls fail, /git/ is
// not served, and assets are still kept in R2. Each wrangler is stopped by the
// process group it was started in. Prints one line per check and a JSON
// summary; exits non-zero on the first failure.
//
// jj is `CLAX_TEST_JJ`, else `jj` on PATH; the check fails without one.
// `node test-repos.mjs --no-build` skips the build.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, openSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { connect, createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "target/repos-test");
const KEEP_RUNS = 3;
const START_LIMIT_MS = 60_000;
const STOP_LIMIT_MS = 10_000;
/** A bound for a hung git or jj, not a time budget. */
const HANG_MS = 120_000;
const ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";
const TOKEN = "repos-owner-token-" + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);

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

function pruneRuns() {
  if (!existsSync(ROOT)) return;
  const runs = readdirSync(ROOT)
    .filter((n) => /^run-\d+-\d+$/.test(n))
    .sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]));
  for (const n of runs.slice(KEEP_RUNS - 1)) rmSync(join(ROOT, n), { recursive: true, force: true });
}

/** wrangler.jsonc without its custom build (this script builds once) and with absolute paths. */
function writeConfig(dir) {
  const text = readFileSync(join(HERE, "wrangler.jsonc"), "utf8")
    .split("\n")
    .filter((l) => !/^\s*\/\//.test(l))
    .join("\n");
  const config = JSON.parse(text);
  delete config.$schema;
  delete config.build;
  config.main = join(HERE, config.main);
  if (config.assets) config.assets.directory = resolve(HERE, config.assets.directory);
  const path = join(dir, "wrangler.json");
  writeFileSync(path, JSON.stringify(config, null, 2));
  return path;
}

/** Starts one wrangler dev on `state`; resolves once it answers HTTP. */
async function start(dir, config, n, vars = []) {
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
      "--var", "REPO_DEV_ROUTES:1",
      ...vars.flatMap((v) => ["--var", v]),
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
  return { base, stop, log };
}

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

/**
 * One request on a connection of its own; answers its status and body text.
 * Pooled keep-alive connections sit idle while git runs, and one the server
 * closes as it is reused fails a POST with ECONNRESET.
 */
function send(method, url, body) {
  return new Promise((ok, fail) => {
    const req = httpRequest(url, { method, agent: false }, (res) => {
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (d) => (text += d));
      res.on("end", () => ok({ status: res.statusCode, text }));
    });
    req.on("error", fail);
    req.end(body);
  });
}

async function post(base, path) {
  const { status, text } = await send("POST", `${base}/_dev/repos/${path}`);
  if (status !== 200) throw new Error(`FAIL POST ${path}: ${status} ${text}`);
  return JSON.parse(text);
}

/** A GET of `path` exactly as written (no URL normalization); answers the status line and body. */
function rawGet(base, path) {
  const { hostname, port } = new URL(base);
  return new Promise((ok, fail) => {
    const s = connect(Number(port), hostname, () => s.end(`GET ${path} HTTP/1.1\r\nHost: ${hostname}:${port}\r\nConnection: close\r\n\r\n`));
    let text = "";
    s.setEncoding("utf8");
    s.on("data", (d) => (text += d));
    s.on("error", fail);
    s.on("end", () => ok({ status: Number(text.split(" ")[1]), text }));
  });
}

/** A POST of `body` to `path` exactly as written, naming `host`; answers the status and body. */
function rawPost(base, path, host, body) {
  const { hostname, port } = new URL(base);
  return new Promise((ok, fail) => {
    const s = connect(Number(port), hostname, () =>
      s.end(
        `POST ${path} HTTP/1.1\r\nHost: ${host}\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`,
      ),
    );
    let text = "";
    s.setEncoding("utf8");
    s.on("data", (d) => (text += d));
    s.on("error", fail);
    s.on("end", () => ok({ status: Number(text.split(" ")[1]), text }));
  });
}

/**
 * A POST of `mb` MiB to `url` as a chunked stream (no Content-Length), with
 * `headers`, on a connection of its own: the server answers before the body
 * is sent and closes the connection, which a pooled one would be reused after.
 */
function chunkedPost(url, mb, headers = {}) {
  return new Promise((ok, fail) => {
    const req = httpRequest(url, {
      method: "POST",
      agent: false,
      headers: { "content-type": "application/x-git-receive-pack-request", "transfer-encoding": "chunked", ...headers },
    });
    let left = mb << 4;
    req.on("response", (res) => {
      left = 0;
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (d) => (text += d));
      const done = () => {
        ok({ status: res.statusCode, text });
        req.destroy();
      };
      res.on("end", done);
      res.on("aborted", done);
    });
    req.on("error", (e) => (e.code === "ECONNRESET" || e.code === "EPIPE" ? undefined : fail(e)));
    const chunk = Buffer.alloc(1 << 16);
    const pump = () => {
      while (left > 0) {
        left--;
        if (!req.write(chunk)) return void req.once("drain", pump);
      }
      req.end();
    };
    pump();
  });
}

/** Bytes no compressor shrinks. */
function noise(n) {
  const b = Buffer.alloc(n);
  let x = 0x9e3779b9;
  for (let i = 0; i < n; i++) {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    b[i] = x & 0xff;
  }
  return b;
}

/** An API call with the owner token; answers its status and parsed body. */
async function api(base, method, path, body) {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { "content-type": "application/json", authorization: `Bearer ${TOKEN}` },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  return { status: res.status, body: text ? JSON.parse(text) : null };
}

/**
 * Checks that commits pushed with git and with jj to an artifact the API
 * published become versions the API lists, as the owner's (the token's
 * holder), and that publishing goes on from them.
 */
async function checkPushes(base, home, work, jj) {
  const created = await api(base, "POST", "/api/artifacts", {
    title: "Pushed",
    files: { "index.html": { content: "<h1>one</h1>\n" } },
  });
  check(created.status === 201 && created.body.artifact.current_version === 1, "an artifact is published through the API", created);
  const id = created.body.artifact.id;
  const write = await post(base, `token?artifact=${id}&scope=write`);
  const ident = ["-c", "user.name=Pusher", "-c", "user.email=pusher@clax.test"];
  const out = join(work, "pushes-git");
  ok(home, "git", work, [...bearer(write), "clone", "-q", write.remote, out]);
  writeFileSync(join(out, "index.html"), "<h1>git</h1>\n");
  writeFileSync(join(out, "notes.txt"), "from git\n");
  ok(home, "git", out, ["add", "-A"]);
  ok(home, "git", out, [...ident, "commit", "-q", "-m", "Pushed with git"]);
  ok(home, "git", out, [...bearer(write), "push", "-q", "origin", "main"]);
  let versions = (await api(base, "GET", `/api/artifacts/${id}/versions`)).body.versions;
  check(
    versions.length === 2 && versions[1].note === "Pushed with git" && "notes.txt" in versions[1].files,
    "a git push to main is version 2 once the push is answered",
    versions,
  );
  const file = await api(base, "GET", `/api/artifacts/${id}`);
  check(file.body.artifact.current_version === 2, "the artifact's current version is the pushed one", file.body);

  const header = `Authorization: Bearer ${write.token}`;
  const jjEnv = { GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "http.extraHeader", GIT_CONFIG_VALUE_0: header };
  const jjOut = join(work, "pushes-jj");
  ok(home, jj, work, ["git", "clone", write.remote, jjOut], jjEnv);
  writeFileSync(join(jjOut, "index.html"), "<h1>jj</h1>\n");
  ok(home, jj, jjOut, ["describe", "-m", "Pushed with jj"], jjEnv);
  ok(home, jj, jjOut, ["bookmark", "set", "main", "-r", "@"], jjEnv);
  ok(home, jj, jjOut, ["git", "push", "--bookmark", "main"], jjEnv);
  versions = (await api(base, "GET", `/api/artifacts/${id}/versions`)).body.versions;
  check(
    versions.length === 3 && versions[2].note === "Pushed with jj" && "notes.txt" in versions[2].files,
    "a jj git push to main is version 3, carrying version 2's other files",
    versions,
  );

  const stale = await api(base, "POST", `/api/artifacts/${id}/versions`, {
    if_version: 1,
    files: { "index.html": { content: "<h1>stale</h1>\n" } },
  });
  check(stale.status === 409, "a publish that did not see the pushes conflicts", stale);
  const next = await api(base, "POST", `/api/artifacts/${id}/versions`, {
    if_version: 3,
    note: "Published after the pushes",
    files: { "index.html": { content: "<h1>four</h1>\n" } },
  });
  check(next.status === 201 && next.body.version.n === 4, "a publish after the pushes lands as version 4", next);
  ok(home, "git", out, [...bearer(write), "pull", "-q", "--tags", "origin", "main"]);
  const tags = ok(home, "git", out, ["tag"]);
  check(tags === "v1\nv2\nv3\nv4", "every version, pushed or published, is tagged", tags);
  check(readFileSync(join(out, "notes.txt"), "utf8") === "from git\n", "the publish carried the pushed file", null);

  // The instance's internal route answers only other objects: from outside,
  // under any spelling or host (the content origin's too), it is 404 and
  // records nothing.
  const { port } = new URL(base);
  const app = `127.0.0.1:${port}`;
  const body = JSON.stringify({ artifact: id, person: "u_nobody", session: null });
  for (const [path, host] of [
    ["/_internal/pushed", app],
    ["/_internal/pushed", "instance.internal"],
    ["/_internal/pushed", `${id}.localhost:${port}`],
    ["/x/../_internal/pushed", app],
    ["/%2e%2e/_internal/pushed", "instance.internal"],
    ["/%5Finternal/pushed", "instance.internal"],
    ["//_internal/pushed", "instance.internal"],
    ["/_INTERNAL/pushed", "instance.internal"],
    [`/api/artifacts/${id}/versions`, "instance.internal"],
  ]) {
    const r = await rawPost(base, path, host, body);
    check(r.status === 404 && !r.text.includes("recorded"), `POST ${path} naming ${host} is 404`, r.text.slice(0, 300));
  }
  versions = (await api(base, "GET", `/api/artifacts/${id}/versions`)).body.versions;
  check(versions.length === 4, "no internal call from outside recorded a version", versions.length);
}

/**
 * Checks the server's git token routes on the Worker: the owner's write
 * token from `POST /api/artifacts/<id>/git-token` clones with the answer's
 * jj and git commands and pushes through its commands for an existing
 * checkout, the push becoming a version; an agent session's read token from
 * `POST /api/sessions/<sid>/git-token/<id>` clones until the session ends;
 * no credential gets none.
 */
async function checkGitTokenRoutes(base, home, work, jj) {
  const sh = (cwd, cmd) => ok(home, "sh", cwd, ["-c", cmd.replace(" jj git ", ` ${jj} git `)]);
  const created = await api(base, "POST", "/api/artifacts", {
    title: "Routed",
    files: { "index.html": { content: "<h1>routed</h1>\n" } },
  });
  const id = created.body.artifact.id;
  const opts = await api(base, "GET", `/api/artifacts/${id}/git-token`);
  check(
    opts.status === 200 && opts.body.available === true && opts.body.scopes.join(",") === "read,write",
    "the git token route offers the owner read and write tokens",
    opts,
  );
  const write = await api(base, "POST", `/api/artifacts/${id}/git-token`, { scope: "write" });
  check(
    write.status === 200 && write.body.remote === `${base}/git/a-${id}.git` && write.body.token.startsWith("cxm_write_"),
    "the git token route mints a write token on the Worker's /git/ remote",
    write,
  );
  const t = write.body;
  const viaJj = join(work, "routed-jj-parent");
  mkdirSync(viaJj);
  sh(viaJj, `${t.jj_clone_command} routed`);
  check(readFileSync(join(viaJj, "routed", "index.html"), "utf8") === "<h1>routed</h1>\n", "the answer's jj_clone_command clones", null);
  const viaGit = join(work, "routed-git-parent");
  mkdirSync(viaGit);
  sh(viaGit, `${t.clone_command} routed`);
  const out = join(viaGit, "routed");
  writeFileSync(join(out, "index.html"), "<h1>routed, pushed</h1>\n");
  ok(home, "git", out, ["-c", "user.name=Owner", "-c", "user.email=owner@clax.test", "commit", "-qam", "Pushed through the route"]);
  sh(out, t.existing_checkout.add_remote_command);
  sh(out, t.existing_checkout.push_command);
  check(!readFileSync(join(out, ".git", "config"), "utf8").includes(t.token), "the checkout's configuration holds no token", null);
  const versions = (await api(base, "GET", `/api/artifacts/${id}/versions`)).body.versions;
  check(versions.length === 2 && versions[1].note === "Pushed through the route", "a push with the route's token is version 2", versions);

  const session = await api(base, "POST", "/api/sessions", { harness: "claude", cwd: "/w" });
  check(session.status === 201, "an agent session registers", session);
  const sid = session.body.session.id;
  const read = await api(base, "POST", `/api/sessions/${sid}/git-token/${id}`, { scope: "read" });
  check(read.status === 200 && read.body.token.startsWith("cxm_read_"), "an agent session gets a read token", read);
  const agentDir = join(work, "routed-agent");
  mkdirSync(agentDir);
  sh(agentDir, `${read.body.clone_command} first`);
  const ended = await api(base, "PATCH", `/api/sessions/${sid}`, { ended: true });
  check(ended.status === 200, "the agent session ends", ended);
  // The revocation runs beside the answer: bounded, not timed.
  const until = Date.now() + 20_000;
  let refused = false;
  while (!refused && Date.now() < until) {
    refused = run(home, "sh", agentDir, ["-c", `${read.body.clone_command} again-${Date.now()}`]).status !== 0;
    if (!refused) await new Promise((r) => setTimeout(r, 200));
  }
  check(refused, "an ended session's token clones nothing", null);

  const anon = await fetch(`${base}/api/artifacts/${id}/git-token`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ scope: "read" }),
  });
  check(anon.status === 401, "no credential gets no git token", anon.status);
}

/** One `/_dev/repos/` call; answers its status and body text. */
function dev(base, method, path) {
  return send(method, `${base}/_dev/repos/${path}`);
}

/** Checks that assets are kept, replaced and deleted in the ASSETS bucket. */
async function checkAssets(base, id, label) {
  const at = (asset) => `asset?artifact=${id}&asset=${asset}&ext=png`;
  check((await dev(base, "PUT", `${at("logo")}&body=one`)).status === 200, `${label}: an asset is stored`, null);
  check((await dev(base, "PUT", `${at("logo")}&body=two`)).status === 200, `${label}: storing it again replaces it`, null);
  check((await dev(base, "PUT", `${at("other")}&body=x`)).status === 200, `${label}: a second asset is stored`, null);
  const got = await dev(base, "GET", at("logo"));
  check(got.status === 200 && got.text === "two", `${label}: the asset reads back as its last put`, got);
  check((await dev(base, "DELETE", at("other"))).status === 200, `${label}: one asset is deleted`, null);
  const gone = await dev(base, "GET", at("other"));
  check(gone.status === 404, `${label}: a deleted asset reads as missing`, gone);
}

function newArtifactId() {
  let s = "";
  for (let i = 0; i < 12; i++) s += ALPHABET[Math.floor(Math.random() * 32)];
  return s;
}

/** Runs `program` with a home of its own; answers its exit status and output. */
function run(home, program, cwd, args, env = {}) {
  const r = spawnSync(program, args, {
    cwd,
    timeout: HANG_MS,
    encoding: "utf8",
    env: {
      PATH: process.env.PATH,
      HOME: home,
      TMPDIR: process.env.TMPDIR ?? "/tmp",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_TERMINAL_PROMPT: "0",
      JJ_CONFIG: join(home, "jj.toml"),
      JJ_USER: "Test",
      JJ_EMAIL: "test@clax.test",
      ...env,
    },
  });
  if (r.error) throw r.error;
  return { status: r.status, stdout: r.stdout.trim(), stderr: r.stderr };
}

function ok(home, program, cwd, args, env) {
  const r = run(home, program, cwd, args, env);
  if (r.status !== 0) throw new Error(`FAIL ${program} ${args.join(" ")}: ${r.stderr}`);
  return r.stdout;
}

const bearer = (t) => ["-c", `http.extraHeader=Authorization: Bearer ${t.token}`];

function findJj() {
  const candidates = [process.env.CLAX_TEST_JJ, "jj"].filter(Boolean);
  for (const c of candidates) {
    const r = spawnSync(c, ["--version"], { encoding: "utf8" });
    if (r.status === 0) return c;
  }
  throw new Error("no jj: set CLAX_TEST_JJ");
}

async function main() {
  const t0 = Date.now();
  const jj = findJj();
  if (!process.argv.includes("--no-build")) {
    const build = spawn(join(HERE, "build.sh"), [], { cwd: HERE, stdio: ["ignore", "inherit", "inherit"] });
    await exited(build);
    if (build.exitCode !== 0) throw new Error(`./build.sh failed (${build.exitCode ?? build.signalCode})`);
  }
  const built = Date.now();
  pruneRuns();
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  const home = join(dir, "home");
  const work = join(dir, "work");
  mkdirSync(home, { recursive: true });
  mkdirSync(work, { recursive: true });
  const config = writeConfig(dir);
  const running = new Set();
  const cleanup = () => Promise.all([...running].map((w) => w.stop()));
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void cleanup().then(() => process.exit(130)));
  try {
    let w = await start(dir, config, 1, [`CLAX_TOKEN:${TOKEN}`]);
    running.add(w);
    await checkPushes(w.base, home, work, jj);
    await checkGitTokenRoutes(w.base, home, work, jj);
    const id = newArtifactId();
    const v1 = await post(w.base, `publish?artifact=${id}&n=1&body=${encodeURIComponent("<h1>one</h1>\n")}`);
    check(v1.index_html === "<h1>one</h1>\n", "version 1 is written through the content store and reads back", v1);
    const v2 = await post(w.base, `publish?artifact=${id}&n=2&body=${encodeURIComponent("<h1>two</h1>\n")}`);
    check(v2.index_html === "<h1>two</h1>\n", "version 2 is written and reads back", v2);

    const read = await post(w.base, `token?artifact=${id}&scope=read`);
    check(read.remote === `${w.base}/git/a-${id}.git` && read.token.startsWith("cxm_read_"), "a read token names the Worker's /git/ remote", read);

    const none = run(home, "git", work, ["clone", "-q", read.remote, join(work, "none")]);
    check(none.status !== 0, "a clone without a token is refused", none);

    const out = join(work, "git");
    ok(home, "git", work, [...bearer(read), "clone", "-q", read.remote, out]);
    const log = ok(home, "git", out, ["log", "--format=%s"]);
    check(log === "Version 2\nVersion 1", "git clone shows one commit per version", log);
    const tags = ok(home, "git", out, ["tag"]);
    check(tags === "v1\nv2", "the versions are tagged v1 and v2", tags);
    const html = readFileSync(join(out, "index.html"), "utf8");
    check(html === "<h1>two</h1>\n", "the clone's index.html is version 2's", html);
    check(ok(home, "git", out, ["show", "v1:index.html"]) === "<h1>one</h1>", "v1 holds version 1's index.html", null);

    const header = `Authorization: Bearer ${read.token}`;
    const jjEnv = { GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "http.extraHeader", GIT_CONFIG_VALUE_0: header };
    const jjOut = join(work, "jj");
    ok(home, jj, work, ["git", "clone", read.remote, jjOut], jjEnv);
    const jjHtml = readFileSync(join(jjOut, "index.html"), "utf8");
    const jjLog = ok(home, jj, jjOut, ["log", "--no-graph", "-r", "::main@origin", "-T", 'description.first_line() ++ "\\n"'], jjEnv);
    check(
      jjHtml === "<h1>two</h1>\n" && jjLog.split("\n").filter(Boolean).join(",") === "Version 2,Version 1",
      "jj git clone carries both versions",
      { jjHtml, jjLog },
    );

    const ident = ["-c", "user.name=Pusher", "-c", "user.email=pusher@clax.test"];
    writeFileSync(join(out, "pushed.txt"), "pushed\n");
    ok(home, "git", out, ["add", "pushed.txt"]);
    ok(home, "git", out, [...ident, "commit", "-q", "-m", "Pushed"]);
    const readPush = run(home, "git", out, [...bearer(read), "push", "-q", "origin", "main"]);
    check(readPush.status !== 0 && readPush.stderr.includes("403"), "a read token does not push", readPush);
    const write = await post(w.base, `token?artifact=${id}&scope=write`);
    ok(home, "git", out, [...bearer(write), "push", "-q", "origin", "main"]);
    console.log("ok   a write token pushes to main");
    ok(home, "git", out, ["tag", "v3"]);
    const v3 = run(home, "git", out, [...bearer(write), "push", "origin", "v3"]);
    check(v3.status !== 0 && v3.stderr.includes("version tags"), "a push of a version tag is refused", v3);

    await checkAssets(w.base, id, "R2");

    // RepoDO's internal API is not reachable from outside the Worker.
    const mintBody = JSON.stringify({ repo: `a-${id}`, scope: "write", ttl_s: 3600, origin: null });
    for (const path of [`/_repos/refs?repo=a-${id}`, `/git/../_repos/refs?repo=a-${id}`, `/git/%2e%2e/_repos/refs?repo=a-${id}`, `/git/a-${id}.git/../../_repos/refs?repo=a-${id}`, `/do/repos/_repos/refs?repo=a-${id}`]) {
      const r = await rawGet(w.base, path);
      check(r.status !== 200 && !r.text.includes("refs/heads/main"), `the internal API does not answer ${path}`, r.text.slice(0, 300));
    }
    for (const path of ["/_repos/token/mint", "/git/../_repos/token/mint", "/git/x.git/%2e%2e/%2e%2e/_repos/token/mint"]) {
      const { status, text } = await send("POST", `${w.base}${path}`, mintBody);
      check(status !== 200 && !text.includes("cxm_"), `no token is minted through ${path}`, { status, text: text.slice(0, 300) });
    }

    // A push body is bounded, and read only for a live token.
    const pushUrl = `${w.base}/git/a-${id}.git/git-receive-pack`;
    const anon = await chunkedPost(pushUrl, 20);
    check(anon.status === 401, "a chunked push without a token is refused 401", anon);
    const big = await chunkedPost(pushUrl, 20, { authorization: `Bearer ${write.token}` });
    check(big.status === 413, "a chunked push body over the stand-in's bound is refused 413", big);
    const id2 = newArtifactId();
    await post(w.base, `publish?artifact=${id2}&n=1&body=x`);
    const w2 = await post(w.base, `token?artifact=${id2}&scope=write`);
    const big2 = join(work, "big");
    ok(home, "git", work, [...bearer(w2), "clone", "-q", w2.remote, big2]);
    const heads = () => ok(home, "git", big2, [...bearer(w2), "ls-remote", "origin"]);
    const before = heads();
    writeFileSync(join(big2, "noise.bin"), noise(13 << 20));
    ok(home, "git", big2, ["add", "noise.bin"]);
    ok(home, "git", big2, [...ident, "commit", "-q", "-m", "Noise"]);
    const tooBig = run(home, "git", big2, [...bearer(w2), "push", "origin", "main"]);
    check(tooBig.status !== 0 && tooBig.stderr.includes("413") && heads() === before, "a 13 MiB push is refused 413 and changes nothing", tooBig.stderr.slice(-400));
    ok(home, "git", big2, ["reset", "-q", "--hard", "HEAD~1"]);
    writeFileSync(join(big2, "noise.bin"), noise(10 << 20));
    ok(home, "git", big2, ["add", "noise.bin"]);
    ok(home, "git", big2, [...ident, "commit", "-q", "-m", "Noise"]);
    ok(home, "git", big2, [...bearer(w2), "push", "-q", "origin", "main"]);
    check(heads() !== before, "a 10 MiB push fits under the VFS's transaction cap", null);

    const revoked = await post(w.base, `revoke?artifact=${id}&id=${read.id}`);
    check(revoked.revoked === true, "the read token is revoked", revoked);
    const afterRevoke = run(home, "git", out, [...bearer(read), "fetch", "-q", "origin"]);
    check(afterRevoke.status !== 0, "a revoked token fetches nothing", afterRevoke);
    await w.stop();
    running.delete(w);
    const thrown = readFileSync(w.log, "utf8").match(/.*Critical error.*/g) ?? [];
    check(thrown.length === 0, "nothing threw past wasm-bindgen (\"Critical error\") in the first run", thrown.slice(0, 3));

    w = await start(dir, config, 2);
    running.add(w);
    const again = join(work, "again");
    const remote = `${w.base}/git/a-${id}.git`;
    ok(home, "git", work, [...bearer(write), "clone", "-q", remote, again]);
    const againLog = ok(home, "git", again, ["log", "--format=%s"]);
    check(againLog === "Pushed\nVersion 2\nVersion 1", "after a restart the repository holds the versions and the push", againLog);
    const kept = await dev(w.base, "GET", `asset?artifact=${id}&asset=logo&ext=png`);
    check(kept.status === 200 && kept.text === "two", "after a restart the asset reads back from R2", kept);
    check((await dev(w.base, "DELETE", `assets?artifact=${id}`)).status === 200, "an artifact's assets are deleted together", null);
    const allGone = await dev(w.base, "GET", `asset?artifact=${id}&asset=logo&ext=png`);
    check(allGone.status === 404, "after deleting an artifact's assets none reads back", allGone);
    await w.stop();
    running.delete(w);

    // REPO_STORE=artifacts without an account: the binding's calls fail.
    w = await start(dir, config, 3, ["REPO_STORE:artifacts"]);
    running.add(w);
    const res = await send("POST", `${w.base}/_dev/repos/publish?artifact=${newArtifactId()}&n=1&body=x`);
    const why = res.text;
    check(res.status === 409, "under REPO_STORE=artifacts with no account a write fails as the binding's call does", { status: res.status, why });
    const notServed = await send("GET", `${w.base}/git/a-${id}.git/info/refs?service=git-upload-pack`);
    check(notServed.status === 404, "under REPO_STORE=artifacts /git/ is not served", notServed.status);
    console.log(`     (the binding answered: ${why.trim().slice(0, 200)})`);
    await checkAssets(w.base, newArtifactId(), "R2 under REPO_STORE=artifacts");
    await w.stop();
    running.delete(w);
    console.log(JSON.stringify({ ok: true, build_s: (built - t0) / 1000, run_s: (Date.now() - built) / 1000, jj, dir }));
  } finally {
    await cleanup();
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? `${e.message}${e.cause ? ` (${e.cause.code ?? ""} ${e.cause.message ?? e.cause})` : ""}` : e);
  process.exit(1);
});
