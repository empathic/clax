import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer } from "node:net";
import { validateToolArguments, type Tool } from "@mariozechner/pi-ai";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { artifactRef, artifaxExtension, htmlTitle, isText, textPrefix } from "../src/artifax.ts";
import { ensure } from "../src/daemon.ts";
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
  return { pi, ...fakeContext(cwd, sessionId) };
}

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
  it("registers the nine tools with one-line prompt snippets, and the artifax command", () => {
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
