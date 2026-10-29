// clax.dev on this machine (`just clax-dev-local`):
//
//   node demo/clax-dev-local.ts [--fresh] [--setup] [--port <8788-8797>] [--no-build]
//
// The Worker (crates/clax-worker) under `wrangler dev --local` with the
// settings a production deploy gives it (crates/clax-worker/deploy.mjs,
// the vars it writes): hosted, GitHub sign-in against real GitHub, the
// owner's GitHub login, the bundled Terms of Service and Privacy Policy, a
// CLAX_KEK, and no test, dev or diagnostics routes. What differs is what
// cannot run here: the origins are http://localhost:<port> and each
// artifact's site <ID>.localhost:<port>, and versions live in the RepoDO
// stand-in (REPO_STORE=memory), not Cloudflare Artifacts. Nothing is
// seeded: each new person who signs in with GitHub gets the quickstart.
//
// Nothing to configure. GitHub sign-in needs a GitHub App; the first run
// registers one from a manifest (GitHub's "create an App from a manifest"
// flow): it serves a page on the port with one button, GitHub asks to
// confirm, and the code GitHub returns is exchanged for the App's client
// ID and secret. The App's owner, the account that confirmed, is the
// instance's owner. They are kept in ~/.config/clax/local/github.env
// (0600; $XDG_CONFIG_HOME moves it), shared by every worktree; the environment's GITHUB_CLIENT_ID,
// GITHUB_CLIENT_SECRET and OWNER_GITHUB win over it. Delete that file to
// register a new App. The App's callbacks are ports 8788-8797 (GitHub
// requires an exact match and allows ten), and a run takes the first free
// one, so several worktrees can each run one; browsers share localhost's
// cookies across ports, so use one browser profile per instance.
//
// The owner is made through setup, as a first deploy's smoke does, under
// the owner's login; their first Sign in with GitHub links the account and
// renames them from GitHub. --setup leaves setup open instead and prints
// the setup URL and secret, to try /setup itself. The instance's data, its
// generated secrets (secrets.json, 0600), the build log and wrangler's log
// are kept in target/clax-dev-local/ across runs; --fresh starts over.
// Ctrl-C stops the Worker by its process group.

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { BuildLog } from "./lib/build.ts";
import { setupFirstPerson } from "./lib/http.ts";
import { startWorker, type RunningWorker } from "./lib/worker.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIR = join(ROOT, "target/clax-dev-local");
const GITHUB_ENV = join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "clax/local/github.env");
const GITHUB_KEYS = ["GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET", "OWNER_GITHUB"] as const;
type GitHub = Record<(typeof GITHUB_KEYS)[number], string>;
/** The ports the App's callbacks name. */
const PORTS = Array.from({ length: 10 }, (_, i) => 8788 + i);

const { values: opts } = parseArgs({
  options: {
    fresh: { type: "boolean", default: false },
    setup: { type: "boolean", default: false },
    port: { type: "string" },
    "no-build": { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});
if (opts.help === true) {
  console.log("usage: just clax-dev-local [--fresh] [--setup] [--port <8788-8797>] [--no-build]\n  (clax.dev's settings on this machine; data kept in target/clax-dev-local/, --fresh starts over, --setup leaves setup open)");
  process.exit(0);
}

function portTaken(p: number): Promise<boolean> {
  return new Promise((ok) => {
    const s = createServer();
    s.once("error", () => ok(true));
    s.listen(p, "127.0.0.1", () => s.close(() => ok(false)));
  });
}

async function choosePort(): Promise<number> {
  if (opts.port !== undefined) {
    const p = Number(opts.port);
    if (!PORTS.includes(p)) {
      console.error(`--port must be one of ${PORTS[0]}-${PORTS.at(-1)}: the GitHub App's callbacks name only those.`);
      process.exit(2);
    }
    if (await portTaken(p)) {
      console.error(`Port ${p} is in use.`);
      process.exit(1);
    }
    return p;
  }
  for (const p of PORTS) if (!(await portTaken(p))) return p;
  console.error(`Ports ${PORTS[0]}-${PORTS.at(-1)} are all in use.`);
  process.exit(1);
}

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

function openInBrowser(url: string): void {
  if (process.platform === "darwin" && process.stdout.isTTY) spawn("open", [url], { stdio: "ignore", detached: true }).unref();
}

/** The App's client ID and secret and the owner's login: the environment,
 * else GITHUB_ENV, else a new App registered from a manifest (see the
 * module docs) and kept in GITHUB_ENV. */
async function gitHubSettings(port: number): Promise<GitHub> {
  const kept = readEnvFile(GITHUB_ENV);
  const got = Object.fromEntries(GITHUB_KEYS.map((k) => [k, process.env[k] || kept[k] || ""])) as GitHub;
  if (GITHUB_KEYS.every((k) => got[k])) return got;
  const made = await registerApp(port);
  mkdirSync(dirname(GITHUB_ENV), { recursive: true, mode: 0o700 });
  writeFileSync(
    GITHUB_ENV,
    `# The GitHub App demo/clax-dev-local.ts registered (${made.html_url}).\n` +
      `GITHUB_CLIENT_ID=${made.client_id}\nGITHUB_CLIENT_SECRET=${made.client_secret}\nOWNER_GITHUB=${made.owner.login}\n`,
    { mode: 0o600 },
  );
  chmodSync(GITHUB_ENV, 0o600);
  console.log(`Registered the GitHub App ${made.html_url}, kept in ${GITHUB_ENV}.`);
  return { GITHUB_CLIENT_ID: made.client_id, GITHUB_CLIENT_SECRET: made.client_secret, OWNER_GITHUB: made.owner.login };
}

interface MadeApp {
  client_id: string;
  client_secret: string;
  html_url: string;
  owner: { login: string };
}

/** Serves the manifest form on `port` until GitHub redirects back with a
 * code, exchanges it, and resolves with the App. The page GitHub returns
 * to waits for the instance and then opens it. */
function registerApp(port: number): Promise<MadeApp> {
  const base = `http://localhost:${port}`;
  const state = randomBytes(16).toString("hex");
  const manifest = {
    name: `Clax local ${randomBytes(3).toString("hex")}`,
    url: base,
    redirect_url: `${base}/github-app/created`,
    callback_urls: PORTS.map((p) => `http://localhost:${p}/auth/github/callback`),
    public: true,
    default_permissions: {},
  };
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
  const page = (body: string) =>
    `<!doctype html><meta charset="utf-8"><title>Clax local</title><style>body{font:16px system-ui;max-width:36rem;margin:4rem auto;padding:0 1rem}button{font:inherit;padding:.5rem 1rem}</style>${body}`;
  return new Promise((ok, fail) => {
    const server = createHttpServer(async (req, res) => {
      const u = new URL(req.url ?? "/", base);
      res.setHeader("content-type", "text/html; charset=utf-8");
      if (u.pathname === "/github-app/created") {
        if (u.searchParams.get("state") !== state || !u.searchParams.get("code")) {
          res.writeHead(400).end(page("<p>That is not this run's GitHub App. Start again from the page the terminal names.</p>"));
          return;
        }
        const r = await fetch(`https://api.github.com/app-manifests/${encodeURIComponent(u.searchParams.get("code")!)}/conversions`, {
          method: "POST",
          headers: { accept: "application/vnd.github+json", "user-agent": "clax-dev-local" },
        });
        if (!r.ok) {
          const why = `GitHub refused the App's code (${r.status}): ${(await r.text()).slice(0, 300)}`;
          res.writeHead(502).end(page(`<p>${esc(why)}</p>`));
          server.closeAllConnections();
          server.close();
          fail(new Error(why));
          return;
        }
        res.end(
          page(
            "<h1>GitHub App made</h1><p>clax.dev is building and starting; this tab opens it when it is up.</p>" +
              "<script>(async()=>{for(;;){try{if((await fetch('/healthz')).ok){location.href='/';return}}catch{}await new Promise(r=>setTimeout(r,2000))}})()</script>",
          ),
        );
        server.closeAllConnections();
        server.close();
        ok((await r.json()) as MadeApp);
        return;
      }
      res.end(
        page(
          `<h1>Sign-in for your local clax.dev</h1><p>clax.dev signs people in with GitHub, so this one needs a GitHub App of its own. ` +
            `Create it once: GitHub asks you to confirm, then sends you back here. Every worktree's <code>just clax-dev-local</code> shares it.</p>` +
            `<form method="post" action="https://github.com/settings/apps/new?state=${state}">` +
            `<input type="hidden" name="manifest" value="${esc(JSON.stringify(manifest))}"><button>Create the GitHub App</button></form>`,
        ),
      );
    });
    server.once("error", fail);
    server.listen(port, "127.0.0.1", () => {
      console.log(`\nGitHub sign-in needs a GitHub App, made once: open ${base} and click Create the GitHub App.`);
      openInBrowser(base);
    });
  });
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

const port = await choosePort();
const base = `http://localhost:${port}`;
let github: GitHub;
try {
  github = await gitHubSettings(port);
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
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

let { setup_open } = (await fetch(`${base}/api/hosted/me`).then((r) => r.json())) as { setup_open: boolean };
if (setup_open && !opts.setup) {
  try {
    await setupFirstPerson(base, secrets.SETUP_SECRET, github.OWNER_GITHUB);
    setup_open = false;
  } catch (e) {
    console.error(`Setup failed: ${e instanceof Error ? e.message : e}`);
    await stop(1);
  }
}
const people = `Anyone else who signs in with GitHub signs up as a new person, with the quickstart in their gallery: try it in a private window with another GitHub account.`;
console.log(
  setup_open
    ? `
clax.dev is up at ${base}, with setup open.

Make the owner: open ${base}/setup and enter
  setup secret: ${secrets.SETUP_SECRET}
and your name. Then Sign in with GitHub as ${github.OWNER_GITHUB} links that account to the owner. ${people}

The data is kept for the next run; --fresh starts over.
Ctrl-C stops it.`
    : `
clax.dev is up at ${base}.

Open it and Sign in with GitHub as ${github.OWNER_GITHUB}: that account is the owner. ${people}

The data is kept for the next run; --fresh starts over (--setup to try setup itself).
Ctrl-C stops it.`,
);
