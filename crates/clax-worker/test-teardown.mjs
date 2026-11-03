// Tests of teardown.mjs and cloudflare-api.mjs against a fake Cloudflare
// API and a fake GitHub API on a loopback port (CLOUDFLARE_API_BASE,
// GITHUB_API_BASE): a preview's teardown deletes its route, DNS record,
// custom domain, Worker (once it consumes no queue, as Cloudflare
// requires), queue and Artifacts repositories and nothing of
// any other environment, again without error, never dev or prod; a dry run
// deletes nothing; the reaper tears down the previews of closed and missing
// pull requests, a half-torn-down one's included, and keeps the others.
//
//   node --test crates/clax-worker/test-teardown.mjs
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, beforeEach, test } from "node:test";
import { cloudflare, expiryRule, hasRule } from "./cloudflare-api.mjs";
import { parseArgs, previewPr, reap, teardown } from "./teardown.mjs";

const ACCOUNT = "acct0";
const ZONE = "preview.run";
const ZONE_ID = "zone0";
let state;
let calls;
let pulls;

/** A queue as the API lists it, its one consumer the Worker `script`. */
const queue = (queue_id, queue_name, script) => ({ queue_id, queue_name, consumers: [{ consumer_id: `c-${queue_id}`, type: "worker", script }] });

function fresh() {
  state = {
    routes: [
      { id: "r12", pattern: "*.pr-12.preview.run/*", script: "clax-pr-12" },
      { id: "r13", pattern: "*.pr-13.preview.run/*", script: "clax-pr-13" },
    ],
    dns: [
      { id: "d12", name: "*.pr-12.preview.run", type: "AAAA" },
      { id: "d13", name: "*.pr-13.preview.run", type: "AAAA" },
    ],
    domains: [
      { id: "c12", hostname: "pr-12.preview.dev", service: "clax-pr-12" },
      { id: "c13", hostname: "pr-13.preview.dev", service: "clax-pr-13" },
    ],
    scripts: ["clax", "clax-dev", "clax-pr-12", "clax-pr-13"].map((id) => ({ id })),
    lifecycle: [],
    // Each queue's consumer is the Worker of the same environment.
    queues: [
      queue("q", "clax-artifacts-events", "clax"),
      queue("qdev", "clax-artifacts-events-dev", "clax-dev"),
      queue("q12", "clax-artifacts-events-pr-12", "clax-pr-12"),
      queue("q13", "clax-artifacts-events-pr-13", "clax-pr-13"),
    ],
    // Artifacts namespaces and their repositories' names.
    repos: { clax: ["prod1"], "clax-dev": ["dev1"], "clax-pr-12": ["a1", "b2"], "clax-pr-13": ["c3"] },
  };
  calls = [];
}

const ok = (res, result, info) => {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({ success: true, errors: [], result, ...(info ? { result_info: info } : {}) }));
};

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (d) => (body += d));
  req.on("end", () => {
    const url = new URL(req.url, "http://x");
    const p = url.pathname;
    calls.push(`${req.method} ${p}`);
    if (req.headers.authorization !== "Bearer t0ken" && !p.startsWith("/gh/")) {
      res.writeHead(403, { "content-type": "application/json" });
      return res.end(JSON.stringify({ success: false, errors: [{ code: 10000, message: "Authentication error" }] }));
    }
    let m;
    if (p === "/cf/zones") return ok(res, url.searchParams.get("name") === ZONE ? [{ id: ZONE_ID, name: ZONE }] : []);
    if (p === `/cf/zones/${ZONE_ID}/workers/routes`) return ok(res, state.routes);
    if ((m = /^\/cf\/zones\/zone0\/workers\/routes\/(.+)$/.exec(p)) && req.method === "DELETE") {
      state.routes = state.routes.filter((r) => r.id !== m[1]);
      return ok(res, { id: m[1] });
    }
    if (p === `/cf/zones/${ZONE_ID}/dns_records` && req.method === "GET") {
      return ok(res, state.dns.filter((d) => d.name === url.searchParams.get("name")), { page: 1, total_pages: 1 });
    }
    if (p === `/cf/zones/${ZONE_ID}/dns_records` && req.method === "POST") {
      const r = JSON.parse(body);
      state.dns.push({ id: `d${state.dns.length + 100}`, ...r });
      return ok(res, r);
    }
    if ((m = /^\/cf\/zones\/zone0\/dns_records\/(.+)$/.exec(p)) && req.method === "DELETE") {
      state.dns = state.dns.filter((d) => d.id !== m[1]);
      return ok(res, { id: m[1] });
    }
    if (p === `/cf/accounts/${ACCOUNT}/workers/domains`) {
      return ok(res, state.domains.filter((d) => d.service === url.searchParams.get("service")));
    }
    if ((m = /^\/cf\/accounts\/acct0\/workers\/domains\/(.+)$/.exec(p)) && req.method === "DELETE") {
      state.domains = state.domains.filter((d) => d.id !== m[1]);
      return ok(res, null);
    }
    if (p === `/cf/accounts/${ACCOUNT}/workers/scripts`) return ok(res, state.scripts);
    if ((m = /^\/cf\/accounts\/acct0\/workers\/scripts\/(.+)$/.exec(p)) && req.method === "DELETE") {
      assert.equal(url.searchParams.get("force"), "true", "a Worker with Durable Objects is deleted with force");
      if (!state.scripts.some((s) => s.id === m[1])) {
        res.writeHead(404, { "content-type": "application/json" });
        return res.end(JSON.stringify({ success: false, errors: [{ code: 10007, message: "workers.api.error.script_not_found" }] }));
      }
      if (state.queues.some((q) => q.consumers.some((c) => c.script === m[1]))) {
        res.writeHead(403, { "content-type": "application/json" });
        return res.end(JSON.stringify({ success: false, errors: [{ code: 10064, message: "Cannot delete this Worker as it is a consumer for a Queue. Remove it from the Queue's consumers first, then retry." }] }));
      }
      state.scripts = state.scripts.filter((s) => s.id !== m[1]);
      return ok(res, null);
    }
    if (p === `/cf/accounts/${ACCOUNT}/queues` && req.method === "GET") {
      const name = url.searchParams.get("name");
      return ok(res, state.queues.filter((q) => !name || q.queue_name === name), { page: 1, total_pages: 1 });
    }
    if ((m = /^\/cf\/accounts\/acct0\/queues\/([^/]+)\/consumers\/([^/]+)$/.exec(p)) && req.method === "DELETE") {
      const q = state.queues.find((x) => x.queue_id === m[1]);
      if (!q?.consumers.some((c) => c.consumer_id === m[2])) {
        res.writeHead(404, { "content-type": "application/json" });
        return res.end(JSON.stringify({ success: false, errors: [{ code: 11000, message: "Consumer does not exist" }] }));
      }
      q.consumers = q.consumers.filter((c) => c.consumer_id !== m[2]);
      return ok(res, {});
    }
    if ((m = /^\/cf\/accounts\/acct0\/queues\/(.+)$/.exec(p)) && req.method === "DELETE") {
      const q = state.queues.find((x) => x.queue_id === m[1]);
      if (!q) {
        res.writeHead(404, { "content-type": "application/json" });
        return res.end(JSON.stringify({ success: false, errors: [{ code: 11000, message: "Queue does not exist" }] }));
      }
      state.queues = state.queues.filter((x) => x !== q);
      return ok(res, {});
    }
    if ((m = /^\/cf\/accounts\/acct0\/artifacts\/namespaces\/([^/]+)\/repos$/.exec(p)) && req.method === "GET") {
      const repos = state.repos[decodeURIComponent(m[1])];
      if (!repos) {
        res.writeHead(404, { "content-type": "application/json" });
        return res.end(JSON.stringify({ success: false, errors: [{ code: 404, message: "namespace not found" }] }));
      }
      return ok(res, repos.map((name) => ({ name })), { page: 1, total_pages: 1 });
    }
    if ((m = /^\/cf\/accounts\/acct0\/artifacts\/namespaces\/([^/]+)\/repos\/([^/]+)$/.exec(p)) && req.method === "DELETE") {
      const ns = decodeURIComponent(m[1]);
      state.repos[ns] = (state.repos[ns] ?? []).filter((r) => r !== decodeURIComponent(m[2]));
      return ok(res, null);
    }
    if (p === `/cf/accounts/${ACCOUNT}/r2/buckets/clax-assets-preview/lifecycle` && req.method === "GET") return ok(res, { rules: state.lifecycle });
    if (p === `/cf/accounts/${ACCOUNT}/r2/buckets/clax-assets-preview/lifecycle` && req.method === "PUT") {
      state.lifecycle = JSON.parse(body).rules;
      return ok(res, {});
    }
    if ((m = /^\/gh\/repos\/empathic\/clax\/pulls\/(\d+)$/.exec(p))) {
      const s = pulls[m[1]];
      if (s === undefined) {
        res.writeHead(404);
        return res.end("{}");
      }
      if (s === "error") {
        res.writeHead(502);
        return res.end("{}");
      }
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ number: Number(m[1]), state: s }));
    }
    res.writeHead(500);
    res.end(`fake: no route for ${req.method} ${p}`);
  });
});

before(async () => {
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const { port } = server.address();
  process.env.CLOUDFLARE_API_BASE = `http://127.0.0.1:${port}/cf`;
  process.env.GITHUB_API_BASE = `http://127.0.0.1:${port}/gh`;
});
after(() => server.close());
beforeEach(fresh);

const api = () => cloudflare("t0ken");

test("a preview's teardown deletes its route, DNS record, custom domain, Worker, queue and repositories, and nothing else", async () => {
  const did = await teardown(api(), { name: "pr-12", account: ACCOUNT, siteZone: ZONE });
  assert.deepEqual(did, [
    "route *.pr-12.preview.run/*",
    "DNS record *.pr-12.preview.run (AAAA)",
    "custom domain pr-12.preview.dev",
    "consumer clax-pr-12 of queue clax-artifacts-events-pr-12",
    "Worker clax-pr-12 and its Durable Objects",
    "queue clax-artifacts-events-pr-12",
    "Artifacts repository clax-pr-12/a1",
    "Artifacts repository clax-pr-12/b2",
  ]);
  assert.deepEqual(state.routes.map((r) => r.id), ["r13"]);
  assert.deepEqual(state.dns.map((d) => d.id), ["d13"]);
  assert.deepEqual(state.domains.map((d) => d.id), ["c13"]);
  assert.deepEqual(state.scripts.map((s) => s.id), ["clax", "clax-dev", "clax-pr-13"]);
  assert.deepEqual(state.queues.map((q) => q.queue_id), ["q", "qdev", "q13"]);
  assert.deepEqual(state.repos, { clax: ["prod1"], "clax-dev": ["dev1"], "clax-pr-12": [], "clax-pr-13": ["c3"] });
});

test("a preview that never had Artifacts (deployed with --store memory) is torn down without error", async () => {
  state.queues = state.queues.filter((q) => q.queue_id !== "q12");
  delete state.repos["clax-pr-12"];
  const did = await teardown(api(), { name: "pr-12", account: ACCOUNT, siteZone: ZONE });
  assert.equal(did.length, 4, did.join("\n"));
  assert.deepEqual(state.scripts.map((s) => s.id), ["clax", "clax-dev", "clax-pr-13"]);
});

test("a teardown that stopped at the Worker, its queue's consumer, is finished", async () => {
  state.routes = state.routes.filter((r) => r.id !== "r12");
  state.dns = state.dns.filter((d) => d.id !== "d12");
  state.domains = state.domains.filter((d) => d.id !== "c12");
  const did = await teardown(api(), { name: "pr-12", account: ACCOUNT, siteZone: ZONE });
  assert.deepEqual(did.slice(0, 3), ["consumer clax-pr-12 of queue clax-artifacts-events-pr-12", "Worker clax-pr-12 and its Durable Objects", "queue clax-artifacts-events-pr-12"]);
  assert.deepEqual(state.scripts.map((s) => s.id), ["clax", "clax-dev", "clax-pr-13"]);
  assert.deepEqual(state.queues.map((q) => q.queue_id), ["q", "qdev", "q13"]);
});

test("a preview torn down already is torn down again without error", async () => {
  await teardown(api(), { name: "pr-12", account: ACCOUNT, siteZone: ZONE });
  calls = [];
  const did = await teardown(api(), { name: "pr-12", account: ACCOUNT, siteZone: ZONE });
  assert.deepEqual(did, []);
  assert.equal(calls.filter((c) => c.startsWith("DELETE")).length, 0, calls.join("\n"));
});

test("a dry run deletes nothing and names what it would", async () => {
  const did = await teardown(api(), { name: "pr-13", account: ACCOUNT, siteZone: ZONE, dryRun: true });
  assert.equal(did.length, 7, did.join("\n"));
  assert.equal(calls.filter((c) => c.startsWith("DELETE")).length, 0, calls.join("\n"));
  assert.equal(state.scripts.length, 4);
  assert.equal(state.queues.length, 4);
});

test("dev and prod are never torn down", async () => {
  for (const name of ["dev", "prod"]) {
    await assert.rejects(teardown(api(), { name, account: ACCOUNT, siteZone: ZONE }), /not a preview/);
  }
  assert.equal(calls.length, 0, "no call made");
});

test("the reaper tears down closed and missing pull requests' previews and keeps the rest", async () => {
  state.scripts.push({ id: "clax-pr-14" }, { id: "clax-pr-15" });
  // pr-16's Worker is gone already, its queue and repository left behind.
  state.queues.push({ ...queue("q16", "clax-artifacts-events-pr-16", "clax-pr-16"), consumers: [] });
  state.repos["clax-pr-16"] = ["d4"];
  pulls = { 12: "closed", 13: "open", 15: "error", 16: "closed" };
  const r = await reap(api(), { account: ACCOUNT, siteZone: ZONE, repo: "empathic/clax" });
  assert.deepEqual(r, { reaped: ["clax-pr-12", "clax-pr-14", "clax-pr-16"], kept: ["clax-pr-13"], failed: ["clax-pr-15"] });
  assert.deepEqual(state.scripts.map((s) => s.id), ["clax", "clax-dev", "clax-pr-13", "clax-pr-15"]);
  assert.deepEqual(state.queues.map((q) => q.queue_id), ["q", "qdev", "q13"]);
  assert.deepEqual(state.repos["clax-pr-16"], []);
});

test("only clax-pr-<N> Workers are previews to reap", () => {
  assert.equal(previewPr("clax-pr-12"), 12);
  for (const s of ["clax", "clax-dev", "clax-pr-", "clax-pr-0", "clax-pr-012", "clax-pr-12x", "clax-feature", "other-pr-12"]) {
    assert.equal(previewPr(s), null, s);
  }
});

test("the command line takes a preview or --reap, and a site zone", () => {
  assert.deepEqual(parseArgs(["pr-1", "--site-zone", ZONE]), { dryRun: false, reap: false, name: "pr-1", siteZone: ZONE });
  assert.throws(() => parseArgs(["pr-1"]), /--site-zone/);
  assert.throws(() => parseArgs(["--site-zone", ZONE]), /a preview's name/);
  assert.throws(() => parseArgs(["pr-1", "--reap", "--site-zone", ZONE, "--repo", "a/b"]), /a preview's name/);
  assert.throws(() => parseArgs(["--reap", "--site-zone", ZONE]), /--repo/);
  assert.throws(() => parseArgs(["pr-1", "pr-2", "--site-zone", ZONE]), /one preview at a time/);
});

test("the API's errors are named, and a wrong token fails", async () => {
  await assert.rejects(cloudflare("wrong").zoneId(ZONE), /GET \/zones\?name=preview\.run: HTTP 403: Authentication error \[code: 10000\]/);
  await assert.rejects(api().zoneId("other.run"), /the zone other\.run is not on an account this token can read/);
  assert.throws(() => cloudflare(""), /CLOUDFLARE_API_TOKEN/);
});

test("the wildcard record is made proxied, and the expiry rule is set once", async () => {
  await api().createProxiedRecord(ZONE_ID, "*.pr-20.preview.run", "clax: test");
  assert.deepEqual(state.dns.at(-1), { id: "d102", type: "AAAA", name: "*.pr-20.preview.run", content: "100::", proxied: true, ttl: 1, comment: "clax: test" });
  const rule = expiryRule(30);
  assert.equal(rule.deleteObjectsTransition.condition.maxAge, 30 * 86400);
  assert.equal(hasRule((await api().lifecycle(ACCOUNT, "clax-assets-preview")).rules, rule), false);
  await api().putLifecycle(ACCOUNT, "clax-assets-preview", [rule]);
  assert.equal(hasRule((await api().lifecycle(ACCOUNT, "clax-assets-preview")).rules, rule), true);
  assert.equal(hasRule([{ ...rule, enabled: false }], rule), false, "a disabled rule is not the rule");
});
