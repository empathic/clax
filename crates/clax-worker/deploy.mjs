// Deploys hosted Clax to the Cloudflare account `wrangler login` signed in
// to, in one command (README.md, "Deploy to Cloudflare"):
//
//   node deploy.mjs --zone example.com --site-domain example.run   # example.com, sites at <ID>.example.run
//   node deploy.mjs --public clax.example.com --site-domain example.run
//   node deploy.mjs --dry-run ...                 # no account changes; ends at `wrangler deploy --dry-run`
//   node deploy.mjs --dry-run --config-only ...   # only writes the deploy config: no wrangler, no login, no build
//   node deploy.mjs --store memory ...            # versions in the RepoDO stand-in, no Artifacts
//
// In order, it
//
// 1. checks `wrangler whoami` (without a login, a dry run skips every
//    account call and says so; a deploy stops);
// 2. takes the public hostname and the site domain (asked for on a
//    terminal when not given; the site domain, a zone of its own on the
//    account, serves each artifact at `<artifact ID>.<site domain>`; a later
//    deploy takes it from the record);
// 3. creates what is missing, each step idempotent: the R2 bucket, and with
//    `--store artifacts` the Queue, and the Artifacts namespace (made by
//    creating and deleting a probe repository, which also proves the
//    account has Artifacts);
// 4. generates `SETUP_SECRET` and `CLAX_TOKEN` when the Worker has neither,
//    and `CLAX_KEK` (ClaxBot's key-encryption key) when it has none, taking
//    the record's when it has one: never rotated, not even by
//    --rotate-secrets, since a new one opens no key the old one sealed;
//    uploads them with the deploy (`wrangler deploy --secrets-file`, so the
//    first version already has them), prints them once and keeps them in
//    `~/.config/clax/deploy/<public host>.json` (0600; $CLAX_DEPLOY_HOME or $XDG_CONFIG_HOME/clax/deploy when set) for
//    `smoke-remote.mjs`;
// 5. writes `target/deploy/wrangler.deploy.jsonc` (or `--out <path>`):
//    `wrangler.jsonc` with `HOSTED=1`, the public origin, `SITE_DOMAIN`, the
//    store, the public host as a custom domain and the route
//    `*.<site domain>/*` on the site domain's zone, `workers_dev`
//    and preview URLs off, Workers Logs on, and the diagnostics routes off
//    (`--diag` turns them on, for `smoke-remote.mjs`'s probe, git token and
//    reconcile steps, with a reminder before and after the deploy to deploy
//    again without it);
// 6. builds the web UI and then the Worker (release), unless `--no-build`,
//    which deploys what an earlier run built and refuses a Worker that was
//    not built from the checked-out commit;
// 7. runs `wrangler deploy` (with `--dry-run`, `wrangler deploy --dry-run`)
//    and reports the upload size against the Workers Paid limit; when it
//    fails only because the triggers (custom domains) were left partially
//    updated, it runs the same deploy once more, without rebuilding; the
//    record keeps the Worker version it names (`version_id`);
// 8. waits for `/healthz` on the public host to report this upload's build
//    (`CLAX_BUILD`, a build ID made per deploy from the commit, the time and
//    a random part; the instance from before the deploy answers 200 too),
//    naming what it is waiting on (DNS, the certificate, the Worker, the
//    previous build), then checks
//    `https://<random ID>.<site domain>/_clax/healthz`; when that fails it
//    prints the manual steps a wildcard needs (the zone on the account, a
//    proxied `*` DNS record, a certificate covering `*.<site domain>`), each
//    with what it found; then, on a first deploy (it generated
//    SETUP_SECRET or CLAX_TOKEN), the secrets once, the setup URL and the
//    next steps.
//
// `--config-only` stops after step 5 and runs neither wrangler nor a build:
// it needs no login and changes nothing but the config file.
//
// Options: --public <host>, --site-domain <domain>, --zone <zone>, --store
// artifacts|memory (default artifacts), --name <worker> (default clax),
// --account <ID> (else CLOUDFLARE_ACCOUNT_ID, else the login's one account),
// --rotate-secrets, --diag (--no-diag, the default, keeps them off),
// --no-build, --wait <s> (default 600), --out <path>, --config-only,
// --dry-run, --yes (no questions; fails on what it would ask),
// --retired-content-origin <https origin> (the content host an instance
// served assets on before site hosts; else the record's). --content is
// gone: artifacts are served on the site domain's hosts.
//
// --ci: a deploy from a CI runner (scripts/cloudflare-setup.sh --ci): it
// asks nothing, and fails naming each setting not given as a flag (the
// public host, the site domain, --store, the legal documents, the account
// or CLOUDFLARE_ACCOUNT_ID; --owner-github with --github-client-id); it
// refuses --rotate-secrets, --diag and a Worker without SETUP_SECRET,
// CLAX_TOKEN or CLAX_KEK (first-time setup runs from a laptop), keeps the
// Worker's secrets, notes that it keeps no CLAX_KEK backup, and fails when
// the health wait or the site host check does.
//
// --production: this is the production instance. A deploy refuses unless
// the checkout is clean and its HEAD is origin's prod tip (`git ls-remote
// origin refs/heads/prod`, checked again before the upload), and unless it
// runs in GitHub Actions (GITHUB_ACTIONS=true) or is given --from-laptop,
// which lifts only that last condition. A dry run says what a deploy would
// refuse. Production deploys run from GitHub Actions on pushes to prod
// (docs/prod.md).
//
// GitHub sign-in: --github-client-id <ID> (a GitHub OAuth App's) and
// --owner-github <login> (whoever signs in as that GitHub account is the
// owner), both kept in the record so later deploys keep them; the client
// secret is read from GITHUB_CLIENT_SECRET, else asked on a terminal, and
// uploaded with the deploy when given or when the Worker has none yet.
// The OAuth App's callback URL is https://<public host>/auth/github/callback.
//
// Terms of Service and Privacy Policy: --legal bundled serves the documents
// in docs/legal at /terms and /privacy (the service Empathic operates);
// --terms-url <URL> --privacy-url <URL> --legal-version <v> links a
// deployment's own instead. Kept in the record, like GitHub's; without
// either the instance shows none. Changing the version asks every person
// to agree again.

import { spawn } from "node:child_process";
import { randomBytes, randomInt } from "node:crypto";
import { promises as dns } from "node:dns";
import { BlockList, isIPv6 } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { fileURLToPath } from "node:url";
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { createInterface } from "node:readline/promises";
import { dirname, join, resolve } from "node:path";
import { buildId, builtFromRefusal, checkCiOptions, ciSecretsRefusal, doneText, healthOf, partialTriggerUpdate, prodTipOf, productionGate, versionIdOf } from "./deploy-lib.mjs";
import { HERE, baseConfig, embedsWebUi } from "./harness.mjs";

const WRANGLER = join(HERE, "node_modules/.bin/wrangler");
// Nothing generated lives under crates/: the deploy config goes in the
// repository's target/deploy/, and the records (they hold secrets, and must
// outlive `cargo clean`) in the user's config directory.
const ROOT = resolve(HERE, "../..");
const DEPLOY_CONFIG = join(ROOT, "target/deploy/wrangler.deploy.jsonc");
export const RECORDS =
  process.env.CLAX_DEPLOY_HOME || join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "clax/deploy");
// Records an earlier deploy.mjs kept in crates/clax-worker/.deploy move here once.
function moveOldRecords() {
  const old = join(HERE, ".deploy");
  if (!existsSync(old)) return;
  mkdirSync(RECORDS, { recursive: true, mode: 0o700 });
  for (const f of readdirSync(old)) {
    if (!existsSync(join(RECORDS, f))) renameSync(join(old, f), join(RECORDS, f));
  }
  try {
    rmdirSync(old);
  } catch {
    // left: files already in RECORDS under the same name
  }
}
const BUCKET = "clax-assets";
const QUEUE = "clax-artifacts-events";
const NAMESPACE = "clax";
// Workers Paid: 10 MB after compression; Free: 3 MB.
const PAID_LIMIT_KIB = 10 * 1024;

export function parseArgs(argv) {
  const o = { store: "artifacts", storeGiven: false, name: "clax", wait: 600, diag: false, build: true, dryRun: false, yes: false, rotate: false, configOnly: false, ci: false, production: false, fromLaptop: false };
  const takes = new Set(["--public", "--site-domain", "--zone", "--store", "--name", "--account", "--wait", "--out", "--github-client-id", "--owner-github", "--legal", "--terms-url", "--privacy-url", "--legal-version", "--retired-content-origin"]);
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const [flag, inline] = a.includes("=") ? a.split(/=(.*)/s) : [a, undefined];
    const value = () => inline ?? argv[++i];
    if (flag === "--content") {
      throw new Error("--content is gone: each artifact is served at <artifact ID>.<site domain>; give --site-domain <domain> (a zone of its own, e.g. example.run)");
    }
    if (takes.has(flag)) {
      const v = value();
      if (v === undefined) throw new Error(`${flag} takes a value`);
      o[flag.slice(2)] = flag === "--wait" ? Number(v) : v;
      if (flag === "--store") o.storeGiven = true;
    } else if (flag === "--dry-run") o.dryRun = true;
    else if (flag === "--ci") o.ci = o.yes = true;
    else if (flag === "--production") o.production = true;
    else if (flag === "--from-laptop") o.fromLaptop = true;
    else if (flag === "--yes" || flag === "-y") o.yes = true;
    else if (flag === "--diag") o.diag = true;
    else if (flag === "--no-diag") o.diag = false;
    else if (flag === "--no-build") o.build = false;
    else if (flag === "--config-only") o.configOnly = true;
    else if (flag === "--rotate-secrets") o.rotate = true;
    else if (flag === "--help" || flag === "-h") o.help = true;
    else throw new Error(`unknown option ${a} (see the top of deploy.mjs)`);
  }
  if (!["artifacts", "memory"].includes(o.store)) throw new Error(`--store is artifacts or memory, not ${o.store}`);
  if (o["retired-content-origin"] !== undefined && !retiredContentOrigin({ retired_content_origin: o["retired-content-origin"] })) {
    throw new Error(`--retired-content-origin takes an https:// origin (e.g. https://content.example.com), not ${o["retired-content-origin"]}`);
  }
  return o;
}

const step = (s) => console.log(`\n== ${s}`);

// The Terms of Service and Privacy Policy settings: this deploy's flags,
// else the record's. The Worker checks them again at start
// (Legal::from_settings).
function legalSettings(o, early) {
  const own = ["terms-url", "privacy-url", "legal-version"];
  if (o.legal !== undefined || own.some((f) => o[f] !== undefined)) {
    if (o.legal !== undefined && o.legal !== "bundled") throw new Error(`--legal takes bundled, not ${o.legal}`);
    if (o.legal !== undefined && own.some((f) => o[f] !== undefined)) throw new Error("--legal bundled takes no --terms-url, --privacy-url or --legal-version");
    if (o.legal === "bundled") return { legal: "bundled" };
    if (own.some((f) => !o[f])) throw new Error("--terms-url, --privacy-url and --legal-version go together");
    return { terms_url: o["terms-url"], privacy_url: o["privacy-url"], legal_version: o["legal-version"] };
  }
  if (early.legal) return { legal: early.legal };
  if (early.terms_url) return { terms_url: early.terms_url, privacy_url: early.privacy_url, legal_version: early.legal_version };
  return {};
}
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

/** Runs git with `args` in the repository: {code, out}. */
function git(args) {
  return new Promise((resolve) => {
    const child = spawn("git", args, { cwd: ROOT, stdio: ["ignore", "pipe", "ignore"] });
    let out = "";
    child.stdout.on("data", (d) => (out += d));
    child.on("error", () => resolve({ code: -1, out }));
    child.on("close", (code) => resolve({ code, out }));
  });
}

/** The checked-out commit (full lowercase hex, as `CLAX_BUILD_COMMIT`
 * takes it); empty when it cannot be read. */
async function sourceCommit() {
  const r = await git(["rev-parse", "HEAD"]);
  const c = r.out.trim();
  return r.code === 0 && /^[0-9a-f]{7,64}$/.test(c) ? c : "";
}

/** Whether this deploy may go on, by deploy-lib.mjs productionGate: with
 * --production, what git says of the checkout (clean, HEAD) and of
 * origin's prod tip. Throws its refusal; answers what to say, if
 * anything. */
async function checkProduction(o) {
  const facts = {};
  if (o.production) {
    const status = await git(["status", "--porcelain"]);
    const tip = await git(["ls-remote", "origin", "refs/heads/prod"]);
    facts.githubActions = process.env.GITHUB_ACTIONS === "true";
    facts.clean = status.code === 0 && status.out.trim() === "";
    facts.head = await sourceCommit();
    facts.prodTip = tip.code === 0 ? prodTipOf(tip.out) : null;
  }
  const gate = productionGate({ production: o.production, fromLaptop: o.fromLaptop, dryRun: o.dryRun, ...facts });
  if (gate.refuse) throw new Error(gate.refuse);
  return gate.say;
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

export function hostOf(s) {
  // One trailing dot (a fully qualified name) names the same host.
  const h = s.replace(/^https?:\/\//, "").replace(/\/.*$/, "").toLowerCase().replace(/\.$/, "");
  if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(h)) throw new Error(`${s} is not a hostname`);
  return h;
}

/** The site domain `s` for the public host `pub`: a domain whose
 * first-level names are artifact hosts. It shares no host with the public
 * one either way, so no cookie or route of one reaches the other (the
 * Worker refuses the same at start). */
export function siteDomainOf(s, pub) {
  if (s.includes("*")) throw new Error(`--site-domain takes the domain itself (e.g. example.run), not a wildcard: ${s}`);
  const site = hostOf(s);
  if (/^[0-9.]+$/.test(site)) throw new Error(`the site domain ${site} is an IP address: give a registrable domain of its own (e.g. example.run)`);
  if (site === pub) throw new Error(`the site domain ${site} is the public host: give a domain of its own (e.g. example.run)`);
  if (site.endsWith(`.${pub}`)) throw new Error(`the site domain ${site} is under the public host ${pub}: give a domain of its own`);
  if (pub.endsWith(`.${site}`)) throw new Error(`the public host ${pub} is under the site domain ${site}: it would be served as a site; give a domain of its own`);
  // Two hosts sharing a parent below the top-level label (sites.example.com
  // and app.example.com) are one site to browsers, the route's zone_name
  // would not be the zone, and <ID>.sites.example.com is a second-level
  // wildcard Universal SSL does not cover. The check takes the last two
  // labels as the registrable domain, so under a multi-label public suffix
  // (co.uk, com.au) it misses a shared parent such as example.co.uk, and
  // refuses two unrelated domains under that suffix.
  const parent = (h) => h.split(".").slice(-2).join(".");
  if (parent(site) === parent(pub)) {
    throw new Error(
      `the site domain ${site} shares ${parent(site)} with the public host ${pub}: browsers would treat its sites as the same site as Clax, ` +
        `and it is not a zone of its own; give a registrable domain of its own (e.g. example.run)`,
    );
  }
  return site;
}

async function ask(o, question, fallback) {
  if (o.yes || !process.stdin.isTTY) {
    if (fallback !== undefined) return fallback;
    throw new Error(`${question}: give it as an option (see the top of deploy.mjs)`);
  }
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    // Without a default, an empty answer asks again: never undefined.
    for (;;) {
      const a = (await rl.question(`${question}${fallback ? ` [${fallback}]` : ""}: `)).trim();
      if (a || fallback !== undefined) return a || fallback;
      console.log("   an answer is required");
    }
  } finally {
    rl.close();
  }
}

async function hostnames(o) {
  let zone = o.zone ? hostOf(o.zone) : undefined;
  if (!o.public && !zone) {
    zone = hostOf(await ask(o, "The zone (a domain on this Cloudflare account) to serve Clax under, e.g. example.com"));
  }
  const pub = hostOf(o.public ?? (await ask(o, "Public hostname", zone)));
  // No default: the site domain is a zone of its own, never derived from
  // the public one.
  const site = siteDomainOf(
    o["site-domain"] ??
      readRecord(pub).site_domain ??
      (await ask(o, "Site domain (a zone of its own on this account; each artifact is served at <ID>.<site domain>), e.g. example.run")),
    pub,
  );
  return { pub, site };
}

/** The login and the account to use; `null` when not logged in. */
async function login(o) {
  const r = await wrangler(["whoami", "--json"]);
  let who;
  try {
    // `whoami --json` prints the object over many lines.
    who = jsonIn(r.out);
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

const KEK_HEX = /^[0-9a-f]{64}$/i;

/** What the deploy does about `CLAX_KEK`, given `have`, the secret names
 * the Worker has (`null` without a login), `saved`, the backup this
 * machine keeps, if any, and `savedIn`, where it was found: "file" (its own,
 * `kekPath`) or "record" (a `kek` an earlier deploy kept in the record):
 * `upload`, the key to upload (the backup when there is one, else 64 new
 * hex digits) or none; `save`, a key to write to the backup file first (a
 * new one, or one found only in the record, so the record, which may be
 * deleted, is never its only copy); `warn`, when the Worker has a key this
 * machine keeps no backup of; and `say`, the line it prints (never the
 * key). */
export function kekPlan({ have, saved, savedIn, dryRun }) {
  const kept = typeof saved === "string" && KEK_HEX.test(saved);
  const save = kept && savedIn === "record" && !dryRun ? saved : null;
  const moved = save ? "; its backup moved from the record to its own file" : "";
  if (have === null) return { upload: null, save, warn: null, say: `skipped (no login): a deploy uploads CLAX_KEK when the Worker has none${moved}` };
  if (have.includes("CLAX_KEK")) {
    const warn = kept
      ? null
      : "the Worker has CLAX_KEK but this machine keeps no backup of it: losing the Worker's secret loses every stored model key; " +
        "if you have the key, write it to the backup file named below (0600)";
    return { upload: null, save, warn, say: `the Worker has CLAX_KEK; kept (never rotated)${moved}` };
  }
  if (dryRun) return { upload: null, save: null, warn: null, say: `would upload CLAX_KEK ${kept ? "from its backup" : "(new)"} with the deploy` };
  if (kept) return { upload: saved, save, warn: null, say: `CLAX_KEK from its backup; uploaded with the deploy${moved}` };
  const fresh = randomBytes(32).toString("hex");
  return { upload: fresh, save: fresh, warn: null, say: "generated CLAX_KEK; backed up, then uploaded with the deploy" };
}

/** Where the deploy keeps CLAX_KEK's backup: a file of its own beside the
 * record (0600), which, unlike the record, is never to be deleted. */
export function kekPath(pub) {
  return join(RECORDS, `${pub}.kek`);
}

/** The backup of CLAX_KEK kept for `pub`, `{saved, savedIn}`: its own file
 * ("file"), else a `kek` an earlier deploy kept in the record ("record"),
 * else none. */
function readKek(pub, record) {
  try {
    const k = readFileSync(kekPath(pub), "utf8").trim();
    if (KEK_HEX.test(k)) return { saved: k, savedIn: "file" };
  } catch {
    // none yet
  }
  if (typeof record.kek === "string" && KEK_HEX.test(record.kek)) return { saved: record.kek, savedIn: "record" };
  return { saved: null, savedIn: null };
}

function writeKek(pub, kek) {
  mkdirSync(RECORDS, { recursive: true, mode: 0o700 });
  writeFileSync(kekPath(pub), kek + "\n", { mode: 0o600 });
  chmodSync(kekPath(pub), 0o600);
}

/** The secret names the Worker has; empty when it does not exist yet. */
async function secretNames(account, configPath) {
  const r = await wrangler(["secret", "list", "--config", configPath, "--format", "json"], { account });
  if (r.code !== 0) return [];
  try {
    return jsonIn(r.out).map((s) => s.name);
  } catch {
    return [];
  }
}

/** The retired content origin a deploy record names (`retired_content_origin`,
 * or an older record's `content_origin`): its `https://` origin, or null. */
export function retiredContentOrigin(record) {
  const raw = record?.retired_content_origin ?? record?.content_origin;
  if (typeof raw !== "string" || raw === "") return null;
  try {
    const u = new URL(raw);
    return u.protocol === "https:" && u.pathname === "/" && !u.search ? u.origin : null;
  } catch {
    return null;
  }
}

function deployConfig(o, pub, site, account, github, legal, out, retired, build) {
  const c = baseConfig();
  delete c.$schema;
  // The config lives in target/deploy/, so its paths name the crate's files.
  c.main = join(HERE, c.main);
  if (c.assets?.directory) c.assets.directory = resolve(HERE, c.assets.directory);
  // deploy.mjs builds once, before wrangler runs.
  delete c.build;
  c.name = o.name;
  if (account) c.account_id = account;
  c.workers_dev = false;
  c.preview_urls = false;
  // A custom domain cannot be a wildcard: the site domain is a route on
  // its own zone, with a proxied `*` DNS record and a certificate covering
  // `*.<site>` (README.md, "DNS for the public host and the site domain").
  c.routes = [
    { pattern: pub, custom_domain: true },
    { pattern: `*.${site}/*`, zone_name: site },
  ];
  // The retired content host stays a custom domain of the Worker, which
  // answers its old asset addresses with a redirect to their sites.
  if (retired) c.routes.push({ pattern: new URL(retired).host, custom_domain: true });
  c.observability = { enabled: true, head_sampling_rate: 1 };
  // The base config's CONTENT_ORIGIN, if it still has one, is the retired
  // content host's: RETIRED_CONTENT_ORIGIN names it, from the record.
  const { CONTENT_ORIGIN: _old, RETIRED_CONTENT_ORIGIN: _base, ...base } = c.vars ?? {};
  c.vars = {
    ...base,
    ...(retired ? { RETIRED_CONTENT_ORIGIN: retired } : {}),
    HOSTED: "1",
    PUBLIC_ORIGIN: `https://${pub}`,
    SITE_DOMAIN: site,
    REPO_STORE: o.store,
    GIT_SMART_HTTP: o.store === "memory" ? "1" : "0",
    CLAX_TEST_ROUTES: "0",
    CLAX_TEST_LOOPBACK_PROVIDER: "0",
    REPO_DEV_ROUTES: "0",
    CLAX_DIAG_ROUTES: o.diag ? "1" : "0",
    CLAX_BUILD: build,
    ...(github.client_id ? { GITHUB_CLIENT_ID: github.client_id } : {}),
    ...(github.owner ? { OWNER_GITHUB: github.owner } : {}),
    ...(legal.legal ? { LEGAL: legal.legal } : {}),
    ...(legal.terms_url ? { TERMS_URL: legal.terms_url, PRIVACY_URL: legal.privacy_url, LEGAL_VERSION: legal.legal_version } : {}),
  };
  if (o.store === "memory") {
    // No Artifacts: neither its binding nor its events' queue.
    delete c.artifacts;
    delete c.queues;
  }
  const header =
    "// Generated by deploy.mjs from wrangler.jsonc; edit that, or deploy.mjs's\n" +
    "// options, and run deploy.mjs again. Not checked in.\n";
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, header + JSON.stringify(c, null, 2) + "\n");
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

/** Polls `url` until `reached(res, body)` is true (default: a 2xx) or
 * `seconds` pass. `reached` may answer a string instead, saying what a
 * 2xx it does not take is waiting on. */
async function waitHealthy(url, seconds, reached = (res) => res.ok) {
  const end = Date.now() + seconds * 1000;
  const started = Date.now();
  let last = "";
  for (;;) {
    let state;
    let done = null;
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(10_000), redirect: "manual" });
      const body = await res.text();
      const r = reached(res, body);
      if (r === true) done = `HTTP ${res.status}: ${body}`;
      else if (res.ok && typeof r === "string") state = r;
      else if (res.status === 500 && body.includes("could not start")) throw new Error(`the instance did not start: ${body.slice(0, 400)}`);
      else state = `HTTP ${res.status}: ${body.slice(0, 160).replace(/\s+/g, " ")}${res.status === 404 || res.status === 522 || res.status === 530 ? " (the host is not routed to the Worker yet)" : ""}`;
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

// Cloudflare's published edge ranges (https://www.cloudflare.com/ips/): a
// proxied record resolves into them.
const CLOUDFLARE_V4 = ["173.245.48.0/20", "103.21.244.0/22", "103.22.200.0/22", "103.31.4.0/22", "141.101.64.0/18", "108.162.192.0/18", "190.93.240.0/20", "188.114.96.0/20", "197.234.240.0/22", "198.41.128.0/17", "162.158.0.0/15", "104.16.0.0/13", "104.24.0.0/14", "172.64.0.0/13", "131.0.72.0/22"];
const CLOUDFLARE_V6 = ["2400:cb00::/32", "2606:4700::/32", "2803:f800::/32", "2405:b500::/32", "2405:8100::/32", "2a06:98c0::/29", "2c0f:f248::/32"];
const cloudflareIps = new BlockList();
for (const r of CLOUDFLARE_V4) cloudflareIps.addSubnet(r.split("/")[0], Number(r.split("/")[1]), "ipv4");
for (const r of CLOUDFLARE_V6) cloudflareIps.addSubnet(r.split("/")[0], Number(r.split("/")[1]), "ipv6");

const ID_ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";
/** A random artifact-shaped ID (clax_core::ids::ALPHABET, 12 characters). */
export function randomSiteId() {
  return Array.from({ length: 12 }, () => ID_ALPHABET[randomInt(ID_ALPHABET.length)]).join("");
}

/** Whether a certificate's subjectaltname (`DNS:a, DNS:b`) covers `host`. */
export function sanCovers(san, host) {
  return (san ?? "").split(/,\s*/).map((e) => e.replace(/^DNS:/, "").toLowerCase()).some((n) => n === host || (n.startsWith("*.") && host.endsWith(n.slice(1)) && !host.slice(0, -n.length + 1).includes(".")));
}

/** The site domain's zone: its NS records, and whether they are Cloudflare's. */
export async function zoneFound(site) {
  try {
    const ns = await dns.resolveNs(site);
    const cf = ns.every((n) => n.toLowerCase().endsWith(".cloudflare.com"));
    return { ok: cf && ns.length > 0, found: `NS ${ns.join(", ")}${cf ? "" : " (not Cloudflare's)"}` };
  } catch (e) {
    return { ok: false, found: `no NS records (${e.code ?? e.message})` };
  }
}

/** A name under the wildcard: whether it resolves into Cloudflare's ranges. */
export async function wildcardFound(host) {
  try {
    const all = await dns.lookup(host, { all: true });
    const off = all.filter((a) => !cloudflareIps.check(a.address, isIPv6(a.address) ? "ipv6" : "ipv4"));
    const list = all.map((a) => a.address).join(", ");
    return off.length ? { ok: false, found: `${host} resolves to ${list}, not all Cloudflare's: the record is not proxied` } : { ok: true, found: `${host} resolves to ${list} (Cloudflare's)` };
  } catch (e) {
    return { ok: false, found: `${host} does not resolve (${e.code ?? e.message})` };
  }
}

/** The certificate the edge presents for `host`: whether its hosts cover it. */
export function certificateFound(host) {
  return new Promise((done) => {
    const socket = tlsConnect({ host, port: 443, servername: host, rejectUnauthorized: false, timeout: 10_000 });
    const end = (r) => {
      socket.destroy();
      done(r);
    };
    socket.once("secureConnect", () => {
      const san = socket.getPeerCertificate()?.subjectaltname ?? "";
      end(sanCovers(san, host) ? { ok: true, found: `the certificate's hosts are ${san}` } : { ok: false, found: `the certificate's hosts are ${san || "(none)"}, which do not cover ${host}` });
    });
    socket.once("timeout", () => end({ ok: false, found: "no TLS answer within 10 s" }));
    socket.once("error", (e) => end({ ok: false, found: `TLS failed: ${e.code ?? e.message}` }));
  });
}

/** Whether a site host's /_clax/healthz answer is the Worker's: 200, JSON
 * with a `version`, and `Referrer-Policy: no-referrer`. */
export function siteHealthy(res, body) {
  if (res.status !== 200 || res.headers.get("referrer-policy") !== "no-referrer") return false;
  try {
    return typeof JSON.parse(body)?.version === "string";
  } catch {
    return false;
  }
}

/** Checks a random site host's health; failing, prints each manual step
 * with what it found. Answers whether the site host is healthy. */
async function checkSiteHost(site, account, seconds) {
  const host = `${randomSiteId()}.${site}`;
  const url = `https://${host}/_clax/healthz`;
  step(`Checking a site host: ${url}`);
  // The Worker answers /_clax/healthz on every well-formed site host, the
  // random ID's included, with /healthz's JSON (its `version`) and
  // `Referrer-Policy: no-referrer`. Anything else (forbidden_host from a
  // Worker without site hosts, Cloudflare's error pages, a redirect, a
  // parking page) is not the Worker serving sites.
  if (await waitHealthy(url, seconds, siteHealthy)) return true;
  const [zone, wildcard, cert] = await Promise.all([zoneFound(site), wildcardFound(host), certificateFound(host)]);
  const mark = (r) => (r.ok ? "done" : "TO DO");
  info(`the site domain's hosts are not served yet. What a wildcard needs, that deploy.mjs cannot do through wrangler:`);
  info(`1. [${mark(zone)}] the zone ${site} on account ${account ?? "(this one)"} (dashboard > Add a domain), its registrar pointing at Cloudflare's name servers`);
  info(`   found: ${zone.found}`);
  info(`2. [${mark(wildcard)}] a proxied DNS record for * in ${site} (dashboard > ${site} > DNS > Records: type AAAA, name *, content 100::, Proxied)`);
  info(`   found: ${wildcard.found}`);
  info(`3. [${mark(cert)}] a certificate whose hosts include *.${site} (if Universal SSL's does not: Advanced Certificate Manager or Total TLS)`);
  info(`   found: ${cert.found}`);
  info(`then run deploy.mjs again (it checks ${url.replace(host, `<ID>.${site}`)} again)`);
  return false;
}

/** Takes the hostnames and writes the deploy config (steps 2 and 5). */
async function writeDeployConfig(o, account) {
  step("Hostnames");
  const { pub, site } = await hostnames(o);
  info(`public https://${pub}`);
  info(`sites  https://<artifact ID>.${site}`);

  step("Deploy config");
  const early = readRecord(pub);
  const github = {
    client_id: o["github-client-id"] ?? early.github_client_id,
    owner: o["owner-github"] ?? early.owner_github,
  };
  if (github.client_id) info(`GitHub sign-in: OAuth App ${github.client_id}, owner ${github.owner ? `@${github.owner}` : "(none named)"}`);
  const legal = legalSettings(o, early);
  info(`Terms and Privacy: ${legal.legal ? "the bundled documents" : legal.terms_url ? `${legal.terms_url}, ${legal.privacy_url} (version ${legal.legal_version})` : "none"}`);
  // The flag's (a --ci deploy has no record), else the record's.
  const retired = retiredContentOrigin({ retired_content_origin: o["retired-content-origin"] }) ?? retiredContentOrigin(early);
  if (retired) info(`retired content origin ${retired}: its old asset addresses redirect to their sites`);
  const configPath = o.out ? resolve(o.out) : DEPLOY_CONFIG;
  const commit = await sourceCommit();
  const build = buildId(commit, new Date(), randomBytes(4).toString("hex"));
  const config = deployConfig(o, pub, site, account, github, legal, configPath, retired, build);
  info(`wrote ${configPath} (worker ${config.name}, REPO_STORE=${o.store}, SITE_DOMAIN=${site}, diagnostics ${o.diag ? "on" : "off"}, build ${build})`);
  return { pub, site, github, legal, config, configPath, retired, commit, build };
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  if (o.help) {
    console.log(readFileSync(new URL(import.meta.url), "utf8").split("\n").filter((l) => l.startsWith("//")).map((l) => l.slice(3)).join("\n"));
    return;
  }
  if (o.configOnly) {
    console.log(`hosted Clax deploy config (--config-only: no wrangler, no login, no build)`);
    await writeDeployConfig(o, o.account ?? process.env.CLOUDFLARE_ACCOUNT_ID);
    return;
  }
  if (o.ci) checkCiOptions(o, process.env);
  const production = await checkProduction(o);
  moveOldRecords();
  if (!existsSync(WRANGLER)) throw new Error(`no wrangler at ${WRANGLER}: run npm install in ${HERE}`);
  const version = (await wrangler(["--version"])).out.trim();
  console.log(`hosted Clax deploy${o.dryRun ? " (dry run: no account changes)" : ""}${o.ci ? " (--ci: no questions)" : ""}; wrangler ${version}; store ${o.store}`);
  if (production) info(production);

  step("Login");
  const who = await login(o);
  if (!who) {
    if (!o.dryRun && o.ci) throw new Error("wrangler is not logged in: --ci signs in with CLOUDFLARE_API_TOKEN");
    if (!o.dryRun) throw new Error("wrangler is not logged in: run `npx wrangler login` in this directory, then this again");
    info("not logged in: the account calls (bucket, queue, Artifacts, secrets) are skipped, each named below");
  } else {
    info(`${who.email}, account ${who.account.name} (${who.account.id})`);
  }
  const account = who?.account.id;

  const { pub, site, github, legal, config, configPath, retired, commit, build } = await writeDeployConfig(o, account);
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
  const have = who ? await secretNames(account, configPath) : [];
  let secrets = null;
  const needed = ["SETUP_SECRET", "CLAX_TOKEN"].filter((n) => o.rotate || !have.includes(n));
  if (o.ci && who) {
    // First-time setup stays on a laptop: a --ci deploy changes no secret
    // but the GitHub client secret it is given.
    const refusal = ciSecretsRefusal(have, { githubClientId: github.client_id, githubSecretGiven: Boolean(process.env.GITHUB_CLIENT_SECRET?.trim()) });
    if (refusal && !o.dryRun) throw new Error(refusal);
    if (refusal) info(`a deploy would refuse: ${refusal}`);
  }
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
  // CLAX_KEK seals the model keys ClaxBot stores (README.md, "ClaxBot"):
  // made once, backed up in a file of its own before anything is uploaded,
  // and never rotated, since a new one opens nothing the old one sealed. A
  // Worker without it gets the backup's, else a new one.
  const kek = kekPlan({ have: who ? have : null, ...readKek(pub, record), dryRun: o.dryRun });
  info(kek.say);
  // A --ci deploy keeps no backup: the one made by the instance's first
  // deploy is on the machine that ran it.
  if (kek.warn) info(o.ci ? "notice: the Worker has CLAX_KEK and this run keeps no backup of it (--ci: the backup is kept where the instance was first deployed)" : `WARNING: ${kek.warn}: ${kekPath(pub)}`);
  if (kek.save) writeKek(pub, kek.save);
  if (kek.upload) secrets = { ...(secrets ?? {}), CLAX_KEK: kek.upload };
  if (github.client_id) {
    let gh = process.env.GITHUB_CLIENT_SECRET?.trim();
    const missing = who && !have.includes("GITHUB_CLIENT_SECRET");
    if (!gh && missing && !o.dryRun) gh = (await ask(o, "GitHub OAuth App client secret (GITHUB_CLIENT_SECRET)"))?.trim();
    if (gh && !o.dryRun) {
      secrets = { ...(secrets ?? {}), GITHUB_CLIENT_SECRET: gh };
      info("GITHUB_CLIENT_SECRET uploaded with the deploy");
    } else if (gh) info("would upload GITHUB_CLIENT_SECRET with the deploy");
    else info(missing ? "no GITHUB_CLIENT_SECRET: GitHub sign-in stays off until one is set" : "the Worker has GITHUB_CLIENT_SECRET; kept");
  }

  // CLAX_BUILD_COMMIT: the commit the Worker's /healthz reports
  // (src/instance.rs BUILD_COMMIT).
  const buildWorker = () =>
    run(join(HERE, "build.sh"), [], { CARGO_INCREMENTAL: process.env.CARGO_INCREMENTAL ?? "0", ...(commit ? { CLAX_BUILD_COMMIT: commit } : {}) });
  if (o.build) {
    step("Build: web UI");
    await run(join(HERE, "../../scripts/build-web.sh"), []);
    step("Build: Worker (release)");
    await buildWorker();
  } else {
    step("Build: skipped (--no-build)");
    // What an earlier run built, from this commit (deploy-lib.mjs
    // builtFromRefusal): never a stale build, nor one from another commit.
    const wasm = join(HERE, "build/index_bg.wasm");
    const refusal = builtFromRefusal(existsSync(wasm) ? readFileSync(wasm) : null, commit);
    if (refusal) throw new Error(refusal);
    info(commit ? `the Worker in build/ was built from HEAD ${commit.slice(0, 12)}` : "the Worker in build/ is deployed as it is (no commit to check it against)");
  }
  if (!existsSync(config.main)) throw new Error(`no ${config.main}: build first (drop --no-build)`);
  // The Worker embeds web/dist when clax-worker compiles (rust-embed,
  // src/web.rs; build.rs rebuilds it when web/dist changes). A Worker that
  // embeds another web UI serves "ui_not_built" or stale pages for /setup
  // and every other shell page, so the upload is checked, not assumed.
  if (!embedsWebUi()) {
    if (!o.build) throw new Error(`build/index_bg.wasm does not embed web/dist/index.html: drop --no-build`);
    step("Build: the Worker embedded an older web UI; rebuilding clax-worker");
    await run("cargo", ["clean", "-q", "--release", "--target", "wasm32-unknown-unknown", "-p", "clax-worker"]);
    await buildWorker();
    if (!embedsWebUi()) throw new Error("build/index_bg.wasm still does not embed web/dist/index.html");
  }
  info("the Worker embeds the web UI in web/dist");

  // Only prod's tip at the time of the upload goes to production: prod
  // may have moved while this deploy built.
  if (o.production && !o.dryRun) await checkProduction(o);
  step(o.dryRun ? "wrangler deploy --dry-run" : "wrangler deploy");
  const args = ["deploy", "--config", configPath];
  let secretsFile;
  if (o.dryRun) args.push("--dry-run", "--outdir", join(ROOT, "target/deploy/dry-run"));
  if (secrets) {
    secretsFile = join(RECORDS, `.secrets-${process.pid}.json`);
    mkdirSync(RECORDS, { recursive: true, mode: 0o700 });
    writeFileSync(secretsFile, JSON.stringify(secrets), { mode: 0o600 });
    args.push("--secrets-file", secretsFile);
  }
  let r;
  let retried = false;
  try {
    r = await wrangler(args, { account, show: true });
    if (r.code !== 0 && !o.dryRun && partialTriggerUpdate(r.out + r.err)) {
      retried = true;
      // The upload went through and only the routes or custom domains were
      // left half-done; the same deploy again finishes them.
      info("wrangler deploy left the triggers (custom domains) only partially updated: deploying once more, without rebuilding");
      r = await wrangler(args, { account, show: true });
    }
  } finally {
    if (secretsFile) rmSync(secretsFile, { force: true });
  }
  const size = uploadSize(r.out + r.err);
  if (r.code !== 0) throw new Error(`wrangler deploy failed${retried ? " again, after its one retry" : ""}: ${lastError(r)}`);
  if (size) {
    const pct = Math.round((size.gzip / PAID_LIMIT_KIB) * 100);
    info(`upload ${(size.total / 1024).toFixed(2)} MiB, ${(size.gzip / 1024).toFixed(2)} MiB gzipped: ${pct}% of Workers Paid's 10 MiB`);
  }

  if (o.dryRun) {
    step("Dry run done");
    info(`config: ${configPath}; bundle: ${join(ROOT, "target/deploy/dry-run")}`);
    info(who ? "deploy with the same options, without --dry-run" : "after `npx wrangler login`, run the same command without --dry-run");
    return;
  }

  // An older record's content_origin is kept as the retired one; its
  // version_id was an earlier deploy's.
  const { content_origin: _old, retired_content_origin: _kept, version_id: _was, ...kept } = record;
  const versionId = versionIdOf(r.out + r.err);
  const next = {
    ...kept,
    ...(retired ? { retired_content_origin: retired } : {}),
    public_origin: `https://${pub}`,
    site_domain: site,
    worker: o.name,
    account,
    store: o.store,
    diag: o.diag,
    deployed_at: new Date().toISOString(),
    build,
    // The Cloudflare Worker version this deploy made (`wrangler rollback
    // <version ID>` returns to it), when wrangler named it.
    ...(versionId ? { version_id: versionId } : {}),
    ...(github.client_id ? { github_client_id: github.client_id } : {}),
    ...(github.owner ? { owner_github: github.owner } : {}),
    ...legal,
    ...(secrets?.SETUP_SECRET ? { setup_secret: secrets.SETUP_SECRET, setup_done: false } : {}),
    ...(secrets?.CLAX_TOKEN ? { owner_token: secrets.CLAX_TOKEN } : {}),
    ...(secrets?.CLAX_KEK ? { kek_backup: kekPath(pub) } : {}),
  };
  writeRecord(pub, next);

  step(`Waiting for https://${pub}/healthz to report build ${build} (custom domains take a few minutes the first time)`);
  const healthy = await waitHealthy(`https://${pub}/healthz`, o.wait, (res, body) => {
    if (!res.ok) return false;
    const h = healthOf(body, build);
    return h.done ? true : h.waiting;
  });
  if (!healthy) {
    info(`not healthy within ${o.wait} s. Check: the zone of ${pub} is on account ${account}; ` +
      `Workers > ${o.name} > Settings > Domains & Routes lists ${pub} and *.${site}/*; \`npx wrangler tail ${o.name}\``);
  }
  // The wildcard's DNS record and certificate are the account's, not the
  // deploy's: a shorter wait, then the steps that are left.
  const siteOk = await checkSiteHost(site, account, Math.min(o.wait, 120));

  step("Done");
  // The first-time setup text only when this deploy generated the secrets.
  console.log(doneText({ pub, name: o.name, diag: o.diag, generated: secrets, record: recordPath(pub), kek: o.ci ? null : kekPath(pub), smoke: `node smoke-remote.mjs --deploy ${recordPath(pub)}` }));
  if (o.diag) diagReminder(pub);
  // A --ci deploy passes only when the instance answers as this build on
  // the public host and on a site host.
  const failed = [...(healthy ? [] : [`https://${pub}/healthz did not report build ${build} within ${o.wait} s`]), ...(siteOk ? [] : [`a site host of ${site} is not served`])];
  if (o.ci && failed.length) throw new Error(failed.join("; "));
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

// Run as a program; imported (test-deploy-config.mjs checks the generated
// config, test-bot.mjs checks kekPlan), it deploys nothing.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(`\ndeploy failed: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  });
}
