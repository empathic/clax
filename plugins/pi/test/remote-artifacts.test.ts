// The Pi extension on an artifact of a connected Clax instance: it asks the
// daemon what a URL off its base names (`GET /api/resolve`), reads and
// answers comments through the daemon's remote artifact routes, and refuses
// the tools that change an artifact there. The daemon here is a stand-in
// that answers as a local daemon connected to a remote does.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { claxExtension, resolved, withheldNote } from "../src/clax.ts";
import { FakePi, fakeContext, json } from "./fake-api.ts";

const ORIGIN = "https://clax.example";
const RAID = "7q3k9mzx2b4t";
const TID = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
const URL_THERE = `${ORIGIN}/a/${RAID}`;
const UNCONNECTED = "https://other.example/a/8q3k9mzx2b4t";
const REVOKED = "https://revoked.example/a/9q3k9mzx2b4t";
const BASE = `/api/remotes/r_1/artifacts/${RAID}`;

interface Seen { method: string; url: string; headers: IncomingMessage["headers"]; body: string }

let scratch: string;
let home: string;
let seen: Seen[];
let close: () => void;

beforeAll(async () => {
  scratch = mkdtempSync(join(tmpdir(), "clax-pi-remote-"));
  home = join(scratch, "home");
  seen = [];
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", c => { body += c; });
    req.on("end", () => {
      seen.push({ method: req.method ?? "", url: req.url ?? "", headers: req.headers, body });
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
      if (route === "GET /api/resolve") {
        const asked = u.searchParams.get("url");
        if (asked === URL_THERE) return reply(200, { kind: "remote", remote: "r_1", origin: ORIGIN, name: "acme", artifact_id: RAID, url: URL_THERE });
        if (asked === REVOKED) return reply(200, { kind: "remote", remote: "r_2", origin: "https://revoked.example", name: "gone", artifact_id: "9q3k9mzx2b4t", url: REVOKED });
        if (asked === UNCONNECTED) {
          return reply(200, { kind: "sign_in_required", origin: "https://other.example", name: "Other", request: { tool: "connect_instance", command: `clax remote request '${UNCONNECTED}'` } });
        }
        return reply(200, { kind: "page", url: asked });
      }
      if (u.pathname.startsWith("/api/remotes/r_2/")) {
        return reply(502, { error: { code: "remote_sign_in_required", message: "gone no longer takes this Clax's token", origin: "https://revoked.example", name: "gone", request: { tool: "connect_instance", command: "clax remote request 'https://revoked.example/'" } } });
      }
      if (route === `GET ${BASE}`) return reply(200, { artifact: { id: RAID, current_version: 1 }, versions: [{ n: 1, files: { "index.html": { content_type: "text/html" } } }] });
      if (route === `GET ${BASE}/versions/1/files/index.html`) {
        res.setHeader("content-type", "text/html");
        return res.end("<h1>Theirs</h1>");
      }
      const thread = (status: string) => ({
        id: TID, status, sent_to_agent: true, version_n: 1, anchor: { kind: "artifact" },
        comments: [{ id: "c0", author_kind: "viewer", author_name: "Ada", body: "@agent fix it", created_at: "t" }, { id: "c1", author_kind: "viewer", author_name: null, body: null, withheld: true, created_at: "t" }],
        withheld: { count: 1, authors: [], approval: "none" },
      });
      if (route === `GET ${BASE}/threads`) return reply(200, { threads: [thread("open"), { ...thread("open"), id: "01J9Z3K4M5N6P7Q8R9S0T1V2W4" }], next_cursor: null });
      if (route === `GET ${BASE}/threads/${TID}`) return reply(200, { thread: thread("open") });
      if (route === `POST ${BASE}/threads/${TID}/comments`) return reply(201, { comment: { id: "c2" }, thread: thread("open") });
      if (route === `POST ${BASE}/threads/${TID}/resolve`) return reply(200, { thread: thread("resolved") });
      if (route === `POST ${BASE}/threads/${TID}/handoff`) return reply(200, { thread: thread("open") });
      if (u.pathname.endsWith("/feedback")) return reply(200, { feedback: [], answers: [], withheld: [], notes: [] });
      return reply(200, {});
    });
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as { port: number }).port;
  // The daemon.json the extension finds its daemon by.
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
  return { pi, ...fakeContext(scratch, `pi-remote-${Math.random().toString(36).slice(2)}`) };
}

describe("a connected instance's artifact", () => {
  it("reads like a local artifact, naming where it is and what is withheld", async () => {
    const { pi, ctx } = load();
    const r = json(await pi.callTool("clax_read", { url_or_id: URL_THERE }, ctx));
    expect(r).toMatchObject({
      artifact_id: RAID, version: 1, path: "index.html", content: "<h1>Theirs</h1>", url: URL_THERE,
      remote: { origin: ORIGIN, name: "acme" },
      withheld_comments: { count: 2, note: "2 comments from people you have not approved for agents on acme" },
    });
    const c = json(await pi.callTool("clax_comments_read", { url_or_id: URL_THERE, thread_id: TID }, ctx));
    expect(c).toMatchObject({ artifact_id: RAID, url: URL_THERE, remote: { origin: ORIGIN, name: "acme" } });
    expect(c.threads[0].comments[1]).toMatchObject({ body: null, withheld: true });
    expect(c.withheld_comments).toEqual({ count: 1, note: "1 comment from people you have not approved for agents on acme" });
  });

  it("replies with an idempotency key, resolves and hands off through the daemon", async () => {
    const { pi, ctx } = load();
    const r = json(await pi.callTool("clax_comments_reply", { url_or_id: URL_THERE, thread_id: TID, text: "Fixed." }, ctx));
    expect(r).toMatchObject({ thread_id: TID, replied: true, comment_id: "c2" });
    const sent = seen.filter(s => s.method === "POST" && s.url === `${BASE}/threads/${TID}/comments`);
    expect(sent).toHaveLength(1);
    expect(JSON.parse(sent[0]!.body)).toEqual({ body: "Fixed.", author_kind: "agent" });
    expect(sent[0]!.headers["idempotency-key"]).toMatch(/^[0-9a-f-]{36}$/);
    const s = json(await pi.callTool("clax_comments_resolve", { url_or_id: URL_THERE, thread_id: TID }, ctx));
    expect(s).toEqual({ thread_id: TID, resolved: true, status: "resolved", feedback: [] });
    const h = json(await pi.callTool("clax_comments_handoff", { url_or_id: URL_THERE, thread_id: TID }, ctx));
    expect(h).toMatchObject({ artifact_id: RAID, url: URL_THERE, released: true, remote: { origin: ORIGIN, name: "acme" } });
  });

  it("is refused by the tools that change an artifact, and not watched yet", async () => {
    const { pi, ctx } = load();
    for (const [tool, args] of [
      ["clax_delete", { url_or_id: URL_THERE }],
      ["clax_tag", { url_or_id: URL_THERE, add: ["x"] }],
      ["clax_fork", { url_or_id: URL_THERE }],
    ] as const) {
      const out = await pi.callTool(tool, args, ctx);
      expect(out.isError, tool).toBe(true);
      expect(json(out).error, tool).toMatchObject({ code: "not_on_remote", name: "acme", origin: ORIGIN });
    }
    const w = await pi.callTool("clax_watch", { url_or_id: URL_THERE }, ctx);
    expect(json(w).error).toMatchObject({ code: "remote_not_yet", origin: ORIGIN });
  });

  it("names the instance to connect for an unconnected or revoked one", async () => {
    const { pi, ctx } = load();
    const u = json(await pi.callTool("clax_read", { url_or_id: UNCONNECTED }, ctx)).error;
    expect(u).toMatchObject({ code: "remote_sign_in_required", origin: "https://other.example", name: "Other", request: { tool: "connect_instance" } });
    expect(u.next).toContain("connect_instance");
    const r = json(await pi.callTool("clax_read", { url_or_id: REVOKED }, ctx)).error;
    expect(r).toMatchObject({ code: "remote_sign_in_required", origin: "https://revoked.example", name: "gone" });
    expect(r.next).toContain("connect_instance");
  });
});

describe("resolved", () => {
  it("reads each kind the daemon answers, and anything else as the page", () => {
    const U = "https://clax.example/a/7q3k9mzx2b4t/get";
    expect(resolved({ kind: "local", artifact_id: RAID, version: 3 }, U)).toEqual({ kind: "artifact", id: RAID, version: 3 });
    expect(resolved({ kind: "remote", remote: "r_1", origin: ORIGIN, artifact_id: RAID, url: URL_THERE }, U))
      .toEqual({ kind: "remote", remote: "r_1", origin: ORIGIN, name: ORIGIN, artifact_id: RAID, url: URL_THERE });
    expect(resolved({ kind: "sign_in_required", origin: ORIGIN, name: "Acme" }, U)).toEqual({ kind: "sign_in_required", origin: ORIGIN, name: "Acme", request: null });
    for (const odd of [{ kind: "followed" }, { kind: "local", artifact_id: "nope" }, { kind: "remote", origin: ORIGIN }, null]) {
      expect(resolved(odd, U)).toEqual({ kind: "page", url: U });
    }
    expect(withheldNote([{ withheld: { count: 0 } }, {}], "acme")).toBeUndefined();
  });
});
