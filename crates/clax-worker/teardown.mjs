// Tears down a preview environment (docs/environments.md), or every
// preview whose pull request is closed:
//
//   node teardown.mjs pr-12 --site-zone example-preview.run [--account <ID>] [--dry-run]
//   node teardown.mjs --reap --site-zone example-preview.run --repo empathic/clax [--account <ID>] [--dry-run]
//
// A preview <name> (deploy-lib.mjs envOf) is the Worker clax-<name>, its
// custom domain (its public host), its route *.<name>.<site zone>/* and
// the proxied DNS record *.<name>.<site zone> deploy.mjs made for it. This
// deletes, in order, the routes on the site zone whose script is the
// Worker, that DNS record, the Worker's custom domains, and the Worker with
// its Durable Objects (every artifact and person of the preview). The
// assets it kept in the shared bucket expire on their own (deploy-lib.mjs
// PREVIEW_ASSET_DAYS). Each step is idempotent: a preview that is gone, or
// half gone, is torn down again without error. dev and prod are refused.
//
// --reap lists the account's Workers named clax-pr-<N> and tears down each
// whose pull request <N> in --repo is closed, or does not exist; it keeps
// those whose pull request is open, and fails at the end when GitHub could
// not say for one (GH_TOKEN, else GITHUB_TOKEN, reads the pull request).
// The preview workflow tears a preview down when its pull request closes;
// the reaper catches the closes GitHub sends no event for.
//
// It needs CLOUDFLARE_API_TOKEN (Workers Scripts: Edit on the account;
// Workers Routes: Edit, DNS: Edit and Zone: Read on the site zone) and the
// account: --account, else CLOUDFLARE_ACCOUNT_ID. --dry-run reads and says
// what it would delete, deleting nothing.

import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { cloudflare } from "./cloudflare-api.mjs";
import { envOf } from "./deploy-lib.mjs";

const info = (s) => console.log(`   ${s}`);

/** Tears down the preview `name` through `api` (cloudflare-api.mjs):
 * answers the list of what it deleted (or would delete, with `dryRun`). */
export async function teardown(api, { name, account, siteZone, siteDomain, dryRun = false }) {
  const res = envOf(name);
  if (!res.ephemeral) throw new Error(`${name} is not a preview: teardown.mjs deletes previews only`);
  const worker = res.worker;
  const site = siteDomain ?? `${name}.${siteZone}`;
  const did = [];
  const del = async (what, f) => {
    did.push(what);
    info(`${dryRun ? "would delete" : "deleting"} ${what}`);
    if (!dryRun) await f();
  };
  const zone = await api.zoneId(siteZone);
  for (const r of (await api.routes(zone)) ?? []) {
    if (r.script === worker) await del(`route ${r.pattern}`, () => api.deleteRoute(zone, r.id));
  }
  for (const d of await api.dnsRecords(zone, `*.${site}`)) {
    await del(`DNS record ${d.name} (${d.type})`, () => api.deleteDnsRecord(zone, d.id));
  }
  for (const d of await api.workerDomains(account, worker)) {
    if (d.service === worker) await del(`custom domain ${d.hostname}`, () => api.deleteWorkerDomain(account, d.id));
  }
  const scripts = (await api.scripts(account)) ?? [];
  if (scripts.some((s) => s.id === worker)) await del(`Worker ${worker} and its Durable Objects`, () => api.deleteScript(account, worker));
  if (!did.length) info(`nothing left of ${name}`);
  return did;
}

/** The pull request number of a preview Worker's name (`clax-pr-<N>`), or
 * null for any other Worker. */
export function previewPr(script) {
  const m = /^clax-pr-([1-9][0-9]{0,8})$/.exec(script);
  return m ? Number(m[1]) : null;
}

/** The state of pull request `n` in `repo`: "open", "closed", "missing"
 * (GitHub answers 404), or throws. */
export async function prState(repo, n, token) {
  const base = (process.env.GITHUB_API_BASE || "https://api.github.com").replace(/\/$/, "");
  const res = await fetch(`${base}/repos/${repo}/pulls/${n}`, {
    headers: { accept: "application/vnd.github+json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
    signal: AbortSignal.timeout(30_000),
  });
  if (res.status === 404) return "missing";
  if (!res.ok) throw new Error(`GitHub: GET /repos/${repo}/pulls/${n}: HTTP ${res.status}`);
  const j = await res.json();
  return j.state === "open" ? "open" : "closed";
}

/** Tears down every preview Worker whose pull request is closed or
 * missing: answers `{ reaped, kept, failed }`, the names of each. */
export async function reap(api, { account, siteZone, repo, token, dryRun = false }) {
  const out = { reaped: [], kept: [], failed: [] };
  for (const s of (await api.scripts(account)) ?? []) {
    const n = previewPr(s.id);
    if (n === null) continue;
    let state;
    try {
      state = await prState(repo, n, token);
    } catch (e) {
      info(`${s.id}: kept, since GitHub did not say whether #${n} is open (${e.message})`);
      out.failed.push(s.id);
      continue;
    }
    if (state === "open") {
      info(`${s.id}: #${n} is open; kept`);
      out.kept.push(s.id);
      continue;
    }
    console.log(`== ${s.id}: #${n} is ${state}; tearing it down`);
    await teardown(api, { name: `pr-${n}`, account, siteZone, dryRun });
    out.reaped.push(s.id);
  }
  return out;
}

export function parseArgs(argv) {
  const o = { dryRun: false, reap: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} takes a value`);
      return v;
    };
    if (a === "--dry-run") o.dryRun = true;
    else if (a === "--reap") o.reap = true;
    else if (a === "--site-zone") o.siteZone = value();
    else if (a === "--site-domain") o.siteDomain = value();
    else if (a === "--account") o.account = value();
    else if (a === "--repo") o.repo = value();
    else if (a === "--help" || a === "-h") o.help = true;
    else if (a.startsWith("-")) throw new Error(`unknown option ${a} (see the top of teardown.mjs)`);
    else if (o.name === undefined) o.name = a;
    else throw new Error(`one preview at a time: ${o.name} and ${a}`);
  }
  if (o.help) return o;
  if (!o.siteZone) throw new Error("give --site-zone <zone> (the zone the previews' site domains are under)");
  if (o.reap ? o.name !== undefined : o.name === undefined) throw new Error("give a preview's name (e.g. pr-12), or --reap");
  if (o.reap && !o.repo) throw new Error("--reap needs --repo <owner/name>");
  return o;
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  if (o.help) {
    console.log((await import("node:fs")).readFileSync(new URL(import.meta.url), "utf8").split("\n").filter((l) => l.startsWith("//")).map((l) => l.slice(3)).join("\n"));
    return;
  }
  const account = o.account ?? process.env.CLOUDFLARE_ACCOUNT_ID;
  if (!account) throw new Error("give --account <ID> (or CLOUDFLARE_ACCOUNT_ID)");
  const api = cloudflare(process.env.CLOUDFLARE_API_TOKEN?.trim());
  if (o.reap) {
    const r = await reap(api, { account, siteZone: o.siteZone, repo: o.repo, token: process.env.GH_TOKEN || process.env.GITHUB_TOKEN, dryRun: o.dryRun });
    console.log(`\n${o.dryRun ? "would reap" : "reaped"} ${r.reaped.length} (${r.reaped.join(", ") || "none"}); kept ${r.kept.length}; unknown ${r.failed.length}`);
    if (r.failed.length) throw new Error(`GitHub did not say for ${r.failed.join(", ")}: kept, run again`);
    return;
  }
  console.log(`== tearing down ${o.name}${o.dryRun ? " (dry run: nothing is deleted)" : ""}`);
  await teardown(api, { name: o.name, account, siteZone: o.siteZone, siteDomain: o.siteDomain, dryRun: o.dryRun });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(`\nteardown failed: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  });
}
