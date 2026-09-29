import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { artifactRef, artifaxExtension, textPrefix } from "../src/artifax.ts";
import { ensure } from "../src/daemon.ts";
import { api, artifaxBin, startDaemon, type TestDaemon } from "./daemon-fixture.ts";
import { FakePi, fakeContext, json } from "./fake-api.ts";

const TOOLS = ["publish", "read", "list", "delete", "open", "pin", "unpin", "asset_upload", "status"].map(t => `artifax_${t}`);

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

/** This process's environment with `ARTIFAX_BIN` set to `bin`. */
function withBin(bin: string): NodeJS.ProcessEnv {
  return { ...process.env, ARTIFAX_BIN: bin };
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
    expect(notes.at(-1)?.message).toContain("usage: /artifax open [id] | list | status");
  });
});

describe("helpers", () => {
  it("artifactRef reads IDs and versions from every URL form", () => {
    const id = "7q3k9mzx2b4t";
    const cases: [string, number | undefined][] = [
      [id, undefined], [` ${id} `, undefined],
      [`http://localhost:7480/a/${id}`, undefined], [`/a/${id}`, undefined],
      [`http://localhost:7480/a/${id}/v/3`, 3], [`http://127.0.0.1:7480/c/${id}/v/2/img/a.png`, 2],
      [`http://${id}.localhost:7480/v/4/`, 4], [`http://${id}.localhost/v/5/app.js?x#y`, 5],
      [`http://localhost:7480/a/${id}/v/2?x=1`, 2],
    ];
    for (const [s, v] of cases) expect(artifactRef(s), s).toEqual(v === undefined ? { id } : { id, version: v });
    for (const bad of ["http://localhost:7480/c/nope/v/1/", "http://evil.localhost:7480/v/1/", `http://localhost:7480/x/${id}`, "nope"]) {
      expect(() => artifactRef(bad), bad).toThrow(/invalid_id/);
    }
  });

  it("textPrefix never splits a character", () => {
    const s = Buffer.from("aé");
    expect(textPrefix(s, 2)).toBe("a");
    expect(textPrefix(s, 3)).toBe("aé");
    expect(textPrefix(Buffer.from([0x61, 0xff, 0x62]), 10)).toBe("a\uFFFDb");
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
      execFileSync(artifaxBin, ["stop"], { env: { ...process.env, ARTIFAX_HOME: home }, stdio: "ignore" });
    }
  }, 30_000);

  it("names the install command when no artifax binary is found", async () => {
    await expect(ensure(join(scratch, "nobin"), { env: { PATH: "" } })).rejects.toThrow(/cargo install --path crates\/artifax-cli/);
  });
});
