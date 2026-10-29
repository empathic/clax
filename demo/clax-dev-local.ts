// clax.dev on this machine (`just clax-dev-local`):
//
//   node demo/clax-dev-local.ts [--fresh] [--port 8788] [--no-build]
//
// The Worker (crates/clax-worker) under `wrangler dev --local` with the
// settings a production deploy gives it (crates/clax-worker/deploy.mjs,
// the vars it writes): hosted, GitHub sign-in against real GitHub, the
// owner's GitHub login, the bundled Terms of Service and Privacy Policy, a
// CLAX_KEK, and no test, dev or diagnostics routes. What differs is what
// cannot run here: the origins are http://localhost:<port> and each
// artifact's site <ID>.localhost:<port>, and versions live in the RepoDO
// stand-in (REPO_STORE=memory), not Cloudflare Artifacts. Nothing is
// seeded: the first run leaves setup open, as a first deploy does, and
// prints the setup URL and secret; after that everyone signs in or up with
// GitHub, and each new person gets the quickstart at their first sign-in.
//
// GitHub sign-in needs an OAuth App whose callback is
// http://localhost:<port>/auth/github/callback. Its client ID and secret
// and the owner's login are read from the environment (GITHUB_CLIENT_ID,
// GITHUB_CLIENT_SECRET, OWNER_GITHUB), else from
// ~/.config/clax/local/github.env (KEY=value lines, 0600); the first run on
// a terminal asks for any that are missing and keeps them there.
//
// The instance's data, its generated secrets (secrets.json, 0600), the
// build log and wrangler's log are kept in target/clax-dev-local/ across
// runs; --fresh starts over with setup open. The port is fixed (8788 by
// default) because the OAuth App's callback names it. Ctrl-C stops the
// Worker by its process group.

import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline/promises";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { BuildLog } from "./lib/build.ts";
import { startWorker, type RunningWorker } from "./lib/worker.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIR = join(ROOT, "target/clax-dev-local");
const GITHUB_ENV = join(homedir(), ".config/clax/local/github.env");
const GITHUB_KEYS = ["GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET", "OWNER_GITHUB"] as const;
type GitHubKey = (typeof GITHUB_KEYS)[number];

const { values: opts } = parseArgs({
  options: {
    fresh: { type: "boolean", default: false },
    port: { type: "string", default: "8788" },
    "no-build": { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});
if (opts.help === true) {
  console.log("usage: just clax-dev-local [--fresh] [--port 8788] [--no-build]\n  (clax.dev's settings on this machine; data kept in target/clax-dev-local/, --fresh starts over)");
  process.exit(0);
}
const port = Number(opts.port);
const base = `http://localhost:${port}`;

/** KEY=value lines; blank lines and # comments skipped. */
function readEnvFile(path: string): Record<string, string> {
  if (!existsSync(path)) return {};
  const out: Record<string, string> = {};
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const m = /^\s*([A-Z_][A-Z0-9_]*)\s*=\s*(.*?)\s*$/.exec(line);
    if (m) out[m[1]] = m[2];
  }
  return out;
}

/** The OAuth App's client ID and secret and the owner's login: the
 * environment, else GITHUB_ENV; any still missing are asked for on a
 * terminal and kept in GITHUB_ENV. */
async function gitHubSettings(): Promise<Record<GitHubKey, string>> {
  const kept = readEnvFile(GITHUB_ENV);
  const got: Partial<Record<GitHubKey, string>> = {};
  for (const k of GITHUB_KEYS) {
    const v = process.env[k] || kept[k];
    if (v) got[k] = v;
  }
  const missing = GITHUB_KEYS.filter((k) => !got[k]);
  if (missing.length === 0) return got as Record<GitHubKey, string>;
  if (!process.stdin.isTTY) {
    console.error(`GitHub sign-in needs ${missing.join(", ")}: set them in the environment or in ${GITHUB_ENV}.`);
    process.exit(2);
  }
  console.log(`
GitHub sign-in needs an OAuth App of your own, once:
  GitHub › Settings › Developer settings › OAuth Apps › New OAuth App
    Homepage URL:                ${base}
    Authorization callback URL:  ${base}/auth/github/callback
  Register it, then Generate a new client secret.
The answers are kept in ${GITHUB_ENV} (0600).
`);
  const prompts: Record<GitHubKey, string> = {
    GITHUB_CLIENT_ID: "Client ID: ",
    GITHUB_CLIENT_SECRET: "Client secret: ",
    OWNER_GITHUB: "Your GitHub login (the owner's): ",
  };
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  for (const k of missing) {
    const v = (await rl.question(prompts[k])).trim();
    if (!v) {
      console.error(`No ${k}; nothing kept.`);
      process.exit(2);
    }
    got[k] = v;
  }
  rl.close();
  const all = { ...kept, ...got };
  mkdirSync(dirname(GITHUB_ENV), { recursive: true, mode: 0o700 });
  writeFileSync(GITHUB_ENV, GITHUB_KEYS.map((k) => `${k}=${all[k]}`).join("\n") + "\n", { mode: 0o600 });
  chmodSync(GITHUB_ENV, 0o600);
  return got as Record<GitHubKey, string>;
}

/** The secrets a first deploy generates, made once per instance and kept
 * beside its data: the setup secret and CLAX_KEK (64 hex digits). */
function instanceSecrets(): { SETUP_SECRET: string; CLAX_KEK: string } {
  const path = join(DIR, "secrets.json");
  if (existsSync(path)) return JSON.parse(readFileSync(path, "utf8"));
  const s = { SETUP_SECRET: randomBytes(18).toString("base64url"), CLAX_KEK: randomBytes(32).toString("hex") };
  writeFileSync(path, JSON.stringify(s, null, 2) + "\n", { mode: 0o600 });
  return s;
}

function portTaken(p: number): Promise<boolean> {
  return new Promise((ok) => {
    const s = createServer();
    s.once("error", () => ok(true));
    s.listen(p, "127.0.0.1", () => s.close(() => ok(false)));
  });
}

if (await portTaken(port)) {
  console.error(`Port ${port} is in use. Stop what holds it, or pass --port <port> (and give the OAuth App that port's callback).`);
  process.exit(1);
}
const github = await gitHubSettings();
if (opts.fresh) rmSync(DIR, { recursive: true, force: true });
mkdirSync(DIR, { recursive: true });
const secrets = instanceSecrets();

const builds = new BuildLog(DIR);
console.log(`Build output goes to ${builds.path}.`);
try {
  await builds.workerPackages();
  if (!opts["no-build"]) {
    await builds.web();
    await builds.worker();
  }
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}

console.log(`Starting clax.dev's Worker on :${port} (data ${join(DIR, "state")}, log ${join(DIR, "wrangler.log")})…`);
let w: RunningWorker;
try {
  w = await startWorker({
    dir: DIR,
    port,
    secret: secrets.SETUP_SECRET,
    seats: "",
    vars: { ...github, CLAX_KEK: secrets.CLAX_KEK, LEGAL: "bundled" },
  });
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
let stopping = false;
async function stop(code: number): Promise<never> {
  stopping = true;
  await w.stop();
  process.exit(code);
}
process.on("SIGINT", () => void stop(0));
process.on("SIGTERM", () => void stop(0));
w.child.once("exit", () => {
  if (stopping) return;
  console.error(`The Worker exited; see ${w.log}:\n${readFileSync(w.log, "utf8").slice(-2000)}`);
  process.exit(1);
});

const me = (await fetch(`${base}/api/hosted/me`).then((r) => r.json())) as { setup_open?: boolean };
const signIn = `Sign in with GitHub as ${github.OWNER_GITHUB} to sign in as the owner (the first time, it links that account to the owner); anyone else who signs in with GitHub signs up as a new person, with the quickstart in their gallery.`;
console.log(
  me.setup_open
    ? `
clax.dev is up at ${base}, with setup open.

First, make the owner: open ${base}/setup and enter
  setup secret: ${secrets.SETUP_SECRET}
and your name. Then: ${signIn}

The data is kept for the next run; --fresh starts over with setup open.
Ctrl-C stops it.`
    : `
clax.dev is up at ${base}.

${signIn}

The data is kept for the next run; --fresh starts over with setup open.
Ctrl-C stops it.`,
);
