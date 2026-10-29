// clax.dev on this machine (`just clax-dev-local`):
//
//   node demo/clax-dev-local.ts [--fresh] [--owner] [--setup] [--port <8788-8797>] [--no-build]
//
// The Worker (crates/clax-worker) under `wrangler dev --local` with the
// settings a production deploy gives it (crates/clax-worker/deploy.mjs,
// the vars it writes): hosted, GitHub sign-in against real GitHub, the
// bundled Terms of Service and Privacy Policy, a CLAX_KEK, and no test,
// dev or diagnostics routes. What differs is what cannot run here: the
// origins are http://localhost:<port> and each artifact's site
// <ID>.localhost:<port>, and versions live in the RepoDO stand-in
// (REPO_STORE=memory), not Cloudflare Artifacts, and the landing page's
// reel is served by this process rather than Cloudflare Stream, once
// //web/reel:reel has built in the background (target/clax-dev-local/
// reel.log; --no-build serves the one built before). Nothing is seeded.
//
// Nothing to configure. GitHub sign-in needs a GitHub App; the first run
// registers one from a manifest (GitHub's "create an App from a manifest"
// flow): it serves a page on the port with one button, GitHub asks to
// confirm, and the code GitHub returns is exchanged for the App's client
// ID and secret. They and the App owner's login are kept in
// ~/.config/clax/local/github.env (0600; $XDG_CONFIG_HOME moves it),
// shared by every worktree; the environment's GITHUB_CLIENT_ID,
// GITHUB_CLIENT_SECRET and OWNER_GITHUB win over it. Delete that file to
// register a new App. The App's callbacks are ports 8788-8797 (GitHub
// requires an exact match and allows ten), and a run takes the first free
// one, so several worktrees can each run one; browsers share localhost's
// cookies across ports, so use one browser profile per instance.
//
// The owner is made through setup, as a first deploy's smoke does, under
// the name Owner and with no GitHub account, as clax.dev's owner is
// Empathic's and not a visitor's: your GitHub sign-in is then a new
// person's sign-up, which agrees to the Terms and gets the quickstart (the
// owner's gallery is never seeded). The owner's session is kept, and each
// run prints a fresh owner sign-in link. --owner makes the App owner's
// GitHub account the owner instead (OWNER_GITHUB): their first Sign in
// with GitHub links it. --setup leaves setup open and prints the setup URL
// and secret, to try /setup itself. The instance's data, its generated
// secrets and the owner's session (secrets.json, 0600), the build log and
// wrangler's log are kept in target/clax-dev-local/ across runs; --fresh
// starts over. Ctrl-C stops the Worker by its process group.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { chmodSync, copyFileSync, createReadStream, existsSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer, type AddressInfo } from "node:net";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { BuildLog } from "./lib/build.ts";
import { PersonSession, setupFirstPerson, signinLink, type PersonInfo } from "./lib/http.ts";
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
    owner: { type: "boolean", default: false },
    port: { type: "string" },
    "no-build": { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});
if (opts.help === true) {
  console.log("usage: just clax-dev-local [--fresh] [--owner] [--setup] [--port <8788-8797>] [--no-build]\n  (clax.dev's settings on this machine; data kept in target/clax-dev-local/, --fresh starts over, --owner makes your GitHub account the owner, --setup leaves setup open)");
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

/** What is kept beside the instance's data: the secrets a first deploy
 * generates (the setup secret, CLAX_KEK as 64 hex digits) and, once setup
 * made the owner here, their session, to mint them sign-in links. */
interface Kept {
  SETUP_SECRET: string;
  CLAX_KEK: string;
  owner?: { person: PersonInfo; cookie: string };
}
const KEPT = join(DIR, "secrets.json");

function keep(k: Kept): void {
  writeFileSync(KEPT, JSON.stringify(k, null, 2) + "\n", { mode: 0o600 });
}

function instanceSecrets(): Kept {
  if (existsSync(KEPT)) return JSON.parse(readFileSync(KEPT, "utf8"));
  const k = { SETUP_SECRET: randomBytes(18).toString("base64url"), CLAX_KEK: randomBytes(32).toString("hex") };
  keep(k);
  return k;
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

// The landing page's reel, as clax.dev serves it from Cloudflare Stream:
// here from this process, at REEL_URL and REEL_POSTER_URL. It is built in
// the background once the Worker is up (//web/reel:reel, which records the
// vignettes on its first build and is cached after), copied to
// target/clax-dev-local/reel/, and 404 until then.
const REEL_DIR = join(DIR, "reel");
const REEL_FILES: Record<string, string> = { "/reel.mp4": "video/mp4", "/reel-poster.jpg": "image/jpeg" };
const reelServer = createHttpServer((req, res) => {
  const type = REEL_FILES[req.url ?? ""];
  const file = type && join(REEL_DIR, req.url!.slice(1));
  if (!file || !existsSync(file)) {
    res.writeHead(404).end();
    return;
  }
  const size = statSync(file).size;
  const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? "");
  const head = { "content-type": type, "accept-ranges": "bytes", "cache-control": "no-cache", "access-control-allow-origin": "*" };
  if (range && (range[1] || range[2])) {
    const start = range[1] ? Number(range[1]) : Math.max(0, size - Number(range[2]));
    const end = range[1] && range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
    if (start > end || start >= size) {
      res.writeHead(416, { "content-range": `bytes */${size}` }).end();
      return;
    }
    res.writeHead(206, { ...head, "content-range": `bytes ${start}-${end}/${size}`, "content-length": end - start + 1 });
    createReadStream(file, { start, end }).pipe(res);
    return;
  }
  res.writeHead(200, { ...head, "content-length": size });
  createReadStream(file).pipe(res);
});
const reelBase = await new Promise<string>((ok) =>
  reelServer.listen(0, "127.0.0.1", () => ok(`http://localhost:${(reelServer.address() as AddressInfo).port}`)),
);

let reelBuild: ChildProcess | null = null;
/** Builds the reel in the background, logged to reel.log, and copies it to REEL_DIR when done. */
function buildReel(): void {
  const bazel = ["bazelisk", "bazel"].find((b) => spawnSync("sh", ["-c", `command -v ${b}`]).status === 0);
  if (!bazel) {
    console.log(`No Bazel on PATH: the landing page shows no reel${existsSync(REEL_DIR) ? " but the one built before" : ""}.`);
    return;
  }
  const log = join(DIR, "reel.log");
  console.log(`Building the landing page's reel in the background (log ${log}); the first build records every vignette and takes several minutes.`);
  const fd = openSync(log, "w");
  reelBuild = spawn(bazel, ["build", "//web/reel:reel"], { cwd: ROOT, stdio: ["ignore", fd, fd] });
  reelBuild.once("exit", (code) => {
    reelBuild = null;
    if (code !== 0) {
      if (!stopping) console.log(`\nThe reel did not build; see ${log}.`);
      return;
    }
    // Bazel's outputs are read-only and a copy keeps their mode, so each is
    // copied beside its old copy, made writable and renamed over it: the
    // next run replaces it, and the server never serves half a file.
    try {
      mkdirSync(REEL_DIR, { recursive: true });
      for (const f of ["reel.mp4", "reel-poster.jpg"]) {
        const to = join(REEL_DIR, f);
        rmSync(`${to}.new`, { force: true });
        copyFileSync(join(ROOT, "bazel-bin/web/reel", f), `${to}.new`);
        chmodSync(`${to}.new`, 0o644);
        renameSync(`${to}.new`, to);
      }
    } catch (e) {
      console.log(`\nThe reel built but could not be copied to ${REEL_DIR}: ${(e as Error).message}`);
      return;
    }
    console.log(`\nThe landing page's reel is built: reload ${base}/start/ to see it.`);
  });
}

console.log(`Starting clax.dev's Worker on :${port} (data ${join(DIR, "state")}, log ${join(DIR, "wrangler.log")})…`);
let w: RunningWorker;
try {
  w = await startWorker({
    dir: DIR,
    port,
    secret: secrets.SETUP_SECRET,
    seats: "",
    vars: {
      GITHUB_CLIENT_ID: github.GITHUB_CLIENT_ID,
      GITHUB_CLIENT_SECRET: github.GITHUB_CLIENT_SECRET,
      ...(opts.owner ? { OWNER_GITHUB: github.OWNER_GITHUB } : {}),
      CLAX_KEK: secrets.CLAX_KEK,
      LEGAL: "bundled",
      REEL_URL: `${reelBase}/reel.mp4`,
      REEL_POSTER_URL: `${reelBase}/reel-poster.jpg`,
    },
  });
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
let stopping = false;
async function stop(code: number): Promise<never> {
  stopping = true;
  reelBuild?.kill("SIGINT");
  reelServer.close();
  await w.stop();
  process.exit(code);
}
process.on("SIGINT", () => void stop(0));
process.on("SIGTERM", () => void stop(0));
// A crash stops the Worker too, rather than leaving it holding the port.
process.on("uncaughtException", (e) => {
  console.error(e);
  void stop(1);
});
process.on("unhandledRejection", (e) => {
  console.error(e);
  void stop(1);
});
w.child.once("exit", () => {
  if (stopping) return;
  console.error(`The Worker exited; see ${w.log}:\n${readFileSync(w.log, "utf8").slice(-2000)}`);
  process.exit(1);
});

let { setup_open } = (await fetch(`${base}/api/hosted/me`).then((r) => r.json())) as { setup_open: boolean };
if (setup_open && !opts.setup) {
  try {
    const owner = await setupFirstPerson(base, secrets.SETUP_SECRET, opts.owner ? github.OWNER_GITHUB : "Owner");
    secrets.owner = { person: owner.person, cookie: owner.cookie };
    keep(secrets);
    setup_open = false;
  } catch (e) {
    console.error(`Setup failed: ${e instanceof Error ? e.message : e}`);
    await stop(1);
  }
}

/** A one-use sign-in link for the owner setup made here, or why there is none. */
async function ownerLink(): Promise<string> {
  if (!secrets.owner) return "none: setup was not done by this script (--fresh starts over)";
  try {
    return await signinLink(new PersonSession(base, secrets.owner.person, secrets.owner.cookie));
  } catch (e) {
    return `none: ${e instanceof Error ? e.message : e} (--fresh starts over)`;
  }
}

if (opts["no-build"]) console.log(existsSync(REEL_DIR) ? "The landing page's reel is the one built before." : "No reel is built: the landing page shows none.");
else buildReel();

const you = opts.owner
  ? `Sign in with GitHub as ${github.OWNER_GITHUB}: that account is the owner (the owner's gallery gets no quickstart).`
  : `Sign in with GitHub: you sign up as a new person, as a visitor to clax.dev does, agree to the Terms and get the quickstart in your gallery.`;
console.log(
  setup_open
    ? `
clax.dev is up at ${base}, with setup open.

Make the owner: open ${base}/setup and enter
  setup secret: ${secrets.SETUP_SECRET}
and a name.${opts.owner ? ` Then Sign in with GitHub as ${github.OWNER_GITHUB} links that account to the owner.` : ""}

The data is kept for the next run; --fresh starts over.
Ctrl-C stops it.`
    : `
clax.dev is up at ${base}.

Open it and ${you}
Another GitHub account in a private window signs up as another person.
${opts.owner ? "" : `\nThe owner (People, admin), in another browser profile: ${await ownerLink()}\n  (works once, for 10 minutes; the next run prints a new one)\n`}
The data is kept for the next run; --fresh starts over (--setup to try setup itself).
Ctrl-C stops it.`,
);
