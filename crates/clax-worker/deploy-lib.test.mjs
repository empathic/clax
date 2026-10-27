// Tests of deploy.mjs's decisions (deploy-lib.mjs).
//
//   node --test crates/clax-worker/deploy-lib.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { buildId, doneText, healthOf, partialTriggerUpdate } from "./deploy-lib.mjs";

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
