import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer } from "node:net";
import { validateToolArguments, type Tool } from "@mariozechner/pi-ai";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionContext } from "@mariozechner/pi-coding-agent";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { artifactRef, claxExtension, htmlTitle, INJECT_RETRY_MS, isText, RENEW_EVERY_MS, START_BUDGET_MS, target, textPrefix, ToolError } from "../src/clax.ts";
import { DaemonClient, END_TIMEOUT_MS } from "../src/client.ts";
import { discover, endpointOf, ensure } from "../src/daemon.ts";
import { api, claxBin, startDaemon, type TestDaemon } from "./daemon-fixture.ts";
import { FakePi, fakeContext, json } from "./fake-api.ts";
import { fakeExe } from "./fake-exe.ts";

/** Inputs and expected outputs shared with the Rust tools. */
const FIXTURE = JSON.parse(readFileSync(new URL("./fixtures/contract.json", import.meta.url), "utf8"));
const TOOLS: string[] = FIXTURE.tools.map((t: { name: string }) => `clax_${t.name}`);

let daemon: TestDaemon;
let scratch: string;

beforeAll(async () => {
  daemon = await startDaemon();
  scratch = mkdtempSync(join(tmpdir(), "clax-pi-scratch-"));
}, 320_000);

afterAll(async () => {
  await daemon?.stop();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

/** A fresh extension instance for a Pi session `sessionId` in `cwd`, talking
 * to the daemon in `home`. */
function load(home: string, sessionId: string, cwd = scratch) {
  const pi = new FakePi();
  claxExtension({ home, env: withBin(claxBin) })(pi.api);
  const built = { pi, ...fakeContext(cwd, sessionId) };
  loaded.push({ pi, ctx: built.ctx });
  return built;
}

/** Every extension `load` built in the current test; each is shut down after
 * the test, so no feedback long-poll outlives it. */
const loaded: { pi: FakePi; ctx: ExtensionContext }[] = [];
afterEach(async () => {
  for (const l of loaded.splice(0)) await l.pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, l.ctx);
  vi.restoreAllMocks();
});

/** This process's environment with `CLAX_BIN` set to `bin` and Codex push
 * off (`CLAX_CODEX_BIN` empty) for any daemon it starts. */
function withBin(bin: string): NodeJS.ProcessEnv {
  return { ...process.env, CLAX_BIN: bin, CLAX_CODEX_BIN: "" };
}

/** An Clax home whose daemon answers `/healthz` and then never answers
 * `POST /api/sessions` (`hang: "register"`) or any `PATCH` (`hang: "patch"`);
 * `seen` lists the requests it received, as `METHOD path`. */
async function hungHome(hang: "register" | "patch") {
  const home = join(scratch, `hung-${Math.random().toString(36).slice(2)}`);
  mkdirSync(home, { recursive: true });
  const seen: string[] = [];
  const server = createHttpServer((req, res) => {
    seen.push(`${req.method} ${req.url}`);
    const reply = (body: unknown) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(body)); };
    if (req.url === "/healthz") return reply({ version: "0.1.0" });
    if (req.method === "POST" && req.url === "/api/sessions" && hang !== "register") {
      return reply({ session: { id: "s1", harness: "pi", harness_session_id: "h", cwd: "/", pid: 1, parent_pid: 1, started_at: "", last_seen_at: "", ended_at: null } });
    }
    // Otherwise never answer.
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as { port: number }).port;
  writeFileSync(join(home, "daemon.json"), JSON.stringify({
    port, pid: process.pid, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.1.0",
  }));
  return { home, seen, close: () => { server.closeAllConnections(); server.close(); } };
}

async function sessions(): Promise<any[]> {
  return (await api(daemon, "/api/sessions")).sessions;
}

/** A TCP port nothing listens on. */
async function closedPort(): Promise<number> {
  const srv = createServer();
  await new Promise<void>(r => srv.listen(0, "127.0.0.1", () => r()));
  const port = (srv.address() as { port: number }).port;
  await new Promise<void>(r => srv.close(() => r()));
  return port;
}

/** An Clax home whose daemon.json names a daemon that is not running. */
async function deadHome(): Promise<string> {
  const home = join(scratch, `dead-${Math.random().toString(36).slice(2)}`);
  mkdirSync(home, { recursive: true });
  writeFileSync(join(home, "daemon.json"), JSON.stringify({
    port: await closedPort(), pid: 999_999, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.1.0",
  }));
  return home;
}

describe("clax Pi extension", () => {
  it("registers the twenty-four tools with one-line prompt snippets, and the clax command", () => {
    const { pi } = load(daemon.home, "s-tools");
    expect([...pi.tools.keys()].sort()).toEqual([...TOOLS].sort());
    for (const t of pi.tools.values()) {
      expect(t.promptSnippet, t.name).toMatch(/^[^\n]+$/);
      expect(t.description.length, t.name).toBeGreaterThan(0);
    }
    expect([...pi.commands.keys()]).toEqual(["clax"]);
    for (const t of FIXTURE.tools) expect(pi.tools.get(`clax_${t.name}`)?.description, t.name).toBe(t.description);
  });

  it("registers a pi session on session_start and ends it on session_shutdown", async () => {
    const { pi, ctx } = load(daemon.home, "pi-session-1");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const s = (await sessions()).find(s => s.harness_session_id === "pi-session-1");
    expect(s).toMatchObject({ harness: "pi", cwd: scratch, pid: process.pid, parent_pid: process.ppid, ended_at: null });

    await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
    const ended = (await sessions()).find(x => x.id === s.id);
    expect(ended.ended_at).not.toBeNull();
  });

  it("publishes a page and reads it back", async () => {
    const { pi, ctx } = load(daemon.home, "pi-roundtrip");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const html = "<!doctype html><title>Round trip</title><p>hello</p>";
    const pub = await pi.callTool("clax_publish", { html, title: "Round trip" }, ctx);
    expect(pub.isError).toBe(false);
    const p = json(pub);
    expect(p).toMatchObject({ version: 1, title: "Round trip", files: ["index.html"], feedback: [] });
    expect(p.url).toMatch(new RegExp(`^http://localhost:\\d+/a/${p.artifact_id}$`));

    const read = json(await pi.callTool("clax_read", { url_or_id: p.url }, ctx));
    expect(read).toEqual({
      artifact_id: p.artifact_id, version: 1, path: "index.html", content_type: expect.stringMatching(/^text\/html/),
      truncated: false, size: Buffer.byteLength(html), content: html, feedback: [],
    });
  });

  it("resolves a relative file_path against the session's cwd", async () => {
    const cwd = join(scratch, "project");
    mkdirSync(cwd, { recursive: true });
    writeFileSync(join(cwd, "page.html"), "<!doctype html><title>From file</title>");
    const { pi, ctx } = load(daemon.home, "pi-relative", cwd);
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("clax_publish", { file_path: "page.html" }, ctx));
    const read = json(await pi.callTool("clax_read", { url_or_id: p.artifact_id }, ctx));
    expect(read.content).toBe("<!doctype html><title>From file</title>");
  });

  it("reports a stale if_version as a conflict naming the current version", async () => {
    const { pi, ctx } = load(daemon.home, "pi-conflict");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("clax_publish", { html: "<title>v1</title>" }, ctx));
    json(await pi.callTool("clax_publish", { id: p.artifact_id, if_version: 1, html: "<title>v2</title>" }, ctx));

    const stale = await pi.callTool("clax_publish", { id: p.artifact_id, if_version: 1, html: "<title>v3</title>" }, ctx);
    expect(stale.isError).toBe(true);
    const body = json(stale);
    expect(body.feedback).toEqual([]);
    expect(body.error).toMatchObject({
      code: "conflict", current: 2,
      current_version: { n: 2, files: ["index.html"], url: `${p.url}/v/2` },
      hint: "read the current version, merge your change, and retry with if_version = 2",
    });
  });

  it("lists only this session's artifacts with scope mine", async () => {
    const { pi, ctx } = load(daemon.home, "pi-mine");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const mine = json(await pi.callTool("clax_publish", { html: "<title>mine</title>", title: "mine" }, ctx));
    const other = await api(daemon, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "other", files: { "index.html": { content: "<title>other</title>", encoding: "utf8" } } }) });

    const all = json(await pi.callTool("clax_list", {}, ctx)).artifacts.map((a: any) => a.id);
    expect(all).toContain(mine.artifact_id);
    expect(all).toContain(other.artifact.id);

    const listed = json(await pi.callTool("clax_list", { scope: "mine" }, ctx));
    expect(listed.feedback).toEqual([]);
    expect(listed.artifacts).toHaveLength(1);
    expect(listed.artifacts[0]).toMatchObject({ id: mine.artifact_id, url: mine.url, title: "mine", version: 1, pinned: false });
    expect(listed.artifacts[0].owner_session_id).toBeTypeOf("string");
  });

  it("reports daemon_unreachable naming the daemon log when no daemon answers", async () => {
    const home = await deadHome();
    const pi = new FakePi();
    claxExtension({ home, env: withBin(join(scratch, "no-such-clax")) })(pi.api);
    const { ctx } = fakeContext(scratch, "pi-unreachable");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const res = await pi.callTool("clax_status", {}, ctx);
    expect(res.isError).toBe(true);
    const body = json(res);
    expect(body.feedback).toEqual([]);
    expect(body.error).toMatchObject({ code: "daemon_unreachable", log: join(home, "logs", "daemon.log") });
    expect(body.error.message).toContain("see its log");
  });

  it("registers on the first tool call when session_start found no daemon", async () => {
    const home = join(scratch, "late");
    mkdirSync(home, { recursive: true });
    // No daemon.json and no binary: session_start can neither find nor start a daemon.
    const pi = new FakePi();
    claxExtension({ home, env: withBin(join(scratch, "no-such-clax")) })(pi.api);
    const { ctx } = fakeContext(scratch, "pi-late");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    expect((await sessions()).some(s => s.harness_session_id === "pi-late")).toBe(false);

    copyFileSync(join(daemon.home, "daemon.json"), join(home, "daemon.json"));
    const status = json(await pi.callTool("clax_status", {}, ctx));
    expect(status).toMatchObject({ harness: "pi", session: { harness: "pi", harness_session_id: "pi-late" }, watches: [], feedback: [] });
    expect((await sessions()).some(s => s.harness_session_id === "pi-late")).toBe(true);
  });

  it("uploads assets as multipart and returns their URLs", async () => {
    const { pi, ctx } = load(daemon.home, "pi-assets");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("clax_publish", { html: "<title>assets</title>" }, ctx));
    const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
    writeFileSync(join(scratch, "dot.png"), png);
    const res = json(await pi.callTool("clax_asset_upload", { url_or_id: p.artifact_id, file_path: "dot.png" }, ctx));
    expect(res.assets).toHaveLength(1);
    expect(res.assets[0]).toMatchObject({ content_type: "image/png", size: png.length });
    const got = await fetch(res.assets[0].url.replace("localhost", "127.0.0.1"));
    expect(Buffer.from(await got.arrayBuffer())).toEqual(png);
  });

  it("leaves the asset content type to the daemon, which infers it from the file name", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tiff");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("clax_publish", { html: "<title>tiff</title>" }, ctx));
    // The part carries no Content-Type; the daemon infers image/tiff from the name.
    writeFileSync(join(scratch, "scan.tiff"), Buffer.from("II*\0\x08\0\0\0"));
    const res = json(await pi.callTool("clax_asset_upload", { url_or_id: p.artifact_id, file_path: "scan.tiff" }, ctx));
    expect(res.assets[0].content_type).toBe("image/tiff");
  });

  it("takes a new artifact's title from the page's <title> when none is given", async () => {
    const { pi, ctx } = load(daemon.home, "pi-title");
    const p = json(await pi.callTool("clax_publish", { html: "<title>  From &amp; the\n page </title><p>x</p>" }, ctx));
    expect(p).toMatchObject({ version: 1, title: "From & the page" });
    const v2 = json(await pi.callTool("clax_publish", { id: p.artifact_id, html: "<p>no title</p>" }, ctx));
    expect(v2).toMatchObject({ version: 2, title: "From & the page" });
  });

  it("refuses a new artifact with neither a title nor a page <title>", async () => {
    const { pi, ctx } = load(daemon.home, "pi-untitled");
    const res = await pi.callTool("clax_publish", { html: "<p>no title</p>" }, ctx);
    expect(res.isError).toBe(true);
    expect(json(res).error).toEqual({
      code: "invalid_args",
      message: "a new artifact needs a title: pass `title`, or give the page a non-empty <title>",
    });
  });

  it("registers a new session when its session was ended under it, and publishes", async () => {
    const { pi, ctx } = load(daemon.home, "pi-ended");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const first = json(await pi.callTool("clax_status", {}, ctx)).session;
    await api(daemon, `/api/sessions/${first.id}`, { method: "PATCH", body: JSON.stringify({ ended: true }) });

    const pub = await pi.callTool("clax_publish", { html: "<title>after end</title>" }, ctx);
    expect(pub.isError, JSON.stringify(json(pub))).toBe(false);
    const live = (await sessions()).filter(s => s.harness_session_id === "pi-ended" && s.ended_at === null);
    expect(live).toHaveLength(1);
    expect(live[0].id).not.toBe(first.id);
    const got = await api(daemon, `/api/artifacts/${json(pub).artifact_id}`);
    expect(got.artifact.owner_session_id).toBe(live[0].id);
    expect(json(await pi.callTool("clax_status", {}, ctx)).session.id).toBe(live[0].id);
  });

  it("reports opened from the opener's exit status", async () => {
    const { pi, ctx } = load(daemon.home, "pi-open");
    const p = json(await pi.callTool("clax_publish", { html: "<title>open me</title>" }, ctx));
    // An opener that exits is waited for however slowly it starts on a
    // loaded machine; one that lingers is given up on after `openWaitMs`.
    const openWith = async (script: string, openWaitMs = 30_000, extra: NodeJS.ProcessEnv = {}) => {
      const bin = mkdtempSync(join(scratch, "opener-"));
      for (const name of ["open", "xdg-open"]) {
        fakeExe(join(bin, name), `#!/bin/sh\n${script}\n`);
      }
      const env: NodeJS.ProcessEnv = { ...process.env, ...extra, CLAX_BIN: claxBin, PATH: `${bin}:${process.env.PATH}` };
      delete env.CLAX_NO_OPEN;
      const fresh = new FakePi();
      claxExtension({ home: daemon.home, env, openWaitMs })(fresh.api);
      return json(await fresh.callTool("clax_open", { url_or_id: p.artifact_id }, ctx));
    };
    expect(await openWith("exit 1")).toMatchObject({ url: p.url, opened: false });
    expect(await openWith("exit 0")).toMatchObject({ url: p.url, opened: true });
    // This opener never exits on its own, so `opened: true` comes from giving
    // up on it. It names its PID file in the environment, so its text, and
    // the executable fakeExe makes of it, is the same on every run.
    const pidFile = join(scratch, `opener-${Math.random().toString(36).slice(2)}.pid`);
    expect(await openWith('echo $$ > "$OPENER_PID_FILE"\nexec sleep 600', 1, { OPENER_PID_FILE: pidFile })).toMatchObject({ url: p.url, opened: true });
    let pid = 0;
    await expect.poll(() => (pid = Number(/^(\d+)\n$/.exec(existsSync(pidFile) ? readFileSync(pidFile, "utf8") : "")?.[1] ?? 0)), { timeout: 10_000 }).toBeGreaterThan(0);
    process.kill(pid, "SIGKILL");
  });

  it("status reports daemon_version only when the daemon's version differs", async () => {
    const { pi, ctx } = load(daemon.home, "pi-version");
    const s = json(await pi.callTool("clax_status", {}, ctx));
    expect(s.version).toBe(JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version);
    expect(s).not.toHaveProperty("daemon_version");
    // The tests run with CLAX_BIN set, so the binary is the test daemon's.
    expect(s.binary).toMatchObject({ path: claxBin, version: s.version });
    expect(s).not.toHaveProperty("upgrade_held");
  });

  it("status reports upgrade_held while a failed upgrade keeps the daemon at an older version", async () => {
    // A failed upgrade to a newer build, recorded as the Rust client records it.
    const exe = join(scratch, "held-clax");
    fakeExe(exe, "#!/bin/sh\nexit 1\n");
    const record = join(daemon.home, "logs", "failed-upgrade.json");
    mkdirSync(join(daemon.home, "logs"), { recursive: true });
    writeFileSync(record, JSON.stringify({
      version: "99.0.0", exe, mtime_ns: statSync(exe, { bigint: true }).mtimeNs.toString(),
      at: Math.floor(Date.now() / 1000), from_version: "0.1.0", reason: "it crashed",
    }));
    try {
      const { pi, ctx } = load(daemon.home, "pi-held");
      const s = json(await pi.callTool("clax_status", {}, ctx));
      expect(s.upgrade_held).toMatchObject({ version: "99.0.0", exe, from_version: "0.1.0", reason: "it crashed" });
      expect(s.upgrade_held.until).toMatch(/^\d{4}-\d{2}-\d{2}T/);
      expect(s.upgrade_held.advice).toContain("clax stop");
    } finally {
      rmSync(record, { force: true });
    }
  });

  it("refuses a relative path when the session has no working directory", async () => {
    const { pi, ctx } = load(daemon.home, "pi-no-cwd", "");
    const res = await pi.callTool("clax_publish", { file_path: "page.html", title: "t" }, ctx);
    expect(json(res).error).toEqual({
      code: "invalid_args",
      message: "file paths must be absolute: there is no session working directory to resolve 'page.html' against",
    });
  });

  it("reports an unexpected exception as an internal error result", async () => {
    const { pi, ctx } = load(daemon.home, "pi-internal");
    const res = await pi.callTool("clax_list", null, ctx);
    expect(res.isError).toBe(true);
    const body = json(res);
    expect(body.feedback).toEqual([]);
    expect(body.error.code).toBe("internal");
    expect(body.error.message).toEqual(expect.any(String));
  });

  it("gives up registering at session_start when its budget runs out while the daemon hangs", async () => {
    expect(START_BUDGET_MS).toBe(3_000);
    const hung = await hungHome("register");
    try {
      const pi = new FakePi();
      // The daemon never answers, so session_start returns only by giving up.
      claxExtension({ home: hung.home, env: withBin(join(scratch, "no-such-clax")), startBudgetMs: 50 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-hung-start");
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      expect(hung.seen).toContain("POST /api/sessions");
    } finally {
      hung.close();
    }
  });

  it("gives up ending the session when its time runs out while the daemon hangs", async () => {
    expect(END_TIMEOUT_MS).toBe(3_000);
    const hung = await hungHome("patch");
    try {
      const pi = new FakePi();
      // The daemon never answers the PATCH, so session_shutdown returns only by giving up.
      claxExtension({ home: hung.home, env: withBin(join(scratch, "no-such-clax")), endTimeoutMs: 50 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-hung-end");
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
      expect(hung.seen).toContain("PATCH /api/sessions/s1");
    } finally {
      hung.close();
    }
  });

  it("Pi's argument validation accepts valid arguments and rejects unknown keys", () => {
    const { pi } = load(daemon.home, "pi-validate");
    const id = "7q3k9mzx2b4t";
    const valid: Record<string, Record<string, unknown>[]> = {
      clax_publish: [
        { html: "<title>x</title>" },
        { file_path: "page.html", id, if_version: 2, title: "t", description: "d", icon: "chart", label: "l", capabilities: { db: {} },
          files: { "a.css": { content: "body{}" }, "b.png": { content: "iVBORw0K", encoding: "base64", content_type: "image/png" }, "c.js": { path: "c.js" }, "old.txt": null } },
        { html: "<p>", url: `http://localhost:7480/a/${id}` },
      ],
      clax_read: [{ url_or_id: id }, { url_or_id: id, path: "a.css", version: 1, max_bytes: 10 }],
      clax_list: [{}, { limit: 5, scope: "mine" }, { scope: "all" }],
      clax_delete: [{ url_or_id: id }],
      clax_open: [{ url_or_id: id }],
      clax_pin: [{ url_or_id: id }],
      clax_unpin: [{ url_or_id: id }],
      clax_asset_upload: [{ url_or_id: id, file_path: "a.png" }, { url_or_id: id, file_paths: ["a.png", "b.mp4"] }],
      clax_status: [{}],
      clax_comments_read: [{ url_or_id: id }, { url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W", cursor: "01K6AB3Q9X7N2M4P5R6S8T0V1W", include_resolved: true }],
      clax_comments_reply: [{ url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W", text: "done" }, { url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W", text: "done", addressed: true }],
      clax_comments_resolve: [{ url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W" }],
      clax_watch: [{ url_or_id: id }, { url_or_id: id, on: false, replies: false }],
      clax_wait_for_feedback: [{}, { url_or_id: id, timeout_s: 50 }],
      clax_ask: [
        { questions: [{ question: "Which?", header: "Pick", options: [{ label: "A", description: "a", preview: "+-+", recommended: true }, { label: "B" }], multi_select: false, other: true }], url_or_id: id, timeout_s: 60 },
        { question_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W", cancel: true },
      ],
      clax_db_get: [{ url_or_id: id, collection: "tasks", doc_id: "t1" }, { url_or_id: id, collection: "tasks", doc_id: "t1", as_level: "view" }],
      clax_db_list: [{ url_or_id: id, collection: "tasks" }, { url_or_id: id, collection: "tasks", query: { limit: 10, cursor: "t1" } }],
      clax_db_query: [{ url_or_id: id, collection: "tasks", query: { where: [["n", ">", 1]], order_by: { field: "n", direction: "desc" }, limit: 5 } }],
      clax_db_set: [{ url_or_id: id, collection: "tasks", doc_id: "t1", data: { n: 1 } }, { url_or_id: id, collection: "tasks", doc_id: "t1", file_path: "t.json", if_version: 2, as_level: "admin" }],
      clax_db_update: [{ url_or_id: id, collection: "tasks", doc_id: "t1", data: { n: 1 }, if_version: 1 }],
      clax_db_delete: [{ url_or_id: id, collection: "tasks", doc_id: "t1", if_version: 1 }],
      clax_db_str_replace: [{ url_or_id: id, collection: "tasks", doc_id: "t1", field: "html", old_str: "a", new_str: "b", replace_all: true, if_version: 1 }],
      clax_db_batch: [{ url_or_id: id, writes: [{ op: "set", collection: "tasks", doc_id: "t1", data: {} }, { op: "delete", collection: "tasks", doc_id: "t2", if_version: 1 }] }],
      clax_working: [{ url_or_id: id }, { url_or_id: id, thread_ids: ["t1"], message: "m", done: true }],
    };
    const invalid: Record<string, Record<string, unknown>[]> = {
      clax_publish: [{ html: "x", bogus: 1 }, { html: "x", files: { "a.css": { content: "x", nope: 1 } } }, { html: "x", files: { "a.css": { content: "x", encoding: "hex" } } }],
      clax_read: [{ url_or_id: id, bogus: 1 }, {}],
      clax_list: [{ scope: "theirs" }, { bogus: 1 }],
      clax_delete: [{ url_or_id: id, bogus: 1 }],
      clax_open: [{ url_or_id: id, bogus: 1 }],
      clax_pin: [{ url_or_id: id, bogus: 1 }],
      clax_unpin: [{ url_or_id: id, bogus: 1 }],
      clax_asset_upload: [{ url_or_id: id, file_path: "a.png", bogus: 1 }],
      clax_status: [{ bogus: 1 }],
      clax_comments_read: [{}, { url_or_id: id, bogus: 1 }],
      clax_comments_reply: [{ url_or_id: id, thread_id: "x" }, { url_or_id: id, thread_id: "x", text: "t", bogus: 1 }],
      clax_comments_resolve: [{ url_or_id: id }],
      clax_watch: [{ url_or_id: id, on: "yes" }],
      clax_wait_for_feedback: [{ timeout_s: -1 }, { bogus: 1 }],
      clax_ask: [{ bogus: 1 }, { questions: [] }, { questions: [{ question: "Q" }] }, { questions: [{ question: "Q", header: "H", nope: 1 }] }, { questions: [{ question: "Q", header: "H", options: [{ label: "A", nope: 1 }] }] }],
      clax_working: [{ url_or_id: id, done: "yes" }, { url_or_id: id, bogus: 1 }],
      clax_db_get: [{ url_or_id: id, collection: "tasks" }, { url_or_id: id, collection: "tasks", doc_id: "t1", as_level: "owner" }],
      clax_db_query: [{ url_or_id: id, collection: "tasks", query: { bogus: 1 } }],
      clax_db_set: [{ url_or_id: id, collection: "tasks", doc_id: "t1", data: { n: 1 }, if_version: 0 }, { url_or_id: id, collection: "tasks", doc_id: "t1", bogus: 1 }],
      clax_db_batch: [{ url_or_id: id, writes: [] }, { url_or_id: id, writes: [{ op: "move", collection: "tasks", doc_id: "t1" }] }],
    };
    expect(Object.keys(valid).sort()).toEqual([...TOOLS].sort());
    const check = (name: string, args: Record<string, unknown>) =>
      validateToolArguments(pi.tools.get(name) as unknown as Tool, { type: "toolCall", id: "1", name, arguments: structuredClone(args) });
    for (const [name, cases] of Object.entries(valid)) for (const args of cases) expect(() => check(name, args), `${name} ${JSON.stringify(args)}`).not.toThrow();
    for (const [name, cases] of Object.entries(invalid)) for (const args of cases) expect(() => check(name, args), `${name} ${JSON.stringify(args)}`).toThrow();
  });

  it("the clax command reports status and lists artifacts", async () => {
    const { pi, ctx, notes } = load(daemon.home, "pi-command");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    json(await pi.callTool("clax_publish", { html: "<title>cmd</title>", title: "Command page" }, ctx));
    await pi.runCommand("clax", "status", ctx);
    expect(notes.at(-1)?.message).toMatch(/^clax daemon at http:\/\/localhost:\d+ \(v[^)]+\), session /);
    await pi.runCommand("clax", "list", ctx);
    expect(notes.at(-1)?.message).toContain("Command page");
    await pi.runCommand("clax", "bogus", ctx);
    expect(notes.at(-1)).toMatchObject({ type: "error" });
    expect(notes.at(-1)?.message).toContain("usage: /clax open [ID] | list | status");
  });
});

/** Creates a thread on version 1 as a browser does; `@agent` in `body` sends it. */
async function browserThread(aid: string, body: string, base = daemon.base, anchor: object = { kind: "element", selector: "body > h2", quote: "Goals" }): Promise<string> {
  const form = new FormData();
  form.set("anchor", JSON.stringify(anchor));
  form.set("body", body);
  form.set("version", "1");
  const res = await fetch(`${base}/api/artifacts/${aid}/threads`, { method: "POST", body: form });
  expect(res.status).toBe(201);
  return (await res.json()).thread.id;
}

/** A comment on the web page `url` as a named viewer, with its snapshot and
 * clip, as the Chrome extension posts it; the live page's artifact ID and the
 * thread ID. */
async function liveThread(url: string, body: string): Promise<{ aid: string; tid: string }> {
  const me = await fetch(`${daemon.base}/api/viewers/me`);
  const cookie = (me.headers.get("set-cookie") ?? "").split(";")[0];
  const named = await fetch(`${daemon.base}/api/viewers/me`, {
    method: "PUT", headers: { cookie, "content-type": "application/json" }, body: JSON.stringify({ display_name: "Alex" }),
  });
  expect(named.status).toBe(200);
  const form = new FormData();
  form.set("url", url);
  form.set("title", "Settings");
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body", file: "index.html" }));
  form.set("body", body);
  form.set("pending", "[]");
  form.set("snapshot", new Blob(["<!doctype html><p>x"], { type: "text/html" }));
  form.set("clip", new Blob([Buffer.from("89504e470d0a1a0a636c61782d74657374", "hex")], { type: "image/png" }));
  const res = await fetch(`${daemon.base}/api/live/threads`, { method: "POST", headers: { cookie }, body: form });
  expect(res.status).toBe(201);
  const r = await res.json();
  return { aid: r.page.artifact_id, tid: r.thread.id };
}

/** The daemon's ID of the Pi session `harnessId`. */
async function sessionOf(harnessId: string): Promise<string> {
  const s = (await sessions()).find(x => x.harness_session_id === harnessId && x.ended_at === null);
  expect(s, harnessId).toBeDefined();
  return s.id;
}

/** Ends `wait_for_feedback` polls of session `sid` now, as if their wait ran
 * out (a debug-build route), once `n` of them wait. */
async function expireFeedback(sid: string, n = 1): Promise<void> {
  expect((await api(daemon, `/api/_test/sessions/${sid}/feedback/waiters?until=${n}`)).count).toBe(n);
  expect((await api(daemon, `/api/_test/sessions/${sid}/feedback/expire`, { method: "POST", body: "{}" })).expired).toBe(n);
}

/** Ends the poll on question `qid` now, as if its wait ran out (a
 * debug-build route), once it waits. */
async function expireQuestion(qid: string): Promise<void> {
  expect((await api(daemon, `/api/_test/questions/${qid}/waiters?until=1`)).count).toBe(1);
  expect((await api(daemon, `/api/_test/questions/${qid}/expire`, { method: "POST", body: "{}" })).expired).toBe(1);
}

/** The ID of the open question headed `header`, once there is one. */
async function openQuestion(header: string): Promise<string> {
  let id: string | undefined;
  await expect.poll(async () => {
    id = (await api(daemon, "/api/questions?status=open")).questions.find((q: any) => q.questions[0].header === header)?.id;
    return id;
  }, { timeout: 5_000 }).toBeDefined();
  return id!;
}

/** An `ask` question set with one single-select question headed `header`. */
function askBody(header: string) {
  return { source: "ask", questions: [{ question: "Which?", header, options: [{ label: "A" }, { label: "B" }] }] };
}

/** Answers question `qid` with `label` as the owner (the token). */
async function answer(qid: string, label: string): Promise<void> {
  await api(daemon, `/api/questions/${qid}/answer`, { method: "POST", body: JSON.stringify({ answers: [{ selected: [label] }] }) });
}

/** Session `sid` asks a question headed `header`, which the owner answers
 * with `label` while no poll waits on it; its ID. */
async function askAndAnswer(sid: string, header: string, label: string): Promise<string> {
  const qid = (await api(daemon, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify(askBody(header)) })).question.id;
  await answer(qid, label);
  return qid;
}

/** The JSON block and the trailing block of a tool result. */
function parts(o: { content: { type: string; text?: string }[]; isError: boolean }) {
  expect(o.isError).toBe(false);
  return { json: JSON.parse(o.content[0].text!), trailing: o.content[1]?.text };
}

describe("comments", () => {
  it("tier 1: clax tool results carry pending feedback once", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tier1");
    const p = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi loop" }, ctx)).json;
    await browserThread(p.artifact_id, "@agent make it two columns");
    const r = parts(await pi.callToolAsPi("clax_list", {}, ctx));
    expect(r.json.feedback).toHaveLength(1);
    expect(r.trailing).toMatch(/^---\n\[clax\] 1 comment sent to you:\n\[clax\] Comment sent to you on "Pi loop"/);
    const again = await pi.callToolAsPi("clax_list", {}, ctx);
    expect(again.content).toHaveLength(1);
    expect(parts(again).json.feedback).toEqual([]);
  });

  it("read, reply, resolve, and watch match the MCP tools", async () => {
    const { pi, ctx } = load(daemon.home, "pi-comments");
    const aid = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi threads" }, ctx)).json.artifact_id;
    const plain = await browserThread(aid, "plain note");
    const sent = await browserThread(aid, "@agent fix it");
    const read = parts(await pi.callToolAsPi("clax_comments_read", { url_or_id: aid }, ctx)).json;
    expect(read.threads.map((t: any) => t.thread_id)).toEqual([plain, sent]);
    expect(read.threads[0].anchor).toMatchObject({ file: "index.html", selector: "body > h2", area: null, summary: "body > h2  «Goals»" });
    expect(read.note).toContain("people viewing the page");
    expect(read.feedback).toEqual([]);
    const area = { x: 0.1, y: 0.2, w: 0.4213, h: 0.18 };
    await browserThread(aid, "what is this gap?", daemon.base, { kind: "area", selector: "body > h2", area, file: "index.html" });
    const drawn = parts(await pi.callToolAsPi("clax_comments_read", { url_or_id: aid }, ctx)).json.threads.at(-1);
    expect(drawn.anchor).toMatchObject({ kind: "area", area, summary: "area in body > h2 (42% × 18%)" });
    await browserThread(aid, "this thin line", daemon.base, { kind: "area", selector: "body > h2", area: { x: 0, y: 0.5, w: 0.4213, h: 0.003 }, file: "index.html" });
    const thin = parts(await pi.callToolAsPi("clax_comments_read", { url_or_id: aid }, ctx)).json.threads.at(-1);
    expect(thin.anchor.summary).toBe("area in body > h2 (42% × <1%)");
    expect(parts(await pi.callToolAsPi("clax_comments_reply", { url_or_id: aid, thread_id: plain, text: "ok" }, ctx)).json).toMatchObject({ replied: false });
    expect(parts(await pi.callToolAsPi("clax_comments_reply", { url_or_id: aid, thread_id: sent, text: "Fixed." }, ctx)).json).toMatchObject({ replied: true });
    expect(parts(await pi.callToolAsPi("clax_comments_resolve", { url_or_id: aid, thread_id: sent }, ctx)).json).toMatchObject({ resolved: true, status: "resolved" });
    const t = await api(daemon, `/api/artifacts/${aid}/threads/${sent}`);
    expect(t.thread.comments[1]).toMatchObject({ author_kind: "agent", author_name: "pi" });
    expect(parts(await pi.callToolAsPi("clax_watch", { url_or_id: aid, replies: false }, ctx)).json).toMatchObject({ watching: true, replies_armed: false });
    const status = parts(await pi.callToolAsPi("clax_status", {}, ctx)).json;
    expect(status.watches[0]).toMatchObject({ artifact_id: aid, replies_armed: false });
    expect(status.push).toMatchObject({ tier: "inject", available: true });
  });

  it("watch, comments_read, and wait_for_feedback take a page URL", async () => {
    const { pi, ctx } = load(daemon.home, "pi-live");
    const w = parts(await pi.callToolAsPi("clax_watch", { url_or_id: "http://localhost:5173/" }, ctx)).json;
    expect(w).toMatchObject({ page_url: "http://localhost:5173/", scope: "http://localhost:5173/*", watching: true, replies_armed: true, site: { name: "http://localhost:5173", joined: false, origins: ["http://localhost:5173"] } });
    expect(w.url).toMatch(new RegExp(`/a/${w.artifact_id}$`));
    // `@agent` sends the thread.
    const { aid, tid } = await liveThread("http://localhost:5173/settings", "@agent the button overflows");
    const waited = await pi.callToolAsPi("clax_wait_for_feedback", { url_or_id: "http://localhost:5173/settings", timeout_s: 5 }, ctx);
    const got = parts(waited);
    expect(got.json.feedback).toHaveLength(1);
    expect(got.json.feedback[0]).toMatchObject({ thread_id: tid, live: { page_url: "http://localhost:5173/settings" } });
    expect(got.trailing).toContain("(live page http://localhost:5173/settings; Clax view ");
    const read = parts(await pi.callToolAsPi("clax_comments_read", { url_or_id: "http://localhost:5173/settings" }, ctx)).json;
    expect(read.artifact_id).toBe(aid);
    expect(read.threads[0]).toMatchObject({ thread_id: tid, page_url: "http://localhost:5173/settings", addressed_pending: false });
    expect(read.threads[0].snapshot_path).toMatch(/\/versions\/1\/index\.html$/);
    const never = await pi.callTool("clax_comments_read", { url_or_id: "http://localhost:5173/never" }, ctx);
    expect(never.isError).toBe(true);
    expect(json(never).error.code).toBe("invalid_id");
    const off = parts(await pi.callToolAsPi("clax_watch", { url_or_id: "http://localhost:5173/", on: false }, ctx)).json;
    expect(off).toMatchObject({ page_url: "http://localhost:5173/", watching: false });
    const offRoute = parts(await pi.callToolAsPi("clax_watch", { url_or_id: "http://localhost:5173/settings?tab=b#x", on: false }, ctx)).json;
    expect(offRoute).toMatchObject({ page_url: "http://localhost:5173/settings", watching: false });
  });

  it("wait_for_feedback returns on a send, not at its timeout, and asks to call again", async () => {
    const { pi, ctx } = load(daemon.home, "pi-wait");
    const aid = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi wait" }, ctx)).json.artifact_id;
    const sid = await sessionOf("pi-wait");
    // The longest wait: only the send can end it within the test's time.
    const waiting = pi.callToolAsPi("clax_wait_for_feedback", { url_or_id: aid, timeout_s: 600 }, ctx);
    expect((await api(daemon, `/api/_test/sessions/${sid}/feedback/waiters?until=1`)).count).toBe(1);
    await browserThread(aid, "@agent live");
    const r = parts(await waiting);
    expect(r.json).toMatchObject({ call_again: false });
    expect(r.json.feedback).toHaveLength(1);
    const empty = pi.callToolAsPi("clax_wait_for_feedback", { timeout_s: 30 }, ctx);
    await expireFeedback(sid);
    expect(parts(await empty).json).toEqual({ feedback: [], waited_s: 0, call_again: true });
  }, 20_000);

  it("tier 5: the extension long-polls and hands comments to Pi as a follow-up", async () => {
    const polls = trackPolls();
    const { pi, ctx } = load(daemon.home, "pi-inject");
    await pi.emit("session_start", {}, ctx);
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Pi inject" }, ctx)).json.artifact_id;
    await browserThread(aid, "@agent please shorten it");
    await expect.poll(() => pi.sent.length, { timeout: 15_000 }).toBe(1);
    expect(pi.sent[0].options).toEqual({ deliverAs: "followUp" });
    expect(String(pi.sent[0].content)).toMatch(/^\[clax\] 1 comment sent to you:\n\[clax\] Comment sent to you on "Pi inject"/);
    // Shutdown aborts the loop's poll; the loop never polls or sends once it is aborted.
    await pi.emit("session_shutdown", {}, ctx);
    expect(polls.length).toBeGreaterThan(0);
    for (const poll of polls) expect(poll.signal.aborted).toBe(true);
    await Promise.all(polls.map(poll => poll.settled));
    expect(pi.sent).toHaveLength(1);
  }, 20_000);

  it("tier 5 yields to wait_for_feedback: a comment sent during the wait goes to the wait", async () => {
    const polls = trackPolls();
    const { pi, ctx } = load(daemon.home, "pi-inject-wait");
    await pi.emit("session_start", {}, ctx);
    await expect.poll(() => polls.length, { timeout: 10_000 }).toBe(1);
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Pi inject wait" }, ctx)).json.artifact_id;
    const sid = await sessionOf("pi-inject-wait");
    const waiting = pi.callToolAsPi("clax_wait_for_feedback", { url_or_id: aid, timeout_s: 600 }, ctx);
    expect((await api(daemon, `/api/_test/sessions/${sid}/feedback/waiters?until=1`)).count).toBe(1);
    await browserThread(aid, "@agent during the wait");
    const got = parts(await waiting).json;
    expect(got.feedback).toHaveLength(1);
    expect(pi.sent).toHaveLength(0);
  }, 20_000);

  it("tier 5 yields to wait_for_feedback: a loop started during the wait holds until it ends, then polls", async () => {
    // session_start finds no daemon, so the loop starts at the first tool
    // result after the daemon appears, which comes while the wait is in progress.
    const home = join(scratch, "inject-yield");
    mkdirSync(home, { recursive: true });
    const pi = new FakePi();
    // A clock that never moves: every empty answer came back at once.
    claxExtension({ home, env: withBin(join(scratch, "no-such-clax")), now: () => 0 })(pi.api);
    const { ctx } = fakeContext(scratch, "pi-inject-yield");
    loaded.push({ pi, ctx });
    await pi.emit("session_start", {}, ctx);
    copyFileSync(join(daemon.home, "daemon.json"), join(home, "daemon.json"));
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Pi inject yield" }, ctx)).json.artifact_id;
    const sid = await sessionOf("pi-inject-yield");
    const polls = trackPolls();
    const pauses = trackTimers(INJECT_RETRY_MS);
    // wait_for_feedback's result carries no tier 1 feedback and so does not start the loop.
    const waiting = pi.callToolAsPi("clax_wait_for_feedback", { url_or_id: aid, timeout_s: 600 }, ctx);
    expect((await api(daemon, `/api/_test/sessions/${sid}/feedback/waiters?until=1`)).count).toBe(1);
    expect(polls).toHaveLength(0);
    // The status result starts the loop, which makes no poll and no pause while the wait runs.
    parts(await pi.callToolAsPi("clax_status", {}, ctx));
    await turns();
    expect(polls).toHaveLength(0);
    expect(pauses.created.size).toBe(0);
    await browserThread(aid, "@agent during the wait");
    expect(parts(await waiting).json.feedback).toHaveLength(1);
    // The wait ended: the loop polls at once.
    await expect.poll(() => polls.length, { timeout: 10_000 }).toBe(1);
    expect(pauses.created.size).toBe(0);
    expect(pi.sent).toHaveLength(0);
  }, 20_000);

  // The fake daemon answers as the daemon does: a parked inject poll is woken
  // empty when a wait starts, and an inject poll during a wait answers empty
  // at once. The poll was parked under or over INJECT_EARLY_MS when the wait
  // began; neither changes what the loop does. Timers are fake and never
  // advanced, so a retry pause would never end.
  it.each([0, 2_000])("tier 5 holds while wait_for_feedback runs and polls again as soon as it ends (parked %i ms)", async parkedMs => {
    const empty = { feedback: [], answers: [], text: null, waited_s: 0 };
    const parked: ((body: unknown) => void)[] = [];
    let inWait = false;
    let endWait: (() => void) | undefined;
    const fake = await fakeDaemonHome((req, reply) => {
      if (!req.url?.startsWith("/api/sessions/s1/feedback")) return false;
      const tier = new URL(req.url, "http://x").searchParams.get("tier");
      if (tier === "inject") {
        if (inWait) reply(empty);
        else parked.push(reply);
      } else if (tier === "wait") {
        inWait = true;
        for (const r of parked.splice(0)) r(empty);
        endWait = () => { inWait = false; reply(empty); };
      } else return false;
    });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const pauses = trackTimers(INJECT_RETRY_MS);
      const polls = trackPolls();
      let clock = 0;
      const pi = new FakePi();
      claxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-clax")), now: () => clock })(pi.api);
      const { ctx } = fakeContext(scratch, `pi-inject-hold-${parkedMs}`);
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      await expect.poll(() => parked.length, { timeout: 10_000 }).toBe(1);
      clock += parkedMs;
      const waiting = pi.callToolAsPi("clax_wait_for_feedback", { timeout_s: 600 }, ctx);
      await expect.poll(() => endWait !== undefined, { timeout: 10_000 }).toBe(true);
      // The woken poll answered empty; the loop neither polls again nor pauses.
      expect(await polls[0].settled).toBe("answered");
      await turns();
      expect(polls).toHaveLength(1);
      expect(pauses.created.size).toBe(0);
      endWait!();
      expect(parts(await waiting).json).toEqual({ feedback: [], waited_s: 0, call_again: true });
      // Polled again with no timer run: the next poll parks.
      await expect.poll(() => parked.length, { timeout: 10_000 }).toBe(1);
      expect(polls).toHaveLength(2);
      expect(pauses.created.size).toBe(0);
      await pi.emit("session_shutdown", {}, ctx);
      loaded.splice(loaded.findIndex(l => l.pi === pi), 1);
    } finally {
      vi.useRealTimers();
      fake.close();
    }
  });

  // A wait that starts and ends while the loop's poll is in flight (one that
  // finds pending feedback at once): the poll, woken empty by the wait's
  // start, answers after the wait ended. It answered for the wait, not
  // early, so the loop polls again at once rather than taking a retry pause.
  it("tier 5 polls again at once after a wait that started and ended during one poll", async () => {
    const empty = { feedback: [], answers: [], text: null, waited_s: 0 };
    const parked: ((body: unknown) => void)[] = [];
    const fake = await fakeDaemonHome((req, reply) => {
      if (!req.url?.startsWith("/api/sessions/s1/feedback")) return false;
      const tier = new URL(req.url, "http://x").searchParams.get("tier");
      if (tier === "inject") parked.push(reply);
      else if (tier === "wait") reply(empty);
      else return false;
    });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const pauses = trackTimers(INJECT_RETRY_MS);
      const polls = trackPolls();
      const pi = new FakePi();
      // A clock that never moves: an empty answer looks early.
      claxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-clax")), now: () => 0 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-epoch");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      await expect.poll(() => parked.length, { timeout: 10_000 }).toBe(1);
      parts(await pi.callToolAsPi("clax_wait_for_feedback", { timeout_s: 5 }, ctx));
      parked.shift()!(empty);
      expect(await polls[0].settled).toBe("answered");
      await expect.poll(() => parked.length, { timeout: 10_000 }).toBe(1);
      expect(polls).toHaveLength(2);
      expect(pauses.created.size).toBe(0);
    } finally {
      vi.useRealTimers();
      fake.close();
    }
  });

  // An aborted wait_for_feedback (the person stopped the turn) ends its
  // request, and the loop's hold with it, at once: no timer runs before the
  // loop polls again.
  it("tier 5 resumes at once when a wait_for_feedback call is aborted", async () => {
    const empty = { feedback: [], answers: [], text: null, waited_s: 0 };
    const parked: ((body: unknown) => void)[] = [];
    let waitClosed = false;
    const fake = await fakeDaemonHome((req, reply) => {
      if (!req.url?.startsWith("/api/sessions/s1/feedback")) return false;
      const tier = new URL(req.url, "http://x").searchParams.get("tier");
      if (tier === "inject") parked.push(reply);
      else if (tier === "wait") {
        // Never answered: only the abort ends it. A parked inject poll is woken empty.
        req.on("close", () => { waitClosed = true; });
        for (const r of parked.splice(0)) r(empty);
      } else return false;
    });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const pauses = trackTimers(INJECT_RETRY_MS);
      const polls = trackPolls();
      const pi = new FakePi();
      claxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-clax")), now: () => 0 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-abort");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      await expect.poll(() => parked.length, { timeout: 10_000 }).toBe(1);
      const stop = new AbortController();
      const waiting = pi.callTool("clax_wait_for_feedback", { timeout_s: 600 }, ctx, stop.signal);
      expect(await polls[0].settled).toBe("answered");
      await turns();
      expect(polls).toHaveLength(1);
      stop.abort();
      expect((await waiting).isError).toBe(true);
      await expect.poll(() => waitClosed, { timeout: 10_000 }).toBe(true);
      await expect.poll(() => parked.length, { timeout: 10_000 }).toBe(1);
      expect(polls).toHaveLength(2);
      expect(pauses.created.size).toBe(0);
    } finally {
      vi.useRealTimers();
      fake.close();
    }
  });

  it("tier 1 leaves error results alone and the feedback pending", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tier1-error");
    const aid = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi error" }, ctx)).json.artifact_id;
    await browserThread(aid, "@agent after the error");
    const failed = await pi.callToolAsPi("clax_read", { url_or_id: aid, path: "missing.css" }, ctx);
    expect(failed.isError).toBe(true);
    expect(failed.content).toHaveLength(1);
    expect(json(failed)).toMatchObject({ error: { code: "not_found" }, feedback: [] });
    expect(parts(await pi.callToolAsPi("clax_list", {}, ctx)).json.feedback).toHaveLength(1);
  });

  it("tier 1 does not piggyback on wait_for_feedback or on tools it did not register", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tier1-skip");
    const a = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi quiet" }, ctx)).json.artifact_id;
    const b = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi busy" }, ctx)).json.artifact_id;
    await browserThread(b, "@agent on the other page");
    // Waiting on `a` hands over nothing; the comment on `b` is not appended.
    const waiting = pi.callToolAsPi("clax_wait_for_feedback", { url_or_id: a, timeout_s: 30 }, ctx);
    await expireFeedback(await sessionOf("pi-tier1-skip"));
    const waited = await waiting;
    expect(waited.content).toHaveLength(1);
    expect(parts(waited).json).toEqual({ feedback: [], waited_s: 0, call_again: true });
    const foreign = { type: "tool_result", toolName: "clax_foreign", toolCallId: "call-2", input: {}, content: [{ type: "text", text: "{}" }], isError: false, details: undefined };
    for (const h of pi.handlers.get("tool_result") ?? []) expect(await h(foreign, ctx)).toBeUndefined();
    expect(parts(await pi.callToolAsPi("clax_list", {}, ctx)).json.feedback).toHaveLength(1);
  });

  it("wait_for_feedback hands over a late answer and ends the wait", async () => {
    const { pi, ctx } = load(daemon.home, "pi-late-wait");
    parts(await pi.callToolAsPi("clax_list", {}, ctx));
    const sid = await sessionOf("pi-late-wait");
    const qid = await askAndAnswer(sid, "Layout", "A");
    const r = parts(await pi.callToolAsPi("clax_wait_for_feedback", { timeout_s: 5 }, ctx));
    expect(r.json).toMatchObject({ feedback: [], call_again: false });
    expect(r.json.answers.map((a: any) => a.id)).toEqual([qid]);
    expect(r.trailing).toMatch(new RegExp(`^---\\n\\[clax\\] The person answered your question "Layout" \\(${qid}, asked `));
    // Handed over once.
    expect(parts(await pi.callToolAsPi("clax_list", {}, ctx)).json.answers).toBeUndefined();
  });

  it("tier 1 hands over a late answer with no feedback beside it, once", async () => {
    const { pi, ctx } = load(daemon.home, "pi-late-tier1");
    parts(await pi.callToolAsPi("clax_list", {}, ctx));
    const sid = await sessionOf("pi-late-tier1");
    const qid = await askAndAnswer(sid, "Data", "B");
    const r = parts(await pi.callToolAsPi("clax_list", {}, ctx));
    expect(r.json.feedback).toEqual([]);
    expect(r.json.answers.map((a: any) => a.id)).toEqual([qid]);
    expect(r.trailing).toContain(`[clax] The person answered your question "Data" (${qid}`);
    const again = await pi.callToolAsPi("clax_list", {}, ctx);
    expect(again.content).toHaveLength(1);
    expect(parts(again).json.answers).toBeUndefined();
  });

  it("an answer given during wait_for_feedback goes to the wait, not the injection loop", async () => {
    const { pi, ctx } = load(daemon.home, "pi-late-inject");
    await pi.emit("session_start", {}, ctx);
    const sid = await sessionOf("pi-late-inject");
    const qid = (await api(daemon, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify(askBody("Scope")) })).question.id;
    // The longest wait: only the answer can end it within the test's time.
    const waiting = pi.callToolAsPi("clax_wait_for_feedback", { timeout_s: 600 }, ctx);
    expect((await api(daemon, `/api/_test/sessions/${sid}/feedback/waiters?until=1`)).count).toBe(1);
    await answer(qid, "A");
    const got = parts(await waiting);
    expect(got.json.answers.map((a: any) => a.id)).toEqual([qid]);
    expect(got.json.call_again).toBe(false);
    expect(pi.sent).toHaveLength(0);
  }, 20_000);

  it("clax_ask asks to call again while no answer came, with late answers apart from its reply", async () => {
    const { pi, ctx } = load(daemon.home, "pi-ask");
    parts(await pi.callToolAsPi("clax_list", {}, ctx));
    const sid = await sessionOf("pi-ask");
    // An earlier question, answered while nothing waits on it: a late answer.
    const late = await askAndAnswer(sid, "Earlier", "A");
    const asking = pi.callToolAsPi("clax_ask", { questions: askBody("Pick").questions, timeout_s: 30 }, ctx);
    await expireQuestion(await openQuestion("Pick"));
    const first = parts(await asking).json;
    expect(first).toMatchObject({ status: "open", reply: null, call_again: true, surface_open: false, feedback: [] });
    expect(first.answers.map((a: any) => a.id)).toEqual([late]);
    const qid: string = first.question_id;
    expect(first.url).toMatch(new RegExp(`^http://localhost:\\d+/inbox\\?q=${qid}$`));
    const waiting = pi.callToolAsPi("clax_ask", { question_id: qid, timeout_s: 30 }, ctx);
    expect((await api(daemon, `/api/_test/questions/${qid}/waiters?until=1`)).count).toBe(1);
    await answer(qid, "B");
    const got = parts(await waiting).json;
    expect(got).toMatchObject({ question_id: qid, status: "answered", call_again: false, note: expect.stringContaining("own words") });
    expect(got.reply).toEqual([{ question: "Which?", header: "Pick", selected: ["B"], text: null }]);
    expect(got.answers).toBeUndefined();
    expect(got.surface_open).toBeUndefined();
    // Handed over once: no feedback tier repeats it.
    expect(parts(await pi.callToolAsPi("clax_list", {}, ctx)).json.answers).toBeUndefined();
  }, 20_000);

  it("clax_ask returns its own answer in reply and an earlier question's in answers", async () => {
    const { pi, ctx } = load(daemon.home, "pi-ask-two");
    parts(await pi.callToolAsPi("clax_list", {}, ctx));
    const sid = await sessionOf("pi-ask-two");
    // Q1 is open and no call waits on it (as after call_again); Q2 is asked.
    const q1 = (await api(daemon, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify(askBody("One")) })).question.id;
    const waiting = pi.callToolAsPi("clax_ask", { questions: askBody("Two").questions, timeout_s: 30 }, ctx);
    const q2 = await openQuestion("Two");
    expect((await api(daemon, `/api/_test/questions/${q2}/waiters?until=1`)).count).toBe(1);
    await answer(q1, "A");
    await answer(q2, "B");
    const got = parts(await waiting).json;
    expect(got.question_id).toBe(q2);
    expect(got.reply).toEqual([{ question: "Which?", header: "Two", selected: ["B"], text: null }]);
    expect(got.answers.map((a: any) => a.id)).toEqual([q1]);
    expect(parts(await pi.callToolAsPi("clax_list", {}, ctx)).json.answers).toBeUndefined();
  }, 20_000);

  it("clax_ask cancels, refuses another session's question, and checks its arguments", async () => {
    const { pi, ctx } = load(daemon.home, "pi-ask-cancel");
    const other = load(daemon.home, "pi-ask-other");
    parts(await pi.callToolAsPi("clax_list", {}, ctx));
    const sid = await sessionOf("pi-ask-cancel");
    const qid = (await api(daemon, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify(askBody("Scope")) })).question.id;
    const foreign = await other.pi.callTool("clax_ask", { question_id: qid }, other.ctx);
    expect(json(foreign).error.code).toBe("not_found");
    const c = parts(await pi.callToolAsPi("clax_ask", { question_id: qid, cancel: true }, ctx)).json;
    expect(c).toMatchObject({ status: "withdrawn", reply: null, call_again: false });
    // A second ask that was answered before it was cancelled hands the answer over.
    const q2 = await askAndAnswer(sid, "Later", "A");
    const c2 = parts(await pi.callToolAsPi("clax_ask", { question_id: q2, cancel: true }, ctx)).json;
    expect(c2).toMatchObject({ status: "answered", reply: [{ header: "Later", selected: ["A"] }] });
    for (const args of [{}, { question_id: "../x" }, { questions: askBody("H").questions, cancel: true }, { questions: askBody("H").questions, question_id: qid }]) {
      const r = await pi.callTool("clax_ask", args, ctx);
      expect(json(r).error.code, JSON.stringify(args)).toBe("invalid_args");
    }
    const long = await pi.callTool("clax_ask", { questions: askBody("Thirteen char").questions }, ctx);
    expect(json(long).error.code).toBe("invalid_question");
  });

  it("sends addressed with a reply, which the daemon refuses on an artifact", async () => {
    const { pi, ctx } = load(daemon.home, "pi-addressed");
    const aid = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi addressed" }, ctx)).json.artifact_id;
    const sent = await browserThread(aid, "@agent fix it");
    const res = await pi.callTool("clax_comments_reply", { url_or_id: aid, thread_id: sent, text: "Fixed.", addressed: true }, ctx);
    expect(res.isError).toBe(true);
    expect(json(res).error).toMatchObject({ code: "invalid_args", message: "addressed is for live pages; publish with addresses instead" });
    const t = await api(daemon, `/api/artifacts/${aid}/threads/${sent}`);
    expect(t.thread.comments).toHaveLength(1);
  });

  it("refuses a thread ID that is not a canonical ULID", async () => {
    const { pi, ctx } = load(daemon.home, "pi-bad-thread");
    const aid = parts(await pi.callToolAsPi("clax_publish", { html: "<h2>Goals</h2>", title: "Pi ULID" }, ctx)).json.artifact_id;
    const res = await pi.callTool("clax_comments_read", { url_or_id: aid, thread_id: "81K6AB3Q9X7N2M4P5R6S8T0V1W" }, ctx);
    expect(res.isError).toBe(true);
    expect(json(res).error).toEqual({ code: "invalid_args", message: "'81K6AB3Q9X7N2M4P5R6S8T0V1W' is not a thread ID" });
  });

  it("session_shutdown stops the injection loop during its poll without leaving a retry timer", async () => {
    const polls = trackPolls();
    const { pi, ctx } = load(daemon.home, "pi-inject-stop");
    await pi.emit("session_start", {}, ctx);
    await expect.poll(() => polls.length, { timeout: 10_000 }).toBe(1);
    const pauses = trackTimers(INJECT_RETRY_MS);
    await pi.emit("session_shutdown", {}, ctx);
    // The aborted poll settles, and the loop, which awaited it first, has returned.
    expect(await polls[0].settled).toBe("failed");
    expect(polls).toHaveLength(1);
    expect(pauses.alive()).toHaveLength(0);
  });

  it("pauses the injection loop after an answer that came back empty at once, until session_shutdown clears the pause", async () => {
    let polls = 0;
    const fake = await fakeDaemonHome((req, reply) => {
      if (req.url?.startsWith("/api/sessions/s1/feedback")) { polls++; return reply({ feedback: [], text: null, waited_s: 0 }); }
      return false;
    });
    // A pause that outlasts the test, so only shutdown ends it.
    const injectRetryMs = 3_600_000;
    const pauses = trackTimers(injectRetryMs);
    try {
      const pi = new FakePi();
      // A clock that never moves: every empty answer came back at once.
      claxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-clax")), injectRetryMs, now: () => 0 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-empty");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      await expect.poll(() => pauses.created.size, { timeout: 10_000 }).toBe(1);
      expect(polls).toBe(1);
      await pi.emit("session_shutdown", {}, ctx);
      expect(pauses.alive()).toHaveLength(0);
      expect(polls).toBe(1);
    } finally {
      fake.close();
    }
  });

  it("status passes the push object through, including a codex queue failure", async () => {
    const push = { tier: "queue", available: true, last_error: "codex queue exited with code 1", last_error_at: "2026-09-29T10:00:00Z" };
    const fake = await fakeDaemonHome((req, reply) => {
      if (req.url?.startsWith("/api/sessions/s1/feedback")) return reply({ feedback: [], text: null, waited_s: 0 });
      if (req.url === "/api/sessions/s1/watches") return reply({ watches: [] });
      if (req.url === "/api/sessions/s1") return reply({ session: { id: "s1" }, push });
      return false;
    });
    try {
      const pi = new FakePi();
      claxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-clax")) })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-status-push");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      expect(parts(await pi.callTool("clax_status", {}, ctx)).json.push).toEqual(push);
    } finally {
      fake.close();
    }
  });

  it("the injection loop never starts a stopped daemon, and resumes when one is back", async () => {
    const home = join(scratch, "inject-restart");
    const env = withBin(claxBin);
    const stop = () => execFileSync(claxBin, ["stop"], { env: { ...env, CLAX_HOME: home }, stdio: "ignore" });
    // A short pause between polls, so the loop polls the stopped daemon
    // several times soon (the default pause is pinned by the tests that
    // count INJECT_RETRY_MS timers).
    const injectRetryMs = 250;
    const polls = trackPolls();
    try {
      await ensure(home, { env, port: 0 });
      const pi = new FakePi();
      claxExtension({ home, env, port: 0, injectRetryMs })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-restart");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      await expect.poll(() => polls.length, { timeout: 10_000 }).toBe(1);
      stop();
      // The poll in progress fails with the daemon, and the loop polls the stopped daemon again, twice.
      await expect.poll(() => polls.filter(p => p.state === "failed").length, { timeout: 10_000 }).toBeGreaterThanOrEqual(3);
      expect(existsSync(join(home, "daemon.json"))).toBe(false);
      expect(await discover(home)).toBeNull();

      const d = endpointOf(await ensure(home, { env, port: 0 }));
      const live = async () => ((await api(d, "/api/sessions")).sessions as any[]).find(s => s.harness_session_id === "pi-inject-restart" && s.ended_at === null);
      await expect.poll(live, { timeout: 10_000, interval: 200 }).toBeTruthy();
      const sid = (await live()).id;
      const aid = (await api(d, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "Back", files: { "index.html": { content: "<h2>Goals</h2>", encoding: "utf8" } } }) })).artifact.id;
      await api(d, `/api/sessions/${sid}/watches/${aid}`, { method: "PUT", body: JSON.stringify({ replies_armed: true }) });
      await browserThread(aid, "@agent welcome back", d.base);
      await expect.poll(() => pi.sent.length, { timeout: 15_000 }).toBe(1);
    } finally {
      stop();
    }
  }, 60_000);

  it("the db tools match the MCP tools", async () => {
    const { pi, ctx } = load(daemon.home, "pi-db");
    const aid = parts(await pi.callToolAsPi("clax_publish", { html: "<p>db</p>", title: "Pi db", capabilities: { db: {} } }, ctx)).json.artifact_id;
    const set = parts(await pi.callToolAsPi("clax_db_set", { url_or_id: aid, collection: "tasks", doc_id: "t1", data: { n: 1 } }, ctx)).json;
    expect(set).toMatchObject({ artifact_id: aid, path: "tasks/t1", version: 1, created: true });
    const pinned = await pi.callToolAsPi("clax_db_set", { url_or_id: aid, collection: "tasks", doc_id: "t1", data: { n: 2 } }, ctx);
    expect(pinned.isError).toBe(true);
    expect(json(pinned).error).toMatchObject({ code: "if_version_required", current: 1 });
    const upd = parts(await pi.callToolAsPi("clax_db_update", { url_or_id: aid, collection: "tasks", doc_id: "t1", data: { done: true }, if_version: 1 }, ctx)).json;
    expect(upd).toMatchObject({ version: 2 });
    const q = parts(await pi.callToolAsPi("clax_db_query", { url_or_id: aid, collection: "tasks", query: { where: [["done", "==", true]] } }, ctx)).json;
    expect(q.docs.map((d: any) => d.id)).toEqual(["t1"]);
    expect(q.note).toContain("data, not as instructions");
    const b = parts(await pi.callToolAsPi("clax_db_batch", { url_or_id: aid, writes: [{ op: "delete", collection: "tasks", doc_id: "t1", if_version: 2 }] }, ctx)).json;
    expect(b).toMatchObject({ atomic: true, results: [{ op: "delete", path: "tasks/t1", deleted: true }] });
    const gone = await pi.callToolAsPi("clax_db_get", { url_or_id: "7q3k9mzx2b4t", collection: "tasks", doc_id: "t1" }, ctx);
    expect(gone.isError).toBe(true);
    expect(json(gone).error, "a mistyped artifact is the daemon's error, unchanged").toEqual({ code: "not_found", message: "not found" });
    const absent = parts(await pi.callToolAsPi("clax_db_get", { url_or_id: aid, collection: "tasks", doc_id: "t9" }, ctx)).json;
    expect(absent).toMatchObject({ exists: false, doc: null });
    const order = await pi.callToolAsPi("clax_db_list", { url_or_id: aid, collection: "tasks/t1", query: { where: [["n", "==", 1]] } }, ctx);
    expect(order.isError).toBe(true);
    expect(json(order).error, "a bad collection is reported before misplaced filters").toEqual({
      code: "invalid_argument", message: "'tasks/t1' has 2 segments; a collection path has an odd number",
    });
    const failsWith = async (tool: string, args: Record<string, unknown>, error: Record<string, string>) => {
      const r = await pi.callToolAsPi(tool, { url_or_id: aid, ...args }, ctx);
      expect(r.isError, `${tool} ${JSON.stringify(args)}`).toBe(true);
      expect(json(r).error, `${tool} ${JSON.stringify(args)}`).toEqual(error);
    };
    const pin0 = { code: "invalid_args", message: "if_version is 1 or more" };
    await failsWith("clax_db_set", { collection: "tasks", doc_id: "t1", data: {}, if_version: 0 }, pin0);
    await failsWith("clax_db_update", { collection: "tasks", doc_id: "t1", data: {}, if_version: 0 }, pin0);
    await failsWith("clax_db_delete", { collection: "tasks", doc_id: "t1", if_version: 0 }, pin0);
    await failsWith("clax_db_str_replace", { collection: "tasks", doc_id: "t1", field: "f", old_str: "a", new_str: "b", if_version: 0 }, pin0);
    await failsWith("clax_db_batch", { writes: [{ op: "delete", collection: "tasks", doc_id: "t1", if_version: 0 }] }, pin0);
    await failsWith("clax_db_batch", { writes: [] }, { code: "invalid_args", message: "writes holds 1 to 50 entries" });
    for (const limit of [0, 1001]) {
      for (const tool of ["clax_db_list", "clax_db_query"]) {
        await failsWith(tool, { collection: "tasks", query: { limit } }, { code: "invalid_args", message: "query.limit is 1 to 1000" });
      }
    }
    const oneSegment = { code: "invalid_argument", message: "doc_id is one path segment; 'b1/columns/c1' contains /" };
    await failsWith("clax_db_get", { collection: "boards", doc_id: "b1/columns/c1" }, oneSegment);
    await failsWith("clax_db_set", { collection: "boards", doc_id: "b1/columns/c1", data: {} }, oneSegment);
    await failsWith("clax_db_batch", { writes: [{ op: "delete", collection: "boards", doc_id: "b1/columns/c1" }] }, oneSegment);
    for (const [tool, args] of [["clax_db_get", { doc_id: "p" }], ["clax_db_list", {}]] as const) {
      const me = await pi.callToolAsPi(tool, { url_or_id: aid, collection: "data/users/me", ...args }, ctx);
      expect(me.isError, tool).toBe(true);
      expect(json(me).error.code, tool).toBe("invalid_args");
    }
  });

  it("the tool schemas match the daemon's /mcp schemas", async () => {
    const mcp = await mcpTools(daemon);
    const { pi } = load(daemon.home, "pi-schemas");
    const base = (t: unknown) => (Array.isArray(t) ? t.filter(x => x !== "null") : [t]).map(x => (x === "number" ? "integer" : x)).sort().join("|");
    for (const name of TOOLS) {
      const ours = (pi.tools.get(name)!.parameters as any);
      const theirs = mcp.get(name.replace(/^clax_/, ""))!;
      // An MCP schema node with `$ref`s resolved and `Option`'s null branch dropped.
      const resolve = (n: any): any => {
        if (n?.$ref !== undefined) return resolve(theirs.$defs?.[n.$ref.replace(/^#\/\$defs\//, "")]);
        const branches = (n?.anyOf ?? n?.oneOf)?.filter((b: any) => b.type !== "null");
        return branches?.length === 1 ? resolve({ ...n, anyOf: undefined, oneOf: undefined, ...branches[0] }) : n;
      };
      // The allowed values of an enum node, as `enum` or as `const` branches
      // (schemars writes documented variants that way); null is dropped.
      const enumOf = (n: any): unknown[] | undefined => {
        const branches = n.oneOf ?? n.anyOf;
        const values = n.enum ?? (branches?.every((b: any) => b.const !== undefined || b.type === "null") ? branches.map((b: any) => b.const) : undefined);
        return values?.filter((v: unknown) => v !== null && v !== undefined).sort();
      };
      // Property names, required names, base types, enums, bounds, and closed objects, down
      // through nested objects and array items.
      const compare = (o: any, t: any, at: string) => {
        t = resolve(t);
        if (!o || !t || typeof o !== "object" || typeof t !== "object") return;
        if (o.type !== undefined && t.type !== undefined) expect(base(o.type), at).toBe(base(t.type));
        for (const k of ["minimum", "maximum", "minItems", "maxItems"]) {
          if (o[k] !== undefined || t[k] !== undefined) expect(o[k], `${at}.${k}`).toBe(t[k]);
        }
        expect(enumOf(o), `${at}.enum`).toEqual(enumOf(t));
        if (base(o.type) === "object" || base(t.type) === "object") {
          expect(o.additionalProperties === false, `${at}.additionalProperties`).toBe(t.additionalProperties === false);
        }
        if (o.properties !== undefined || t.properties !== undefined) {
          expect(Object.keys(o.properties ?? {}).sort(), at).toEqual(Object.keys(t.properties ?? {}).sort());
          expect([...(o.required ?? [])].sort(), at).toEqual([...(t.required ?? [])].sort());
          for (const k of Object.keys(t.properties ?? {})) compare(o.properties[k], t.properties[k], `${at}.${k}`);
        }
        if (o.items !== undefined && t.items !== undefined) compare(o.items, t.items, `${at}[]`);
      };
      compare(ours, theirs, name);
    }
  });

  it("clax_working marks the artifact and done clears it, as the MCP tool does", async () => {
    const { pi, ctx } = load(daemon.home, "pi-working");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const pub = JSON.parse((await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "W" }, ctx)).content[0].text!);
    const set = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, message: "Two columns" }, ctx);
    expect(set.isError).toBe(false);
    expect(JSON.parse(set.content[0].text!)).toMatchObject({ working: true, message: "Two columns", expires_in_s: 120, message_truncated: false });
    const seen = await (await fetch(`${daemon.base}/api/artifacts/${pub.artifact_id}/working`)).json();
    expect(seen.working[0]).toMatchObject({ harness: "pi", message: "Two columns" });
    expect(JSON.stringify(seen)).not.toContain("session_id");
    const done = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, done: true }, ctx);
    expect(JSON.parse(done.content[0].text!)).toMatchObject({ working: false, cleared: true });
    const bad = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, thread_ids: ["x"] }, ctx);
    expect(bad.isError).toBe(true);
    expect(JSON.parse(bad.content[0].text!).error.code).toBe("invalid_args");
  });

  it("tool calls renew working records at most every 15 s, and agent_end ends them", async () => {
    const { pi, ctx } = load(daemon.home, "pi-renew");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const pub = JSON.parse((await pi.callTool("clax_publish", { html: "<p>r</p>", title: "R" }, ctx)).content[0].text!);
    await pi.callTool("clax_working", { url_or_id: pub.artifact_id, message: "m" }, ctx);
    const renews: number[] = [];
    const spy = vi.spyOn(DaemonClient.prototype, "renewWorking").mockImplementation(async function (this: DaemonClient) { renews.push(Date.now()); return { renewed: 1 }; });
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      await pi.emit("tool_call", { type: "tool_call", toolName: "bash", toolCallId: "1", input: {} }, ctx);
      await pi.emit("tool_call", { type: "tool_call", toolName: "edit", toolCallId: "2", input: {} }, ctx);
      expect(renews.length).toBe(1);
      vi.setSystemTime(Date.now() + RENEW_EVERY_MS);
      await pi.emit("tool_call", { type: "tool_call", toolName: "read", toolCallId: "3", input: {} }, ctx);
      expect(renews.length).toBe(2);
    } finally {
      vi.useRealTimers();
      spy.mockRestore();
    }
    await pi.emit("agent_end", { type: "agent_end", messages: [] }, ctx);
    const seen = await (await fetch(`${daemon.base}/api/artifacts/${pub.artifact_id}/working`)).json();
    expect(seen.working).toEqual([]);
  });

  it("clax_publish carries a note and addresses, as the MCP tool does", async () => {
    const { pi, ctx } = load(daemon.home, "pi-note");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const v1 = JSON.parse((await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "N" }, ctx)).content[0].text!);
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "plain");
    form.set("version", "1");
    const tid = (await (await fetch(`${daemon.base}/api/artifacts/${v1.artifact_id}/threads`, { method: "POST", body: form })).json()).thread.id;
    const v2 = JSON.parse((await pi.callTool("clax_publish", { id: v1.artifact_id, html: "<h2>Goals</h2>", note: "Done", addresses: [tid] }, ctx)).content[0].text!);
    expect(v2).toMatchObject({ version: 2, note: "Done", note_truncated: false, addressed: [tid] });
  });

  it("tier 5: a batch reaches Pi as one follow-up message led by its note", async () => {
    const { pi, ctx } = load(daemon.home, "pi-batch");
    await pi.emit("session_start", {}, ctx);
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Pi batch" }, ctx)).json.artifact_id;
    const ids = [await browserThread(aid, "one"), await browserThread(aid, "two")];
    const res = await fetch(`${daemon.base}/api/artifacts/${aid}/threads:send`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ thread_ids: ids, note: "Together" }) });
    expect(res.status).toBe(200);
    await expect.poll(() => pi.sent.length, { timeout: 15_000 }).toBe(1);
    expect(String(pi.sent[0].content)).toMatch(/^\[clax\] 2 comments sent to you:\n\[clax\] 2 comments on "Pi batch", sent together by Viewer\. Note: "Together"\n/);
    await pi.emit("session_shutdown", {}, ctx);
  });
});

/** One long-poll of an injection loop (`DaemonClient.pollFeedback`): the
 * signal that aborts it, its outcome so far, and a promise of that outcome
 * which settles after the loop awaiting the poll has run on. */
interface Poll {
  signal: AbortSignal;
  state: "pending" | "answered" | "failed";
  settled: Promise<"answered" | "failed">;
}

/** Records every injection loop long-poll from here on, until the test ends. */
function trackPolls(): Poll[] {
  const polls: Poll[] = [];
  const real = DaemonClient.prototype.pollFeedback;
  vi.spyOn(DaemonClient.prototype, "pollFeedback").mockImplementation(function (this: DaemonClient, tier: string, waitS: number, signal: AbortSignal) {
    const res = real.call(this, tier, waitS, signal);
    const poll: Poll = { signal, state: "pending", settled: undefined as never };
    // The loop awaits `res` itself, so its reaction runs before any test
    // reaction to `settled`, which is chained after this one.
    poll.settled = res.then(() => (poll.state = "answered"), () => (poll.state = "failed"));
    polls.push(poll);
    return res;
  });
  return polls;
}

/** Lets pending promise reactions and I/O callbacks run (`setImmediate`,
 * which the fake timers here leave real). */
async function turns(n = 5): Promise<void> {
  for (let i = 0; i < n; i++) await new Promise<void>(r => setImmediate(r));
}

/** Records the timers of `ms` milliseconds created from here on, and which of
 * them were cleared, until the test ends. */
function trackTimers(ms: number) {
  const created = new Set<unknown>();
  const cleared = new Set<unknown>();
  const realSet = globalThis.setTimeout;
  const realClear = globalThis.clearTimeout;
  vi.spyOn(globalThis, "setTimeout").mockImplementation(((fn: () => void, wait?: number, ...rest: unknown[]) => {
    const t = realSet(fn, wait, ...rest);
    if (wait === ms) created.add(t);
    return t;
  }) as typeof setTimeout);
  vi.spyOn(globalThis, "clearTimeout").mockImplementation(((t: unknown) => { cleared.add(t); realClear(t as NodeJS.Timeout); }) as typeof clearTimeout);
  return { created, alive: () => [...created].filter(t => !cleared.has(t)) };
}

/** An Clax home whose daemon answers `/healthz`, registers every session as
 * `s1` and ends it, and passes other requests to `handle`, which answers through `reply` or
 * returns false to leave the request unanswered. */
async function fakeDaemonHome(handle: (req: import("node:http").IncomingMessage, reply: (body: unknown) => void) => unknown) {
  const home = join(scratch, `fake-${Math.random().toString(36).slice(2)}`);
  mkdirSync(home, { recursive: true });
  const server = createHttpServer((req, res) => {
    const reply = (body: unknown) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(body)); };
    if (req.url === "/healthz") return reply({ version: "0.1.0" });
    if (req.method === "POST" && req.url === "/api/sessions") {
      return reply({ session: { id: "s1", harness: "pi", harness_session_id: "h", cwd: "/", pid: 1, parent_pid: 1, started_at: "", last_seen_at: "", ended_at: null } });
    }
    if (req.method === "PATCH" && req.url === "/api/sessions/s1") {
      return reply({ session: { id: "s1", harness: "pi", harness_session_id: "h", cwd: "/", pid: 1, parent_pid: 1, started_at: "", last_seen_at: "", ended_at: "2026-01-01T00:00:00Z" } });
    }
    handle(req, reply);
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as { port: number }).port;
  writeFileSync(join(home, "daemon.json"), JSON.stringify({
    port, pid: process.pid, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.1.0",
  }));
  return { home, close: () => { server.closeAllConnections(); server.close(); } };
}

/** The daemon's /mcp `tools/list`, as input schemas by tool name. */
async function mcpTools(d: { base: string; token: string }): Promise<Map<string, any>> {
  const headers: Record<string, string> = { authorization: `Bearer ${d.token}`, "content-type": "application/json", accept: "application/json, text/event-stream" };
  const rpc = async (body: unknown) => {
    const res = await fetch(`${d.base}/mcp`, { method: "POST", headers, body: JSON.stringify(body) });
    const sid = res.headers.get("mcp-session-id");
    if (sid) headers["mcp-session-id"] = sid;
    const text = await res.text();
    const data = text.split("\n").filter(l => l.startsWith("data: ")).map(l => l.slice(6)).find(l => l.includes("\"id\""));
    return data ? JSON.parse(data) : text ? JSON.parse(text) : {};
  };
  await rpc({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "pi-test", version: "0" } } });
  await rpc({ jsonrpc: "2.0", method: "notifications/initialized" });
  const list = await rpc({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  return new Map(list.result.tools.map((t: any) => [t.name, t.inputSchema]));
}

describe("helpers match the shared contract fixture", () => {
  it("artifactRef", () => {
    for (const c of FIXTURE.artifact_ref) {
      if (c.error) expect(() => artifactRef(c.input), c.input).toThrow(new RegExp(c.error));
      else expect(artifactRef(c.input), c.input).toEqual(c.version === null ? { id: c.id } : { id: c.id, version: c.version });
    }
  });

  it("target", () => {
    const { daemon_base: section, cases } = FIXTURE.target;
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      const base = c.daemon_base ?? section;
      if (c.error) {
        let code: unknown;
        try {
          target(c.input, base);
        } catch (e) {
          code = e instanceof ToolError ? e.error.code : e;
        }
        expect(code, c.input).toBe(c.error);
      } else if (c.page) {
        expect(target(c.input, base), c.input).toEqual({ kind: "page", url: c.page });
      } else {
        expect(target(c.input, base), c.input).toEqual(c.version === null ? { kind: "artifact", id: c.id } : { kind: "artifact", id: c.id, version: c.version });
      }
    }
  });

  it("textPrefix", () => {
    for (const c of FIXTURE.text_prefix) expect(textPrefix(Buffer.from(c.hex, "hex"), c.max), `${c.hex} ${c.max}`).toBe(c.text);
  });

  it("isText", () => {
    for (const c of FIXTURE.is_text) expect(isText(c.content_type), c.content_type).toBe(c.text);
  });

  it("htmlTitle", () => {
    for (const c of FIXTURE.html_title) expect(htmlTitle(c.html) ?? null, c.html).toBe(c.title);
  });
});
