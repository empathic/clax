// The smoke as a staging deploy runs it (deploy.mjs --staging,
// smoke-remote.mjs --mint-agent-token), against a fresh local Worker that
// no one has set up: it mints an agent token with the owner token alone,
// acts as that token, passes, revokes it, and prints neither the owner
// token nor the agent token (a CI log may be public).
//
//   node test-smoke-minted.mjs [--no-build]

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { join } from "node:path";
import { HERE } from "./harness.mjs";

// In this directory, which CLAX_WORKER_BUNDLE's relative path is from.
const child = spawn(process.execPath, [join(HERE, "smoke-remote.mjs"), "--local", "--as-minted-agent", ...process.argv.slice(2)], {
  stdio: ["ignore", "pipe", "pipe"],
});
let out = "";
child.stdout.on("data", (d) => (out += d));
child.stderr.on("data", (d) => (out += d));
const code = await new Promise((done) => child.on("close", done));
process.stdout.write(out);

assert.equal(code, 0, "the smoke failed");
assert.match(out, /minted an agent token with the owner token/);
assert.match(out, /acting as the agent token/);
assert.match(out, /the minted agent token: revoked/);
// The local Worker's owner token is smoke-remote.mjs's `smoke-owner-…`.
assert.doesNotMatch(out, /cxa_[A-Za-z0-9]/, "an agent token was printed");
assert.doesNotMatch(out, /smoke-owner-/, "the owner token was printed");
console.log("smoke as a minted agent token: ok");
