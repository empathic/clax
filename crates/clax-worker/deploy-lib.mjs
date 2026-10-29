// What deploy.mjs decides from what it reads, as pure functions, so they are
// tested without an account (deploy-lib.test.mjs).

import { createHash } from "node:crypto";

/** Whether a failed `wrangler deploy`'s output (stdout and stderr) says
 * only its triggers were left half-done: the script was uploaded, but the
 * routes or custom domains attached to it were "only partially updated"
 * (a Cloudflare API error such as code 10013 on the custom-domains call).
 * Running the same deploy again finishes them; any other failure is not
 * this one. */
export function partialTriggerUpdate(text) {
  const s = String(text ?? "");
  if (!/\bpartially updated\b/i.test(s)) return false;
  return /\btrigger configuration\b/i.test(s) || /\bcustom[ -]?domains?\b/i.test(s);
}

/** A build identity unique to one upload, which deploy.mjs gives the Worker
 * as `CLAX_BUILD` and `/healthz` reports as `build`: the commit's first
 * twelve characters (when known), the UTC time and four random bytes, e.g.
 * `0a2f4805a1b2-20261023T101500Z-9f86d081`. */
export function buildId(commit, now = new Date(), random = "") {
  const at = now.toISOString().replace(/[-:]/g, "").replace(/\.\d+Z$/, "Z");
  return [commit ? commit.slice(0, 12) : "", at, random].filter(Boolean).join("-");
}

/** The Worker version a `wrangler deploy` made, from its output's
 * "Current Version ID: <UUID>" line (the last, when it printed several),
 * or null when it named none. `wrangler rollback <version ID>` takes it. */
export function versionIdOf(text) {
  const ids = [...String(text ?? "").matchAll(/Current Version ID:\s*([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\b/gi)];
  return ids.length ? ids[ids.length - 1][1].toLowerCase() : null;
}

/** Why a `--no-build` deploy must not upload `wasm` (index_bg.wasm's
 * bytes as a Buffer, null when there is none), or null when it may.
 * `record` is built-from.json's text, which build.sh writes beside the
 * build when given `CLAX_BUILD_COMMIT` (null when there is none): the
 * commit and the sha256 of the index_bg.wasm it built. A build is `head`'s
 * only when its record names `head` and hashes these bytes, so a build
 * from another commit, from none, or rebuilt since is refused rather than
 * deployed as `head`'s. Without `head` (not a git checkout) there is
 * nothing to compare, and only a missing build is refused. */
export function builtFromRefusal(wasm, record, head) {
  if (!wasm) return "no index_bg.wasm: build first (drop --no-build)";
  if (!head) return null;
  let built = null;
  try {
    built = JSON.parse(record);
  } catch {
    // no record, or not its JSON
  }
  if (built?.commit !== head) return `the Worker was not built from HEAD ${head.slice(0, 12)}: build it from this checkout (drop --no-build)`;
  if (built.wasm_sha256 !== createHash("sha256").update(wasm).digest("hex")) return `index_bg.wasm changed after it was built from HEAD ${head.slice(0, 12)}: build it again (drop --no-build)`;
  return null;
}

/** What a 200 from `/healthz` says while waiting for build `want`:
 * `{ done: true }` once it reports that build, else `{ waiting }` naming
 * what answers instead (an instance from before this deploy). */
export function healthOf(body, want) {
  let j = null;
  try {
    j = JSON.parse(body);
  } catch {
    // not JSON
  }
  if (!j || typeof j !== "object") return { waiting: `/healthz answered something that is not its JSON (${String(body).slice(0, 80)}); waiting for build ${want}` };
  if (j.build === want) return { done: true };
  const was = j.build ? `build ${j.build}` : "a build that reports none";
  return { waiting: `the instance from before this deploy (${was}, started ${j.started_at ?? "?"}) still answers; waiting for build ${want}` };
}

/** The commit `git ls-remote origin refs/heads/prod` printed for prod, or
 * null when it printed none (no such branch, or the call failed). */
export function prodTipOf(lsRemote) {
  const line = String(lsRemote ?? "")
    .split("\n")
    .map((l) => l.trim().split(/\s+/))
    .find(([sha, ref]) => ref === "refs/heads/prod" && /^[0-9a-f]{40,64}$/.test(sha ?? ""));
  return line ? line[0] : null;
}

/** Whether a deploy of the production instance may go on. A deploy given
 * `production` runs only from a clean checkout whose HEAD is origin's
 * prod tip (`prodTip`, null when origin has none), and only in GitHub
 * Actions (`githubActions`) unless `fromLaptop` lifts that one condition.
 * Answers `{ ok: true, say }`, or `{ refuse }` naming what is wrong; a dry
 * run changes nothing, so it is never refused and `say` names what a
 * deploy would refuse. */
export function productionGate({ production, fromLaptop, githubActions, clean, head, prodTip, dryRun }) {
  if (!production) {
    if (fromLaptop) return { refuse: "--from-laptop goes with --production" };
    return { ok: true, say: null };
  }
  const why = [];
  if (!githubActions && !fromLaptop) {
    why.push("production deploys run from GitHub Actions on pushes to prod (docs/prod.md); --from-laptop deploys from here instead");
  }
  if (!clean) why.push("the checkout has changes: commit or remove them");
  if (!prodTip) why.push("origin has no branch prod (git ls-remote origin refs/heads/prod printed none)");
  else if (head !== prodTip) why.push(`HEAD is ${head ? head.slice(0, 12) : "unknown"}, not origin's prod tip ${prodTip.slice(0, 12)}`);
  if (why.length === 0) {
    return { ok: true, say: `production: HEAD is origin's prod tip ${prodTip.slice(0, 12)}, clean, ${githubActions ? "in GitHub Actions" : "from a laptop (--from-laptop)"}` };
  }
  if (dryRun) return { ok: true, say: `production: a deploy would refuse: ${why.join("; ")}` };
  return { refuse: `refusing to deploy the production instance: ${why.join("; ")}` };
}

/** What `--ci` requires of the options before anything runs: every
 * setting given as a flag (a CI run has no record and asks nothing), and
 * none of the options that need a person. Throws naming each flag
 * missing or refused. `env` is the environment (CLOUDFLARE_ACCOUNT_ID). */
export function checkCiOptions(o, env = {}) {
  const missing = [];
  if (!o.public && !o.zone) missing.push("--public <host>");
  if (!o["site-domain"]) missing.push("--site-domain <domain>");
  if (!o.storeGiven) missing.push("--store artifacts|memory");
  if (o.legal === undefined && !o["terms-url"] && !o["privacy-url"] && !o["legal-version"]) {
    missing.push("--legal bundled (or --terms-url, --privacy-url and --legal-version)");
  }
  if (!o.account && !env.CLOUDFLARE_ACCOUNT_ID) missing.push("--account <ID> (or CLOUDFLARE_ACCOUNT_ID)");
  if (o["github-client-id"] && !o["owner-github"]) missing.push("--owner-github <login> (with --github-client-id)");
  const refused = [];
  if (o.rotate) refused.push("--rotate-secrets (new secrets are shown once, on a terminal: rotate from a laptop)");
  if (o.diag) refused.push("--diag (the diagnostics routes take the owner token, which a CI run does not have)");
  const parts = [];
  if (missing.length) parts.push(`--ci asks nothing: give ${missing.join(", ")}`);
  if (refused.length) parts.push(`--ci takes no ${refused.join(", no ")}`);
  if (parts.length) throw new Error(parts.join("; "));
}

/** What a `--ci` deploy refuses about the Worker's secrets, given `have`,
 * the names the Worker has: first-time setup stays on a laptop (it
 * generates secrets that are shown once and backs up CLAX_KEK on the
 * machine that ran it), so a Worker missing SETUP_SECRET, CLAX_TOKEN or
 * CLAX_KEK is refused; so is a GitHub sign-in that would turn off (the
 * Worker has GITHUB_CLIENT_SECRET but no --github-client-id was given) or
 * stay off (--github-client-id, and neither the Worker nor
 * GITHUB_CLIENT_SECRET has its secret). Answers the reason, or null. */
export function ciSecretsRefusal(have, { githubClientId, githubSecretGiven }) {
  const missing = ["SETUP_SECRET", "CLAX_TOKEN", "CLAX_KEK"].filter((n) => !have.includes(n));
  if (missing.length) {
    return `the Worker has no ${missing.join(", ")} (or wrangler secret list failed): its first deploy, which generates them, runs from a laptop (scripts/cloudflare-setup.sh), not --ci`;
  }
  if (!githubClientId && have.includes("GITHUB_CLIENT_SECRET")) {
    return "the Worker has GITHUB_CLIENT_SECRET but no --github-client-id was given: this deploy would turn GitHub sign-in off; give --github-client-id and --owner-github";
  }
  if (githubClientId && !githubSecretGiven && !have.includes("GITHUB_CLIENT_SECRET")) {
    return "--github-client-id, but neither the Worker nor GITHUB_CLIENT_SECRET has the client secret";
  }
  return null;
}

/** What deploy.mjs prints when it is done. The first-time setup text (the
 * secrets shown once, the setup URL, the next steps) only when this deploy
 * generated SETUP_SECRET or CLAX_TOKEN (`generated`, the secrets it made);
 * every deploy names the record and the logs, and the CLAX_KEK backup
 * when `kek` names it (a --ci deploy keeps none). */
export function doneText({ pub, name, diag, generated, record, kek, smoke }) {
  const first = Boolean(generated?.SETUP_SECRET || generated?.CLAX_TOKEN);
  const lines = [];
  if (first) {
    lines.push("", "   Shown once (also in the record below, 0600; delete the record once set up):");
    if (generated.SETUP_SECRET) lines.push(`     SETUP_SECRET  ${generated.SETUP_SECRET}`);
    if (generated.CLAX_TOKEN) lines.push(`     CLAX_TOKEN    ${generated.CLAX_TOKEN}`);
  }
  lines.push("", `   Record:  ${record}`);
  if (kek) {
    lines.push(
      `   CLAX_KEK backup: ${kek} (0600; keep it: without it and the Worker's`,
      "            secret, every model key ClaxBot stored is lost)",
    );
  }
  if (first) {
    lines.push(
      `   Setup:   https://${pub}/setup  (enter SETUP_SECRET and your name; this browser becomes the owner)`,
      "",
      "   Next:",
      "   1. Smoke the platform first (it can do the setup itself if you have not):",
      `        ${smoke}`,
      `   2. Open https://${pub}/setup in your browser, or, if the smoke did the setup,`,
      "      sign in with the link it printed.",
      `   3. Logs: npx wrangler tail ${name}`,
    );
    if (diag) lines.push("   4. Once the smoke is green, turn the diagnostics off: the same command without --diag");
  } else {
    lines.push(`   Logs:    npx wrangler tail ${name}`);
    if (diag) lines.push("   Once the smoke is green, turn the diagnostics off: the same command without --diag");
  }
  return lines.join("\n");
}

/** Who smoke-remote.mjs acts as, in this order: the owner's browser
 * session when given ("session"); an agent token when given ("agent", as
 * the deploy workflow's smoke with CLAX_SMOKE_TOKEN); the owner made by
 * setup with the secret while setup is open ("setup"); the owner token
 * ("token", every deploy after the first); else null, which fails the
 * smoke. */
export function smokeCredential({ session, agent, setupOpen, secret, token }) {
  if (session) return "session";
  if (agent) return "agent";
  if (setupOpen && secret) return "setup";
  if (token) return "token";
  return null;
}

const BROWSER_SIGNIN = "only a signed-in browser is signed in to a site at /site-signin (--session <cookie> runs this step)";
const OWNER_TOKEN = "the diagnostics take the owner token, which an agent token's smoke does not use";

/** The smoke's steps a credential cannot take, by credential, each with
 * why: the step is skipped and the reason printed. The owner token cannot
 * take what only a browser session can (minting an agent token, a site's
 * sign-in); an agent token cannot take what only its person can (the
 * owner token's diagnostics, the browser's pages, sign-ins and stream, and
 * opening a site to the world, which waits on the person's approval).
 * Every other step runs as the credential. */
export const SMOKE_CANNOT = {
  token: {
    "agent token": "only a signed-in browser mints agent tokens (--session <cookie> runs this step); the MCP steps use the owner token",
    "site: /site-signin, then /_clax/session (SameSite=Lax)": BROWSER_SIGNIN,
    "site: two db artifacts kept apart": `each site is signed in to at /site-signin; ${BROWSER_SIGNIN}`,
  },
  agent: {
    "diagnostics: env": OWNER_TOKEN,
    "artifacts: the binding's raw calls (probe)": OWNER_TOKEN,
    "artifacts: tree hashes (binding vs clone)": OWNER_TOKEN,
    "web UI: the shell's pages": "the shell's pages refuse an agent token (403 forbidden): they serve a person's browser",
    "agent token": "the smoke is an agent token already; minting one takes the owner's browser session",
    "site: /site-signin, then /_clax/session (SameSite=Lax)": BROWSER_SIGNIN,
    "site: the shell's code (state-bound, Partitioned, single use)": "only the owner's browser asks for a site's code (an agent token is refused 403)",
    "site: open to the world, then private again": "opening a site to the world waits on its person's approval",
    "site: two db artifacts kept apart": `each site is signed in to at /site-signin; ${BROWSER_SIGNIN}`,
    "live: a comment on the WebSocket stream": "the live stream is a browser's: an agent token's socket is refused",
  },
};

/** Why credential `as` cannot take smoke step `name`, or null when it can. */
export function smokeCannot(as, name) {
  return SMOKE_CANNOT[as]?.[name] ?? null;
}
