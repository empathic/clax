import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer } from "node:net";
import { validateToolArguments, type Tool } from "@mariozechner/pi-ai";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionContext } from "@mariozechner/pi-coding-agent";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { artifactRef, artifaxExtension, htmlTitle, INJECT_RETRY_MS, isText, textPrefix } from "../src/artifax.ts";
import { discover, endpointOf, ensure } from "../src/daemon.ts";
import { api, artifaxBin, startDaemon, type TestDaemon } from "./daemon-fixture.ts";
import { FakePi, fakeContext, json } from "./fake-api.ts";

/** Inputs and expected outputs shared with the Rust tools. */
const FIXTURE = JSON.parse(readFileSync(new URL("./fixtures/contract.json", import.meta.url), "utf8"));
const TOOLS: string[] = FIXTURE.tools.map((t: { name: string }) => `artifax_${t.name}`);

let daemon: TestDaemon;
let scratch: string;

beforeAll(async () => {
  daemon = await startDaemon();
  scratch = mkdtempSync(join(tmpdir(), "artifax-pi-scratch-"));
}, 320_000);

afterAll(async () => {
  await daemon?.stop();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

/** A fresh extension instance for a Pi session `sessionId` in `cwd`, talking
 * to the daemon in `home`. */
function load(home: string, sessionId: string, cwd = scratch) {
  const pi = new FakePi();
  artifaxExtension({ home, env: withBin(artifaxBin) })(pi.api);
  const built = { pi, ...fakeContext(cwd, sessionId) };
  loaded.push({ pi, ctx: built.ctx });
  return built;
}

/** Every extension `load` built in the current test; each is shut down after
 * the test, so no feedback long-poll outlives it. */
const loaded: { pi: FakePi; ctx: ExtensionContext }[] = [];
afterEach(async () => {
  for (const l of loaded.splice(0)) await l.pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, l.ctx);
});

/** This process's environment with `ARTIFAX_BIN` set to `bin` and Codex push
 * off (`ARTIFAX_CODEX_BIN` empty) for any daemon it starts. */
function withBin(bin: string): NodeJS.ProcessEnv {
  return { ...process.env, ARTIFAX_BIN: bin, ARTIFAX_CODEX_BIN: "" };
}

/** An Artifax home whose daemon answers `/healthz` and then never answers
 * `POST /api/sessions` (`hang: "register"`) or any `PATCH` (`hang: "patch"`). */
async function hungHome(hang: "register" | "patch") {
  const home = join(scratch, `hung-${Math.random().toString(36).slice(2)}`);
  mkdirSync(home, { recursive: true });
  const server = createHttpServer((req, res) => {
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
  return { home, close: () => { server.closeAllConnections(); server.close(); } };
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

/** An Artifax home whose daemon.json names a daemon that is not running. */
async function deadHome(): Promise<string> {
  const home = join(scratch, `dead-${Math.random().toString(36).slice(2)}`);
  mkdirSync(home, { recursive: true });
  writeFileSync(join(home, "daemon.json"), JSON.stringify({
    port: await closedPort(), pid: 999_999, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.1.0",
  }));
  return home;
}

describe("artifax Pi extension", () => {
  it("registers the fourteen tools with one-line prompt snippets, and the artifax command", () => {
    const { pi } = load(daemon.home, "s-tools");
    expect([...pi.tools.keys()].sort()).toEqual([...TOOLS].sort());
    for (const t of pi.tools.values()) {
      expect(t.promptSnippet, t.name).toMatch(/^[^\n]+$/);
      expect(t.description.length, t.name).toBeGreaterThan(0);
    }
    expect([...pi.commands.keys()]).toEqual(["artifax"]);
    for (const t of FIXTURE.tools) expect(pi.tools.get(`artifax_${t.name}`)?.description, t.name).toBe(t.description);
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
    const pub = await pi.callTool("artifax_publish", { html, title: "Round trip" }, ctx);
    expect(pub.isError).toBe(false);
    const p = json(pub);
    expect(p).toMatchObject({ version: 1, title: "Round trip", files: ["index.html"], feedback: [] });
    expect(p.url).toMatch(new RegExp(`^http://localhost:\\d+/a/${p.artifact_id}$`));

    const read = json(await pi.callTool("artifax_read", { url_or_id: p.url }, ctx));
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
    const p = json(await pi.callTool("artifax_publish", { file_path: "page.html" }, ctx));
    const read = json(await pi.callTool("artifax_read", { url_or_id: p.artifact_id }, ctx));
    expect(read.content).toBe("<!doctype html><title>From file</title>");
  });

  it("reports a stale if_version as a conflict naming the current version", async () => {
    const { pi, ctx } = load(daemon.home, "pi-conflict");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("artifax_publish", { html: "<title>v1</title>" }, ctx));
    json(await pi.callTool("artifax_publish", { id: p.artifact_id, if_version: 1, html: "<title>v2</title>" }, ctx));

    const stale = await pi.callTool("artifax_publish", { id: p.artifact_id, if_version: 1, html: "<title>v3</title>" }, ctx);
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
    const mine = json(await pi.callTool("artifax_publish", { html: "<title>mine</title>", title: "mine" }, ctx));
    const other = await api(daemon, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "other", files: { "index.html": { content: "<title>other</title>", encoding: "utf8" } } }) });

    const all = json(await pi.callTool("artifax_list", {}, ctx)).artifacts.map((a: any) => a.id);
    expect(all).toContain(mine.artifact_id);
    expect(all).toContain(other.artifact.id);

    const listed = json(await pi.callTool("artifax_list", { scope: "mine" }, ctx));
    expect(listed.feedback).toEqual([]);
    expect(listed.artifacts).toHaveLength(1);
    expect(listed.artifacts[0]).toMatchObject({ id: mine.artifact_id, url: mine.url, title: "mine", version: 1, pinned: false });
    expect(listed.artifacts[0].owner_session_id).toBeTypeOf("string");
  });

  it("reports daemon_unreachable naming the daemon log when no daemon answers", async () => {
    const home = await deadHome();
    const pi = new FakePi();
    artifaxExtension({ home, env: withBin(join(scratch, "no-such-artifax")) })(pi.api);
    const { ctx } = fakeContext(scratch, "pi-unreachable");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const res = await pi.callTool("artifax_status", {}, ctx);
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
    artifaxExtension({ home, env: withBin(join(scratch, "no-such-artifax")) })(pi.api);
    const { ctx } = fakeContext(scratch, "pi-late");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    expect((await sessions()).some(s => s.harness_session_id === "pi-late")).toBe(false);

    copyFileSync(join(daemon.home, "daemon.json"), join(home, "daemon.json"));
    const status = json(await pi.callTool("artifax_status", {}, ctx));
    expect(status).toMatchObject({ harness: "pi", session: { harness: "pi", harness_session_id: "pi-late" }, watches: [], feedback: [] });
    expect((await sessions()).some(s => s.harness_session_id === "pi-late")).toBe(true);
  });

  it("uploads assets as multipart and returns their URLs", async () => {
    const { pi, ctx } = load(daemon.home, "pi-assets");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("artifax_publish", { html: "<title>assets</title>" }, ctx));
    const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
    writeFileSync(join(scratch, "dot.png"), png);
    const res = json(await pi.callTool("artifax_asset_upload", { url_or_id: p.artifact_id, file_path: "dot.png" }, ctx));
    expect(res.assets).toHaveLength(1);
    expect(res.assets[0]).toMatchObject({ content_type: "image/png", size: png.length });
    const got = await fetch(res.assets[0].url.replace("localhost", "127.0.0.1"));
    expect(Buffer.from(await got.arrayBuffer())).toEqual(png);
  });

  it("leaves the asset content type to the daemon, which infers it from the file name", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tiff");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const p = json(await pi.callTool("artifax_publish", { html: "<title>tiff</title>" }, ctx));
    // The part carries no Content-Type; the daemon infers image/tiff from the name.
    writeFileSync(join(scratch, "scan.tiff"), Buffer.from("II*\0\x08\0\0\0"));
    const res = json(await pi.callTool("artifax_asset_upload", { url_or_id: p.artifact_id, file_path: "scan.tiff" }, ctx));
    expect(res.assets[0].content_type).toBe("image/tiff");
  });

  it("takes a new artifact's title from the page's <title> when none is given", async () => {
    const { pi, ctx } = load(daemon.home, "pi-title");
    const p = json(await pi.callTool("artifax_publish", { html: "<title>  From &amp; the\n page </title><p>x</p>" }, ctx));
    expect(p).toMatchObject({ version: 1, title: "From & the page" });
    const v2 = json(await pi.callTool("artifax_publish", { id: p.artifact_id, html: "<p>no title</p>" }, ctx));
    expect(v2).toMatchObject({ version: 2, title: "From & the page" });
  });

  it("refuses a new artifact with neither a title nor a page <title>", async () => {
    const { pi, ctx } = load(daemon.home, "pi-untitled");
    const res = await pi.callTool("artifax_publish", { html: "<p>no title</p>" }, ctx);
    expect(res.isError).toBe(true);
    expect(json(res).error).toEqual({
      code: "invalid_args",
      message: "a new artifact needs a title: pass `title`, or give the page a non-empty <title>",
    });
  });

  it("registers a new session when its session was ended under it, and publishes", async () => {
    const { pi, ctx } = load(daemon.home, "pi-ended");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const first = json(await pi.callTool("artifax_status", {}, ctx)).session;
    await api(daemon, `/api/sessions/${first.id}`, { method: "PATCH", body: JSON.stringify({ ended: true }) });

    const pub = await pi.callTool("artifax_publish", { html: "<title>after end</title>" }, ctx);
    expect(pub.isError, JSON.stringify(json(pub))).toBe(false);
    const live = (await sessions()).filter(s => s.harness_session_id === "pi-ended" && s.ended_at === null);
    expect(live).toHaveLength(1);
    expect(live[0].id).not.toBe(first.id);
    const got = await api(daemon, `/api/artifacts/${json(pub).artifact_id}`);
    expect(got.artifact.owner_session_id).toBe(live[0].id);
    expect(json(await pi.callTool("artifax_status", {}, ctx)).session.id).toBe(live[0].id);
  });

  it("reports opened from the opener's exit status", async () => {
    const { pi, ctx } = load(daemon.home, "pi-open");
    const p = json(await pi.callTool("artifax_publish", { html: "<title>open me</title>" }, ctx));
    const openWith = async (script: string) => {
      const bin = mkdtempSync(join(scratch, "opener-"));
      for (const name of ["open", "xdg-open"]) {
        writeFileSync(join(bin, name), `#!/bin/sh\n${script}\n`);
        chmodSync(join(bin, name), 0o755);
      }
      const env: NodeJS.ProcessEnv = { ...process.env, ARTIFAX_BIN: artifaxBin, PATH: `${bin}:${process.env.PATH}` };
      delete env.ARTIFAX_NO_OPEN;
      const fresh = new FakePi();
      artifaxExtension({ home: daemon.home, env })(fresh.api);
      const t0 = Date.now();
      const r = json(await fresh.callTool("artifax_open", { url_or_id: p.artifact_id }, ctx));
      return { ...r, ms: Date.now() - t0 };
    };
    expect(await openWith("exit 1")).toMatchObject({ url: p.url, opened: false });
    expect(await openWith("exit 0")).toMatchObject({ url: p.url, opened: true });
    const lingering = await openWith("sleep 5");
    expect(lingering).toMatchObject({ opened: true });
    expect(lingering.ms).toBeGreaterThanOrEqual(1_400);
    expect(lingering.ms).toBeLessThan(3_000);
  });

  it("status reports daemon_version only when the daemon's version differs", async () => {
    const { pi, ctx } = load(daemon.home, "pi-version");
    const s = json(await pi.callTool("artifax_status", {}, ctx));
    expect(s.version).toBe(JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version);
    expect(s).not.toHaveProperty("daemon_version");
  });

  it("refuses a relative path when the session has no working directory", async () => {
    const { pi, ctx } = load(daemon.home, "pi-no-cwd", "");
    const res = await pi.callTool("artifax_publish", { file_path: "page.html", title: "t" }, ctx);
    expect(json(res).error).toEqual({
      code: "invalid_args",
      message: "file paths must be absolute: there is no session working directory to resolve 'page.html' against",
    });
  });

  it("reports an unexpected exception as an internal error result", async () => {
    const { pi, ctx } = load(daemon.home, "pi-internal");
    const res = await pi.callTool("artifax_list", null, ctx);
    expect(res.isError).toBe(true);
    const body = json(res);
    expect(body.feedback).toEqual([]);
    expect(body.error.code).toBe("internal");
    expect(body.error.message).toEqual(expect.any(String));
  });

  it("gives up registering at session_start after about 3 s when the daemon hangs", async () => {
    const hung = await hungHome("register");
    try {
      const pi = new FakePi();
      artifaxExtension({ home: hung.home, env: withBin(join(scratch, "no-such-artifax")) })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-hung-start");
      const t0 = Date.now();
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      expect(Date.now() - t0).toBeLessThan(4_500);
    } finally {
      hung.close();
    }
  });

  it("gives up ending the session after about 3 s when the daemon hangs", async () => {
    const hung = await hungHome("patch");
    try {
      const pi = new FakePi();
      artifaxExtension({ home: hung.home, env: withBin(join(scratch, "no-such-artifax")) })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-hung-end");
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      const t0 = Date.now();
      await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
      expect(Date.now() - t0).toBeLessThan(4_500);
    } finally {
      hung.close();
    }
  });

  it("Pi's argument validation accepts valid arguments and rejects unknown keys", () => {
    const { pi } = load(daemon.home, "pi-validate");
    const id = "7q3k9mzx2b4t";
    const valid: Record<string, Record<string, unknown>[]> = {
      artifax_publish: [
        { html: "<title>x</title>" },
        { file_path: "page.html", id, if_version: 2, title: "t", description: "d", icon: "chart", label: "l", capabilities: { db: {} },
          files: { "a.css": { content: "body{}" }, "b.png": { content: "iVBORw0K", encoding: "base64", content_type: "image/png" }, "c.js": { path: "c.js" }, "old.txt": null } },
        { html: "<p>", url: `http://localhost:7480/a/${id}` },
      ],
      artifax_read: [{ url_or_id: id }, { url_or_id: id, path: "a.css", version: 1, max_bytes: 10 }],
      artifax_list: [{}, { limit: 5, scope: "mine" }, { scope: "all" }],
      artifax_delete: [{ url_or_id: id }],
      artifax_open: [{ url_or_id: id }],
      artifax_pin: [{ url_or_id: id }],
      artifax_unpin: [{ url_or_id: id }],
      artifax_asset_upload: [{ url_or_id: id, file_path: "a.png" }, { url_or_id: id, file_paths: ["a.png", "b.mp4"] }],
      artifax_status: [{}],
      artifax_comments_read: [{ url_or_id: id }, { url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W", cursor: "01K6AB3Q9X7N2M4P5R6S8T0V1W", include_resolved: true }],
      artifax_comments_reply: [{ url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W", text: "done" }],
      artifax_comments_resolve: [{ url_or_id: id, thread_id: "01K6AB3Q9X7N2M4P5R6S8T0V1W" }],
      artifax_watch: [{ url_or_id: id }, { url_or_id: id, on: false, replies: false }],
      artifax_wait_for_feedback: [{}, { url_or_id: id, timeout_s: 50 }],
    };
    const invalid: Record<string, Record<string, unknown>[]> = {
      artifax_publish: [{ html: "x", bogus: 1 }, { html: "x", files: { "a.css": { content: "x", nope: 1 } } }, { html: "x", files: { "a.css": { content: "x", encoding: "hex" } } }],
      artifax_read: [{ url_or_id: id, bogus: 1 }, {}],
      artifax_list: [{ scope: "theirs" }, { bogus: 1 }],
      artifax_delete: [{ url_or_id: id, bogus: 1 }],
      artifax_open: [{ url_or_id: id, bogus: 1 }],
      artifax_pin: [{ url_or_id: id, bogus: 1 }],
      artifax_unpin: [{ url_or_id: id, bogus: 1 }],
      artifax_asset_upload: [{ url_or_id: id, file_path: "a.png", bogus: 1 }],
      artifax_status: [{ bogus: 1 }],
      artifax_comments_read: [{}, { url_or_id: id, bogus: 1 }],
      artifax_comments_reply: [{ url_or_id: id, thread_id: "x" }, { url_or_id: id, thread_id: "x", text: "t", bogus: 1 }],
      artifax_comments_resolve: [{ url_or_id: id }],
      artifax_watch: [{ url_or_id: id, on: "yes" }],
      artifax_wait_for_feedback: [{ timeout_s: -1 }, { bogus: 1 }],
    };
    expect(Object.keys(valid).sort()).toEqual([...TOOLS].sort());
    const check = (name: string, args: Record<string, unknown>) =>
      validateToolArguments(pi.tools.get(name) as unknown as Tool, { type: "toolCall", id: "1", name, arguments: structuredClone(args) });
    for (const [name, cases] of Object.entries(valid)) for (const args of cases) expect(() => check(name, args), `${name} ${JSON.stringify(args)}`).not.toThrow();
    for (const [name, cases] of Object.entries(invalid)) for (const args of cases) expect(() => check(name, args), `${name} ${JSON.stringify(args)}`).toThrow();
  });

  it("the artifax command reports status and lists artifacts", async () => {
    const { pi, ctx, notes } = load(daemon.home, "pi-command");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    json(await pi.callTool("artifax_publish", { html: "<title>cmd</title>", title: "Command page" }, ctx));
    await pi.runCommand("artifax", "status", ctx);
    expect(notes.at(-1)?.message).toMatch(/^artifax daemon at http:\/\/localhost:\d+ \(v[^)]+\), session /);
    await pi.runCommand("artifax", "list", ctx);
    expect(notes.at(-1)?.message).toContain("Command page");
    await pi.runCommand("artifax", "bogus", ctx);
    expect(notes.at(-1)).toMatchObject({ type: "error" });
    expect(notes.at(-1)?.message).toContain("usage: /artifax open [ID] | list | status");
  });
});

/** Creates a thread on version 1 as a browser does; `@agent` in `body` sends it. */
async function browserThread(aid: string, body: string, base = daemon.base): Promise<string> {
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > h2", quote: "Goals" }));
  form.set("body", body);
  form.set("version", "1");
  const res = await fetch(`${base}/api/artifacts/${aid}/threads`, { method: "POST", body: form });
  expect(res.status).toBe(201);
  return (await res.json()).thread.id;
}

/** The JSON block and the trailing block of a tool result. */
function parts(o: { content: { type: string; text?: string }[]; isError: boolean }) {
  expect(o.isError).toBe(false);
  return { json: JSON.parse(o.content[0].text!), trailing: o.content[1]?.text };
}

describe("comments", () => {
  it("tier 1: artifax tool results carry pending feedback once", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tier1");
    const p = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi loop" }, ctx)).json;
    await browserThread(p.artifact_id, "@agent make it two columns");
    const r = parts(await pi.callToolAsPi("artifax_list", {}, ctx));
    expect(r.json.feedback).toHaveLength(1);
    expect(r.trailing).toMatch(/^---\n\[artifax\] 1 comment sent to you:\n\[artifax\] Comment sent to you on "Pi loop"/);
    const again = await pi.callToolAsPi("artifax_list", {}, ctx);
    expect(again.content).toHaveLength(1);
    expect(parts(again).json.feedback).toEqual([]);
  });

  it("read, reply, resolve, and watch match the MCP tools", async () => {
    const { pi, ctx } = load(daemon.home, "pi-comments");
    const aid = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi threads" }, ctx)).json.artifact_id;
    const plain = await browserThread(aid, "plain note");
    const sent = await browserThread(aid, "@agent fix it");
    const read = parts(await pi.callToolAsPi("artifax_comments_read", { url_or_id: aid }, ctx)).json;
    expect(read.threads.map((t: any) => t.thread_id)).toEqual([plain, sent]);
    expect(read.note).toContain("people viewing the page");
    expect(read.feedback).toEqual([]);
    expect(parts(await pi.callToolAsPi("artifax_comments_reply", { url_or_id: aid, thread_id: plain, text: "ok" }, ctx)).json).toMatchObject({ replied: false });
    expect(parts(await pi.callToolAsPi("artifax_comments_reply", { url_or_id: aid, thread_id: sent, text: "Fixed." }, ctx)).json).toMatchObject({ replied: true });
    expect(parts(await pi.callToolAsPi("artifax_comments_resolve", { url_or_id: aid, thread_id: sent }, ctx)).json).toMatchObject({ resolved: true, status: "resolved" });
    const t = await api(daemon, `/api/artifacts/${aid}/threads/${sent}`);
    expect(t.thread.comments[1]).toMatchObject({ author_kind: "agent", author_name: "pi" });
    expect(parts(await pi.callToolAsPi("artifax_watch", { url_or_id: aid, replies: false }, ctx)).json).toMatchObject({ watching: true, replies_armed: false });
    const status = parts(await pi.callToolAsPi("artifax_status", {}, ctx)).json;
    expect(status.watches[0]).toMatchObject({ artifact_id: aid, replies_armed: false });
    expect(status.push).toMatchObject({ tier: "inject", available: true });
  });

  it("wait_for_feedback returns within a second of a send and asks to call again", async () => {
    const { pi, ctx } = load(daemon.home, "pi-wait");
    const aid = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi wait" }, ctx)).json.artifact_id;
    const waiting = pi.callToolAsPi("artifax_wait_for_feedback", { url_or_id: aid, timeout_s: 5 }, ctx);
    // The timestamp is taken before the send starts, so it exists whichever finishes first.
    const sending = (async () => {
      await new Promise(r => setTimeout(r, 300));
      const at = Date.now();
      await browserThread(aid, "@agent live");
      return at;
    })();
    const r = parts(await waiting);
    const answered = Date.now();
    const sentAt = await sending;
    expect(answered - sentAt).toBeLessThan(1000);
    expect(r.json).toMatchObject({ call_again: false });
    expect(r.json.feedback).toHaveLength(1);
    expect(parts(await pi.callToolAsPi("artifax_wait_for_feedback", { timeout_s: 1 }, ctx)).json).toEqual({ feedback: [], waited_s: 1, call_again: true });
  }, 20_000);

  it("tier 5: the extension long-polls and hands comments to Pi as a follow-up", async () => {
    const { pi, ctx } = load(daemon.home, "pi-inject");
    await pi.emit("session_start", {}, ctx);
    const aid = parts(await pi.callTool("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi inject" }, ctx)).json.artifact_id;
    await browserThread(aid, "@agent please shorten it");
    await expect.poll(() => pi.sent.length, { timeout: 5000 }).toBe(1);
    expect(pi.sent[0].options).toEqual({ deliverAs: "followUp" });
    expect(String(pi.sent[0].content)).toMatch(/^\[artifax\] 1 comment sent to you:\n\[artifax\] Comment sent to you on "Pi inject"/);
    const started = Date.now();
    await pi.emit("session_shutdown", {}, ctx);
    expect(Date.now() - started).toBeLessThan(3500);
    await browserThread(aid, "@agent one more");
    await new Promise(r => setTimeout(r, 500));
    expect(pi.sent).toHaveLength(1);
  }, 20_000);

  it("tier 5 yields to wait_for_feedback: the comment goes to the wait and the loop pauses", async () => {
    const { pi, ctx } = load(daemon.home, "pi-inject-wait");
    await pi.emit("session_start", {}, ctx);
    const aid = parts(await pi.callTool("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi inject wait" }, ctx)).json.artifact_id;
    await new Promise(r => setTimeout(r, 300));
    let pauses = 0;
    const set = vi.spyOn(globalThis, "setTimeout");
    set.mockImplementation(((fn: () => void, ms?: number, ...rest: unknown[]) => {
      if (ms === INJECT_RETRY_MS) pauses++;
      return realSetTimeout(fn, ms, ...rest);
    }) as typeof setTimeout);
    try {
      const waiting = pi.callToolAsPi("artifax_wait_for_feedback", { url_or_id: aid, timeout_s: 5 }, ctx);
      await new Promise(r => realSetTimeout(r, 300));
      await browserThread(aid, "@agent during the wait");
      const got = parts(await waiting).json;
      expect(got.feedback).toHaveLength(1);
      expect(pi.sent).toHaveLength(0);
      // The daemon answered the loop's inject poll `{feedback: [], text: null, waited_s: 0}` during the wait.
      expect(pauses).toBeGreaterThanOrEqual(1);
    } finally {
      set.mockRestore();
      await pi.emit("session_shutdown", {}, ctx);
    }
  }, 20_000);

  it("tier 1 leaves error results alone and the feedback pending", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tier1-error");
    const aid = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi error" }, ctx)).json.artifact_id;
    await browserThread(aid, "@agent after the error");
    const failed = await pi.callToolAsPi("artifax_read", { url_or_id: aid, path: "missing.css" }, ctx);
    expect(failed.isError).toBe(true);
    expect(failed.content).toHaveLength(1);
    expect(json(failed)).toMatchObject({ error: { code: "not_found" }, feedback: [] });
    expect(parts(await pi.callToolAsPi("artifax_list", {}, ctx)).json.feedback).toHaveLength(1);
  });

  it("tier 1 does not piggyback on wait_for_feedback or on tools it did not register", async () => {
    const { pi, ctx } = load(daemon.home, "pi-tier1-skip");
    const a = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi quiet" }, ctx)).json.artifact_id;
    const b = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi busy" }, ctx)).json.artifact_id;
    await browserThread(b, "@agent on the other page");
    // Waiting on `a` hands over nothing; the comment on `b` is not appended.
    const waited = await pi.callToolAsPi("artifax_wait_for_feedback", { url_or_id: a, timeout_s: 1 }, ctx);
    expect(waited.content).toHaveLength(1);
    expect(parts(waited).json).toEqual({ feedback: [], waited_s: 1, call_again: true });
    const foreign = { type: "tool_result", toolName: "artifax_foreign", toolCallId: "call-2", input: {}, content: [{ type: "text", text: "{}" }], isError: false, details: undefined };
    for (const h of pi.handlers.get("tool_result") ?? []) expect(await h(foreign, ctx)).toBeUndefined();
    expect(parts(await pi.callToolAsPi("artifax_list", {}, ctx)).json.feedback).toHaveLength(1);
  });

  it("refuses a thread ID that is not a canonical ULID", async () => {
    const { pi, ctx } = load(daemon.home, "pi-bad-thread");
    const aid = parts(await pi.callToolAsPi("artifax_publish", { html: "<h2>Goals</h2>", title: "Pi ULID" }, ctx)).json.artifact_id;
    const res = await pi.callTool("artifax_comments_read", { url_or_id: aid, thread_id: "81K6AB3Q9X7N2M4P5R6S8T0V1W" }, ctx);
    expect(res.isError).toBe(true);
    expect(json(res).error).toEqual({ code: "invalid_args", message: "'81K6AB3Q9X7N2M4P5R6S8T0V1W' is not a thread ID" });
  });

  it("session_shutdown stops the injection loop without leaving a retry timer", async () => {
    const { pi, ctx } = load(daemon.home, "pi-inject-stop");
    await pi.emit("session_start", {}, ctx);
    await new Promise(r => setTimeout(r, 300));
    // Timers of the retry length created from here on, and those cleared.
    const created = new Set<unknown>();
    const cleared = new Set<unknown>();
    const set = vi.spyOn(globalThis, "setTimeout");
    const clear = vi.spyOn(globalThis, "clearTimeout");
    set.mockImplementation(((fn: () => void, ms?: number, ...rest: unknown[]) => {
      const t = realSetTimeout(fn, ms, ...rest);
      if (ms === INJECT_RETRY_MS) created.add(t);
      return t;
    }) as typeof setTimeout);
    clear.mockImplementation(((t: unknown) => { cleared.add(t); realClearTimeout(t as NodeJS.Timeout); }) as typeof clearTimeout);
    try {
      await pi.emit("session_shutdown", {}, ctx);
      await new Promise(r => realSetTimeout(r, 100));
    } finally {
      set.mockRestore();
      clear.mockRestore();
    }
    const alive = [...created].filter(t => !cleared.has(t));
    for (const t of alive) realClearTimeout(t as NodeJS.Timeout);
    expect(alive).toHaveLength(0);
  });

  it("pauses the injection loop after an answer that came back empty at once", async () => {
    let polls = 0;
    const fake = await fakeDaemonHome((req, reply) => {
      if (req.url?.startsWith("/api/sessions/s1/feedback")) { polls++; return reply({ feedback: [], text: null, waited_s: 0 }); }
      return false;
    });
    try {
      const pi = new FakePi();
      artifaxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-artifax")) })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-empty");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      await new Promise(r => setTimeout(r, 1500));
      expect(polls).toBeGreaterThanOrEqual(1);
      expect(polls).toBeLessThanOrEqual(2);
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
      artifaxExtension({ home: fake.home, env: withBin(join(scratch, "no-such-artifax")) })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-status-push");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      expect(parts(await pi.callTool("artifax_status", {}, ctx)).json.push).toEqual(push);
    } finally {
      fake.close();
    }
  });

  it("the injection loop never starts a stopped daemon, and resumes when one is back", async () => {
    const home = join(scratch, "inject-restart");
    const env = withBin(artifaxBin);
    const stop = () => execFileSync(artifaxBin, ["stop"], { env: { ...env, ARTIFAX_HOME: home }, stdio: "ignore" });
    try {
      await ensure(home, { env, port: 0 });
      const pi = new FakePi();
      artifaxExtension({ home, env, port: 0 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-restart");
      loaded.push({ pi, ctx });
      await pi.emit("session_start", {}, ctx);
      stop();
      await new Promise(r => setTimeout(r, 6000));
      expect(existsSync(join(home, "daemon.json"))).toBe(false);
      expect(await discover(home)).toBeNull();

      const d = endpointOf(await ensure(home, { env, port: 0 }));
      const live = async () => ((await api(d, "/api/sessions")).sessions as any[]).find(s => s.harness_session_id === "pi-inject-restart" && s.ended_at === null);
      await expect.poll(live, { timeout: 10_000, interval: 200 }).toBeTruthy();
      const sid = (await live()).id;
      const aid = (await api(d, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "Back", files: { "index.html": { content: "<h2>Goals</h2>", encoding: "utf8" } } }) })).artifact.id;
      await api(d, `/api/sessions/${sid}/watches/${aid}`, { method: "PUT", body: JSON.stringify({ replies_armed: true }) });
      await browserThread(aid, "@agent welcome back", d.base);
      await expect.poll(() => pi.sent.length, { timeout: 8000 }).toBe(1);
    } finally {
      stop();
    }
  }, 60_000);

  it("the tool schemas match the daemon's /mcp schemas", async () => {
    const mcp = await mcpTools(daemon);
    const { pi } = load(daemon.home, "pi-schemas");
    const base = (t: unknown) => (Array.isArray(t) ? t.filter(x => x !== "null") : [t]).map(x => (x === "number" ? "integer" : x)).sort().join("|");
    for (const name of TOOLS) {
      const ours = (pi.tools.get(name)!.parameters as any);
      const theirs = mcp.get(name.replace(/^artifax_/, ""))!;
      expect(Object.keys(ours.properties ?? {}).sort(), name).toEqual(Object.keys(theirs.properties ?? {}).sort());
      expect([...(ours.required ?? [])].sort(), name).toEqual([...(theirs.required ?? [])].sort());
      for (const [k, v] of Object.entries<any>(theirs.properties ?? {})) {
        if (v.type !== undefined && ours.properties[k].type !== undefined) expect(base(ours.properties[k].type), `${name}.${k}`).toBe(base(v.type));
      }
    }
  });
});

/** The timer functions as they were before any test replaced them. */
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;

/** An Artifax home whose daemon answers `/healthz`, registers every session as
 * `s1`, and passes other requests to `handle`, which answers through `reply` or
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

describe("ensure", () => {
  it("starts a daemon with `artifax serve --json` when none is running", async () => {
    const home = join(scratch, "fresh");
    try {
      const info = await ensure(home, { env: withBin(artifaxBin), port: 0 });
      expect(info.port).toBeGreaterThan(0);
      expect((await fetch(`http://127.0.0.1:${info.port}/healthz`)).ok).toBe(true);
    } finally {
      execFileSync(artifaxBin, ["stop"], { env: { ...process.env, ARTIFAX_HOME: home, ARTIFAX_CODEX_BIN: "" }, stdio: "ignore" });
    }
  }, 30_000);

  it("names the install command when no artifax binary is found", async () => {
    await expect(ensure(join(scratch, "nobin"), { env: { PATH: "" } })).rejects.toThrow(/cargo install --path crates\/artifax-cli/);
  });
});
