// Tests of deploy.mjs's decisions (deploy-lib.mjs).
//
//   node --test crates/clax-worker/deploy-lib.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  SMOKE_CANNOT,
  buildId,
  builtFromRefusal,
  checkCiOptions,
  ciSecretsRefusal,
  doneText,
  healthOf,
  partialTriggerUpdate,
  prodTipOf,
  productionGate,
  smokeCannot,
  smokeCredential,
  versionIdOf,
} from "./deploy-lib.mjs";

// What `wrangler deploy` printed when the custom-domains call failed after
// the upload (Cloudflare API code 10013); running it again succeeded.
const PARTIAL = `Total Upload: 31456.12 KiB / gzip: 7801.55 KiB
Worker Startup Time: 412 ms
Uploaded clax (21.37 sec)

✘ [ERROR] A request to the Cloudflare API (/accounts/0123456789abcdef0123456789abcdef/workers/scripts/clax/domains/records) failed.

  Trigger configuration for "clax" was only partially updated [code: 10013]

  If you think this is a bug, please open an issue at:
  https://github.com/cloudflare/workers-sdk/issues/new/choose
`;

test("the real partial trigger update is retried", () => {
  assert.equal(partialTriggerUpdate(PARTIAL), true);
});

test("the trigger line alone, in any case, is retried", () => {
  assert.equal(partialTriggerUpdate('Trigger configuration for "clax" was only partially updated'), true);
  assert.equal(partialTriggerUpdate('TRIGGER CONFIGURATION for "my-worker" was only PARTIALLY UPDATED'), true);
});

test("a custom domain left partially updated is retried", () => {
  assert.equal(partialTriggerUpdate("✘ [ERROR] Custom domains for clax were only partially updated [code: 10013]"), true);
});

test("other failures are not retried", () => {
  for (const text of [
    "",
    undefined,
    "✘ [ERROR] A request to the Cloudflare API (/accounts/x/workers/scripts/clax) failed.\n\n  Authentication error [code: 10000]",
    "✘ [ERROR] Your Worker exceeded the size limit of 10 MiB. [code: 10027]",
    "✘ [ERROR] Could not resolve \"build/index.js\"",
    // The code without the partial update is some other failure.
    "✘ [ERROR] A request to the Cloudflare API (/accounts/x/workers/scripts/clax/domains/records) failed.\n\n  Hostname already has a custom domain [code: 10013]",
    // A partial update of something that is not a trigger.
    "the upload was only partially updated",
  ]) {
    assert.equal(partialTriggerUpdate(text), false, String(text));
  }
});

test("a build ID names the commit, the time and the random part", () => {
  const at = new Date("2026-10-23T10:15:00.123Z");
  assert.equal(buildId("0a2f4805a1b2c3d4e5f6", at, "9f86d081"), "0a2f4805a1b2-20261023T101500Z-9f86d081");
  assert.equal(buildId("", at, "9f86d081"), "20261023T101500Z-9f86d081");
});

test("the wait is done only when /healthz names the build just uploaded", () => {
  const want = "0a2f4805a1b2-20261023T101500Z-9f86d081";
  assert.deepEqual(healthOf(JSON.stringify({ version: "0.6.0", pid: null, started_at: "t", commit: "0a2f4805a1b2", build: want }), want), { done: true });
});

test("an instance from before the deploy, of the same version, is waited past", () => {
  const want = "0a2f4805a1b2-20261023T101500Z-9f86d081";
  // What answered after the real deploy: the version alone, no build.
  const before = healthOf(JSON.stringify({ version: "0.6.0", pid: null, started_at: "2026-10-22T09:00:00Z" }), want);
  assert.equal(before.done, undefined);
  assert.match(before.waiting, /before this deploy.*reports none.*waiting for build 0a2f4805a1b2-20261023T101500Z-9f86d081/);
  const older = healthOf(JSON.stringify({ version: "0.6.0", build: "0a2f4805a1b2-20261023T090000Z-00000000" }), want);
  assert.match(older.waiting, /build 0a2f4805a1b2-20261023T090000Z-00000000/);
  assert.match(healthOf("ok", want).waiting, /not its JSON/);
});

const TIP = "0a2f4805a1b2c3d4e5f60718293a4b5c6d7e8f90";
const OTHER = "1111111111111111111111111111111111111111";

test("prod's tip is read from git ls-remote's line for refs/heads/prod alone", () => {
  assert.equal(prodTipOf(`${TIP}\trefs/heads/prod\n`), TIP);
  assert.equal(prodTipOf(`${OTHER}\trefs/heads/production\n${TIP}\trefs/heads/prod\n`), TIP);
  for (const out of ["", undefined, `${OTHER}\trefs/heads/main\n`, "fatal: 'origin' does not appear to be a git repository\n"]) {
    assert.equal(prodTipOf(out), null, String(out));
  }
});

test("a deploy not of the production instance is not checked, and --from-laptop needs --production", () => {
  assert.deepEqual(productionGate({ production: false }), { ok: true, say: null });
  assert.match(productionGate({ production: false, fromLaptop: true }).refuse, /--from-laptop goes with --production/);
});

test("production deploys from GitHub Actions at a clean checkout of prod's tip", () => {
  const g = productionGate({ production: true, githubActions: true, clean: true, head: TIP, prodTip: TIP });
  assert.equal(g.ok, true);
  assert.match(g.say, /prod tip 0a2f4805a1b2, clean, in GitHub Actions/);
});

test("production off GitHub Actions is refused unless --from-laptop", () => {
  const off = productionGate({ production: true, githubActions: false, clean: true, head: TIP, prodTip: TIP });
  assert.match(off.refuse, /run from GitHub Actions on pushes to prod \(docs\/prod\.md\); --from-laptop/);
  const laptop = productionGate({ production: true, fromLaptop: true, githubActions: false, clean: true, head: TIP, prodTip: TIP });
  assert.equal(laptop.ok, true);
  assert.match(laptop.say, /from a laptop \(--from-laptop\)/);
});

test("--from-laptop lifts only the Actions requirement: a dirty checkout or another commit is refused", () => {
  for (const [facts, why] of [
    [{ clean: false, head: TIP, prodTip: TIP }, /the checkout has changes/],
    [{ clean: true, head: OTHER, prodTip: TIP }, /HEAD is 111111111111, not origin's prod tip 0a2f4805a1b2/],
    [{ clean: true, head: TIP, prodTip: null }, /origin has no branch prod/],
  ]) {
    for (const githubActions of [true, false]) {
      const g = productionGate({ production: true, fromLaptop: true, githubActions, ...facts });
      assert.equal(g.ok, undefined);
      assert.match(g.refuse, why);
    }
  }
});

test("a production dry run is never refused, and says what a deploy would refuse", () => {
  const g = productionGate({ production: true, githubActions: false, clean: false, head: OTHER, prodTip: TIP, dryRun: true });
  assert.equal(g.ok, true);
  assert.match(g.say, /a deploy would refuse: .*GitHub Actions.*changes.*not origin's prod tip/);
});

const CI = { public: "clax.example.com", "site-domain": "example.run", store: "artifacts", storeGiven: true, legal: "bundled", account: "abc" };

test("--ci takes every setting as a flag", () => {
  assert.doesNotThrow(() => checkCiOptions(CI));
  assert.doesNotThrow(() => checkCiOptions({ ...CI, public: undefined, zone: "example.com" }));
  assert.doesNotThrow(() => checkCiOptions({ ...CI, account: undefined }, { CLOUDFLARE_ACCOUNT_ID: "abc" }));
  assert.doesNotThrow(() => checkCiOptions({ ...CI, legal: undefined, "terms-url": "https://t", "privacy-url": "https://p", "legal-version": "1" }));
  assert.doesNotThrow(() => checkCiOptions({ ...CI, "github-client-id": "Iv1", "owner-github": "someone" }));
});

test("--ci names each missing flag at once, the store's default included", () => {
  // What parseArgs gives with no flags: the store defaulted, not given.
  assert.throws(() => checkCiOptions({ store: "artifacts", storeGiven: false }), (e) => {
    for (const f of ["--public <host>", "--site-domain <domain>", "--store artifacts|memory", "--legal bundled", "--account <ID> (or CLOUDFLARE_ACCOUNT_ID)"]) {
      assert.ok(e.message.includes(f), `${f} in ${e.message}`);
    }
    return true;
  });
  assert.throws(() => checkCiOptions({ ...CI, "github-client-id": "Iv1" }), /--owner-github <login> \(with --github-client-id\)/);
});

test("--ci refuses what needs a person: new secrets and the diagnostics", () => {
  assert.throws(() => checkCiOptions({ ...CI, rotate: true }), /--ci takes no --rotate-secrets/);
  assert.throws(() => checkCiOptions({ ...CI, diag: true }), /--ci takes no --diag/);
});

const ALL = ["SETUP_SECRET", "CLAX_TOKEN", "CLAX_KEK"];

test("--ci deploys a Worker set up from a laptop, keeping its secrets", () => {
  assert.equal(ciSecretsRefusal(ALL, {}), null);
  assert.equal(ciSecretsRefusal([...ALL, "GITHUB_CLIENT_SECRET"], { githubClientId: "Iv1" }), null);
  assert.equal(ciSecretsRefusal(ALL, { githubClientId: "Iv1", githubSecretGiven: true }), null);
});

test("--ci refuses a first deploy: the Worker lacks a secret, or the list failed", () => {
  assert.match(ciSecretsRefusal([], {}), /no SETUP_SECRET, CLAX_TOKEN, CLAX_KEK .*runs from a laptop/);
  assert.match(ciSecretsRefusal(["SETUP_SECRET", "CLAX_TOKEN"], {}), /no CLAX_KEK/);
});

test("--ci refuses to turn GitHub sign-in off, or to leave it without its secret", () => {
  assert.match(ciSecretsRefusal([...ALL, "GITHUB_CLIENT_SECRET"], {}), /would turn GitHub sign-in off/);
  assert.match(ciSecretsRefusal(ALL, { githubClientId: "Iv1" }), /neither the Worker nor GITHUB_CLIENT_SECRET/);
});

const DONE = { pub: "clax.example.com", name: "clax", diag: false, record: "/r/clax.example.com.json", kek: "/r/clax.example.com.kek", smoke: "node smoke-remote.mjs --deploy /r/clax.example.com.json" };
const FIRST_TIME = [/Shown once/, /Setup: +https:\/\/clax\.example\.com\/setup/, /Next:/, /Smoke the platform first/];

test("the first deploy prints its secrets once and the setup steps", () => {
  const t = doneText({ ...DONE, generated: { SETUP_SECRET: "s3cret", CLAX_TOKEN: "t0ken", CLAX_KEK: "kek-never-shown" } });
  for (const re of FIRST_TIME) assert.match(t, re);
  assert.match(t, /SETUP_SECRET {2}s3cret/);
  assert.match(t, /CLAX_TOKEN {4}t0ken/);
  assert.doesNotMatch(t, /kek-never-shown/);
  assert.match(t, /Record: +\/r\/clax\.example\.com\.json/);
});

test("a redeploy prints no first-time setup text, whatever else it uploaded", () => {
  for (const generated of [null, undefined, {}, { GITHUB_CLIENT_SECRET: "gh" }, { CLAX_KEK: "k" }]) {
    const t = doneText({ ...DONE, generated });
    for (const re of FIRST_TIME) assert.doesNotMatch(t, re, JSON.stringify(generated));
    assert.match(t, /Record: +\/r\/clax\.example\.com\.json/);
    assert.match(t, /CLAX_KEK backup: \/r\/clax\.example\.com\.kek/);
    assert.match(t, /Logs: +npx wrangler tail clax/);
  }
});

test("a --ci deploy names no CLAX_KEK backup", () => {
  assert.doesNotMatch(doneText({ ...DONE, kek: null, generated: null }), /CLAX_KEK/);
});

test("the smoke's credential: a session, then an agent token, then setup, then the owner token", () => {
  assert.equal(smokeCredential({ session: "s", agent: "cxa_a", setupOpen: true, secret: "x", token: "t" }), "session");
  assert.equal(smokeCredential({ agent: "cxa_a", setupOpen: true, secret: "x", token: "t" }), "agent");
  assert.equal(smokeCredential({ setupOpen: true, secret: "x", token: "t" }), "setup");
  assert.equal(smokeCredential({ setupOpen: true, token: "t" }), "token");
  assert.equal(smokeCredential({ setupOpen: false, secret: "x", token: "t" }), "token");
  // No credential at all fails the smoke.
  assert.equal(smokeCredential({ setupOpen: false, secret: "x" }), null);
  assert.equal(smokeCredential({}), null);
});

test("an agent token skips only what its person alone can do, each with a reason", () => {
  for (const name of [
    "diagnostics: env",
    "agent token",
    "web UI: the shell's pages",
    "site: /site-signin, then /_clax/session (SameSite=Lax)",
    "site: open to the world, then private again",
    "live: a comment on the WebSocket stream",
  ]) {
    assert.ok(smokeCannot("agent", name), name);
  }
  for (const name of ["health", "mcp: initialize and tools/list", "publish v1 (mcp)", "fork", "site host: health", "site: a private site refused without a session", "git clone", "git push", "the push becomes a version"]) {
    assert.equal(smokeCannot("agent", name), null, name);
  }
  for (const why of Object.values(SMOKE_CANNOT.agent)) assert.ok(why.length > 20);
});

test("the owner token skips only what a browser session alone can do; a session and setup skip nothing", () => {
  assert.deepEqual(Object.keys(SMOKE_CANNOT.token).sort(), ["agent token", "site: /site-signin, then /_clax/session (SameSite=Lax)", "site: two db artifacts kept apart"]);
  assert.equal(smokeCannot("token", "diagnostics: env"), null);
  for (const as of ["session", "setup", "", null]) assert.equal(smokeCannot(as, "agent token"), null);
});

// What `wrangler deploy` printed after a deploy that went through.
const DEPLOYED = `Total Upload: 31456.12 KiB / gzip: 7801.55 KiB
Worker Startup Time: 412 ms
Uploaded clax (21.37 sec)
Deployed clax triggers (3.02 sec)
  clax.dev (custom domain)
  *.clax.run/* (zone name: clax.run)
Current Version ID: 3f1c9a2e-7b4d-4e8f-9a01-5c6d7e8f9a0b
`;

test("the version ID is the one wrangler deploy names as current", () => {
  assert.equal(versionIdOf(DEPLOYED), "3f1c9a2e-7b4d-4e8f-9a01-5c6d7e8f9a0b");
  assert.equal(versionIdOf(DEPLOYED.replace("Current Version ID: ", "Current Version ID:   ")), "3f1c9a2e-7b4d-4e8f-9a01-5c6d7e8f9a0b");
  // A deploy that printed none (an older wrangler, or a dry run) has none.
  assert.equal(versionIdOf(PARTIAL), null);
  assert.equal(versionIdOf("Current Version ID: not-a-uuid"), null);
  assert.equal(versionIdOf(""), null);
  assert.equal(versionIdOf(undefined), null);
});

test("--no-build deploys only a Worker built from the checked-out commit", () => {
  const head = "0a2f4805a1b2c3d4e5f60718293a4b5c6d7e8f90";
  const built = (commit) => Buffer.concat([Buffer.from([0, 97, 115, 109]), Buffer.from(`...${commit}...`)]);
  assert.equal(builtFromRefusal(built(head), head), null);
  const other = "ffffffffffffffffffffffffffffffffffffffff";
  assert.match(builtFromRefusal(built(other), head), /not built from HEAD 0a2f4805a1b2/);
  // A Worker built with no commit reports "unknown": not this one either.
  assert.match(builtFromRefusal(built("unknown"), head), /not built from HEAD/);
  // Nothing built at all.
  assert.match(builtFromRefusal(null, head), /no build\/index_bg\.wasm/);
  // No commit to compare (not a git checkout): nothing to refuse on.
  assert.equal(builtFromRefusal(built(other), ""), null);
});
