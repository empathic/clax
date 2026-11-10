// The landing page's reel on Cloudflare Stream, for a production deploy
// (.github/workflows/deploy-prod.yml):
//
//   node scripts/reel-stream.mjs <reel.mp4>   upload it, unless Stream has it
//   node scripts/reel-stream.mjs --latest     the newest reel Stream has
//
// Prints `reel_url=<address>` and `reel_poster_url=<address>` (lines for
// $GITHUB_OUTPUT), which deploy.mjs takes as --reel-url and
// --reel-poster-url: the video's MP4 download (Stream serves it from its
// CDN; a plain <video> plays it without a script, where HLS needs one but
// in Safari) and its thumbnail, the first frame, which is the poster frame
// (web/reel/compose/landing.sh opens the teaser on it).
//
// A reel is named clax-reel-<first 16 hex of its SHA-256>, so an unchanged
// reel is never uploaded again; the three newest are kept and older ones
// deleted. Needs CLOUDFLARE_API_TOKEN (Stream: Edit) and
// CLOUDFLARE_ACCOUNT_ID. Waits at most 15 minutes for Stream to encode.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename } from "node:path";

const KEEP = 3;
const PREFIX = "clax-reel-";
const WAIT_MS = 15 * 60 * 1000;
const POLL_MS = 5000;

const token = process.env.CLOUDFLARE_API_TOKEN;
const account = process.env.CLOUDFLARE_ACCOUNT_ID;
if (!token || !account) {
  console.error("reel-stream: CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID are needed");
  process.exit(2);
}
const API = `https://api.cloudflare.com/client/v4/accounts/${account}/stream`;

async function call(path, init = {}) {
  const res = await fetch(API + path, { ...init, headers: { authorization: `Bearer ${token}`, ...(init.headers ?? {}) } });
  const body = await res.json().catch(() => ({}));
  if (!res.ok || body.success === false) {
    const why = (body.errors ?? []).map((e) => `${e.code}: ${e.message}`).join("; ") || res.statusText;
    throw new Error(`Stream ${init.method ?? "GET"} ${path || "/"}: ${res.status} ${why}`);
  }
  return body.result;
}

const json = (body) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const pause = () => new Promise((ok) => setTimeout(ok, POLL_MS));

/** Every reel Stream has, newest first. */
async function reels() {
  const all = await call(`?search=${PREFIX}`);
  return all
    .filter((v) => v.meta?.name?.startsWith(PREFIX) && v.status?.state !== "error")
    .sort((a, b) => Date.parse(b.created) - Date.parse(a.created));
}

/** `uid`'s details once Stream can serve it. */
async function ready(uid, deadline) {
  for (;;) {
    const v = await call(`/${uid}`);
    if (v.readyToStream) return v;
    if (v.status?.state === "error") throw new Error(`Stream could not encode ${uid}: ${v.status.errorReasonText ?? "no reason given"}`);
    if (Date.now() > deadline) throw new Error(`Stream had not encoded ${uid} after ${WAIT_MS / 60000} minutes`);
    await pause();
  }
}

/** The address of `uid`'s MP4 download, made once it is ready. */
async function download(uid, deadline) {
  let d = (await call(`/${uid}/downloads`, json({}))).default;
  while (d.status !== "ready") {
    if (d.status === "error") throw new Error(`Stream could not make ${uid}'s MP4 download`);
    if (Date.now() > deadline) throw new Error(`Stream had not made ${uid}'s MP4 download after ${WAIT_MS / 60000} minutes`);
    await pause();
    d = (await call(`/${uid}/downloads`)).default;
  }
  return d.url;
}

async function upload(file) {
  const data = readFileSync(file);
  const name = PREFIX + createHash("sha256").update(data).digest("hex").slice(0, 16);
  const have = (await reels()).find((v) => v.meta.name === name);
  if (have) {
    console.error(`reel-stream: Stream has ${name} (${have.uid})`);
    return have.uid;
  }
  console.error(`reel-stream: uploading ${basename(file)} (${(data.length / 1048576).toFixed(1)} MiB) as ${name}`);
  const form = new FormData();
  form.append("file", new Blob([data], { type: "video/mp4" }), `${name}.mp4`);
  const v = await call("", { method: "POST", body: form });
  await call(`/${v.uid}`, json({ meta: { name }, requireSignedURLs: false }));
  return v.uid;
}

async function main() {
  const arg = process.argv[2];
  if (!arg) {
    console.error("usage: node scripts/reel-stream.mjs <reel.mp4> | --latest");
    process.exit(2);
  }
  const deadline = Date.now() + WAIT_MS;
  let uid;
  if (arg === "--latest") {
    const newest = (await reels()).find((v) => v.readyToStream);
    if (!newest) throw new Error("Stream has no reel yet");
    uid = newest.uid;
  } else {
    uid = await upload(arg);
  }
  const v = await ready(uid, deadline);
  const url = await download(uid, deadline);
  if (arg !== "--latest") {
    for (const old of (await reels()).filter((r) => r.uid !== uid).slice(KEEP - 1)) {
      console.error(`reel-stream: deleting the older reel ${old.meta.name} (${old.uid})`);
      await call(`/${old.uid}`, { method: "DELETE" });
    }
  }
  console.log(`reel_url=${url}`);
  console.log(`reel_poster_url=${v.thumbnail}`);
}

main().catch((e) => {
  console.error(`reel-stream: ${e.message}`);
  process.exit(1);
});
