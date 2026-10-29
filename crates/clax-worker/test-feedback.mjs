// clax.dev's own feedback path on the Worker (docs/feedback.md, "What is
// clax.dev's own"), over HTTP as the shell and its overlay make the
// requests, under `wrangler dev --local` as a hosted Clax
// (HOSTED=1, `http://localhost:<port>` its public origin):
//
// 1. the first person sets up with the secret; the owner token makes the
//    instance's own queue for its public origin and a service grant;
//    the person's session reads GET /api/me/flags, and the admin route
//    turns their `feedback` flag on;
// 2. wrangler restarts on the same state with CLAX_FEEDBACK_QUEUE and
//    CLAX_FEEDBACK_SERVICE_GRANT (the Worker reads them as it builds the
//    server); the session's GET /_app/feedback/config answers enabled, the
//    instance, the queue, the person's public ID and the context, and
//    POST /_app/feedback/token mints a reporter token for a fresh Ed25519
//    key;
// 3. a report is filed with the token, signed as the SDK signs
//    (web/sdk/src/sign.ts) and carrying the page's `Origin`;
// 4. the reporter badge and the reporter's report list, read as a
//    browser reads them from the shell's own page (no `Origin`, one
//    `Sec-Fetch-Site: same-origin`, the public `Host`), answer 200 with
//    the report; the same reads with `Sec-Fetch-Site: cross-site`, or with
//    neither header, are 403 `wrong_origin`;
// 5. with the flag turned off the config answers disabled and the token
//    route 403 `no_flag`.
//
// Requests go to wrangler's 127.0.0.1 with the public `Host` set
// explicitly (node:http), so what reaches the server is what a browser on
// the public origin sends. Prints one line per check and a JSON summary;
// exits non-zero on the first failure. `node test-feedback.mjs --no-build`
// skips the build.

import { mkdirSync } from "node:fs";
import { request } from "node:http";
import { join } from "node:path";
import { STATE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";
import { base64url, generateSigner, newNonce, signature } from "../../web/sdk/src/sign.ts";

const ROOT = join(STATE, "feedback-test");
const TOKEN = "feedback-test-token-" + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
const SECRET = "feedback setup secret " + Math.random().toString(36).slice(2);

const summary = { ok: false, checks: 0 };

function check(ok, what, detail) {
  summary.checks++;
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

/** One request to wrangler on `port`, with exactly `headers` (`Host`
 * included): its status, headers, and the body's text and JSON. */
function send(port, method, path, headers, body) {
  return new Promise((ok, fail) => {
    const req = request({ host: "127.0.0.1", port, method, path, headers, setHost: false }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        let json = null;
        try {
          json = text ? JSON.parse(text) : null;
        } catch {
          // not JSON
        }
        ok({ status: res.statusCode, headers: res.headers, text, json });
      });
      res.on("error", fail);
    });
    req.on("error", fail);
    if (body !== undefined) req.write(body);
    req.end();
  });
}

/** The `name=value` pairs of a response's `Set-Cookie`s, as a `Cookie`. */
function cookiesOf(res) {
  const set = res.headers["set-cookie"] ?? [];
  return set.map((c) => c.split(";")[0]).join("; ");
}

/** A `multipart/form-data` body of text `fields`, as the SDK's `multipart` builds one. */
function multipart(fields) {
  const boundary = `clax-${base64url(crypto.getRandomValues(new Uint8Array(18)))}`;
  let s = "";
  for (const [name, value] of fields) {
    s += `--${boundary}\r\ncontent-disposition: form-data; name="${name}"\r\n\r\n${value}\r\n`;
  }
  s += `--${boundary}--\r\n`;
  return { body: new TextEncoder().encode(s), contentType: `multipart/form-data; boundary=${boundary}` };
}

async function main() {
  await build();
  pruneRuns(ROOT, 3);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const port = await freePort();
  const host = `localhost:${port}`;
  const origin = `http://${host}`;
  const vars = { HOSTED: "1", PUBLIC_ORIGIN: origin, SITE_DOMAIN: "localhost", SETUP_SECRET: SECRET, CLAX_TOKEN: TOKEN };
  let w;
  const cleanup = async () => {
    if (w) await w.stop();
  };
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void cleanup().then(() => process.exit(130)));
  try {
    w = await start(dir, writeConfig(dir, vars), 1, port);
    const owner = (method, path, body) =>
      send(port, method, path, { host, authorization: `Bearer ${TOKEN}`, "content-type": "application/json" }, body === undefined ? undefined : JSON.stringify(body));

    // 1. The first person, their session, the queue and the grant.
    const setup = await send(port, "POST", "/api/hosted/setup", { host, origin, "content-type": "application/json", "sec-fetch-site": "same-origin" }, JSON.stringify({ secret: SECRET, name: "Ada" }));
    const cookie = cookiesOf(setup);
    check(setup.status < 300 && cookie.length > 0, "the first person sets up with the secret and holds a session", { status: setup.status, text: setup.text.slice(0, 300) });
    // The shell's own requests: its session, from its own page.
    const shellGet = (path) => send(port, "GET", path, { host, cookie, "sec-fetch-site": "same-origin" });
    const shellWrite = (method, path, body) =>
      send(port, method, path, { host, cookie, origin, "sec-fetch-site": "same-origin", "content-type": "application/json" }, JSON.stringify(body));

    const q = await owner("POST", "/api/queues", { name: "clax-feedback", origins: [origin], reporters_see: "own" });
    check(q.status < 300 && q.json?.queue?.id, "the owner token makes the instance's own queue for its public origin", q.text.slice(0, 300));
    const qid = q.json.queue.id;
    const g = await owner("POST", `/api/queues/${qid}/service-grants`, { name: "clax.dev feedback" });
    check(g.status < 300 && g.json?.token && g.json?.private_key, "and a service grant for it", { status: g.status });

    const before = await shellGet("/api/me/flags");
    check(before.status === 200 && before.json?.flags?.feedback === false, "GET /api/me/flags: the person's feedback flag starts off", before.text.slice(0, 300));
    const on = await shellWrite("PUT", "/api/admin/people/%2Aowner/flags", { feedback: true });
    check(on.status === 200, "the admin route turns the person's feedback flag on, from the shell's session", on.text.slice(0, 300));
    const after = await shellGet("/api/me/flags");
    check(after.json?.flags?.feedback === true, "GET /api/me/flags then says it is on", after.text.slice(0, 300));
    const offBeforeGrant = await shellGet("/_app/feedback/config");
    check(offBeforeGrant.json?.enabled === false, "with no feedback secrets the config is disabled", offBeforeGrant.text.slice(0, 300));

    // 2. Restarted with the instance's feedback secrets.
    await w.stop();
    w = undefined;
    const grant = JSON.stringify({ token: g.json.token, private_key: g.json.private_key });
    w = await start(dir, writeConfig(dir, { ...vars, CLAX_FEEDBACK_QUEUE: qid, CLAX_FEEDBACK_SERVICE_GRANT: grant }), 2, port);
    const me = await shellGet("/api/viewers/me");
    const publicId = me.json?.viewer?.public_id;
    check(me.status === 200 && publicId, "the session outlives the restart", me.text.slice(0, 300));
    const config = await shellGet("/_app/feedback/config");
    const c = config.json ?? {};
    check(
      config.status === 200 && c.enabled === true && c.instance === origin && c.queue === qid && c.user === publicId &&
        c.context?.flags?.feedback === true && c.context?.app?.repo === "https://github.com/empathic/clax" &&
        c.context?.release?.channel === "dev" && typeof c.context?.release?.version === "string",
      "GET /_app/feedback/config: enabled, the public origin, the queue, the person's public ID and the context",
      c,
    );

    const signer = await generateSigner();
    const minted = await shellWrite("POST", "/_app/feedback/token", { public_key: signer.publicKey });
    const reporterToken = minted.json?.token;
    check(
      minted.status === 200 && typeof reporterToken === "string" && reporterToken.startsWith("cxr_") && minted.headers["cache-control"] === "no-store",
      "POST /_app/feedback/token mints a reporter token for the browser's key",
      { status: minted.status, text: minted.text.slice(0, 300) },
    );

    /** A reporter call signed as the SDK's `signedFetch` signs it, for the public `Host`. */
    const reporter = async (method, path, extra, payload) => {
      const body = payload?.body ?? new Uint8Array();
      const sig = await signature(signer, {
        method, pathAndQuery: path, host, t: Math.floor(Date.now() / 1000), nonce: newNonce(), body, token: reporterToken,
      });
      const headers = { host, authorization: `Clax-Reporter ${reporterToken}`, "clax-signature": sig, ...extra };
      if (payload) headers["content-type"] = payload.contentType;
      return send(port, method, path, headers, payload ? Buffer.from(body) : undefined);
    };

    // 3. A report, filed from the page (a POST carries its `Origin`).
    const report = multipart([
      ["url", `${origin}/`],
      ["title", "Clax"],
      // The overlay's whole-page anchor (web/overlay/src/model.ts PAGE_ANCHOR).
      ["anchor", JSON.stringify({ kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" })],
      ["body", "The gallery's tag bar overlaps the first card."],
      ["context", JSON.stringify({ v: 1, captured_at: new Date().toISOString(), ...c.context, app: { ...c.context.app, view: "gallery", artifact: null } })],
    ]);
    const filed = await reporter("POST", `/api/queues/${qid}/reporter/reports`, { origin, "sec-fetch-site": "same-origin" }, report);
    const rid = filed.json?.report?.id;
    check(filed.status === 201 && rid && filed.json.report.reporter_id === publicId, "the reporter files a report with the token, as the person", { status: filed.status, text: filed.text.slice(0, 300) });

    // 4. The overlay's same-origin reads: no `Origin`.
    const badgePath = `/api/queues/${qid}/reporter/badge`;
    const listPath = `/api/queues/${qid}/reporter/reports`;
    const badge = await reporter("GET", badgePath, { "sec-fetch-site": "same-origin" });
    check(badge.status === 200 && badge.json?.waiting === 0, "the badge, read with no Origin, Sec-Fetch-Site: same-origin and the public Host, is 200", { status: badge.status, text: badge.text.slice(0, 300) });
    const list = await reporter("GET", listPath, { "sec-fetch-site": "same-origin" });
    check(list.status === 200 && list.json?.reports?.some((r) => r.id === rid), "the reporter's list, read the same way, is 200 with the report", { status: list.status, text: list.text.slice(0, 300) });
    for (const [what, extra] of [["Sec-Fetch-Site: cross-site", { "sec-fetch-site": "cross-site" }], ["neither Origin nor Sec-Fetch-Site", {}]]) {
      for (const [name, path] of [["badge", badgePath], ["list", listPath]]) {
        const r = await reporter("GET", path, extra);
        check(r.status === 403 && r.json?.error?.code === "wrong_origin", `the ${name} read with ${what} is 403 wrong_origin`, { status: r.status, text: r.text.slice(0, 300) });
      }
    }

    // 5. The flag off: nothing for the person.
    const off = await shellWrite("PUT", "/api/admin/people/%2Aowner/flags", { feedback: false });
    check(off.status === 200, "the admin route turns the flag off", off.text.slice(0, 300));
    const disabled = await shellGet("/_app/feedback/config");
    check(
      disabled.json?.enabled === false && disabled.json?.queue === null && (await shellGet("/api/me/flags")).json?.flags?.feedback === false,
      "with the flag off the config is disabled and GET /api/me/flags says so",
      disabled.text.slice(0, 300),
    );
    const refused = await shellWrite("POST", "/_app/feedback/token", { public_key: signer.publicKey });
    check(refused.status === 403 && refused.json?.error?.code === "no_flag", "and the token route is 403 no_flag", refused.text.slice(0, 300));

    summary.ok = true;
    console.log(JSON.stringify(summary));
  } finally {
    await cleanup();
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  console.log(JSON.stringify(summary));
  process.exit(1);
});
