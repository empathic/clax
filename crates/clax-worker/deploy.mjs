// Deploys hosted Clax to the Cloudflare account `wrangler login` signed in
// to, in one command (README.md, "Deploy to Cloudflare"):
//
//   node deploy.mjs --zone example.com            # clax.example.com, clax-content.example.com
//   node deploy.mjs --public clax.example.com --content clax-content.example.com
//   node deploy.mjs --dry-run ...                 # no account changes; ends at `wrangler deploy --dry-run`
//   node deploy.mjs --store memory ...            # versions in the RepoDO stand-in, no Artifacts
//
// In order, it
//
// 1. checks `wrangler whoami` (without a login, a dry run skips every
//    account call and says so; a deploy stops);
// 2. takes the two hostnames (asked for on a terminal when not given);
// 3. creates what is missing, each step idempotent: the R2 bucket, and with
//    `--store artifacts` the Queue, and the Artifacts namespace (made by
//    creating and deleting a probe repository, which also proves the
//    account has Artifacts);
// 4. generates `SETUP_SECRET` and `CLAX_TOKEN` when the Worker has neither,
//    uploads them with the deploy (`wrangler deploy --secrets-file`, so the
//    first version already has them), prints them once and keeps them in
//    `.deploy/<public host>.json` (0600, git-ignored) for
//    `smoke-remote.mjs`;
// 5. writes `wrangler.deploy.jsonc`: `wrangler.jsonc` with `HOSTED=1`, the
//    two origins, the store, both hosts as custom domains, `workers_dev`
//    and preview URLs off, Workers Logs on, and the diagnostics routes off
//    (`--diag` turns them on, for `smoke-remote.mjs`'s probe, git token and
//    reconcile steps, with a reminder before and after the deploy to deploy
//    again without it);
// 6. builds the web UI and then the Worker (release), unless `--no-build`;
// 7. runs `wrangler deploy` (with `--dry-run`, `wrangler deploy --dry-run`)
//    and reports the upload size against the Workers Paid limit;
// 8. waits for `/healthz` on the public host, naming what it is waiting
//    on (DNS, the certificate, the Worker), and prints the setup URL and
//    the next steps.
//
// Options: --public <host>, --content <host>, --zone <zone>, --store
// artifacts|memory (default artifacts), --name <worker> (default clax),
// --account <ID> (else CLOUDFLARE_ACCOUNT_ID, else the login's one account),
// --rotate-secrets, --diag (--no-diag, the default, keeps them off),
// --no-build, --wait <s> (default 600),
// --dry-run, --yes (no questions; fails on what it would ask).

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { join } from "node:path";
import { HERE, baseConfig } from "./harness.mjs";

const WRANGLER = join(HERE, "node_modules/.bin/wrangler");
const DEPLOY_CONFIG = join(HERE, "wrangler.deploy.jsonc");
const RECORDS = join(HERE, ".deploy");
const BUCKET = "clax-assets";
const QUEUE = "clax-artifacts-events";
const NAMESPACE = "clax";
// Workers Paid: 10 MB after compression; Free: 3 MB.
const PAID_LIMIT_KIB = 10 * 1024;

function parseArgs(argv) {
  const o = { store: "artifacts", name: "clax", wait: 600, diag: false, build: true, dryRun: false, yes: false, rotate: false };
  const takes = new Set(["--public", "--content", "--zone", "--store", "--name", "--account", "--wait"]);
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const [flag, inline] = a.includes("=") ? a.split(/=(.*)/s) : [a, undefined];
    const value = () => inline ?? argv[++i];
    if (takes.has(flag)) {
      const v = value();
      if (v === undefined) throw new Error(`${flag} takes a value`);
      o[flag.slice(2)] = flag === "--wait" ? Number(v) : v;
    } else if (flag === "--dry-run") o.dryRun = true;
    else if (flag === "--yes" || flag === "-y") o.yes = true;
    else if (flag === "--diag") o.diag = true;
    else if (flag === "--no-diag") o.diag = false;
    else if (flag === "--no-build") o.build = false;
    else if (flag === "--rotate-secrets") o.rotate = true;
    else if (flag === "--help" || flag === "-h") o.help = true;
    else throw new Error(`unknown option ${a} (see the top of deploy.mjs)`);
  }
  if (!["artifacts", "memory"].includes(o.store)) throw new Error(`--store is artifacts or memory, not ${o.store}`);
  return o;
}

const step = (s) => console.log(`\n== ${s}`);
const info = (s) => console.log(`   ${s}`);

/** Runs wrangler with `args`; resolves {code, out, err}. `show` streams its
 * output too. */
function wrangler(args, { account, show = false, input } = {}) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, WRANGLER_SEND_METRICS: "false", FORCE_COLOR: "0" };
    if (account) env.CLOUDFLARE_ACCOUNT_ID = account;
    const child = spawn(WRANGLER, args, { cwd: HERE, env, stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"] });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => {
      out += d;
      if (show) process.stdout.write(d);
    });
    child.stderr.on("data", (d) => {
      err += d;
      if (show) process.stderr.write(d);
    });
    if (input !== undefined) child.stdin.end(input);
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, out, err }));
  });
}

function run(cmd, args, env = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd: HERE, stdio: ["ignore", "inherit", "inherit"], env: { ...process.env, ...env } });
    child.on("error", reject);
    child.on("close", (code) => (code === 0 ? resolve() : reject(new Error(`${cmd} ${args.join(" ")} exited ${code}`))));
  });
}

const firstLine = (s) => s.split("\n").map((l) => l.trim()).find((l) => l && !l.startsWith("⛅️") && !/^─+$/.test(l)) ?? "";
const lastError = (r) => {
  const lines = `${r.err}\n${r.out}`.split("\n").map((l) => l.trim()).filter(Boolean);
  return lines.find((l) => /^✘|ERROR|error/i.test(l)) ?? lines.at(-1) ?? `exit ${r.code}`;
};

/** The JSON value wrangler printed after any banner. */
function jsonIn(text) {
  const at = [text.indexOf("["), text.indexOf("{")].filter((i) => i >= 0);
  return JSON.parse(text.slice(Math.min(...at)));
}

function hostOf(s) {
  const h = s.replace(/^https?:\/\//, "").replace(/\/.*$/, "").toLowerCase();
  if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(h)) throw new Error(`${s} is not a hostname`);
  return h;
}

async function ask(o, question, fallback) {
  if (o.yes || !process.stdin.isTTY) {
    if (fallback !== undefined) return fallback;
    throw new Error(`${question}: give it as an option (see the top of deploy.mjs)`);
  }
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    const a = (await rl.question(`${question}${fallback ? ` [${fallback}]` : ""}: `)).trim();
    return a || fallback;
  } finally {
    rl.close();
  }
}

async function hostnames(o) {
  let zone = o.zone ? hostOf(o.zone) : undefined;
  if (!o.public && !zone) {
    zone = hostOf(await ask(o, "The zone (a domain on this Cloudflare account) to serve Clax under, e.g. example.com"));
  }
  const pub = hostOf(o.public ?? (await ask(o, "Public hostname", `clax.${zone}`)));
  const content = hostOf(o.content ?? (await ask(o, "Content hostname (a host of its own)", `clax-content.${zone ?? pub.split(".").slice(1).join(".")}`)));
  if (pub === content) throw new Error("the content hostname must differ from the public one");
  if (content.endsWith(`.${pub}`)) {
    info(`note: ${content} is under ${pub}; Universal SSL covers one level of subdomain, the custom domain gets its own certificate`);
  }
  return { pub, content };
}

/** The login and the account to use; `null` when not logged in. */
async function login(o) {
  const r = await wrangler(["whoami", "--json"]);
  let who;
  try {
    who = JSON.parse(r.out.trim().split("\n").filter((l) => l.startsWith("{")).pop() ?? "{}");
  } catch {
    who = {};
  }
  if (!who.loggedIn) return null;
  const accounts = who.accounts ?? [];
  const wanted = o.account ?? process.env.CLOUDFLARE_ACCOUNT_ID;
  let account = wanted ? accounts.find((a) => a.id === wanted) ?? { id: wanted, name: "(given)" } : undefined;
  if (!account && accounts.length === 1) account = accounts[0];
  if (!account) {
    const list = accounts.map((a) => `${a.id}  ${a.name}`).join("\n     ");
    throw new Error(`the login has ${accounts.length} accounts; pick one with --account <ID>:\n     ${list}`);
  }
  return { email: who.email ?? "(unknown)", account };
}

/** Creates with `args` unless it exists: `exists` matches the refusal. */
async function ensure(what, args, exists, account) {
  const r = await wrangler(args, { account });
  const text = `${r.out}\n${r.err}`;
  if (r.code === 0) return info(`${what}: created`);
  if (exists.test(text)) return info(`${what}: exists`);
  throw new Error(`${what}: ${lastError(r)}\n   (wrangler ${args.join(" ")})`);
}

async function artifactsNamespace(account, dryRun) {
  const list = await wrangler(["artifacts", "namespaces", "list", "--json"], { account });
  if (list.code !== 0) {
    throw new Error(
      `Artifacts is not reachable on this account: ${lastError(list)}\n` +
        "   Is the Artifacts beta enabled for it (dashboard > Storage & databases > Artifacts)? " +
        "Deploy with --store memory to run without it.",
    );
  }
  let names = [];
  try {
    const j = jsonIn(list.out);
    const arr = Array.isArray(j) ? j : j.namespaces ?? j.result ?? [];
    names = arr.map((n) => (typeof n === "string" ? n : n.name));
  } catch {
    names = list.out.includes(`"${NAMESPACE}"`) ? [NAMESPACE] : [];
  }
  if (names.includes(NAMESPACE)) return info(`Artifacts namespace ${NAMESPACE}: exists`);
  if (dryRun) return info(`Artifacts namespace ${NAMESPACE}: missing; a deploy makes it (creating and deleting a probe repository)`);
  // A namespace is made by its first repository.
  const probe = `clax-deploy-probe-${Date.now()}`;
  const made = await wrangler(["artifacts", "repos", "create", probe, "--namespace", NAMESPACE, "--default-branch", "main", "--json"], { account });
  if (made.code !== 0) throw new Error(`creating the namespace's probe repository failed: ${lastError(made)}`);
  const gone = await wrangler(["artifacts", "repos", "delete", probe, "--namespace", NAMESPACE, "--force"], { account });
  info(`Artifacts namespace ${NAMESPACE}: created${gone.code === 0 ? "" : ` (deleting the probe repository ${probe} failed: ${lastError(gone)})`}`);
}

/** The secret names the Worker has; empty when it does not exist yet. */
async function secretNames(account) {
  const r = await wrangler(["secret", "list", "--config", DEPLOY_CONFIG, "--format", "json"], { account });
  if (r.code !== 0) return [];
  try {
    return jsonIn(r.out).map((s) => s.name);
  } catch {
    return [];
  }
}

function deployConfig(o, pub, content, account) {
  const c = baseConfig();
  delete c.$schema;
  // deploy.mjs builds once, before wrangler runs.
  delete c.build;
  c.name = o.name;
  if (account) c.account_id = account;
  c.workers_dev = false;
  c.preview_urls = false;
  c.routes = [
    { pattern: pub, custom_domain: true },
    { pattern: content, custom_domain: true },
  ];
  c.observability = { enabled: true, head_sampling_rate: 1 };
  c.vars = {
    ...c.vars,
    HOSTED: "1",
    PUBLIC_ORIGIN: `https://${pub}`,
    CONTENT_ORIGIN: `https://${content}`,
    REPO_STORE: o.store,
    GIT_SMART_HTTP: o.store === "memory" ? "1" : "0",
    CLAX_TEST_ROUTES: "0",
    REPO_DEV_ROUTES: "0",
    CLAX_DIAG_ROUTES: o.diag ? "1" : "0",
  };
  if (o.store === "memory") {
    // No Artifacts: neither its binding nor its events' queue.
    delete c.artifacts;
    delete c.queues;
  }
  const header =
    "// Generated by deploy.mjs from wrangler.jsonc; edit that, or deploy.mjs's\n" +
    "// options, and run deploy.mjs again. Not checked in.\n";
  writeFileSync(DEPLOY_CONFIG, header + JSON.stringify(c, null, 2) + "\n");
  return c;
}

function recordPath(pub) {
  return join(RECORDS, `${pub}.json`);
}

function readRecord(pub) {
  try {
    return JSON.parse(readFileSync(recordPath(pub), "utf8"));
  } catch {
    return {};
  }
}

function writeRecord(pub, record) {
  mkdirSync(RECORDS, { recursive: true, mode: 0o700 });
  writeFileSync(recordPath(pub), JSON.stringify(record, null, 2) + "\n", { mode: 0o600 });
  chmodSync(recordPath(pub), 0o600);
}

/** Whether the built Worker carries web/dist's current index.html. */
function embedsWebUi() {
  try {
    const wasm = readFileSync(join(HERE, "build/index_bg.wasm"));
    return wasm.indexOf(readFileSync(join(HERE, "../../web/dist/index.html"))) >= 0;
  } catch {
    return false;
  }
}

/** The upload size `wrangler deploy` printed: {total, gzip} in KiB. */
function uploadSize(text) {
  const m = /Total Upload:\s*([\d.]+)\s*KiB\s*\/\s*gzip:\s*([\d.]+)\s*KiB/.exec(text);
  return m ? { total: Number(m[1]), gzip: Number(m[2]) } : null;
}

/** What a failed fetch of the public host says about where the deploy is. */
function waitingOn(e) {
  const code = e?.cause?.code ?? e?.code ?? "";
  const msg = `${e?.cause?.message ?? e?.message ?? e}`;
  if (code === "ENOTFOUND" || code === "EAI_AGAIN") return "DNS: the hostname does not resolve yet";
  if (/CERT|SSL|TLS|ERR_SSL|EPROTO|alert/i.test(code + msg)) return `the certificate is not issued yet (${code || msg})`;
  if (code === "ECONNRESET" || code === "ECONNREFUSED") return `the edge refused the connection (${code}); usually the certificate`;
  if (/abort|timeout/i.test(msg)) return "no answer within 10 s";
  return `${code} ${msg}`.trim();
}

async function waitHealthy(pub, seconds) {
  const url = `https://${pub}/healthz`;
  const end = Date.now() + seconds * 1000;
  const started = Date.now();
  let last = "";
  for (;;) {
    let state;
    let done = null;
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(10_000), redirect: "manual" });
      const body = await res.text();
      if (res.ok) done = body;
      else if (res.status === 500 && body.includes("could not start")) throw new Error(`the instance did not start: ${body.slice(0, 400)}`);
      else state = `HTTP ${res.status}: ${body.slice(0, 160).replace(/\s+/g, " ")}${res.status === 404 || res.status === 522 || res.status === 530 ? " (the custom domain is not routed to the Worker yet)" : ""}`;
    } catch (e) {
      if (`${e.message}`.startsWith("the instance did not start")) throw e;
      state = waitingOn(e);
    }
    if (done !== null) {
      info(`healthy after ${Math.round((Date.now() - started) / 1000)} s: ${done.slice(0, 200)}`);
      return true;
    }
    if (state !== last) {
      info(`${Math.round((Date.now() - started) / 1000)} s: waiting on ${state}`);
      last = state;
    }
    if (Date.now() > end) return false;
    await new Promise((r) => setTimeout(r, 3000));
  }
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  if (o.help) {
    console.log(readFileSync(new URL(import.meta.url), "utf8").split("\n").filter((l) => l.startsWith("//")).map((l) => l.slice(3)).join("\n"));
    return;
  }
  if (!existsSync(WRANGLER)) throw new Error(`no wrangler at ${WRANGLER}: run npm install in ${HERE}`);
  const version = (await wrangler(["--version"])).out.trim();
  console.log(`hosted Clax deploy${o.dryRun ? " (dry run: no account changes)" : ""}; wrangler ${version}; store ${o.store}`);

  step("Login");
  const who = await login(o);
  if (!who) {
    if (!o.dryRun) throw new Error("wrangler is not logged in: run `npx wrangler login` in this directory, then this again");
    info("not logged in: the account calls (bucket, queue, Artifacts, secrets) are skipped, each named below");
  } else {
    info(`${who.email}, account ${who.account.name} (${who.account.id})`);
  }
  const account = who?.account.id;

  step("Hostnames");
  const { pub, content } = await hostnames(o);
  info(`public  https://${pub}`);
  info(`content https://${content}`);

  step("Deploy config");
  const config = deployConfig(o, pub, content, account);
  info(`wrote ${DEPLOY_CONFIG} (worker ${config.name}, REPO_STORE=${o.store}, diagnostics ${o.diag ? "on" : "off"})`);
  if (o.diag) diagReminder(pub);

  step("Resources");
  if (!who) {
    info(`skipped (no login): wrangler r2 bucket create ${BUCKET}`);
    if (o.store === "artifacts") {
      info(`skipped (no login): wrangler queues create ${QUEUE}`);
      info(`skipped (no login): wrangler artifacts namespaces list (the namespace ${NAMESPACE}; made by its first repository)`);
    }
  } else if (o.dryRun) {
    info(`would make, if missing: R2 bucket ${BUCKET}${o.store === "artifacts" ? `, queue ${QUEUE}` : ""}`);
    if (o.store === "artifacts") await artifactsNamespace(account, true);
  } else {
    await ensure(`R2 bucket ${BUCKET}`, ["r2", "bucket", "create", BUCKET], /already exists|10004/i, account);
    if (o.store === "artifacts") {
      await ensure(`Queue ${QUEUE}`, ["queues", "create", QUEUE], /already (exists|taken)|11009/i, account);
      await artifactsNamespace(account, false);
      info(
        "Artifacts sends `pushed` events only to a subscription per repository (source artifacts.repo); " +
          "none is made here, so a git push becomes a version at the artifact's next publish " +
          "(smoke-remote.mjs subscribes its own repository to check the queue path)",
      );
    }
  }

  step("Secrets");
  const record = readRecord(pub);
  const have = who ? await secretNames(account) : [];
  let secrets = null;
  const needed = ["SETUP_SECRET", "CLAX_TOKEN"].filter((n) => o.rotate || !have.includes(n));
  if (!who) info("skipped (no login): wrangler secret list; a deploy generates SETUP_SECRET and CLAX_TOKEN when the Worker has neither");
  else if (needed.length === 0) {
    info(`the Worker has SETUP_SECRET and CLAX_TOKEN; kept${record.setup_secret ? ` (in ${recordPath(pub)})` : " (not shown: --rotate-secrets makes new ones)"}`);
  } else if (o.dryRun) info(`would generate ${needed.join(" and ")} and upload them with the deploy`);
  else {
    secrets = {};
    if (needed.includes("SETUP_SECRET")) secrets.SETUP_SECRET = randomBytes(24).toString("base64url");
    if (needed.includes("CLAX_TOKEN")) secrets.CLAX_TOKEN = randomBytes(32).toString("hex");
    info(`generated ${needed.join(" and ")}; uploaded with the deploy`);
  }

  const buildWorker = () => run(join(HERE, "build.sh"), [], { CARGO_INCREMENTAL: process.env.CARGO_INCREMENTAL ?? "0" });
  if (o.build) {
    step("Build: web UI");
    await run(join(HERE, "../../scripts/build-web.sh"), []);
    step("Build: Worker (release)");
    await buildWorker();
  } else {
    step("Build: skipped (--no-build)");
  }
  if (!existsSync(join(HERE, config.main))) throw new Error(`no ${config.main}: build first (drop --no-build)`);
  // The server embeds web/dist when clax-server compiles (rust-embed), and
  // cargo does not see web/dist change: a Worker built before the web UI
  // serves "ui_not_built" for /setup and every other shell page.
  if (!embedsWebUi()) {
    if (!o.build) throw new Error(`build/index_bg.wasm does not embed web/dist/index.html: drop --no-build`);
    step("Build: the Worker embedded an older web UI; rebuilding clax-server");
    await run("cargo", ["clean", "-q", "--release", "--target", "wasm32-unknown-unknown", "-p", "clax-server"]);
    await buildWorker();
    if (!embedsWebUi()) throw new Error("build/index_bg.wasm still does not embed web/dist/index.html");
  }
  info("the Worker embeds the web UI in web/dist");

  step(o.dryRun ? "wrangler deploy --dry-run" : "wrangler deploy");
  const args = ["deploy", "--config", DEPLOY_CONFIG];
  let secretsFile;
  if (o.dryRun) args.push("--dry-run", "--outdir", join(HERE, "build/dry-run"));
  if (secrets) {
    secretsFile = join(RECORDS, `.secrets-${process.pid}.json`);
    mkdirSync(RECORDS, { recursive: true, mode: 0o700 });
    writeFileSync(secretsFile, JSON.stringify(secrets), { mode: 0o600 });
    args.push("--secrets-file", secretsFile);
  }
  let r;
  try {
    r = await wrangler(args, { account, show: true });
  } finally {
    if (secretsFile) rmSync(secretsFile, { force: true });
  }
  const size = uploadSize(r.out + r.err);
  if (r.code !== 0) throw new Error(`wrangler deploy failed: ${lastError(r)}`);
  if (size) {
    const pct = Math.round((size.gzip / PAID_LIMIT_KIB) * 100);
    info(`upload ${(size.total / 1024).toFixed(2)} MiB, ${(size.gzip / 1024).toFixed(2)} MiB gzipped: ${pct}% of Workers Paid's 10 MiB`);
  }

  if (o.dryRun) {
    step("Dry run done");
    info(`config: ${DEPLOY_CONFIG}; bundle: ${join(HERE, "build/dry-run")}`);
    info(who ? "deploy with the same options, without --dry-run" : "after `npx wrangler login`, run the same command without --dry-run");
    return;
  }

  const next = {
    ...record,
    public_origin: `https://${pub}`,
    content_origin: `https://${content}`,
    worker: o.name,
    account,
    store: o.store,
    diag: o.diag,
    deployed_at: new Date().toISOString(),
    ...(secrets?.SETUP_SECRET ? { setup_secret: secrets.SETUP_SECRET, setup_done: false } : {}),
    ...(secrets?.CLAX_TOKEN ? { owner_token: secrets.CLAX_TOKEN } : {}),
  };
  writeRecord(pub, next);

  step(`Waiting for https://${pub}/healthz (custom domains take a few minutes the first time)`);
  const healthy = await waitHealthy(pub, o.wait);
  if (!healthy) {
    info(`not healthy within ${o.wait} s. Check: the zone of ${pub} is on account ${account}; ` +
      `Workers > ${o.name} > Settings > Domains & Routes lists both hosts; \`npx wrangler tail ${o.name}\``);
  }
  const contentRes = await fetch(`https://${content}/`, { signal: AbortSignal.timeout(10_000) }).then(
    (res) => `HTTP ${res.status}`,
    (e) => waitingOn(e),
  );
  info(`content host https://${content}/: ${contentRes} (404 from the Worker is right: it serves no gallery)`);

  step("Done");
  if (secrets) {
    console.log("\n   Shown once (also in the record below, 0600; delete it once set up):");
    if (secrets.SETUP_SECRET) console.log(`     SETUP_SECRET  ${secrets.SETUP_SECRET}`);
    if (secrets.CLAX_TOKEN) console.log(`     CLAX_TOKEN    ${secrets.CLAX_TOKEN}`);
  }
  console.log(`
   Record:  ${recordPath(pub)}
   Setup:   https://${pub}/setup  (enter SETUP_SECRET and your name; this browser becomes the owner)

   Next:
   1. Smoke the platform first (it can do the setup itself if you have not):
        node smoke-remote.mjs --deploy ${recordPath(pub)}
   2. Open https://${pub}/setup in your browser, or, if the smoke did the setup,
      sign in with the link it printed.
   3. Logs: npx wrangler tail ${o.name}${o.diag ? `
   4. Once the smoke is green, turn the diagnostics off: the same command without --diag` : ""}`);
  if (o.diag) diagReminder(pub);
}

/** The reminder a deploy with the diagnostics routes on prints. */
function diagReminder(pub) {
  const bar = "!".repeat(76);
  console.log(`
${bar}
!!  THE DIAGNOSTICS ROUTES ARE ON at https://${pub}/_diag/ (CLAX_DIAG_ROUTES=1).
!!  With the owner token they create and delete Artifacts repositories and mint
!!  write tokens for any artifact. Once smoke-remote.mjs is green, deploy again
!!  without --diag:  node deploy.mjs --public ${pub} ...
${bar}`);
}

main().catch((e) => {
  console.error(`\ndeploy failed: ${e instanceof Error ? e.message : e}`);
  process.exit(1);
});
