// What deploy.mjs decides from what it reads, as pure functions, so they are
// tested without an account (deploy-lib.test.mjs).

import { createHash, randomBytes } from "node:crypto";

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
 * deployed as `head`'s. A build from another commit is `head`'s too when
 * `trees` (commit to tree, as `git rev-parse <commit>^{tree}` names them)
 * gives both the same tree: Deploy prod checks out the merge commit on
 * prod and deploys the candidate built from the release branch's head,
 * whose tree it is. Without `head` (not a git checkout) there is nothing
 * to compare, and only a missing build is refused. */
export function builtFromRefusal(wasm, record, head, trees = {}) {
  if (!wasm) return "no index_bg.wasm: build first (drop --no-build)";
  if (!head) return null;
  const commit = builtCommitOf(record);
  const sameTree = commit !== null && trees[head] !== undefined && trees[commit] === trees[head];
  if (commit !== head && !sameTree) return `the Worker was not built from HEAD ${head.slice(0, 12)} or a commit with its tree: build it from this checkout (drop --no-build)`;
  if (JSON.parse(record).wasm_sha256 !== createHash("sha256").update(wasm).digest("hex")) return `index_bg.wasm changed after it was built from HEAD ${head.slice(0, 12)}: build it again (drop --no-build)`;
  return null;
}

/** The commit built-from.json's text `record` names (lowercase hex), or
 * null when there is no record, or it names none. */
export function builtCommitOf(record) {
  try {
    const c = JSON.parse(record)?.commit;
    return typeof c === "string" && /^[0-9a-f]{7,64}$/.test(c) ? c : null;
  } catch {
    return null;
  }
}

/** What a 200 from `/healthz` says while waiting for build `want`:
 * `{ done: true, kek }` once it reports that build (`kek`: whether it
 * reports a key-encryption key; undefined from a build before the field),
 * else `{ waiting }` naming what answers instead (an instance from before
 * this deploy). */
export function healthOf(body, want) {
  let j = null;
  try {
    j = JSON.parse(body);
  } catch {
    // not JSON
  }
  if (!j || typeof j !== "object") return { waiting: `/healthz answered something that is not its JSON (${String(body).slice(0, 80)}); waiting for build ${want}` };
  if (j.build === want) return { done: true, kek: j.kek };
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

/** The R2 bucket every preview's assets share (`envOf`): a preview's
 * objects expire after `PREVIEW_ASSET_DAYS` days, so a torn-down preview
 * leaves nothing behind for longer. */
export const PREVIEW_BUCKET = "clax-assets-preview";
export const PREVIEW_ASSET_DAYS = 30;

/** The Cloudflare resources the environment `name` deploys to
 * (docs/environments.md): `prod` (the default; clax.dev's own names),
 * `dev`, or a preview's name (`pr-<N>`, or any other lowercase name of
 * letters, digits and inner hyphens, at most 40 characters). Answers
 * `{ env, worker, bucket, queue, namespace, ephemeral }`: every
 * environment has a queue and Artifacts namespace of its own (a queue takes
 * one consumer Worker, and repository names carry no instance), and each
 * persistent one a bucket of its own (asset keys carry no instance either);
 * a preview is ephemeral (teardown.mjs deletes its Worker, queue and
 * repositories) and shares `PREVIEW_BUCKET`, whose objects expire. Throws
 * on any other name. */
export function envOf(name = "prod") {
  if (name === "prod") return { env: "prod", worker: "clax", bucket: "clax-assets", queue: "clax-artifacts-events", namespace: "clax", ephemeral: false };
  if (name === "dev") return { env: "dev", worker: "clax-dev", bucket: "clax-assets-dev", queue: "clax-artifacts-events-dev", namespace: "clax-dev", ephemeral: false };
  if (!/^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/.test(String(name))) {
    throw new Error(`--env takes prod, dev or a preview's name (lowercase letters, digits and inner hyphens, at most 40; e.g. pr-12), not ${name}`);
  }
  return { env: name, worker: `clax-${name}`, bucket: PREVIEW_BUCKET, queue: `clax-artifacts-events-${name}`, namespace: `clax-${name}`, ephemeral: true };
}

/** What `--staging` requires: the environment of a release pull request
 * into `prod` (`pr-<N>`, an ephemeral instance), a CI deploy (the runner
 * alone holds the owner token it makes: `secretsToMake`), and not
 * `--production` (staging is configured as production is, but is never
 * the production instance). Throws naming what is wrong. */
export function checkStaging({ staging, production, env, ci }) {
  if (!staging) return;
  if (production) throw new Error("--staging is not --production: staging is a release pull request's own instance");
  if (!/^pr-[1-9][0-9]*$/.test(String(env ?? ""))) throw new Error(`--staging takes --env pr-<N>, the release pull request's number, not ${env ?? "none"}`);
  if (!ci) throw new Error("--staging goes with --ci: its owner token is made for the runner's smoke");
}

/** The secrets a deploy generates, given `have`, the names the Worker
 * has: a staging deploy (`staging`) a new CLAX_TOKEN every time, for its
 * smoke to make an owner sign-in link and mint an agent token with (no
 * run can read the last one back), and no SETUP_SECRET (the owner signs in
 * with GitHub); a preview deployed by CI neither; any other deploy
 * SETUP_SECRET and CLAX_TOKEN when the Worker lacks them, or both with
 * `rotate`. */
export function secretsToMake({ have, ci, ephemeral, staging = false, rotate = false }) {
  if (staging) return ["CLAX_TOKEN"];
  if (ci && ephemeral) return [];
  return ["SETUP_SECRET", "CLAX_TOKEN"].filter((n) => rotate || !have.includes(n));
}

/** The zone a site domain's wildcard route is on: `zone` when given (the
 * site domain itself, or a domain it is under, as a preview's
 * `pr-12.<zone>` is), else the site domain. Throws when the site domain is
 * not in `zone`. */
export function siteZoneOf(site, zone) {
  if (zone === undefined || zone === null || zone === "") return site;
  const z = String(zone).toLowerCase().replace(/\.$/, "");
  if (site === z || site.endsWith(`.${z}`)) return z;
  throw new Error(`the site domain ${site} is not in the zone ${z} (--site-zone names the zone the site domain is, or is under)`);
}

/** The DNS name of the proxied wildcard record a site domain below its
 * zone needs (`*.pr-12.<zone>`), or null when the site domain is the zone
 * (whose `*` record the account keeps: README.md, "DNS for the public host
 * and the site domain"). */
export function wildcardRecordOf(site, zone) {
  return site === zone ? null : `*.${site}`;
}

/** What `--ci` requires of the options before anything runs: every
 * setting given as a flag (a CI run has no record and asks nothing), and
 * none of the options that need a person. Throws naming each flag
 * missing or refused. `env` is the environment (CLOUDFLARE_ACCOUNT_ID).
 * A preview (`o.ephemeral`) needs no --store (its default, Artifacts, is
 * the only one CI deploys it with), and needs GitHub sign-in: nobody can
 * sign in to it otherwise. */
export function checkCiOptions(o, env = {}) {
  const missing = [];
  if (!o.public && !o.zone) missing.push("--public <host>");
  if (!o["site-domain"]) missing.push("--site-domain <domain>");
  if (!o.storeGiven && !o.ephemeral) missing.push("--store artifacts|memory");
  if (o.ephemeral && !o["github-client-id"]) missing.push("--github-client-id <ID> (a preview's only sign-in is GitHub's)");
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
 * GITHUB_CLIENT_SECRET has its secret). Answers the reason, or null.
 *
 * A preview (`ephemeral`) is first deployed by CI: it is given no
 * SETUP_SECRET (setup stays closed) and no CLAX_TOKEN (the instance makes
 * its own), the owner signs in with GitHub, and a missing CLAX_KEK is
 * generated with no backup kept, since its data is thrown away with it. */
export function ciSecretsRefusal(have, { githubClientId, githubSecretGiven, ephemeral = false }) {
  const missing = ephemeral ? [] : ["SETUP_SECRET", "CLAX_TOKEN", "CLAX_KEK"].filter((n) => !have.includes(n));
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

const KEK_HEX = /^[0-9a-f]{64}$/i;

/** Where to read about CLAX_KEK's backup. */
const KEK_DOC = 'docs/prod.md, "CLAX_KEK"';

/** What the deploy does about `CLAX_KEK`, ClaxBot's key-encryption key,
 * given `have`, the secret names the Worker has (`null` without a login),
 * `given`, the key in the deploy's CLAX_KEK (from the backup its operator
 * keeps; never from a file the deploy keeps), and whether the Worker is
 * a preview's (`ephemeral`). Answers `upload` (the key to upload, or
 * null), `say` (the line it prints, never the key) and `error` (why the
 * deploy refuses, or null):
 *
 * - a Worker with CLAX_KEK keeps it: never rotated, since a new key opens
 *   nothing the old one sealed, and never compared with `given`, which no
 *   one can read back from the Worker (an admin compares fingerprints);
 * - a Worker without one gets `given`, which must be 64 hex digits;
 * - a preview's gets a new key, kept nowhere else (its stored keys go with
 *   it);
 * - any other is refused: its key is made and backed up by its operator,
 *   outside any machine, never by a deploy, so no machine is the only copy.
 *
 * A dry run uploads and generates nothing. */
export function kekPlan({ have, given, ephemeral = false, dryRun = false }) {
  const g = given?.trim() || null;
  if (g && !KEK_HEX.test(g)) return { upload: null, say: "", error: "CLAX_KEK is not 64 hex digits (32 random bytes)" };
  if (have === null) return { upload: null, say: "skipped (no login): a deploy keeps the Worker's CLAX_KEK, or uploads the one given", error: null };
  if (have.includes("CLAX_KEK")) {
    const note = g ? "; the CLAX_KEK given is not uploaded (compare fingerprints in Admin, Provider)" : "";
    return { upload: null, say: `the Worker has CLAX_KEK; kept (never rotated)${note}`, error: null };
  }
  if (g) return { upload: dryRun ? null : g, say: `${dryRun ? "would upload" : "uploading"} the CLAX_KEK given with the deploy`, error: null };
  if (ephemeral) {
    return { upload: dryRun ? null : randomBytes(32).toString("hex"), say: `${dryRun ? "would generate" : "generated"} CLAX_KEK for this preview (kept nowhere else)`, error: null };
  }
  return {
    upload: null,
    say: "",
    error:
      "the Worker has no CLAX_KEK and none was given: give the instance's key from its backup in CLAX_KEK, " +
      `or make one and back it up first (${KEK_DOC})`,
  };
}

/** What deploy.mjs prints when it is done. The first-time setup text (the
 * secrets shown once, the setup URL, the next steps) only when this deploy
 * generated SETUP_SECRET or CLAX_TOKEN (`generated`, the secrets it made),
 * and never for a CI deploy (`ci`), whose log may be public: its record
 * keeps them; every deploy names the record and the logs. */
export function doneText({ pub, name, diag, generated, record, smoke, ci = false }) {
  const first = !ci && Boolean(generated?.SETUP_SECRET || generated?.CLAX_TOKEN);
  const lines = [];
  if (first) {
    lines.push("", "   Shown once (also in the record below, 0600; delete the record once set up):");
    if (generated.SETUP_SECRET) lines.push(`     SETUP_SECRET  ${generated.SETUP_SECRET}`);
    if (generated.CLAX_TOKEN) lines.push(`     CLAX_TOKEN    ${generated.CLAX_TOKEN}`);
  }
  lines.push("", `   Record:  ${record}`);
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
    "claxbot: one web search and one fetch (AI binding)": "the admin's web settings refuse an agent token (403 not_for_agents)",
  },
};

/** Why credential `as` cannot take smoke step `name`, or null when it can. */
export function smokeCannot(as, name) {
  return SMOKE_CANNOT[as]?.[name] ?? null;
}

/** The Worker vars that serve the landing page's reel from elsewhere
 * (src/instance.rs: REEL_URL, REEL_POSTER_URL), from `--reel-url` and
 * `--reel-poster-url`: none without `--reel-url`, so the page shows no
 * player. Throws when an address is not https://, or a poster comes
 * without a video. */
export function reelVars(o) {
  const video = o["reel-url"], poster = o["reel-poster-url"];
  if (video === undefined) {
    if (poster !== undefined) throw new Error("--reel-poster-url needs --reel-url");
    return {};
  }
  for (const [flag, v] of [["--reel-url", video], ["--reel-poster-url", poster]]) {
    if (v === undefined) continue;
    let ok = false;
    try { ok = new URL(v).protocol === "https:"; } catch { /* not a URL */ }
    if (!ok) throw new Error(`${flag} takes an https:// URL, not ${v}`);
  }
  return { REEL_URL: video, ...(poster ? { REEL_POSTER_URL: poster } : {}) };
}

/** What a failed wrangler run `r` ({code, out, err}) says went wrong, on
 * one line without colour or wrangler's "✘ [ERROR]": its first error line, with the lines after it
 * up to wrangler's "If you think this is a bug" footer, since wrangler puts
 * the Cloudflare API's own reason (and its code) on the line after its
 * "A request to the Cloudflare API (…) failed."; else the last line it
 * printed, else its exit code. */
export function wranglerError(r) {
  // eslint-disable-next-line no-control-regex
  const lines = `${r.err}\n${r.out}`.replace(/\x1b\[[0-9;]*m/g, "").split("\n").map((l) => l.trim()).filter(Boolean);
  const at = lines.findIndex((l) => /^✘|ERROR|error/i.test(l));
  if (at < 0) return lines.at(-1) ?? `exit ${r.code}`;
  const end = lines.findIndex((l, i) => i > at && (/^If you think this is a bug/.test(l) || l.startsWith("🪵")));
  return lines.slice(at, end < 0 ? undefined : end).join(" ").replace(/^✘ \[ERROR\] /, "");
}

/** The Worker's errors in `text`, the output of `wrangler tail --format
 * json` (one JSON object per invocation, one after another): for each
 * invocation, its exceptions, its logs at level error or warn, and its
 * outcome when not "ok", one line each, headed by the invocation's time
 * and what it served (a request's method and path, without its query; a
 * queue batch; an alarm; a scheduled run). Nothing else of an invocation is
 * printed: not its headers, its query or its `cf` object. Credentials are
 * masked (maskTokens). Text that is not such an object is skipped. A last
 * line counts the invocations of each kind, naming a log without a queue
 * batch, since an event that never arrived logs nothing. */
export function tailErrors(text) {
  const out = [];
  const seen = { request: 0, queue: 0, alarm: 0, scheduled: 0, other: 0 };
  for (const ev of jsonObjects(text)) {
    const k = ev.event?.request?.url ? "request" : ev.event?.queue ? "queue" : ev.event?.cron !== undefined ? "scheduled"
      : ev.eventType === "alarm" || ev.event?.scheduledTime ? "alarm" : "other";
    seen[k]++;
    const at = Number.isFinite(ev.eventTimestamp) ? new Date(ev.eventTimestamp).toISOString() : "?";
    const e = ev.event ?? {};
    let what = "invocation";
    if (e.request?.url) {
      let path = e.request.url;
      try { path = new URL(e.request.url).pathname; } catch { /* not a URL */ }
      what = `${e.request.method ?? "?"} ${path}${e.response?.status ? ` → ${e.response.status}` : ""}`;
    } else if (e.queue) what = `queue ${e.queue} (${e.batchSize ?? "?"} message(s))`;
    else if (e.cron !== undefined) what = `scheduled ${e.cron}`;
    else if (ev.eventType === "alarm" || e.scheduledTime) what = "alarm";
    const head = `${at} ${ev.scriptName ?? ""}${ev.entrypoint ? `:${ev.entrypoint}` : ""} ${what}`.replace(/ {2,}/g, " ");
    const lines = [];
    if (ev.outcome && ev.outcome !== "ok") lines.push(`outcome ${ev.outcome}`);
    for (const x of ev.exceptions ?? []) lines.push(`exception ${x.name ?? "Error"}: ${x.message ?? ""}`);
    for (const l of ev.logs ?? []) {
      if (l.level !== "error" && l.level !== "warn") continue;
      const msg = (Array.isArray(l.message) ? l.message : [l.message]).map((m) => (typeof m === "string" ? m : JSON.stringify(m))).join(" ");
      lines.push(`${l.level} ${msg}`);
    }
    for (const l of lines) out.push(maskTokens(`${head}: ${l}`));
  }
  const total = Object.values(seen).reduce((a, b) => a + b, 0);
  if (total) {
    out.push(`${total} invocation(s) in the log: ` +
      Object.entries(seen).filter(([, n]) => n).map(([k, n]) => `${n} ${k}`).join(", ") +
      (seen.queue ? "" : " (no queue batch)"));
  }
  return out;
}

/** `s` with credentials masked: Artifacts' tokens (`art_v1_…`), a bearer
 * credential, and any run of 32 or more token characters (Clax's tokens,
 * secrets and cookies; a commit's 40-hex ID is masked with them, which the
 * errors can spare). */
export function maskTokens(s) {
  return s
    .replace(/art_v1_[A-Za-z0-9_.-]+(\?expires=\d+)?/g, "art_v1_<masked>")
    .replace(/\b(Bearer|bearer)\s+\S+/g, "$1 <masked>")
    .replace(/[A-Za-z0-9_=-]{32,}/g, "<masked>");
}

/** Each top-level JSON object in `text`, which holds objects one after
 * another (pretty-printed or not) among other lines. */
function* jsonObjects(text) {
  let depth = 0, start = -1, inString = false, escaped = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"' && depth > 0) inString = true;
    else if (ch === "{") {
      if (depth === 0) start = i;
      depth++;
    } else if (ch === "}" && depth > 0) {
      depth--;
      if (depth === 0) {
        try {
          const v = JSON.parse(text.slice(start, i + 1));
          if (v && typeof v === "object") yield v;
        } catch { /* not JSON after all */ }
      }
    }
  }
}
