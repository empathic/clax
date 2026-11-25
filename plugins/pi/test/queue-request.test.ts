// `clax_queue_request` against a stand-in daemon: it files the request
// (`POST /api/queue-requests`), waits on it (`GET /api/queue-requests/<id>`),
// takes the answer (`POST …/take`), and hands over the queue's service
// grant once, into `write_env` when given.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { claxExtension } from "../src/clax.ts";
import { FakePi, fakeContext, json } from "./fake-api.ts";

const QUEUE = "01K6AB3Q9X7N2M4P5R6S8T0V1Q";
const TOKEN = "cqg_secret_token";
const KEY = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEI\n-----END PRIVATE KEY-----";

interface Seen { method: string; url: string; body: string }

let scratch: string;
let home: string;
let seen: Seen[];
let close: () => void;

beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), "clax-pi-queue-request-"));
  home = join(scratch, "home");
  seen = [];
  // Each request's answer is chosen by its name: `approve` is approved,
  // anything else stays pending. An approved request's first read carries
  // the grant, and later reads say it was taken.
  const requests = new Map<string, { name: string; reads: number }>();
  let next = 0;
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", c => { body += c; });
    req.on("end", () => {
      seen.push({ method: req.method ?? "", url: req.url ?? "", body });
      const reply = (status: number, v: unknown) => {
        res.statusCode = status;
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify(v));
      };
      const u = new URL(req.url ?? "/", "http://x");
      const route = `${req.method} ${u.pathname}`;
      if (route === "GET /healthz") return reply(200, { version: "0.6.0" });
      if (route === "POST /api/sessions") {
        return reply(201, { session: { id: "s1", harness: "pi", harness_session_id: "h", cwd: "/", pid: 1, parent_pid: 1, started_at: "", last_seen_at: "", ended_at: null } });
      }
      if (route === "POST /api/queue-requests") {
        const id = `01K6AB3Q9X7N2M4P5R6S8T0V${String(next++).padStart(2, "0")}`;
        const name = JSON.parse(body).name;
        requests.set(id, { name, reads: 0 });
        return reply(201, { request: { id, name, status: "pending", queue_id: null } });
      }
      // A read changes nothing; the first take of an approved request
      // carries the grant, and later ones say it was taken.
      const m = /^(GET|POST) \/api\/queue-requests\/([0-9A-Z]{26})(\/take)?$/.exec(route);
      if (m && (m[1] === "GET") === (m[3] === undefined)) {
        const r = requests.get(m[2]!);
        if (!r) return reply(404, { error: { code: "not_found", message: "no such request" } });
        if (r.name !== "approve") {
          const request = { id: m[2], name: r.name, status: "pending", queue_id: null };
          return m[3] ? reply(409, { error: { code: "request_pending", message: "waits", request } }) : reply(200, { request });
        }
        const request = { id: m[2], name: r.name, status: "approved", queue_id: QUEUE };
        if (!m[3]) return reply(200, r.reads > 0 ? { request, grant_taken: true } : { request });
        if (r.reads++ > 0) return reply(200, { request, grant_taken: true });
        return reply(200, { request, grant: { id: "g1", queue_id: QUEUE }, token: TOKEN, private_key: KEY });
      }
      if (u.pathname.endsWith("/feedback")) return reply(200, { feedback: [], answers: [], withheld: [], notes: [] });
      return reply(200, {});
    });
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as { port: number }).port;
  mkdirSync(home, { recursive: true });
  writeFileSync(join(home, "daemon.json"), JSON.stringify({
    port, pid: process.pid, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.6.0",
  }));
  close = () => { server.closeAllConnections(); server.close(); };
});

afterAll(() => {
  close?.();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

function load() {
  const pi = new FakePi();
  claxExtension({ home, env: { ...process.env, CLAX_BIN: join(scratch, "no-such-clax"), CLAX_CODEX_BIN: "", CLAX_NO_OPEN: "1" } })(pi.api);
  return { pi, ...fakeContext(scratch, `pi-queue-${Math.random().toString(36).slice(2)}`) };
}

const filed = () => seen.filter(s => s.method === "POST" && s.url === "/api/queue-requests");

describe("clax_queue_request", () => {
  it("files the request, waits until it is approved, and writes the grant to write_env without returning it", async () => {
    const { pi, ctx } = load();
    const env = join(scratch, "app.env");
    writeFileSync(env, "PORT=3000");
    const before = filed().length;
    const out = await pi.callTool("clax_queue_request", { name: "approve", origins: ["https://app.example.com"], reporters_see: "all", note: "for the beta", write_env: env }, ctx);
    expect(out.isError).toBe(false);
    const r = json(out);
    expect(r).toMatchObject({ status: "approved", call_again: false, opened: false, wrote: env, variables: ["CLAX_QUEUE", "CLAX_QUEUE_TOKEN", "CLAX_QUEUE_KEY"], grant: { id: "g1", queue_id: QUEUE } });
    expect(r.request).toMatchObject({ id: r.request_id, status: "approved", queue_id: QUEUE });
    for (const k of ["token", "private_key", "secret"]) expect(r, k).not.toHaveProperty(k);
    expect(JSON.stringify(r)).not.toContain(TOKEN);
    expect(readFileSync(env, "utf8")).toBe(`PORT=3000\nCLAX_QUEUE=${QUEUE}\nCLAX_QUEUE_TOKEN=${TOKEN}\nCLAX_QUEUE_KEY=${KEY}\n`);
    expect(filed().length).toBe(before + 1);
    expect(JSON.parse(filed().at(-1)!.body)).toEqual({ name: "approve", origins: ["https://app.example.com"], reporters_see: "all", note: "for the beta" });
  });

  it("makes a missing write_env file owner-only", async () => {
    const { pi, ctx } = load();
    const env = join(scratch, "fresh.env");
    const r = json(await pi.callTool("clax_queue_request", { name: "approve", origins: ["https://app.example.com"], write_env: env }, ctx));
    expect(r.wrote).toBe(env);
    expect(readFileSync(env, "utf8")).toBe(`CLAX_QUEUE=${QUEUE}\nCLAX_QUEUE_TOKEN=${TOKEN}\nCLAX_QUEUE_KEY=${KEY}\n`);
    if (process.platform !== "win32") expect(statSync(env).mode & 0o777).toBe(0o600);
  });

  it("without write_env returns the token and key once, with the secret note, and then that the grant was taken", async () => {
    const { pi, ctx } = load();
    const r = json(await pi.callTool("clax_queue_request", { name: "approve", origins: ["https://app.example.com"] }, ctx));
    expect(r).toMatchObject({ status: "approved", call_again: false, token: TOKEN, private_key: KEY, grant: { id: "g1" } });
    expect(r.secret).toBe("shown this once: put token and private_key in the app's secret store, never in a reply, a commit or a page");
    const again = json(await pi.callTool("clax_queue_request", { request_id: r.request_id }, ctx));
    expect(again).toMatchObject({ request_id: r.request_id, status: "approved", grant_taken: true, note: "an earlier read took this queue's service grant; your person can make another on the Queues page" });
    for (const k of ["token", "private_key", "secret", "opened"]) expect(again, k).not.toHaveProperty(k);
  });

  it("refuses a relative write_env before anything is filed", async () => {
    const { pi, ctx } = load();
    const before = seen.length;
    const out = await pi.callTool("clax_queue_request", { name: "approve", origins: ["https://app.example.com"], write_env: "app/.env" }, ctx);
    expect(out.isError).toBe(true);
    expect(JSON.parse(out.content[0]!.text!).error).toMatchObject({ code: "invalid_args", message: "write_env is an absolute path" });
    expect(seen.slice(before).filter(s => s.url.startsWith("/api/queue-requests"))).toEqual([]);
  });

  it("refuses a write_env it cannot append to before anything is filed", async () => {
    const { pi, ctx } = load();
    const before = seen.length;
    const env = join(scratch, "no-such-dir", ".env");
    const out = await pi.callTool("clax_queue_request", { name: "approve", origins: ["https://app.example.com"], write_env: env }, ctx);
    expect(out.isError).toBe(true);
    expect(JSON.parse(out.content[0]!.text!).error).toMatchObject({ code: "file_unwritable", path: env });
    expect(seen.slice(before).filter(s => s.url.startsWith("/api/queue-requests"))).toEqual([]);
  });

  it("answers call_again while the request is pending, and keeps waiting on it by request_id", async () => {
    const { pi, ctx } = load();
    const r = json(await pi.callTool("clax_queue_request", { name: "later", origins: ["https://app.example.com"], timeout_s: 1 }, ctx));
    expect(r).toMatchObject({ status: "pending", call_again: true, request: { status: "pending" } });
    for (const k of ["token", "private_key", "grant", "wrote"]) expect(r, k).not.toHaveProperty(k);
    const polls = seen.filter(s => s.url.startsWith(`/api/queue-requests/${r.request_id}?`));
    expect(polls.map(p => new URL(p.url, "http://x").searchParams.get("wait"))).toEqual(["1"]);
    const again = json(await pi.callTool("clax_queue_request", { request_id: r.request_id, timeout_s: 0 }, ctx));
    expect(again).toMatchObject({ request_id: r.request_id, status: "pending", call_again: true });
    expect(again).not.toHaveProperty("opened");
    expect(filed().filter(s => JSON.parse(s.body).name === "later").length).toBe(1);
  });

  it("refuses a request_id that is not a ULID, and a call naming neither name nor request_id", async () => {
    const { pi, ctx } = load();
    const bad = await pi.callTool("clax_queue_request", { request_id: "q1" }, ctx);
    expect(JSON.parse(bad.content[0]!.text!).error).toMatchObject({ code: "invalid_args", message: "'q1' is not a request ID" });
    const neither = await pi.callTool("clax_queue_request", {}, ctx);
    expect(JSON.parse(neither.content[0]!.text!).error.code).toBe("invalid_args");
  });
});
