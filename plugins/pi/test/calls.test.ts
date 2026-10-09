// Tool-call identity (spec 2026-10-06-toolpath-audit-design §6.7, §12.2):
// the argument hash against the vectors the Rust side checks, and what the
// extension sends a daemon (here a recording stand-in) for each call.
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { argsSha256, beginCall, canonicalize, newUlid } from "../src/calls.ts";
import { claxExtension } from "../src/clax.ts";
import { FakePi, fakeContext } from "./fake-api.ts";

const VECTORS: { n: number; arguments: string; canonical: string; args_sha256: string }[] = JSON.parse(
  readFileSync(new URL("../../../crates/clax-core/tests/toolpath/args-hash-vectors.json", import.meta.url), "utf8"),
);

describe("argument hash", () => {
  it("args hash vectors", () => {
    expect(VECTORS).toHaveLength(7);
    for (const v of VECTORS) {
      // Pi hands the extension the arguments already parsed.
      const params = JSON.parse(v.arguments);
      expect(canonicalize(params), `vector ${v.n}`).toBe(v.canonical);
      expect(argsSha256(params), `vector ${v.n}`).toBe(v.args_sha256);
    }
    expect(argsSha256(undefined)).toBe(VECTORS[0].args_sha256);
  });

  it("mints ULIDs", () => {
    const a = newUlid(Date.UTC(2026, 9, 6));
    expect(a).toMatch(/^[0-7][0-9A-HJKMNP-TV-Z]{25}$/);
    expect(a.slice(0, 10)).toBe("01M4785400");
    expect(newUlid()).not.toBe(newUlid());
  });
});

/** One request the stand-in daemon received. */
interface Seen {
  method: string;
  url: string;
  headers: IncomingHttpHeaders;
  body: any;
}

/** A Clax home whose daemon is a stand-in that records every request and
 * answers registration, `list`, `pin` and the call reports. */
async function recordingHome(scratch: string) {
  const home = mkdtempSync(join(scratch, "home-"));
  const seen: Seen[] = [];
  const waiting: { n: number; done: () => void }[] = [];
  const reportCount = () => seen.filter(s => s.url.endsWith("/tool-calls")).length;
  /** Resolves once `n` call reports have arrived. */
  const reports = (n: number) => new Promise<void>(done => {
    if (reportCount() >= n) return done();
    waiting.push({ n, done });
  });
  const server: Server = createServer((req, res) => {
    let raw = "";
    req.on("data", c => { raw += c; });
    req.on("end", () => {
      const reply = (status: number, body: unknown) => {
        res.statusCode = status;
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify(body));
      };
      if (req.url === "/healthz") return reply(200, { version: "0.1.0" });
      seen.push({ method: req.method ?? "", url: req.url ?? "", headers: req.headers, body: raw ? JSON.parse(raw) : undefined });
      if (req.method === "POST" && req.url === "/api/sessions") {
        return reply(201, { session: { id: "S1", harness: "pi", harness_session_id: "h", cwd: "/", pid: 1, parent_pid: 1, started_at: "", last_seen_at: "", ended_at: null } });
      }
      if (req.url === "/api/artifacts") return reply(200, { artifacts: [] });
      if (req.method === "PATCH" && req.url === "/api/artifacts/zzzzzzzzzzzz") return reply(404, { error: { code: "not_found", message: "no such artifact" } });
      if (req.method === "PATCH" && req.url?.startsWith("/api/artifacts/")) return reply(200, { artifact: { pinned: true } });
      if (req.url === "/api/sessions/S1/tool-calls") {
        reply(201, { recorded: true, seq: 1 });
        for (const w of waiting.filter(w => reportCount() >= w.n)) w.done();
        return;
      }
      reply(200, {});
    });
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as { port: number }).port;
  writeFileSync(join(home, "daemon.json"), JSON.stringify({ port, pid: process.pid, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.1.0" }));
  return { home, seen, reports, close: () => { server.closeAllConnections(); server.close(); } };
}

/** The JSON an `x-clax-call` or `x-clax-git` header carries. */
function decoded(h: string | string[] | undefined): any {
  if (typeof h !== "string") throw new Error(`no header: ${h}`);
  return JSON.parse(Buffer.from(h, "base64url").toString("utf8"));
}

describe("tool calls", () => {
  let scratch: string;
  let repo: string;
  const cleanup: (() => void)[] = [];

  beforeAll(() => {
    scratch = realpathSync(mkdtempSync(join(tmpdir(), "clax-pi-calls-")));
    repo = join(scratch, "app");
    mkdirSync(repo);
    const git = (...args: string[]) => execFileSync("git", ["-c", "user.name=T", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", ...args], { cwd: repo, env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" } });
    git("init", "-q");
    writeFileSync(join(repo, "a.txt"), "one\n");
    git("add", "a.txt");
    git("commit", "-q", "-m", "one");
  });
  afterEach(() => { for (const c of cleanup.splice(0)) c(); });
  afterAll(() => rmSync(scratch, { recursive: true, force: true }));

  async function session() {
    const d = await recordingHome(scratch);
    cleanup.push(d.close);
    const pi = new FakePi();
    claxExtension({ home: d.home, env: { ...process.env, CLAX_NO_OPEN: "1" }, gitDeadlineMs: 20_000 })(pi.api);
    const { ctx } = fakeContext(repo, "pi-sess-1");
    cleanup.push(() => void pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx));
    return { d, pi, ctx };
  }

  it("tool call carries toolCallId", async () => {
    const { d, pi, ctx } = await session();
    const listParams = { limit: 5 };
    expect((await pi.callTool("clax_list", listParams, ctx, "toolu_pi_7")).isError).toBe(false);
    const pinParams = { url_or_id: "k3m9q2w8x1ab" };
    expect((await pi.callTool("clax_pin", pinParams, ctx, "toolu_pi_8")).isError).toBe(false);
    await d.reports(2);

    // Registration: the session's git state, and the call it was made for.
    const reg = d.seen.find(s => s.url === "/api/sessions")!;
    expect(decoded(reg.headers["x-clax-git"]).repo_root).toBe(repo);
    expect(decoded(reg.headers["x-clax-call"]).harness_call_id).toBe("toolu_pi_7");

    // A read-only call: its identity, no git.
    const list = d.seen.find(s => s.url === "/api/artifacts")!;
    const lc = decoded(list.headers["x-clax-call"]);
    expect(lc).toEqual({
      call_id: expect.stringMatching(/^[0-7][0-9A-HJKMNP-TV-Z]{25}$/),
      tool: "list",
      harness_tool: "clax_list",
      args_sha256: argsSha256(listParams),
      started_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/),
      harness_call_id: "toolu_pi_7",
    });
    expect(list.headers["x-clax-git"]).toBeUndefined();

    // A call that changes history carries the git state, waited for.
    const pin = d.seen.find(s => s.method === "PATCH")!;
    const pc = decoded(pin.headers["x-clax-call"]);
    expect([pc.tool, pc.harness_tool, pc.harness_call_id]).toEqual(["pin", "clax_pin", "toolu_pi_8"]);
    expect(pc.args_sha256).toBe(argsSha256(pinParams));
    const git = decoded(pin.headers["x-clax-git"]);
    expect([git.repo_root, git.branch, git.dirty]).toEqual([repo, "main", false]);

    // Each call is reported once its result is back.
    const reports = d.seen.filter(s => s.url === "/api/sessions/S1/tool-calls").map(s => s.body);
    expect(reports).toHaveLength(2);
    const [lr, pr] = [reports.find(r => r.tool === "list"), reports.find(r => r.tool === "pin")];
    expect(lr).toMatchObject({ ...lc, outcome: "ok" });
    expect(lr.artifact_id).toBeUndefined();
    expect(pr).toMatchObject({ ...pc, outcome: "ok", artifact_id: "k3m9q2w8x1ab" });
    expect(pr.ended_at >= pr.started_at).toBe(true);
    // Reports are not part of a call.
    for (const s of d.seen.filter(s => s.url.endsWith("/tool-calls"))) expect(s.headers["x-clax-call"]).toBeUndefined();
  });

  it("requests send via pi", async () => {
    const { d, pi, ctx } = await session();
    await pi.callTool("clax_list", {}, ctx, "toolu_pi_9");
    await pi.callTool("clax_pin", { url_or_id: "k3m9q2w8x1ab" }, ctx, "toolu_pi_10");
    await d.reports(2);
    await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
    expect(d.seen.map(s => s.url)).toContain("/api/sessions/S1");
    expect(d.seen.length).toBeGreaterThanOrEqual(6);
    for (const s of d.seen) expect(s.headers["x-clax-via"], `${s.method} ${s.url}`).toBe("pi");
  });

  it("arguments without a canonical form lose the record, not the call", async () => {
    for (const params of [{ n: Infinity }, { n: 1n }, { a: [undefined] }]) {
      expect(() => argsSha256(params)).toThrow();
      expect(beginCall({ tool: "list", harnessTool: "clax_list", harnessCallId: "t", params })).toBeUndefined();
    }
    const { d, pi, ctx } = await session();
    const out = await pi.callTool("clax_list", { limit: 5, extra: 10n }, ctx, "toolu_pi_big");
    expect(out.isError).toBe(false);
    // A later call is recorded, and is the only one reported.
    await pi.callTool("clax_list", {}, ctx, "toolu_pi_after");
    await d.reports(1);
    await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
    const list = d.seen.filter(s => s.url === "/api/artifacts");
    expect(list).toHaveLength(2);
    expect(list[0].headers["x-clax-call"]).toBeUndefined();
    expect(decoded(list[1].headers["x-clax-call"]).harness_call_id).toBe("toolu_pi_after");
    expect(d.seen.filter(s => s.url.endsWith("/tool-calls")).map(s => s.body.harness_call_id)).toEqual(["toolu_pi_after"]);
  });

  it("a failed call is reported as an error", async () => {
    const { d, pi, ctx } = await session();
    const out = await pi.callTool("clax_pin", { url_or_id: "zzzzzzzzzzzz" }, ctx, "toolu_pi_bad");
    expect(out.isError).toBe(true);
    await d.reports(1);
    const report = d.seen.find(s => s.url.endsWith("/tool-calls"))!.body;
    expect(report).toMatchObject({ tool: "pin", harness_call_id: "toolu_pi_bad", outcome: "error", artifact_id: "zzzzzzzzzzzz" });
  });
});
