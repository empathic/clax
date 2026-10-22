// A short smoke of a deployed hosted Clax on the real platform, step by
// step with timings, built to say which call misbehaves:
//
//   node smoke-remote.mjs --deploy ~/.config/clax/deploy/example.com.json   # deploy.mjs's record
//   node smoke-remote.mjs --url https://clax.example.com --setup-secret … --token …
//   node smoke-remote.mjs --url … --session <the clax session cookie's value> --token …
//   node smoke-remote.mjs --url … --site-domain example.run …   # the site domain (else the record's site_domain)
//   node smoke-remote.mjs --local [--no-build]   # against a fresh `wrangler dev --local`, REPO_STORE=memory
//   node smoke-remote.mjs --local --setup-done   # the same, its setup done beforehand, as after an earlier
//                                                # deploy (--without-token: and no owner token: a FAIL)
//   node smoke-remote.mjs --local --remote-artifacts   # the same Worker locally on the account's real
//                                                      # Artifacts (binding `remote: true`; needs the login)
//
// Steps: health; the diagnostics' view of the bindings; with Artifacts, the
// binding's raw calls on a scratch repository (src/diag.rs's probe: get of
// a missing repository, create, tokens, reads of missing objects, the
// remote's advertisements, a fork, deletes); setup with the secret when
// setup is open (else the given owner session); an agent token; two
// versions published over /mcp and read back; a fork, read back; the
// artifact's own site host (`<ID>.<site domain>`, from --site-domain or the
// record; `localhost` with --local): its health, a private site refused
// without a session (a navigation redirected to /site-signin, an API call
// 401), /site-signin's code redeemed at /_clax/session for a `SameSite=Lax`
// cookie, the shell's code (`POST /api/artifacts/<ID>/site-code`) for a
// Partitioned one that works once, the site opened to the world and served
// signed out, then private again and refused, and two db artifacts whose
// documents stay apart and whose cookies do not cross; a git
// token, the remote's receive-pack advertisement, `git clone`, the
// clone's tree against the binding's (Artifacts); with Artifacts and a
// wrangler login, a queue subscription for the artifact's repository;
// `git push`, and the push becoming a version (on Artifacts through the
// queue's `pushed` event; when none arrives, a direct reconcile tells the
// event's absence from a reconcile failure); a comment arriving live on a
// WebSocket stream. Then it cleans up (--keep keeps the artifacts) and,
// when it made the owner, prints a sign-in link for the owner's browser.
//
// Every step prints `ok`/`FAIL`/`skip`, its time and what it saw; a step
// whose inputs failed is skipped; when the site host's health answers 404
// or 403 forbidden_host (a Worker without site hosts) every site step skips. The diagnostics (`/_diag/`, the owner
// token, CLAX_DIAG_ROUTES=1: `deploy.mjs --diag`; `--local` turns them on
// for its own Worker) are needed for the probe, the git token and the
// reconcile; without them those steps are skipped. When a deployed Worker
// answers them, the smoke ends by saying to deploy again without them. Exits 1 when any
// step failed. --json prints the whole report as JSON at the end; --probe
// runs the Artifacts probe whatever the store (under `wrangler dev --local`
// every call on the binding throws); --queue-wait <s> (default 120) bounds
// the wait for the pushed event; --no-subscribe makes no subscription.

import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { HERE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";

const args = process.argv.slice(2);
const flag = (n) => args.includes(n);
const opt = (n) => {
  const i = args.findIndex((a) => a === n || a.startsWith(`${n}=`));
  if (i < 0) return undefined;
  return args[i].includes("=") ? args[i].split(/=(.*)/s)[1] : args[i + 1];
};

const RUN = Math.random().toString(36).slice(2, 8);
const STEP_LIMIT = 30_000;
const results = [];
const failed = new Set();

function short(v, n = 300) {
  const s = typeof v === "string" ? v : JSON.stringify(v);
  return s && s.length > n ? `${s.slice(0, n)}…` : s;
}

/** Runs step `name` unless one of `needs` failed or was skipped; `fn`
 * answers its detail, or throws. `skip(reason)` inside skips it. */
async function step(name, fn, needs = []) {
  const missing = needs.filter((n) => failed.has(n));
  if (missing.length) {
    failed.add(name);
    results.push({ name, status: "skip", ms: 0, detail: `needs ${missing.join(", ")}` });
    console.log(`skip ${name} (needs ${missing.join(", ")})`);
    return undefined;
  }
  const t0 = performance.now();
  try {
    const value = await fn();
    const ms = Math.round(performance.now() - t0);
    const detail = value?.detail ?? value;
    results.push({ name, status: "ok", ms, detail });
    console.log(`ok   ${name} (${ms} ms)${detail === undefined ? "" : `: ${short(detail)}`}`);
    return value;
  } catch (e) {
    const ms = Math.round(performance.now() - t0);
    failed.add(name);
    if (e instanceof Skip) {
      results.push({ name, status: "skip", ms, detail: e.message });
      console.log(`skip ${name}: ${e.message}`);
    } else {
      const detail = e instanceof Error ? e.message : String(e);
      results.push({ name, status: "fail", ms, detail });
      console.log(`FAIL ${name} (${ms} ms): ${detail}`);
    }
    return undefined;
  }
}

class Skip extends Error {}
const skip = (why) => {
  throw new Skip(why);
};

async function until(what, f, ms) {
  const end = Date.now() + ms;
  let last;
  for (;;) {
    try {
      last = await f();
      if (last) return last;
    } catch (e) {
      last = e instanceof Error ? e.message : e;
    }
    if (Date.now() > end) throw new Error(`${what}: not within ${ms / 1000} s (last: ${short(last, 200)})`);
    await new Promise((r) => setTimeout(r, 1000));
  }
}

function run(cmd, argv, opts = {}) {
  return new Promise((resolve) => {
    const child = spawn(cmd, argv, { ...opts, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("error", (e) => resolve({ code: -1, out, err: `${err}${e.message}` }));
    child.on("close", (code) => resolve({ code, out, err }));
  });
}

/** Fetches `url` (any origin; redirects not followed unless `init.redirect`
 * says so): {res, status, text, json}. */
async function at(url, init = {}) {
  const res = await fetch(url, { redirect: "manual", ...init, signal: AbortSignal.timeout(STEP_LIMIT) });
  const text = await res.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    // not JSON
  }
  return { res, status: res.status, text, json };
}

/** The client: the public origin, the owner token, the owner's session cookie. */
class Client {
  constructor(base) {
    this.base = base.replace(/\/$/, "");
    this.token = "";
    this.cookie = "";
    this.as = "";
    this.mcpSession = "";
    this.agent = "";
  }

  /** What `/mcp` is called with: the agent token, else (as the owner
   * token) the owner token. */
  get mcpBearer() {
    return this.agent || (this.as === "token" ? this.token : "");
  }

  async raw(path, init = {}) {
    return at(`${this.base}${path}`, { redirect: "follow", ...init });
  }

  /** The owner's credential as headers: the owner token, or the cookie. */
  credential() {
    if (this.as === "token") return { authorization: `Bearer ${this.token}` };
    return this.cookie ? { cookie: this.cookie } : {};
  }

  /** A call as the owner, same origin: the browser's session cookie, or
   * the owner token as a bearer token when the smoke acts as the token. */
  async owner(path, init = {}) {
    const headers = { origin: this.base, ...this.credential(), ...(init.headers ?? {}) };
    if (init.body !== undefined && typeof init.body === "string") headers["content-type"] ??= "application/json";
    const r = await this.raw(path, { ...init, headers });
    if (r.status >= 400) throw new Error(`${init.method ?? "GET"} ${path}: ${r.status} ${short(r.text)}`);
    return r;
  }

  /** A diagnostics call with the owner token. */
  async diag(path, method = "GET") {
    if (!this.token) skip("no owner token (--token, or deploy.mjs's record)");
    const r = await this.raw(`/_diag/${path}`, { method, headers: { authorization: `Bearer ${this.token}` } });
    if (r.status === 404 && !r.json) skip(`the diagnostics answer 404 (${short(r.text, 120).trim()}): deploy with them on for the smoke (deploy.mjs --diag), then again without`);
    if (r.status === 401) throw new Error("the diagnostics refused the owner token (401): is it the Worker's CLAX_TOKEN?");
    return r;
  }

  async mcp(method, params, id = 1) {
    const headers = { "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${this.mcpBearer}` };
    if (this.mcpSession && method !== "initialize") headers["mcp-session-id"] = this.mcpSession;
    const r = await this.raw("/mcp", { method: "POST", headers, body: JSON.stringify({ jsonrpc: "2.0", id, method, params }) });
    if (r.status >= 400) throw new Error(`/mcp ${method}: ${r.status} ${short(r.text)}`);
    if (method === "initialize") this.mcpSession = r.res.headers.get("mcp-session-id") ?? "";
    let j = r.json;
    if (!j && r.text.includes("data:")) {
      j = JSON.parse(r.text.split("\n").filter((l) => l.startsWith("data:")).pop().slice(5));
    }
    if (j?.error) throw new Error(`/mcp ${method}: ${JSON.stringify(j.error)}`);
    return j.result;
  }

  async tool(name, a) {
    const r = await this.mcp("tools/call", { name, arguments: a }, Date.now());
    const text = (r?.content ?? []).map((c) => c.text ?? "").join("");
    if (r?.isError) throw new Error(`${name}: ${short(text, 500)}`);
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      // a message
    }
    return { text, json };
  }
}

/** The site cookie a response set: {pair: "name=value", raw: the whole Set-Cookie}. */
function siteCookie(res) {
  const raw = (res.headers.getSetCookie?.() ?? []).find((s) => /^(__Host-)?clax_site=/.test(s));
  return raw ? { pair: raw.split(";")[0], raw } : null;
}

/** An artifact's site origin: the public origin's scheme and port on
 * `<ID>.<site domain>`. */
function siteOrigin(base, site, id) {
  const u = new URL(base);
  return `${u.protocol}//${id}.${site}${u.port ? `:${u.port}` : ""}`;
}

function sessionCookie(res) {
  const all = res.headers.getSetCookie?.() ?? [];
  const c = all.find((s) => /clax_session=/.test(s));
  return c ? c.split(";")[0] : "";
}

/** A smart HTTP advertisement's service, refs and capabilities. */
function advertisement(buf) {
  const out = { service: null, refs: [], capabilities: [] };
  let at = 0;
  while (at + 4 <= buf.length) {
    const n = parseInt(buf.subarray(at, at + 4).toString(), 16);
    if (Number.isNaN(n)) break;
    if (n === 0) {
      at += 4;
      continue;
    }
    const line = buf.subarray(at + 4, at + n).toString().replace(/\n$/, "");
    at += n;
    if (line.startsWith("# service=")) out.service = line.slice(10);
    else {
      const [ref, caps] = line.split("\0");
      out.refs.push(ref);
      if (caps) out.capabilities = caps.split(" ").filter(Boolean);
    }
  }
  return out;
}

/** What the probe's steps say against what the binding's types promise. */
function probeSurprises(steps) {
  const by = (call) => steps.find((s) => s.call.startsWith(call));
  const out = [];
  const expect = (call, ok, what) => {
    const s = by(call);
    if (!s) return out.push(`${call}: not run`);
    if (!ok(s)) out.push(`${call}: ${what}; it ${s.ok ? `answered ${short(s.value, 160)}` : `threw ${short(s.threw, 200)}`}`);
  };
  expect("get(missing)", (s) => !s.ok && s.threw?.code === "NOT_FOUND", "expected a throw with code NOT_FOUND (artifacts.rs maps only that code to a missing repository)");
  expect("create(", (s) => s.ok && typeof s.value?.remote === "string", "expected {remote, token, …}");
  expect("get(repo)", (s) => s.ok, "expected a handle");
  expect("info()", (s) => s.ok && typeof s.value?.remote === "string", "expected {remote, …} (js.rs reads info().remote)");
  expect("createToken(", (s) => s.ok && typeof s.value?.id === "string" && s.value?.plaintext && typeof s.value?.expiresAt === "string", "expected {id, plaintext, expiresAt} (js.rs reads all three)");
  for (const m of ["readCommit(zero", "readTree(zero", "readBlob(zero"]) expect(m, (s) => s.ok && s.value === null, "expected null for a missing object (js.rs takes null as missing, a throw as a failure)");
  expect("readFile(", (s) => s.ok && s.value === null, "expected null on an empty repository");
  expect("fork(", (s) => s.ok, "expected the fork to start");
  expect("delete(repo)", (s) => s.ok && s.value === true, "expected true");
  expect("get(repo) after delete", (s) => !s.ok && s.threw?.code === "NOT_FOUND", "expected NOT_FOUND");
  const adverts = steps.filter((s) => s.call.startsWith("GET info/refs"));
  const works = adverts.filter((s) => s.ok && s.value?.status === 200).map((s) => s.call);
  if (adverts.length && !works.some((c) => c.includes("git-receive-pack (bearer, whole token)"))) {
    out.push(`the remote's receive-pack advertisement refused the whole token as Bearer, which artifacts.rs sends; worked: ${works.join("; ") || "none"}`);
  }
  return out;
}

function probeLine(s) {
  const what = s.ok
    ? s.value?.status !== undefined && s.value?.advertisement !== undefined
      ? `HTTP ${s.value.status}${s.value.advertisement?.capabilities ? ` caps=[${s.value.advertisement.capabilities.join(" ")}]` : ` ${short(s.value.advertisement, 120)}`}`
      : `→ ${short(s.value, 220)}`
    : `threw ${short(s.threw, 220)}`;
  return `       ${String(Math.round(s.ms ?? 0)).padStart(5)} ms  ${s.call} ${what}`;
}

const PAGE = (marker) =>
  `<!doctype html><title>Smoke ${RUN}</title><style>:root{--bg:#fff;--fg:#111}@media (prefers-color-scheme: dark){:root{--bg:#111;--fg:#eee}}body{background:var(--bg);color:var(--fg);font:16px system-ui;margin:0 16px}</style><main><h1>Smoke ${RUN}</h1><p id=m>${marker}</p></main>`;

/** The step that makes the smoke the owner: setup with the secret while
 * setup is open, the given session, else the owner token. */
const OWNER = "owner: setup, session or owner token";
/** The steps only a signed-in browser can take: they skip, saying so,
 * when the smoke acts with the owner token (POST and DELETE
 * /api/hosted/agent-tokens refuse any request with Authorization; the
 * MCP steps then use the owner token, as the daemon token's sessionless
 * agent). */
const SESSION_ONLY = ["agent token"];

async function smoke(c, o) {
  const store = { value: o.store ?? null };
  const git = mkdtempSync(join(o.work, "git-"));
  let aid = "";
  let fork = "";
  const dbIds = [];
  let agentTokenId = "";
  let subscription = "";
  let madeOwner = false;

  await step("health", async () => {
    const r = await c.raw("/healthz");
    if (r.status !== 200) throw new Error(`/healthz: ${r.status} ${short(r.text)}`);
    return r.json ?? r.text;
  });

  await step("diagnostics: env", async () => {
    const r = await c.diag("env");
    if (r.status !== 200) throw new Error(`${r.status} ${short(r.text)}`);
    store.value = r.json.repo_store;
    const missing = Object.entries(r.json.bindings).filter(([k, v]) => !v && !(k === "ARTIFACTS" && store.value === "memory")).map(([k]) => k);
    if (missing.length) throw new Error(`bindings missing: ${missing.join(", ")}; ${short(r.json)}`);
    if (store.value === "artifacts" && !["create", "get", "delete"].every((f) => r.json.artifacts_functions.includes(f))) {
      return { detail: `REPO_STORE=${store.value}; the ARTIFACTS binding lists functions [${r.json.artifacts_functions.join(", ")}] (an RPC stub may list none)` };
    }
    return { detail: `REPO_STORE=${store.value}, hosted=${r.json.hosted}, setup secret set=${r.json.setup_secret_set}, bindings ${JSON.stringify(r.json.bindings)}` };
  }, ["health"]);

  await step("artifacts: the binding's raw calls (probe)", async () => {
    if (store.value !== "artifacts" && !flag("--probe")) skip(`REPO_STORE is ${store.value ?? "unknown"} (--probe runs it anyway)`);
    const r = await c.diag(`artifacts/probe?repo=clax-diag-${RUN}`, "POST");
    if (r.status !== 200) throw new Error(`${r.status} ${short(r.text)}`);
    for (const s of r.json.steps) console.log(probeLine(s));
    const surprises = probeSurprises(r.json.steps);
    if (surprises.length) throw new Error(`${surprises.length} surprise(s):\n       - ${surprises.join("\n       - ")}`);
    return { detail: `${r.json.steps.length} calls as the types say` };
  }, ["diagnostics: env"]);

  await step(OWNER, async () => {
    const me = await c.raw("/api/hosted/me");
    if (me.status !== 200) throw new Error(`/api/hosted/me: ${me.status} ${short(me.text)}`);
    if (me.json.origin !== c.base) throw new Error(`the instance names its origin ${me.json.origin}, not ${c.base}: PUBLIC_ORIGIN differs from the URL used`);
    if (o.session) {
      c.cookie = o.session.includes("=") ? o.session : `${c.base.startsWith("https:") ? "__Host-clax_session" : "clax_session"}=${o.session}`;
      c.as = "session";
      const who = await c.owner("/api/hosted/me");
      if (!who.json.signed_in) throw new Error(`the given session is not signed in: ${short(who.json)}`);
      return { detail: `the given session, signed in as ${who.json.person?.name ?? "?"}` };
    }
    if (!me.json.setup_open || !o.secret) {
      // Setup was done by an earlier deploy (or there is no secret to do
      // it with): act as the owner with the owner token, which the routes
      // take as on a local daemon. Without it nothing is tested: FAIL.
      const why = me.json.setup_open ? "setup is open but no --setup-secret was given" : "setup is closed (done already)";
      if (!c.token) throw new Error(`${why}, and there is no owner token: pass --deploy <deploy.mjs's record> or --token <CLAX_TOKEN>, or --session <the owner's session cookie>`);
      c.as = "token";
      const r = await c.owner("/api/artifacts?limit=1").catch((e) => {
        throw new Error(`${why}; the owner token was refused: ${e.message}`);
      });
      if (!Array.isArray(r.json?.artifacts)) throw new Error(`${why}; GET /api/artifacts with the owner token answered ${short(r.json ?? r.text)}`);
      return { detail: `${why}: acting with the owner token; the steps that need a browser session (${SESSION_ONLY.join(", ")}) skip` };
    }
    const page = await c.raw("/setup", { headers: { accept: "text/html" } });
    if (page.status !== 200 || !/<!doctype html/i.test(page.text)) throw new Error(`the /setup page: ${page.status} ${short(page.text, 200)}`);
    const r = await c.raw("/api/hosted/setup", {
      method: "POST",
      headers: { "content-type": "application/json", origin: c.base },
      body: JSON.stringify({ secret: o.secret, name: o.ownerName }),
    });
    if (r.status !== 200 && r.status !== 201) throw new Error(`setup: ${r.status} ${short(r.text)}`);
    c.cookie = sessionCookie(r.res);
    if (!c.cookie) throw new Error(`setup answered ${r.status} without a session cookie`);
    madeOwner = true;
    c.as = "session";
    o.onSetup?.();
    return { detail: `set up as ${o.ownerName}; cookie ${c.cookie.split("=")[0]}` };
  }, ["health"]);

  await step("web UI: the shell's pages", async () => {
    const out = [];
    for (const path of ["/", "/people"]) {
      const r = await c.owner(path, { headers: { accept: "text/html" } });
      const type = r.res.headers.get("content-type") ?? "";
      if (!type.includes("text/html") || !/<!doctype html/i.test(r.text)) throw new Error(`${path}: ${r.status} ${type} ${short(r.text, 200)}`);
      out.push(`${path} ${r.status}`);
    }
    const script = /src="(\/_clax\/[^"]+\.js)"/.exec((await c.owner("/")).text)?.[1];
    if (script) {
      const s = await c.raw(script);
      if (s.status !== 200) throw new Error(`${script}: ${s.status}`);
      out.push(`${script} ${s.status}`);
    }
    return { detail: out.join(", ") };
  }, [OWNER]);

  await step("agent token", async () => {
    if (c.as === "token") skip("only a signed-in browser mints agent tokens (--session <cookie> runs this step); the MCP steps use the owner token");
    const r = await c.owner("/api/hosted/agent-tokens", { method: "POST", body: JSON.stringify({ label: `smoke-remote ${RUN}` }) });
    c.agent = r.json.token;
    agentTokenId = r.json.id;
    if (!c.agent?.startsWith("cxa_")) throw new Error(`no agent token: ${short(r.json)}`);
    if (r.json.mcp_url !== `${c.base}/mcp`) throw new Error(`mcp_url is ${r.json.mcp_url}`);
    return { detail: `minted; mcp_url ${r.json.mcp_url}` };
  }, [OWNER]);

  await step("mcp: initialize and tools/list", async () => {
    const init = await c.mcp("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "smoke-remote", version: "1" } });
    // An agent token's initialize opens a session; the owner token's
    // tools are the sessionless agent and get none.
    if (c.agent && !c.mcpSession) throw new Error("initialize answered no Mcp-Session-Id");
    const list = await c.mcp("tools/list", {}, 2);
    const names = (list.tools ?? []).map((t) => t.name);
    for (const n of ["publish", "read", "delete"]) if (!names.includes(n)) throw new Error(`no ${n} tool among ${names.join(", ")}`);
    return { detail: `${init.serverInfo?.name} ${init.serverInfo?.version}; ${names.length} tools; as ${c.agent ? "the agent token" : "the owner token"}` };
  }, c.as === "token" ? [OWNER] : ["agent token"]);

  await step("publish v1 (mcp)", async () => {
    const r = await c.tool("publish", { html: PAGE(`v1 ${RUN}`), title: `Smoke ${RUN}` });
    aid = r.json?.id ?? r.json?.artifact?.id ?? /\b([0-9a-z]{12})\b/.exec(r.text)?.[1] ?? "";
    if (!aid) throw new Error(`no artifact ID in ${short(r.text)}`);
    return { detail: `artifact ${aid}` };
  }, ["mcp: initialize and tools/list"]);

  await step("publish v2 (mcp)", async () => {
    const r = await c.tool("publish", { id: aid, if_version: 1, html: PAGE(`v2 ${RUN}`) });
    const v = r.json?.version ?? r.json?.current_version ?? r.json?.artifact?.current_version;
    if (v !== undefined && v !== 2) throw new Error(`published as version ${v}: ${short(r.text)}`);
    return { detail: `version 2` };
  }, ["publish v1 (mcp)"]);

  await step("read back v1 and v2 (mcp)", async () => {
    for (const n of [1, 2]) {
      const r = await c.tool("read", { url_or_id: aid, version: n });
      if (!r.text.includes(`v${n} ${RUN}`)) throw new Error(`version ${n} reads ${short(r.text)}`);
    }
    return { detail: "both versions' content" };
  }, ["publish v2 (mcp)"]);

  await step("fork", async () => {
    const r = await c.owner(`/api/artifacts/${aid}/fork`, { method: "POST", body: "{}" });
    fork = r.json?.artifact?.id;
    if (!fork) throw new Error(`no fork in ${short(r.json)}`);
    const read = await c.tool("read", { url_or_id: fork });
    if (!read.text.includes(`v2 ${RUN}`)) throw new Error(`the fork reads ${short(read.text)}`);
    return { detail: `fork ${fork} of v${r.json.version?.n ?? r.json.version ?? "?"} reads v2's content` };
  }, ["read back v1 and v2 (mcp)"]);

  // The artifact's own site host. A Worker without site hosts refuses the
  // host (403 forbidden_host) or answers 404, and every site step skips.
  const site = o.siteDomain;
  const siteOf = (id) => siteOrigin(c.base, site, id);
  const html = { accept: "text/html" };
  /** Signs the owner in to artifact `id`'s site through /site-signin:
   * the site cookie /_clax/session set. */
  const signInTo = async (id) => {
    const host = new URL(siteOf(id)).host;
    const signin = await at(`${c.base}/site-signin?host=${encodeURIComponent(host)}&next=%2F`, { headers: { ...html, cookie: c.cookie } });
    const loc = new URL(signin.res.headers.get("location") ?? "/", c.base).href;
    if (signin.status !== 302 || !loc.startsWith(`${siteOf(id)}/_clax/session?`)) {
      throw new Error(`/site-signin for ${host} answered ${signin.status} ${signin.res.headers.get("location") ?? short(signin.text, 160)}, not a 302 to ${siteOf(id)}/_clax/session`);
    }
    const session = await at(loc, { headers: html });
    const cookie = siteCookie(session.res);
    if (!cookie) throw new Error(`${siteOf(id)}/_clax/session answered ${session.status} without a site cookie`);
    return cookie;
  };
  await step("site host: health", async () => {
    if (!site) skip("no site domain (--site-domain, or deploy.mjs's record's site_domain); every site step skips");
    const r = await at(`${siteOf(aid)}/_clax/healthz`);
    if (r.status === 404 || (r.status === 403 && r.json?.error?.code === "forbidden_host")) {
      skip(`${siteOf(aid)}/_clax/healthz answers ${r.status}${r.status === 403 ? " forbidden_host" : ""}: this Worker does not serve site hosts yet (SITE_DOMAIN); every site step skips`);
    }
    if (r.status !== 200) throw new Error(`${siteOf(aid)}/_clax/healthz: ${r.status} ${short(r.text, 200)}`);
    return { detail: `${siteOf(aid)}: ${short(r.json ?? r.text, 160)}` };
  }, ["publish v2 (mcp)"]);

  await step("site: a private site refused without a session", async () => {
    const nav = await at(`${siteOf(aid)}/`, { headers: html });
    const loc = nav.res.headers.get("location") ?? "";
    if (nav.status !== 302 || !loc.startsWith(`${c.base}/site-signin?`)) {
      throw new Error(`a signed-out navigation answered ${nav.status} ${loc || short(nav.text, 160)}, not a 302 to ${c.base}/site-signin`);
    }
    const host = new URL(loc).searchParams.get("host");
    if (host !== new URL(siteOf(aid)).host) throw new Error(`/site-signin is asked for host ${host}, not ${new URL(siteOf(aid)).host}`);
    const api = await at(`${siteOf(aid)}/_clax/api/whoami`);
    if (api.status !== 401) throw new Error(`a signed-out API call answered ${api.status} ${short(api.text, 160)}, not 401`);
    return { detail: `navigation 302 to /site-signin; API call 401 ${api.json?.error?.code ?? ""}`.trim() };
  }, ["site host: health"]);

  await step("site: /site-signin, then /_clax/session (SameSite=Lax)", async () => {
    const cookie = await signInTo(aid);
    const attrs = cookie.raw.split(";").slice(1).map((a) => a.trim()).join("; ");
    if (!/SameSite=Lax/i.test(cookie.raw) || /Partitioned/i.test(cookie.raw) || !/HttpOnly/i.test(cookie.raw)) {
      throw new Error(`the top-level site cookie is ${attrs}: expected HttpOnly, SameSite=Lax, not Partitioned`);
    }
    const page = await at(`${siteOf(aid)}/`, { headers: { ...html, cookie: cookie.pair } });
    if (page.status !== 200 || !page.text.includes(`Smoke ${RUN}`)) throw new Error(`with the cookie, / answered ${page.status} ${short(page.text, 160)}`);
    return { detail: `${cookie.pair.split("=")[0]}; ${attrs}; / served` };
  }, ["site: a private site refused without a session", "owner: setup or session"]);

  await step("site: the shell's code (Partitioned, single use)", async () => {
    const r = await c.browser(`/api/artifacts/${aid}/site-code`, { method: "POST", body: JSON.stringify({ next: "/", partitioned: true }) });
    const src = r.json?.src ?? "";
    if (!src.startsWith(`${siteOf(aid)}/_clax/session?`)) throw new Error(`site-code answered src ${short(src || r.text, 200)}`);
    const first = await at(src, { headers: html });
    const cookie = siteCookie(first.res);
    if (!cookie) throw new Error(`redeeming the code answered ${first.status} without a site cookie`);
    if (!/Partitioned/i.test(cookie.raw) || !/SameSite=None/i.test(cookie.raw)) throw new Error(`the framed site cookie is ${cookie.raw.split(";").slice(1).join(";")}: expected SameSite=None; Partitioned`);
    const again = await at(src, { headers: html });
    if (siteCookie(again.res)) throw new Error("the code was redeemed a second time");
    const page = await at(`${siteOf(aid)}/`, { headers: { ...html, cookie: cookie.pair } });
    if (page.status !== 200) throw new Error(`with the partitioned cookie, / answered ${page.status}`);
    return { detail: `Partitioned cookie, / served; a second redeem answered ${again.status} without a cookie` };
  }, ["site: /site-signin, then /_clax/session (SameSite=Lax)"]);

  await step("site: open to the world, then private again", async () => {
    const access = async (v) => {
      const r = await c.browser(`/api/artifacts/${aid}`, { method: "PATCH", body: JSON.stringify({ site_access: v }) });
      if (r.status < 200 || r.status > 299) throw new Error(`PATCH site_access ${v}: ${r.status} ${short(r.text, 160)}`);
      return r;
    };
    await access("world");
    let open;
    try {
      open = await at(`${siteOf(aid)}/`, { headers: html });
    } finally {
      await access("private");
    }
    if (open.status !== 200 || !open.text.includes(`Smoke ${RUN}`)) throw new Error(`world-open, a signed-out visitor got ${open.status} ${short(open.text, 160)}`);
    const closed = await at(`${siteOf(aid)}/`, { headers: html });
    const loc = closed.res.headers.get("location") ?? "";
    if (closed.status !== 302 || !loc.startsWith(`${c.base}/site-signin?`)) {
      throw new Error(`private again, a signed-out navigation answered ${closed.status} ${loc || short(closed.text, 160)}, not a 302 to ${c.base}/site-signin`);
    }
    return { detail: "world: 200 signed out; private again: 302 to /site-signin" };
  }, ["site: a private site refused without a session", "owner: setup or session"]);

  await step("site: two db artifacts kept apart", async () => {
    for (const n of ["A", "B"]) {
      const r = await c.tool("publish", { html: PAGE(`db ${n} ${RUN}`), title: `Smoke db ${n} ${RUN}`, capabilities: { db: {} } });
      const id = r.json?.id ?? r.json?.artifact?.id ?? /\b([0-9a-z]{12})\b/.exec(r.text)?.[1];
      if (!id) throw new Error(`no artifact ID in ${short(r.text)}`);
      dbIds.push(id);
    }
    const [a, b] = dbIds;
    const ca = (await signInTo(a)).pair;
    const cb = (await signInTo(b)).pair;
    const doc = (id) => `${siteOf(id)}/_clax/api/docs/smoke/one`;
    const put = (id, cookie, who) =>
      at(doc(id), { method: "PUT", headers: { cookie, origin: siteOf(id), "content-type": "application/json" }, body: JSON.stringify({ data: { who } }) });
    const get = (id, cookie) => at(doc(id), { headers: { cookie } });
    const pa = await put(a, ca, "A");
    if (pa.status !== 200) throw new Error(`PUT on A's host: ${pa.status} ${short(pa.text, 200)}`);
    const before = await get(b, cb);
    if (before.status !== 404) throw new Error(`B's host reads smoke/one before B wrote it: ${before.status} ${short(before.text, 160)}`);
    const pb = await put(b, cb, "B");
    if (pb.status !== 200) throw new Error(`PUT on B's host: ${pb.status} ${short(pb.text, 200)}`);
    const ga = await get(a, ca);
    const gb = await get(b, cb);
    if (ga.json?.doc?.data?.who !== "A" || gb.json?.doc?.data?.who !== "B") throw new Error(`A reads ${short(ga.json)}, B reads ${short(gb.json)}`);
    const cross = await get(b, ca);
    if (cross.status < 400) throw new Error(`A's site cookie was taken on B's host: ${cross.status}`);
    return { detail: `${a} and ${b} each read their own smoke/one; A's cookie on B's host: ${cross.status}` };
  }, ["site: /site-signin, then /_clax/session (SameSite=Lax)", "mcp: initialize and tools/list"]);

  let remote = "";
  let gitToken = "";
  await step("git token (diagnostics)", async () => {
    const r = await c.diag(`repos/token?artifact=${aid}&scope=write`, "POST");
    if (r.status !== 200) throw new Error(`${r.status} ${short(r.json ?? r.text)}`);
    remote = r.json.remote;
    gitToken = r.json.token;
    return { detail: `${r.json.ms} ms to mint; remote ${remote}; token ${JSON.stringify(r.json.token_shape)}; expires ${r.json.expires_at}` };
  }, ["publish v2 (mcp)"]);

  await step("remote: advertisements", async () => {
    const out = [];
    for (const service of ["git-upload-pack", "git-receive-pack"]) {
      const res = await fetch(`${remote.replace(/\/$/, "")}/info/refs?service=${service}`, {
        headers: { authorization: `Bearer ${gitToken}`, "user-agent": "git/2.45.0 (smoke-remote)" },
        signal: AbortSignal.timeout(STEP_LIMIT),
      });
      const buf = Buffer.from(await res.arrayBuffer());
      if (res.status !== 200) throw new Error(`${service}: ${res.status} ${short(buf.toString(), 200)}`);
      const a = advertisement(buf);
      out.push(`${service}: ${a.refs.length} ref(s) [${a.capabilities.join(" ")}]`);
    }
    return { detail: out.join("; ") };
  }, ["git token (diagnostics)"]);

  const dir = join(git, "clone");
  const gitEnv = {
    ...process.env,
    GIT_TERMINAL_PROMPT: "0",
    GIT_AUTHOR_NAME: "Smoke",
    GIT_AUTHOR_EMAIL: "smoke@users.clax.dev",
    GIT_COMMITTER_NAME: "Smoke",
    GIT_COMMITTER_EMAIL: "smoke@users.clax.dev",
    GIT_CONFIG_NOSYSTEM: "1",
    HOME: git,
  };
  const header = () => `http.extraHeader=Authorization: Bearer ${gitToken}`;
  await step("git clone", async () => {
    const r = await run("git", ["-c", header(), "clone", "--quiet", remote, dir], { env: gitEnv });
    if (r.code !== 0) throw new Error(`git clone exited ${r.code}: ${short(r.err.replaceAll(gitToken, "<token>"), 600)}`);
    const html = readFileSync(join(dir, "index.html"), "utf8");
    if (!html.includes(`v2 ${RUN}`)) throw new Error(`the clone's index.html is not v2's: ${short(html, 200)}`);
    const log = await run("git", ["log", "--format=%h %s", "-n", "3"], { cwd: dir, env: gitEnv });
    return { detail: `index.html is v2's; log: ${log.out.trim().split("\n").join(" | ")}` };
  }, ["git token (diagnostics)"]);

  await step("artifacts: tree hashes (binding vs clone)", async () => {
    if (store.value !== "artifacts") skip(`REPO_STORE is ${store.value ?? "unknown"}`);
    const r = await c.diag(`artifacts/tree?repo=a-${aid}&ref=main`);
    if (r.status !== 200) throw new Error(`${r.status} ${short(r.text)}`);
    for (const s of r.json.steps ?? []) console.log(probeLine(s));
    const ls = await run("git", ["ls-tree", "-r", "-t", "HEAD"], { cwd: dir, env: gitEnv });
    const local = new Map(ls.out.trim().split("\n").map((l) => {
      const [meta, path] = l.split("\t");
      const [mode, , hash] = meta.split(" ");
      return [path, { mode, hash }];
    }));
    const head = await run("git", ["rev-parse", "HEAD^{tree}"], { cwd: dir, env: gitEnv });
    const bad = [];
    for (const e of r.json.entries ?? []) {
      if (e.path === "/") {
        if (e.hash !== head.out.trim()) bad.push(`root tree ${e.hash} vs the clone's ${head.out.trim()}`);
        continue;
      }
      const path = e.path.replace(/\/$/, "");
      const l = local.get(path);
      if (!l) bad.push(`${path}: not in the clone`);
      else if (l.hash !== e.hash) bad.push(`${path}: ${e.hash} vs the clone's ${l.hash}`);
      else if (e.mode && l.mode !== e.mode && `0${e.mode}` !== l.mode) bad.push(`${path}: mode ${e.mode} vs ${l.mode}`);
    }
    if (bad.length) throw new Error(bad.join("; "));
    return { detail: `${(r.json.entries ?? []).length} entries agree with the clone` };
  }, ["git clone"]);

  await step("artifacts: queue subscription for the repository", async () => {
    if (store.value !== "artifacts") skip(`REPO_STORE is ${store.value ?? "unknown"}`);
    if (flag("--no-subscribe")) skip("--no-subscribe");
    const wr = join(HERE, "node_modules/.bin/wrangler");
    const env = { ...process.env, WRANGLER_SEND_METRICS: "false", FORCE_COLOR: "0", ...(o.account ? { CLOUDFLARE_ACCOUNT_ID: o.account } : {}) };
    const name = `clax-smoke-${RUN}`;
    const r = await run(wr, ["queues", "subscription", "create", "clax-artifacts-events", "--source", "artifacts.repo", "--events", "pushed", "--source-namespace", "clax", "--source-repo-name", `a-${aid}`, "--name", name], { cwd: HERE, env });
    if (r.code !== 0) throw new Error(`wrangler queues subscription create: ${short((r.err || r.out).trim(), 500)} (logged in? \`npx wrangler login\`)`);
    const list = await run(wr, ["queues", "subscription", "list", "clax-artifacts-events", "--json"], { cwd: HERE, env });
    try {
      const j = JSON.parse(list.out.slice(list.out.indexOf("[")));
      subscription = j.find((s) => s.name === name)?.id ?? "";
    } catch {
      subscription = /\b([0-9a-f]{32}|[0-9a-f-]{36})\b/.exec(r.out)?.[1] ?? "";
    }
    return { detail: `subscription ${name}${subscription ? ` (${subscription})` : ""}` };
  }, ["git clone"]);

  await step("git push", async () => {
    writeFileSync(join(dir, "index.html"), PAGE(`pushed ${RUN}`));
    const commit = await run("git", ["commit", "--quiet", "-am", `Pushed by smoke-remote ${RUN}`], { cwd: dir, env: gitEnv });
    if (commit.code !== 0) throw new Error(`git commit: ${short(commit.err)}`);
    const r = await run("git", ["-c", header(), "push", "--porcelain", "origin", "HEAD:main"], { cwd: dir, env: gitEnv });
    if (r.code !== 0) throw new Error(`git push exited ${r.code}: ${short((r.err + r.out).replaceAll(gitToken, "<token>"), 600)}`);
    return { detail: short(r.out.trim().split("\n").filter((l) => l.includes("refs/")).join(" "), 200) };
  }, ["git clone"]);

  await step("the push becomes a version", async () => {
    const queue = store.value === "artifacts";
    const limit = queue ? Number(opt("--queue-wait") ?? 120) * 1000 : 20_000;
    const version = async () => (await c.owner(`/api/artifacts/${aid}`)).json.artifact.current_version;
    try {
      const v = await until(queue ? "the pushed event's version" : "the pushed version", async () => ((await version()) >= 3 ? await version() : false), limit);
      return { detail: `version ${v}${queue ? ", through the queue's pushed event" : ", recorded as the stand-in took the push"}` };
    } catch (e) {
      // Tell a lost event from a reconcile that fails.
      const rec = await c.diag(`reconcile?artifact=${aid}`, "POST").catch((x) => ({ json: { error: x.message } }));
      const after = await version().catch(() => "?");
      throw new Error(`${e.message}; a direct reconcile answered ${short(rec.json)} and the artifact is then at version ${after}` +
        (after >= 3 ? " (the reconcile works: the event never arrived; is the repository subscribed?)" : " (the reconcile itself does not record the push)"));
    }
  }, ["git push"]);

  await step("read the pushed version (mcp)", async () => {
    const r = await c.tool("read", { url_or_id: aid, version: 3 });
    if (!r.text.includes(`pushed ${RUN}`)) throw new Error(`version 3 reads ${short(r.text)}`);
    return { detail: "version 3 is the pushed commit" };
  }, ["the push becomes a version"]);

  await step("live: a comment on the WebSocket stream", async () => {
    const ws = new WebSocket(`${c.base.replace(/^http/, "ws")}/api/stream`, { headers: { ...c.credential(), origin: c.base } });
    const frames = [];
    let opened = false;
    let closed = null;
    ws.onopen = () => (opened = true);
    ws.onmessage = (m) => frames.push(String(m.data));
    ws.onclose = (e) => (closed = `${e.code} ${e.reason}`);
    try {
      await until("the socket opens", () => opened || (closed && Promise.reject(new Error(`closed ${closed}`))), 15_000);
      const ready = await until("the stream's ready", () => frames.find((f) => f.includes("event: ready")), 15_000);
      const data = JSON.parse(ready.split("\n").find((l) => l.startsWith("data:")).slice(5));
      await c.owner(`/api/stream/${data.stream}`, { method: "POST", body: JSON.stringify({ subscribe: [`artifact:${aid}`] }) });
      const form = new FormData();
      form.set("anchor", JSON.stringify({ kind: "element", selector: "main", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      form.set("body", `Live from smoke-remote ${RUN}`);
      form.set("version", "3");
      const t0 = performance.now();
      const posted = await c.owner(`/api/artifacts/${aid}/threads`, { method: "POST", body: form });
      const tid = posted.json?.thread?.id ?? posted.json?.id;
      await until("the comment's frame", () => frames.some((f) => (tid && f.includes(tid)) || f.includes(`smoke-remote ${RUN}`)), 15_000);
      return { detail: `thread ${tid} arrived ${Math.round(performance.now() - t0)} ms after it was posted (stream ${data.stream})` };
    } finally {
      ws.close();
    }
  }, ["publish v2 (mcp)", OWNER]);

  // Cleanup: never fails the smoke, but says what it left.
  console.log("\n-- cleanup");
  if (subscription) {
    const wr = join(HERE, "node_modules/.bin/wrangler");
    const r = await run(wr, ["queues", "subscription", "delete", "clax-artifacts-events", "--id", subscription, "--force"], { cwd: HERE, env: { ...process.env, WRANGLER_SEND_METRICS: "false" } });
    console.log(`     subscription ${subscription}: ${r.code === 0 ? "deleted" : `left (${short(r.err || r.out, 200)})`}`);
  }
  if (!flag("--keep") && c.agent) {
    for (const id of [...dbIds, fork, aid].filter(Boolean)) {
      const r = await c.tool("delete", { url_or_id: id }).then(() => "deleted", (e) => `left: ${short(e.message, 160)}`);
      console.log(`     artifact ${id}: ${r}`);
    }
  }
  if (agentTokenId) {
    const r = await c.owner(`/api/hosted/agent-tokens/${agentTokenId}`, { method: "DELETE" }).then(() => "revoked", (e) => `left: ${e.message}`);
    console.log(`     agent token: ${r}`);
  }
  rmSync(git, { recursive: true, force: true });
  if (madeOwner && !o.local) {
    const r = await c.owner("/api/hosted/signin-links", { method: "POST", body: "{}" }).catch(() => null);
    if (r?.json?.url) console.log(`\n   The smoke made the owner (${o.ownerName}). Sign your browser in as the owner, within ten minutes, once:\n     ${r.json.url}`);
  }
}

async function local(o) {
  await build();
  const root = join(HERE, "target/smoke-remote");
  pruneRuns(root, 3);
  const dir = join(root, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const port = await freePort();
  const secret = `smoke secret ${RUN}`;
  const token = `smoke-owner-${RUN}-${Math.random().toString(36).slice(2)}${Math.random().toString(36).slice(2)}`;
  const remoteArtifacts = flag("--remote-artifacts");
  const config = writeConfig(dir, {
    HOSTED: "1",
    PUBLIC_ORIGIN: `http://localhost:${port}`,
    // Node's fetch resolves `<ID>.localhost` to the loopback address.
    SITE_DOMAIN: "localhost",
    REPO_STORE: remoteArtifacts ? "artifacts" : "memory",
    GIT_SMART_HTTP: remoteArtifacts ? "0" : "1",
    SETUP_SECRET: secret,
    CLAX_TOKEN: token,
    CLAX_DIAG_ROUTES: "1",
    CLAX_BUILD: `smoke-local-${RUN}`,
  });
  if (remoteArtifacts) {
    // The real Artifacts service from a local Worker (needs the login).
    const c = JSON.parse(readFileSync(config, "utf8"));
    for (const a of c.artifacts ?? []) a.remote = true;
    writeFileSync(config, JSON.stringify(c, null, 2));
  }
  const w = await start(dir, config, 1, port);
  console.log(`local: wrangler dev --local on ${w.base} (log ${w.log})`);
  const base = `http://localhost:${port}`;
  if (flag("--setup-done")) {
    // As after an earlier deploy: someone else did the setup, whose session
    // the smoke never sees.
    const r = await fetch(`${base}/api/hosted/setup`, {
      method: "POST",
      headers: { "content-type": "application/json", origin: base },
      body: JSON.stringify({ secret, name: "Earlier owner" }),
    }).catch((e) => ({ status: e.message }));
    if (r.status !== 200 && r.status !== 201) {
      await w.stop();
      throw new Error(`--setup-done: the setup answered ${r.status}`);
    }
    console.log("local: setup done beforehand (--setup-done)");
  }
  return { base, secret, token: flag("--without-token") ? "" : token, stop: w.stop, work: dir };
}

async function main() {
  const o = {
    ownerName: opt("--name") ?? "Owner",
    session: opt("--session") ?? process.env.CLAX_SESSION,
    local: flag("--local"),
    account: opt("--account") ?? process.env.CLOUDFLARE_ACCOUNT_ID,
    siteDomain: opt("--site-domain"),
  };
  let base = opt("--url");
  let token = opt("--token") ?? process.env.CLAX_TOKEN;
  o.secret = opt("--setup-secret") ?? process.env.CLAX_SETUP_SECRET;
  let stop = async () => {};
  const record = opt("--deploy");
  if (record) {
    const r = JSON.parse(readFileSync(record, "utf8"));
    base ??= r.public_origin;
    token ??= r.owner_token;
    o.secret ??= r.setup_secret;
    o.account ??= r.account;
    o.siteDomain ??= r.site_domain;
    o.onSetup = () => writeFileSync(record, JSON.stringify({ ...r, setup_done: true }, null, 2) + "\n", { mode: 0o600 });
  }
  if (o.local) {
    const l = await local(o);
    base = l.base;
    token = l.token;
    o.secret = l.secret;
    o.work = l.work;
    o.siteDomain = "localhost";
    stop = l.stop;
  }
  if (!base) throw new Error("give --deploy <record>, --url <public origin> or --local (see the top of smoke-remote.mjs)");
  if (!o.work) {
    o.work = join(HERE, "target/smoke-remote");
    mkdirSync(o.work, { recursive: true });
  }
  const c = new Client(base);
  c.token = token ?? "";
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void stop().then(() => process.exit(130)));
  console.log(`smoke-remote ${RUN} against ${base}${c.token ? "" : " (no owner token: the diagnostics' steps skip)"}\n`);
  const t0 = performance.now();
  try {
    await smoke(c, o);
  } finally {
    await stop();
  }
  const counts = { ok: 0, fail: 0, skip: 0 };
  for (const r of results) counts[r.status] += 1;
  console.log(`\n${counts.ok} ok, ${counts.fail} failed, ${counts.skip} skipped in ${Math.round((performance.now() - t0) / 1000)} s`);
  for (const r of results.filter((r) => r.status === "fail")) console.log(`  FAIL ${r.name}: ${short(r.detail, 400)}`);
  if (flag("--json")) console.log(JSON.stringify({ run: RUN, base, results }, null, 2));
  if (!o.local && results.some((r) => r.name === "diagnostics: env" && r.status === "ok")) {
    const bar = "!".repeat(76);
    console.log(`\n${bar}\n!!  ${base} STILL SERVES THE DIAGNOSTICS ROUTES (/_diag/, CLAX_DIAG_ROUTES=1).\n!!  Deploy again without them now: node deploy.mjs (without --diag).\n${bar}`);
  }
  if (counts.fail) process.exit(1);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
