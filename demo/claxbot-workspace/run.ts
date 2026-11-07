// ClaxBot fixes a reported bug from a container, end to end, on this machine:
//
//   node demo/claxbot-workspace/run.ts            a private repository, as a queue's admin connects one
//   node demo/claxbot-workspace/run.ts --public   a public repository an operator's workspace.toml names
//
// Real: a daemon on a scratch home, its queue, a report filed through the
// client SDK, ClaxBot's own run loop and sandbox, containers, and a model.
// Stand-ins, in this file: a forge that deploys a preview per pull request and,
// by default, demands credentials for every git and API call and acts as a
// GitHub App's GitHub (a JWT check, installation tokens, the Git Data API, token
// revocation); and an adapter that gives ClaxBot's Messages client a model
// through `claude -p` with its tools off, for a machine with no API key.
// Everything it makes is under target/.
//
// The private run connects the repository through Clax's HTTP API, as people
// do, and checks what must hold of it (`privateChecks`); any failure ends the
// run with an error.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { createHash, createPublicKey, generateKeyPairSync, randomBytes, verify as verifySignature } from "node:crypto";
import { cpSync, createWriteStream, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync, existsSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
// The SDK as it ships: bazelisk build //web:dist_sdk //crates/clax-cli:clax
import { ReporterClient } from "../../bazel-bin/web/dist-sdk/index.js";
import { ServiceClient } from "../../bazel-bin/web/dist-sdk/service.js";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const isPublic = process.argv.includes("--public");
const state = join(root, isPublic ? "target/claxbot-workspace-demo" : "target/claxbot-workspace-demo-private");
const repos = join(state, "repos");
const bare = join(repos, "demo/app.git");
const IMAGE = "clax-demo-workspace:latest";
const APP_ID = 4242;
const INSTALLATION = 77;
const ACCOUNT = "demo-org";
const SIGNATURE = "demo-forge-signature";
/** Planted in the private source: nothing a reporter reads may hold it. */
const MARKER = `CLAX-PRIVATE-${randomBytes(6).toString("hex")}`;
const log = (what: string) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${what}`);
const children: ChildProcess[] = [];
const bridges: string[] = [];
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    for (const c of children) c.kill("SIGTERM");
    process.exit(1);
  });
}

/** Fails the demo unless `ok`, saying what held. */
function check(ok: unknown, what: string): asserts ok {
  if (!ok) throw new Error(`FAILED: ${what}`);
  log(`ok: ${what}`);
}

/** git, with no configuration of this machine's. */
function git(args: string[], cwd = state, input?: string | Buffer, env: Record<string, string> = {}): string {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    input,
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1", ...env },
  }).trim();
}
const inBare = (args: string[], input?: string | Buffer, env?: Record<string, string>) => git(["--git-dir", bare, ...args], state, input, env);

const raw = async (req: IncomingMessage) => {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  return Buffer.concat(chunks);
};
const body = async (req: IncomingMessage) => (await raw(req)).toString("utf8");
const json = (res: ServerResponse, status: number, v: unknown) => {
  res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(v));
};
const listen = (s: ReturnType<typeof createServer>) =>
  new Promise<number>(r => s.listen(0, "::", () => r((s.address() as AddressInfo).port)));

// ---------------------------------------------------------------------------
// The forge and its previews
// ---------------------------------------------------------------------------

type Pr = { number: number; html_url: string; title: string; body: string; head: string; base: string; deployed?: string };
type Minted = { token: string; permissions: Record<string, string>; repositories: string[]; revoked: boolean };
/** One git request: from a container or this machine, for what, with which token. */
type GitCall = { container: boolean; service: string; token: string | null };
/** What the forge saw. */
type Forge = {
  port: number;
  /** The same forge, for containers alone: what arrives here came from one. */
  containerPort: number;
  stop: () => void;
  prs: Pr[];
  minted: Minted[];
  gitCalls: GitCall[];
  /** Commits made through the Git Data API. */
  commits: string[];
  privateKey: string;
};

function deploy(name: string, ref: string): string {
  const commit = inBare(["rev-parse", ref]);
  const dir = join(state, "previews", name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const tar = join(state, `${name}.tar`);
  inBare(["archive", "--format=tar", "-o", tar, ref]);
  execFileSync("tar", ["-xf", tar, "-C", dir]);
  writeFileSync(join(dir, ".commit"), commit);
  return commit;
}

/** The site `name` serves `path` from: its commit at `/healthz`, else a file. */
function site(res: ServerResponse, name: string, path: string) {
  const dir = join(state, "previews", name);
  if (!existsSync(join(dir, ".commit"))) return json(res, 404, { error: "no deployment" });
  if (path === "/healthz") return json(res, 200, { commit: readFileSync(join(dir, ".commit"), "utf8") });
  const file = join(dir, path === "/" || path === "" ? "index.html" : path);
  if (!file.startsWith(dir) || !existsSync(file)) return json(res, 404, { error: "not found" });
  const type = file.endsWith(".js") ? "text/javascript" : "text/html";
  res.writeHead(200, { "content-type": type }).end(readFileSync(file));
}

const bearerOf = (req: IncomingMessage) => /^Bearer (.+)$/.exec(req.headers.authorization ?? "")?.[1];
const basicToken = (req: IncomingMessage) => {
  const m = /^Basic (.+)$/.exec(req.headers.authorization ?? "");
  return m ? Buffer.from(m[1], "base64").toString("utf8").split(":")[1] ?? null : null;
};

/** Whether `jwt` speaks for the App, as GitHub checks one. */
function jwtHolds(jwt: string | undefined, publicKey: string): boolean {
  const [head, claims, signature] = (jwt ?? "").split(".");
  if (!head || !claims || !signature) return false;
  const signed = verifySignature("RSA-SHA256", Buffer.from(`${head}.${claims}`), createPublicKey(publicKey), Buffer.from(signature, "base64url"));
  if (!signed) return false;
  const c = JSON.parse(Buffer.from(claims, "base64url").toString("utf8")) as { iss: string; exp: number; iat: number };
  const now = Date.now() / 1000;
  return c.iss === String(APP_ID) && c.exp > now && c.exp - c.iat <= 660;
}

/** One request through `git http-backend`, by the CGI rules it speaks. A push
 * is refused: a private run's commits arrive through the API or not at all. */
async function gitBackend(req: IncomingMessage, res: ServerResponse, url: URL) {
  const child = spawn("git", ["http-backend"], {
    env: {
      PATH: process.env.PATH ?? "",
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_PROJECT_ROOT: repos,
      GIT_HTTP_EXPORT_ALL: "1",
      REMOTE_USER: "x-access-token",
      REQUEST_METHOD: req.method ?? "GET",
      PATH_INFO: url.pathname,
      QUERY_STRING: url.search.slice(1),
      CONTENT_TYPE: req.headers["content-type"] ?? "",
      HTTP_CONTENT_ENCODING: req.headers["content-encoding"] ?? "",
      GIT_PROTOCOL: String(req.headers["git-protocol"] ?? ""),
    },
    stdio: ["pipe", "pipe", "ignore"],
  });
  req.pipe(child.stdin);
  const out: Buffer[] = [];
  for await (const c of child.stdout) out.push(c as Buffer);
  const all = Buffer.concat(out);
  const split = all.indexOf("\r\n\r\n");
  const headers: Record<string, string> = {};
  let status = 200;
  for (const line of all.subarray(0, split).toString("utf8").split("\r\n")) {
    const [name, value] = [line.slice(0, line.indexOf(":")), line.slice(line.indexOf(":") + 2)];
    if (name.toLowerCase() === "status") status = Number(value.split(" ")[0]);
    else headers[name] = value;
  }
  res.writeHead(status, headers).end(all.subarray(split + 4));
}

/** The Git Data route `path` of a write token's call, over the bare repository. */
function gitData(forge: Forge, method: string, path: string, b: any, res: ServerResponse) {
  if (method === "POST" && path === "blobs") {
    const sha = inBare(["hash-object", "-w", "--stdin"], Buffer.from(b.content, "base64"));
    return json(res, 201, { sha });
  }
  if (method === "POST" && path === "trees") {
    const index = join(state, `index-${randomBytes(4).toString("hex")}`);
    const env = { GIT_INDEX_FILE: index };
    inBare(["read-tree", b.base_tree], undefined, env);
    for (const e of b.tree as { path: string; mode: string; sha: string | null }[]) {
      if (e.sha) inBare(["update-index", "--add", "--cacheinfo", `${e.mode},${e.sha},${e.path}`], undefined, env);
      else inBare(["update-index", "--force-remove", e.path], undefined, env);
    }
    const sha = inBare(["write-tree"], undefined, env);
    rmSync(index, { force: true });
    return json(res, 201, { sha });
  }
  if (method === "POST" && path === "commits") {
    if (b.author || b.committer || b.signature) return json(res, 422, { message: "a commit with its own author loses the forge's signature" });
    const at = `${Math.floor(Date.now() / 1000)} +0000`;
    const lines = [
      `tree ${b.tree}`,
      ...(b.parents as string[]).map(p => `parent ${p}`),
      `author demo-app[bot] <bot@forge.localhost> ${at}`,
      `committer Demo Forge <noreply@forge.localhost> ${at}`,
      `gpgsig -----BEGIN DEMO SIGNATURE-----`,
      ` ${SIGNATURE}`,
      ` -----END DEMO SIGNATURE-----`,
      "",
      b.message,
    ];
    const sha = inBare(["hash-object", "-t", "commit", "-w", "--stdin"], lines.join("\n"));
    forge.commits.push(sha);
    log(`forge: made and signed commit ${sha.slice(0, 8)}`);
    return json(res, 201, { sha, verification: { verified: true, reason: "valid" } });
  }
  if (method === "GET" && path.startsWith("commits/")) {
    return json(res, 200, { sha: path.slice("commits/".length), verification: { verified: true, reason: "valid" } });
  }
  if (method === "POST" && path === "refs") {
    inBare(["update-ref", b.ref, b.sha]);
    return json(res, 201, { ref: b.ref, object: { sha: b.sha } });
  }
  if (method === "PATCH" && path.startsWith("refs/")) {
    if (b.force) return json(res, 422, { message: "this forge's refs are never forced here" });
    inBare(["update-ref", path, b.sha]);
    return json(res, 200, { ref: path, object: { sha: b.sha } });
  }
  return json(res, 404, { message: "Not Found" });
}

async function startForge(): Promise<Forge> {
  const keys = generateKeyPairSync("rsa", {
    modulusLength: 2048,
    publicKeyEncoding: { type: "spki", format: "pem" },
    privateKeyEncoding: { type: "pkcs1", format: "pem" },
  });
  const forge: Forge = { port: 0, containerPort: 0, stop: () => {}, prs: [], minted: [], gitCalls: [], commits: [], privateKey: keys.privateKey };
  const live = (t: string | null | undefined) => forge.minted.find(m => m.token === t && !m.revoked);
  const pulls = async (req: IncomingMessage, res: ServerResponse, url: URL) => {
    if (req.method === "GET") {
      const head = (url.searchParams.get("head") ?? "").split(":")[1];
      return json(res, 200, forge.prs.filter(p => p.head === head));
    }
    const b = JSON.parse(await body(req));
    const number = forge.prs.length + 1;
    const pr: Pr = { number, html_url: `http://localhost:${forge.port}/pull/${number}`, title: b.title, body: b.body, head: b.head, base: b.base };
    forge.prs.push(pr);
    log(`forge: pull request #${number} opened: ${b.title}`);
    return json(res, 201, pr);
  };
  const api = async (req: IncomingMessage, res: ServerResponse, url: URL) => {
    const path = url.pathname.slice("/api/v3".length);
    if (req.method === "DELETE" && path === "/installation/token") {
      const m = live(bearerOf(req));
      if (!m) return json(res, 401, { message: "Bad credentials" });
      m.revoked = true;
      log(`forge: a ${m.permissions.contents} token was revoked`);
      res.writeHead(204).end();
      return;
    }
    if (path === `/app/installations/${INSTALLATION}/access_tokens` || path.endsWith("/installation")) {
      if (!jwtHolds(bearerOf(req), keys.publicKey)) return json(res, 401, { message: "A JSON web token could not be decoded" });
      if (req.method === "GET") {
        if (path !== "/repos/demo/app/installation") return json(res, 404, { message: "Not Found" });
        return json(res, 200, { id: INSTALLATION, account: { login: ACCOUNT, type: "Organization" } });
      }
      const b = JSON.parse(await body(req));
      const token = `ghs_demo_${randomBytes(18).toString("hex")}`;
      forge.minted.push({ token, permissions: b.permissions, repositories: b.repositories, revoked: false });
      log(`forge: minted a ${b.permissions.contents} token for ${b.repositories}`);
      return json(res, 201, { token, expires_at: new Date(Date.now() + 3_600_000).toISOString() });
    }
    const m = live(bearerOf(req));
    if (!m) return json(res, 401, { message: "Bad credentials" });
    if (m.permissions.contents !== "write") return json(res, 403, { message: "Resource not accessible by integration" });
    if (path === "/repos/demo/app/pulls") return pulls(req, res, url);
    const data = /^\/repos\/demo\/app\/git\/(.+)$/.exec(path);
    if (data) return gitData(forge, req.method ?? "GET", data[1], req.method === "GET" ? null : JSON.parse(await body(req)), res);
    return json(res, 404, { message: "Not Found" });
  };
  const handle = (req: IncomingMessage, res: ServerResponse) => {
    void (async () => {
      const url = new URL(req.url ?? "/", "http://forge");
      const host = (req.headers.host ?? "").split(":")[0];
      if (url.pathname.startsWith("/demo/app.git/")) {
        const service = url.searchParams.get("service") ?? url.pathname.split("/").at(-1) ?? "";
        const token = basicToken(req);
        forge.gitCalls.push({ container: req.socket.localPort === forge.containerPort, service, token });
        const m = live(token);
        const write = service === "git-receive-pack";
        if (!m || (write && m.permissions.contents !== "write")) {
          res.writeHead(401, { "www-authenticate": 'Basic realm="git"' }).end();
          return;
        }
        return gitBackend(req, res, url);
      }
      if (url.pathname.startsWith("/api/v3/")) return api(req, res, url);
      if (url.pathname === "/repos/demo/app/pulls") return pulls(req, res, url);
      // A site per path, or per host as previews often are: pr-<N>.<zone>,
      // and main at the zone.
      const path = /^\/pr\/(\d+)(\/.*)?$/.exec(url.pathname);
      if (path) return site(res, `pr-${path[1]}`, path[2] ?? "/");
      const m = /^pr-(\d+)\./.exec(host);
      return site(res, m ? `pr-${m[1]}` : "main", url.pathname);
    })().catch(e => json(res, 500, { error: String(e) }));
  };
  const server = createServer(handle);
  const containers = createServer(handle);
  forge.port = await listen(server);
  forge.containerPort = await listen(containers);
  // Each move of a pull request's branch deploys its preview.
  const deployer = setInterval(() => {
    for (const pr of forge.prs) {
      const head = inBare(["rev-parse", `refs/heads/${pr.head}`]);
      if (head !== pr.deployed) {
        deploy(`pr-${pr.number}`, head);
        pr.deployed = head;
        log(`forge: deployed ${head.slice(0, 8)} to pull request ${pr.number}'s preview`);
      }
    }
  }, 500);
  forge.stop = () => { clearInterval(deployer); server.close(); containers.close(); };
  return forge;
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

const MODELS = { data: [{ type: "model", id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5", created_at: "2026-09-01T00:00:00Z" }], has_more: false, first_id: "claude-sonnet-5-5", last_id: "claude-sonnet-5-5" };

type Block = { type: string; text?: string; name?: string; input?: unknown; content?: unknown };
const blockText = (b: Block): string => {
  if (b.type === "text") return b.text ?? "";
  if (b.type === "tool_use") return `[you called ${b.name}: ${JSON.stringify(b.input)}]`;
  if (b.type === "tool_result") return `[tool result: ${typeof b.content === "string" ? b.content : JSON.stringify(b.content)}]`;
  return "";
};

function flatten(req: { system?: { text: string }[] | string; messages: { role: string; content: string | Block[] }[]; tools?: { name: string; description: string; input_schema: unknown }[] }): string {
  const system = typeof req.system === "string" ? req.system : (req.system ?? []).map(s => s.text).join("\n");
  const tools = (req.tools ?? []).map(t => `- ${t.name}: ${t.description} Input schema: ${JSON.stringify(t.input_schema)}`).join("\n");
  const turns = req.messages.map(m => {
    const text = typeof m.content === "string" ? m.content : m.content.map(blockText).join("\n");
    return `=== ${m.role} ===\n${text}`;
  }).join("\n\n");
  return `${system}\n\n# Tools\n${tools}\n\n# Conversation\n${turns}\n\nReply with exactly one JSON object and nothing else: {"say":"<one line for the person>","tool":"<tool name>","input":{...}}`;
}

/** The model's next call, from `claude -p` with no tools of its own. */
function think(prompt: string): Promise<{ say?: string; tool: string; input: unknown }> {
  return new Promise((done, fail) => {
    const c = spawn("claude", ["-p", "--tools", "", "--setting-sources", "", "--no-session-persistence", "--model", "sonnet"], { stdio: ["pipe", "pipe", "inherit"] });
    let out = "";
    c.stdout.on("data", d => (out += d));
    c.on("error", fail);
    c.on("close", () => {
      const at = out.indexOf("{");
      try {
        done(JSON.parse(out.slice(at, out.lastIndexOf("}") + 1)));
      } catch {
        fail(new Error(`the model did not answer JSON: ${out.slice(0, 300)}`));
      }
    });
    c.stdin.end(prompt);
  });
}

function sse(id: string, say: string | undefined, tool: string, input: unknown): string {
  const out: string[] = [];
  const ev = (name: string, data: Record<string, unknown>) => out.push(`event: ${name}\ndata: ${JSON.stringify({ type: name, ...data })}\n\n`);
  ev("message_start", { message: { id: `msg_${id}`, model: "claude-sonnet-5-5", usage: { input_tokens: 1000, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, output_tokens: 1 } } });
  let i = 0;
  if (say) {
    ev("content_block_start", { index: i, content_block: { type: "text", text: "" } });
    ev("content_block_delta", { index: i, delta: { type: "text_delta", text: say } });
    ev("content_block_stop", { index: i++ });
  }
  ev("content_block_start", { index: i, content_block: { type: "tool_use", id: `tu_${id}`, name: tool, input: {} } });
  ev("content_block_delta", { index: i, delta: { type: "input_json_delta", partial_json: JSON.stringify(input) } });
  ev("content_block_stop", { index: i });
  ev("message_delta", { delta: { stop_reason: "tool_use" }, usage: { output_tokens: 200 } });
  ev("message_stop", {});
  return out.join("");
}

/** The model, and everything it was shown and said. */
async function startModel() {
  let n = 0;
  const saw: string[] = [];
  const said: string[] = [];
  const server = createServer((req, res) => {
    void (async () => {
      if (req.method === "GET") return json(res, 200, MODELS);
      const r = JSON.parse(await body(req));
      const turn = ++n;
      const prompt = flatten(r);
      saw.push(prompt);
      const next = await think(prompt);
      if (next.say) said.push(next.say);
      log(`model turn ${turn}: ${next.tool}${next.say ? `: ${next.say}` : ""}`);
      res.writeHead(200, { "content-type": "text/event-stream" }).end(sse(String(turn), next.say, next.tool, next.input));
    })().catch(e => json(res, 500, { type: "error", error: { type: "api_error", message: String(e) } }));
  });
  return { port: await listen(server), saw, said, stop: () => server.close() };
}

// ---------------------------------------------------------------------------
// The daemon
// ---------------------------------------------------------------------------

async function until<T>(what: string, get: () => Promise<T | null | undefined>, ms: number): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await get();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 500));
  }
}

const home = join(state, "home");
const daemonLog = join(state, "daemon.log");
const clax = join(root, "bazel-bin/crates/clax-cli/clax");
const daemonEnv = { ...process.env, CLAX_HOME: home, CLAX_DEMO_BOT_KEY: "sk-ant-demo-not-a-real-key", CLAX_DEMO_WRITE_TOKEN: "demo", RUST_LOG: "clax_bot_workspace=trace,clax_bot=debug,clax_server=debug,clax_store=debug,reqwest=debug" };

/** A daemon on the scratch home, never ~/.clax, all it writes in the log. */
async function startDaemon(port = 0) {
  rmSync(join(home, "daemon.json"), { force: true });
  const out = createWriteStream(daemonLog, { flags: "a" });
  const daemon = spawn(clax, ["serve", "--foreground", "--port", String(port)], { env: daemonEnv, stdio: ["ignore", "pipe", "pipe"] });
  daemon.stdout.pipe(out);
  daemon.stderr.pipe(out);
  children.push(daemon);
  const info = await until("the daemon", async () => {
    const f = join(home, "daemon.json");
    return existsSync(f) ? (JSON.parse(readFileSync(f, "utf8")) as { port: number; token: string }) : null;
  }, 30_000);
  const base = `http://localhost:${info.port}`;
  await until("the daemon's health", async () => (await fetch(`${base}/healthz`).catch(() => null))?.ok, 30_000);
  return { daemon, base, info };
}

function client(base: string, token: string) {
  const call = async (method: string, path: string, b?: unknown) =>
    fetch(`${base}${path}`, { method, headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: b === undefined ? undefined : JSON.stringify(b) });
  return async (method: string, path: string, b?: unknown): Promise<Record<string, any>> => {
    const r = await call(method, path, b);
    const text = await r.text();
    if (!r.ok) throw Object.assign(new Error(`${method} ${path}: ${r.status} ${text}`), { status: r.status, text });
    return text ? JSON.parse(text) : {};
  };
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

const REPORT = {
  title: "Leave total is one day short",
  anchor: { kind: "element", selector: "#out" },
  body: [
    "The leave page counts one day too few, so people lose a day of leave.",
    "What I did: opened the page for a leave from Monday 2 March 2026 to Friday 6 March 2026.",
    "What it says: 4 days.",
    "What it should say: 5 days. Both the first and the last day of a leave are days off; a leave of one single day is 1 day, not 0.",
    "The repository's own test `node --test` fails on exactly this, so it is a bug to fix in the code, not a question of opinion.",
  ].join("\n"),
};

/** The repository, seeded with the bug and the private marker, and main deployed. */
function seedRepository() {
  const seed = join(state, "seed");
  cpSync(join(here, "sample-app"), seed, { recursive: true });
  const days = join(seed, "days.js");
  writeFileSync(days, readFileSync(days, "utf8").replace("__PRIVATE_MARKER__", MARKER));
  mkdirSync(bare, { recursive: true });
  git(["init", "--quiet", "--bare", "--initial-branch=main", bare]);
  git(["init", "--quiet", "--initial-branch=main"], seed);
  git(["add", "-A"], seed);
  git(["-c", "user.name=Demo", "-c", "user.email=demo@localhost", "commit", "--quiet", "-m", "The leave app"], seed);
  git(["push", "--quiet", bare, "main"], seed);
  deploy("main", "main");
}

/** Everything the reporter's own credentials read of `id`, in one text. */
async function reporterReads(reporter: any, id: string): Promise<string> {
  const page = await reporter.myReports();
  const badge = await reporter.badge();
  const diagnostics = await reporter.diagnostics(id).catch(() => []);
  return JSON.stringify({ page, badge, diagnostics });
}

async function fileAndRun(base: string, api: ReturnType<typeof client>, origin: string, queueId: string, grant: Record<string, any>) {
  const service = new ServiceClient({ instance: base, queue: queueId, token: grant.token, privateKey: grant.private_key });
  const fetchFrom = (input: RequestInfo | URL, init?: RequestInit) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), origin } });
  const reporter = new ReporterClient({
    instance: base,
    queue: queueId,
    fetch: fetchFrom,
    auth: (publicKey: string) => service.mintReporterToken({ reporter_id: "ada", origin, public_key: publicKey }),
  });
  const report = await reporter.fileReport({ url: `${origin}/`, ...REPORT });
  log(`report ${report.id} filed: ${report.state}`);
  // Triage sends it to ClaxBot: the call the report view's Send to ClaxBot makes.
  const { run } = await api("POST", "/api/bot/runs", { artifact_id: report.artifact.id, thread_ids: [report.id] });
  log(`ClaxBot run ${run.id} started`);
  const ended = await until("the run", async () => {
    const r = await api("GET", `/api/bot/runs/${run.id}`);
    return ["finished", "gave_up", "failed", "stopped", "cancelled"].includes(r.status) ? r : null;
  }, 900_000);
  log(`run ended: ${ended.status}${ended.reason ? ` (${ended.reason})` : ""}`);
  return { reporter, report, run, ended };
}

/** Monday to Friday, as the deployment at `at` counts it. */
async function days(at: string) {
  const f = join(state, `days-${Date.now()}-${randomBytes(2).toString("hex")}.mjs`);
  writeFileSync(f, await (await fetch(`${at}/days.js`)).text());
  return (await import(f)).daysBetween("2026-03-02", "2026-03-06") as number;
}

async function publicRun(forge: Forge, model: Awaited<ReturnType<typeof startModel>>) {
  const origin = `http://localhost:${forge.port}`;
  const first = await startDaemon();
  const api = client(first.base, first.info.token);
  log(`daemon on ${first.base}`);
  const { queue } = await api("POST", "/api/queues", { name: "leave-app", origins: [origin, `http://*.localhost:${forge.port}`], reporters_see: "own" });
  const grant = await api("POST", `/api/queues/${queue.id}/service-grants`, { name: "the leave app" });

  // The workspace, read at the daemon's start: restart it with the queue's ID.
  writeFileSync(join(home, "workspace.toml"), `image = "${IMAGE}"
auto_stop_minutes = 20

[runtime]
kind = "docker"
run_args = ["--volume", "${bare}:/srv/repo.git:ro"]

[[target]]
queue = "${queue.id}"
repo_url = "${bare}"
container_repo_url = "/srv/repo.git"
repo = "demo/app"
forge_api_url = "http://localhost:${forge.port}"
write_token_env = "CLAX_DEMO_WRITE_TOKEN"
public = true
allow_unsigned_commits = true
preview_url = "http://pr-{pr}.localhost:${forge.port}"
preview_timeout_secs = 120
preview_poll_secs = 1
`);
  first.daemon.kill("SIGTERM");
  await new Promise(r => first.daemon.once("exit", r));
  const again = await startDaemon(first.info.port);
  const apiAgain = client(again.base, again.info.token);

  const { reporter, report, run, ended } = await fileAndRun(again.base, apiAgain, origin, queue.id, grant);
  check(ended.status === "finished", `the run finished, not ${ended.status} ${ended.reason ?? ""}`);
  const seen = await until("the preview's link", async () => {
    const r = (await apiAgain("GET", `/api/queues/${queue.id}/reports/${report.id}`)).report;
    return r.fixes?.at(-1)?.verify_url ? r : null;
  }, 120_000);
  const fix = seen.fixes.at(-1);
  check(!fix.held, "with a preview to check, the fix is held no longer");
  log(`report is ${seen.state}: ${fix.summary}`);
  log(`change: ${forge.prs.at(-1)?.html_url}   check it here: ${fix.verify_url}`);
  check(seen.comments.some((c: { triage_only?: boolean; body: string }) => c.triage_only && c.body.includes(forge.prs.at(-1)!.html_url)), "the queue's people read what it proposed");
  const told = (await reporter.myReports()).reports.find((r: { id: string }) => r.id === report.id);
  check(told?.fixes?.at(-1)?.verify_url === fix.verify_url, "the reporter has the preview's link");

  // The preview is really different from main, and the reporter hears of it.
  const before = await days(origin);
  const after = await days(fix.verify_url);
  log(`Monday to Friday: main says ${before}, the preview says ${after}`);
  const pr = forge.prs.at(-1);
  log(`diff of ${pr?.head}:\n${inBare(["diff", "main", pr?.head ?? "main"])}`);
  check(after === 5 && before !== 5, "the preview shows the fix and main does not");
  check(model.saw.length > 0, "a real model ran");
  await containerGone(run.id);
  const verified = await reporter.verify(report.id, {});
  check(verified.state === "verified", "the reporter's verdict lands");
}

async function containerGone(run: string) {
  await until("the run's container to go", async () => {
    const left = execFileSync("docker", ["ps", "--all", "--quiet", "--filter", `label=clax.run=${run}`], { encoding: "utf8" }).trim();
    return left === "";
  }, 60_000);
  log(`ok: run ${run} has no container left`);
}

/** Every file under `dir`. */
function filesUnder(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? filesUnder(p) : [p];
  });
}

/** Each form a minted token or the App's key could be written in. */
function secretForms(forge: Forge): string[] {
  const tokens = forge.minted.map(m => m.token);
  const line = forge.privateKey.split("\n")[3];
  const sha = createHash("sha256").update(forge.privateKey.trim()).digest("hex");
  return [
    ...tokens,
    ...tokens.map(t => Buffer.from(`x-access-token:${t}`).toString("base64")),
    line,
    sha,
  ];
}

/** That a real git, with no token or a made-up one, is refused by the remote.
 * Not `git()`: the forge is in this process, and a blocking call would starve
 * it. */
async function theRemoteDemandsCredentials(origin: string) {
  const refused = (args: string[]) =>
    new Promise<boolean>(done => {
      const c = spawn("git", args, {
        env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0" },
        stdio: ["ignore", "ignore", "pipe"],
      });
      let err = "";
      c.stderr.on("data", d => (err += d));
      c.on("close", code => done(code !== 0 && /401|Authentication failed|could not read Username|terminal prompts disabled/.test(err)));
    });
  const clone = (name: string) => ["clone", "--quiet", `${origin}/demo/app.git`, join(state, name)];
  check(await refused(clone("no-token")), "the remote refuses a clone with no token");
  const made = Buffer.from("x-access-token:ghs_made_up").toString("base64");
  check(await refused(["-c", `http.extraHeader=Authorization: Basic ${made}`, ...clone("made-up")]), "the remote refuses a made-up token");
}

/** A container whose loopback port `port` is this machine's port `to`: others
 * join its network. Named, for `main` to end. */
function startBridge(port: number, to: number): string {
  const name = `clax-demo-net-${randomBytes(4).toString("hex")}`;
  const forward = `require("net").createServer(c => { const h = require("net").connect(${to}, "host.docker.internal"); c.pipe(h).pipe(c); c.on("error", () => h.destroy()); h.on("error", () => c.destroy()); }).listen(${port}, "127.0.0.1")`;
  execFileSync("docker", ["run", "--detach", "--rm", "--name", name, "--add-host", "host.docker.internal:host-gateway", IMAGE, "node", "-e", forward], { stdio: "ignore" });
  bridges.push(name);
  const up = `require("net").connect(${port}, "127.0.0.1").on("connect", () => process.exit(0)).on("error", () => process.exit(1))`;
  for (let i = 0; i < 50; i++) {
    try {
      execFileSync("docker", ["exec", name, "node", "-e", up], { stdio: "ignore" });
      return name;
    } catch {
      // Not listening yet.
    }
  }
  throw new Error("the bridge to the forge did not come up");
}

async function privateRun(forge: Forge, model: Awaited<ReturnType<typeof startModel>>) {
  const origin = `http://localhost:${forge.port}`;
  const apiUrl = `${origin}/api/v3`;
  await theRemoteDemandsCredentials(origin);
  check((await fetch(`${apiUrl}/repos/demo/app/pulls`)).status === 401, "the forge's API refuses a call with no token");

  // The operator's part: a runtime and nothing about any repository. A link's
  // one address serves the daemon and its containers, and the owner of a local
  // daemon may name only loopback over http, so the containers share a network
  // whose own loopback is forwarded to this machine's forge.
  const bridge = startBridge(forge.port, forge.containerPort);
  writeFileSync(join(home, "workspace.toml"), `image = "${IMAGE}"
auto_stop_minutes = 20

[runtime]
kind = "docker"
run_args = ["--network", "container:${bridge}"]
`);
  const { daemon, base, info } = await startDaemon();
  const api = client(base, info.token);
  log(`daemon on ${base}`);
  const { queue } = await api("POST", "/api/queues", { name: "leave-app", origins: [origin], reporters_see: "own" });
  const grant = await api("POST", `/api/queues/${queue.id}/service-grants`, { name: "the leave app" });
  const repoPath = `/api/queues/${queue.id}/repo`;

  // The owner sets the App.
  const repos0 = await api("PUT", "/api/admin/repos/app", { app_id: APP_ID, slug: "demo-app", api_url: apiUrl, private_key: forge.privateKey });
  check(repos0.app.key.set === true && !JSON.stringify(repos0).includes("PRIVATE KEY"), "the owner set the App, and its key does not come back");

  // A queue admin links the repository, naming only what they know.
  const link = {
    repo: "demo/app",
    preview_url: `http://localhost:${forge.port}/pr/{pr}`,
    preview_timeout_secs: 120,
    preview_poll_secs: 1,
    access: { kind: "github_app" },
    allow_open_egress: true,
  };
  const notThere = await api("PUT", repoPath, { ...link, repo: "demo/other" }).catch(e => e as { status: number; text: string });
  check("text" in notThere && notThere.status === 400 && notThere.text.includes("app_not_installed") && notThere.text.includes(`${origin}/apps/demo-app/installations/new`),
    "a repository the App is not on is refused, with the install link");
  const linked = await api("PUT", repoPath, link);
  check(linked.repo.access.installation_id === INSTALLATION && linked.repo.access.account === ACCOUNT, `the forge named the installation and its account, ${ACCOUNT}`);
  check(linked.repo.public === false, "the link is confidential: nobody said public");
  check(linked.repo.status === "needs_ci_acceptance", `a private link waits for its CI to be accepted, not ${linked.repo.status}`);
  const accepted = await api("PUT", repoPath, { ...link, ci_accepted: true, if_rev: linked.repo.rev });
  check(accepted.repo.ci_accepted === true, "the queue's admin accepted that the repository's CI may run ClaxBot's branch");

  // The owner approves, reading the account. A local daemon's owner is also
  // this queue's admin, so their link came approved: withdrawn, it stops.
  const line = async () => (await api("GET", "/api/admin/repos")).links[0];
  const seenByOwner = await line();
  check(seenByOwner.account === ACCOUNT, "the owner's approval line shows the account the forge reported");
  check(seenByOwner.public === false && seenByOwner.allow_open_egress === true && seenByOwner.ci_accepted === true, "and what makes the link dangerous");
  await api("PUT", `/api/admin/repos/links/${queue.id}`, { rev: seenByOwner.rev, approved: false });
  check((await api("GET", repoPath)).repo.status === "needs_approval", "a link with its approval withdrawn waits for the owner");
  await api("PUT", `/api/admin/repos/links/${queue.id}`, { rev: (await line()).rev, approved: true });
  const ready = await api("GET", repoPath);
  check(ready.repo.status === "ready", `approved, the link is ready, not ${ready.repo.status}`);
  check(ready.repo.last_ok_at === null, "and not called connected before anything was tried");
  check(ready.runtime.configured === true && ready.runtime.third_party === false, "the runtime the daemon booted is its own");

  const { reporter, report, run, ended } = await fileAndRun(base, api, origin, queue.id, grant);
  log(`the panel says of the link: ${JSON.stringify((await api("GET", repoPath)).repo.last_failure)}`);
  check(ended.status === "finished", `the run finished, not ${ended.status} ${ended.reason ?? ""}`);
  await privateChecks({ forge, model, api, base, origin, queueId: queue.id, reporter, report, run, repoPath });

  // No token in the daemon's log, nor anywhere in its home.
  const closed = new Promise(r => daemon.once("close", r));
  daemon.kill("SIGTERM");
  await closed;
  const forms = secretForms(forge);
  const text = readFileSync(daemonLog, "utf8");
  check(text.length > 0, `the daemon logged ${text.length} bytes`);
  check(!forms.some(f => text.includes(f)), "no token and nothing of the App's key is in the daemon's log");
  const held = filesUnder(home).filter(f => {
    const bytes = readFileSync(f);
    return forms.some(s => bytes.includes(s));
  });
  check(held.length === 0, `no token is in any file of the daemon's home: ${held.join(", ")}`);
}

async function privateChecks(c: {
  forge: Forge; model: Awaited<ReturnType<typeof startModel>>; api: ReturnType<typeof client>; base: string; origin: string; queueId: string;
  reporter: any; report: any; run: { id: string }; repoPath: string;
}) {
  const { forge, model, api, base, origin, queueId, reporter, report, run, repoPath } = c;
  const members = (await api("GET", `/api/queues/${queueId}/reports/${report.id}`)).report;
  const fix = members.fixes?.at(-1);
  check(fix, "ClaxBot proposed a fix");
  const pr = forge.prs.at(-1);
  check(pr, "a pull request is open");
  log(`report is ${members.state}; pull request: ${pr!.title}`);

  // The commit: through the API, signed, with no push.
  check(forge.commits.length >= 1, "the host's commit went through the Git Data API");
  const branchHead = inBare(["rev-parse", `refs/heads/${pr!.head}`]);
  check(forge.commits.includes(branchHead), "the branch stands on the commit the forge made");
  check(inBare(["cat-file", "commit", branchHead]).includes(SIGNATURE), "the commit carries the forge's signature marker");
  check(!forge.gitCalls.some(g => g.service === "git-receive-pack"), "nothing was pushed with git");

  // The tokens.
  const reads = forge.minted.filter(m => m.permissions.contents === "read");
  const writes = forge.minted.filter(m => m.permissions.contents === "write");
  check(reads.length >= 1 && writes.length >= 1, `the forge minted ${reads.length} read and ${writes.length} write tokens`);
  check(reads.every(m => JSON.stringify(m.permissions) === '{"contents":"read"}' && m.repositories.join() === "app"), "each read token is contents: read on this repository alone");
  check(reads.every(m => m.revoked), "each read token was revoked when its clone returned");
  check(writes.every(m => m.permissions.workflows === undefined && m.repositories.join() === "app"), "no write token may touch workflows or another repository");
  const fromContainer = forge.gitCalls.filter(g => g.container && g.token);
  log(`git calls from a container: ${fromContainer.length}, from the host: ${forge.gitCalls.length - fromContainer.length}`);
  check(fromContainer.length >= 1, "the container reached the remote itself");
  const readTokens = new Set(reads.map(m => m.token));
  check(fromContainer.every(g => readTokens.has(g.token!)), "the only token a container ever sent is a read token");
  const stale = reads[0].token;
  const basic = Buffer.from(`x-access-token:${stale}`).toString("base64");
  const reused = await fetch(`${origin}/demo/app.git/info/refs?service=git-upload-pack`, { headers: { authorization: `Basic ${basic}` } });
  check(reused.status === 401, "a revoked read token is refused");

  // The run's container.
  await containerGone(run.id);

  // Confidential: the reporter reads the fixed status and no model text.
  const asReporter = (await reporter.myReports()).reports.find((r: { id: string }) => r.id === report.id);
  const reporterFix = asReporter?.fixes?.at(-1);
  check(reporterFix?.summary === "A fix has been proposed.", `the reporter reads the fixed status: ${reporterFix?.summary}`);
  check(!reporterFix.url && !reporterFix.commit && !reporterFix.verify_url, "and no link, pull request or commit");
  const verdict = await reporter.verify(report.id, {}).catch((e: unknown) => e) as { status?: number; code?: string };
  check(verdict.status === 409 && verdict.code === "not_fix_proposed", `with nothing to check the reporter's verdict is refused: ${verdict.status} ${verdict.code}`);

  // The marker the model read reaches nothing the reporter reads.
  check(model.saw.some(p => p.includes(MARKER)), "the model read the private source, marker and all");
  const words = [MARKER, pr!.title, ...pr!.body.split("\n").filter(l => l.length > 24).slice(0, 3), ...model.said.filter(s => s.length > 24)];
  const sweep = async (when: string) => {
    const readable = await reporterReads(reporter, report.id);
    for (const w of words) check(!readable.includes(w), `${when}: the reporter reads nothing of ${w === MARKER ? "the planted marker" : JSON.stringify(w.slice(0, 40))}`);
    for (const path of [`/api/bot/runs/${run.id}`, `/api/queues/${queueId}/reports/${report.id}`, `/api/queues/${queueId}/repo`]) {
      const r = await fetch(`${base}${path}`);
      check(!(await r.text()).includes(MARKER) && !r.ok, `${when}: nobody without a credential reads ${path.split("/").slice(0, 4).join("/")}`);
    }
  };
  await sweep("before a person gives a link");

  // A person hands the preview over, and the verdict works.
  const preview = `${origin}/pr/${pr!.number}`;
  await until("the preview", async () => (await fetch(`${preview}/healthz`)).ok, 60_000);
  const before = await days(`${origin}`);
  const after = await days(preview);
  log(`Monday to Friday: main says ${before}, the preview says ${after}`);
  log(`diff of ${pr!.head}:\n${inBare(["diff", "main", pr!.head])}`);
  check(after === 5 && before !== 5, "the preview shows the fix and main does not");

  // The daemon saw it too: the members read where, and the reporter does not.
  const reportPath = `/api/queues/${queueId}/reports/${report.id}`;
  await until("the members' preview", async () => (await api("GET", reportPath)).report.fixes.at(-1).preview_url === preview, 60_000);
  const seenAsReporter = (await reporter.myReports()).reports.find((r: { id: string }) => r.id === report.id);
  check(seenAsReporter.fixes.at(-1).held === true && !JSON.stringify(seenAsReporter).includes(preview), "the preview is for the members: the reporter's fix stays held, with no link");
  const noLink = await api("POST", `${reportPath}/fix`, { summary: "Have a look." }).catch(e => e as { status: number; text: string });
  check("text" in noLink && noLink.status === 400 && noLink.text.includes("verify_url_required"), `a hand-over with no link is refused: ${noLink.status}`);

  await api("POST", `${reportPath}/fix`, { summary: "Please check the preview: Monday to Friday should now say 5 days.", verify_url: preview });
  const verified = await reporter.verify(report.id, {});
  check(verified.state === "verified", "after a person gave the reporter a link, the verdict works");
  await sweep("after the verdict");

  // What the panel says of the link now.
  const after_ = await api("GET", repoPath);
  check(after_.repo.last_ok_at && after_.repo.last_failure === null, "the link has worked, and nothing it tried failed");
}

async function main() {
  rmSync(state, { recursive: true, force: true });
  mkdirSync(home, { recursive: true });

  log("building the container image");
  execFileSync("docker", ["build", "-q", "-t", IMAGE, "-f", join(here, "sandbox.Dockerfile"), here], { stdio: "inherit" });
  seedRepository();

  const forge = await startForge();
  const model = await startModel();
  writeFileSync(join(home, "config.toml"), `[sample]\napi_key_env = "CLAX_DEMO_UNSET"\n[bot]\nbase_url = "http://127.0.0.1:${model.port}"\napi_key_env = "CLAX_DEMO_BOT_KEY"\n`);
  await (isPublic ? publicRun(forge, model) : privateRun(forge, model));
  forge.stop();
  model.stop();
}

try {
  await main();
  log("the demo finished");
} finally {
  for (const c of children) if (c.exitCode === null) c.kill("SIGTERM");
  for (const b of bridges) execFileSync("docker", ["rm", "--force", b], { stdio: "ignore" });
}
process.exit(0);
