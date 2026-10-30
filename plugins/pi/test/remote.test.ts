// Remote mode (spec 2026-10-08 §16, §6.6, §6.8): the extension signs every
// request as the Rust clients do, proves its session with the secret its
// registration was answered, keeps that secret in its credential file, and
// fetches a remote daemon's clips into its cache.
import { createHash, generateKeyPairSync, verify, type KeyObject } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { claxExtension } from "../src/clax.ts";
import { sendRemote } from "../src/client.ts";
import { repoAndBranch } from "../src/git.ts";
import { AGENT_HEADERS, canonical, credentialFileName, encodeAgentHeaders, loadRemote, parseClipReference, rewriteClipText, MAX_RESPONSE_BYTES, SessionFiles } from "../src/remote.ts";
import { FakePi, fakeContext } from "./fake-api.ts";

let scratch: string;
beforeAll(() => { scratch = mkdtempSync(join(tmpdir(), "clax-pi-remote-")); });
afterAll(() => { rmSync(scratch, { recursive: true, force: true }); });

const sha = (b: Buffer | string) => createHash("sha256").update(b).digest("hex");
/** A clip's bytes: a PNG's signature, then anything. */
const PNG = Buffer.concat([Buffer.from("89504e470d0a1a0a", "hex"), Buffer.from("png-bytes")]);

describe("remote mode", () => {
  it("remote mode signs with the same canonical string as Rust", () => {
    // `canonical_string_is_fixed` in crates/clax-core/src/sig.rs.
    const agent = ["01J9ZK3M4N5P6Q7R8S9T0V1W2X", "agent", "mcp", undefined, ""];
    expect(encodeAgentHeaders(agent).toString()).toBe("+26:01J9ZK3M4N5P6Q7R8S9T0V1W2X\n+5:agent\n+3:mcp\n-\n+0:\n");
    const input = { method: "POST", pathAndQuery: "/api/artifacts?x=1", host: "192.168.1.5:7480", t: 1_760_000_000, nonce: "n0", body: Buffer.from("{}"), token: "cxg_t", agent };
    const s = canonical(input);
    expect(s).toBe([
      "clax-ed25519 v2", "POST", "/api/artifacts?x=1", "192.168.1.5:7480", "1760000000", "n0",
      "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
      "e363d964cd2aef8c9f8019233052f146fd7c41533e194244bd856d9dedfdfcd4",
      "0200b59637463a41412fc63797d0699f6daaab4bed5d838ac1a4c264398d66b5",
    ].join("\n"));
    // A request with none of the headers.
    expect(sha(encodeAgentHeaders(AGENT_HEADERS.map(() => undefined)))).toBe("a00d0d823baf03fa53cba1260dd2f71fc7e8694679ce444737a7168dfab351e8");
    // The grant request has no token: its token line is empty.
    expect(canonical({ ...input, token: undefined })).toMatch(/\n44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\n\n0200b5/);
  });

  it("names credential files as the Rust clients do", () => {
    expect(credentialFileName("pi", "pi-session-1")).toBe("pi-s-pi-session-1.json");
    const odd = "a/b c";
    expect(credentialFileName("pi", odd)).toBe(`pi-h-${sha(odd).slice(0, 32)}.json`);
    expect(credentialFileName("pi", "x".repeat(129))).toMatch(/^pi-h-[0-9a-f]{32}\.json$/);
    expect(credentialFileName("Pi", "x")).toBeUndefined();
  });

  it("marks its own credential file in use, and no other session's", () => {
    const files = new SessionFiles(join(scratch, `touch-${Math.random().toString(36).slice(2)}`));
    files.write("pi-s-t.json", "t", { session: "S1", secret: `cxs_${"a".repeat(43)}` });
    const path = join(files.dir, "pi-s-t.json");
    const weekAgo = new Date(Date.now() - 8 * 24 * 3600 * 1000);
    utimesSync(path, weekAgo, weekAgo);
    files.touch("pi-s-t.json", "S2");
    expect(statSync(path).mtimeMs).toBeLessThan(Date.now() - 7 * 24 * 3600 * 1000);
    files.touch("pi-s-t.json", "S1");
    expect(statSync(path).mtimeMs).toBeGreaterThan(Date.now() - 60_000);
  });

  it("reads the repository and branch from the files, as the Rust clients do", () => {
    const repo = join(scratch, "web");
    mkdirSync(join(repo, ".git"), { recursive: true });
    mkdirSync(join(repo, "src"), { recursive: true });
    writeFileSync(join(repo, ".git", "HEAD"), "ref: refs/heads/fix/chart-axis\n");
    expect(repoAndBranch(join(repo, "src"))).toEqual({ repo: "web", branch: "fix/chart-axis" });
    // A worktree is named after its repository; a detached HEAD has no branch.
    const wt = join(scratch, "web-wt");
    const gitdir = join(repo, ".git", "worktrees", "wt");
    mkdirSync(gitdir, { recursive: true });
    mkdirSync(wt, { recursive: true });
    writeFileSync(join(wt, ".git"), `gitdir: ${gitdir}\n`);
    writeFileSync(join(gitdir, "commondir"), "../..\n");
    writeFileSync(join(gitdir, "HEAD"), "0123456789abcdef0123456789abcdef01234567\n");
    expect(repoAndBranch(wt)).toEqual({ repo: "web" });
    expect(repoAndBranch("/")).toEqual({});
  });

  it("rewrites clip lines to fetched files, keeps withheld ones without a fetch, and never passes a path through", async () => {
    const aid = "7q3k9mzx2b4t";
    const tid = "01J9ZZZZZZZZZZZZZZZZZZZZZZ";
    expect(parseClipReference(`clax-clip:${aid}/${tid}`)).toEqual([aid, tid]);
    expect(parseClipReference(`clax-clip:${aid}/../remote.key`)).toBeUndefined();
    const fetched: string[] = [];
    const text = `[clax] x\nClip: clax-clip:${aid}/${tid}\nClip: withheld\nClip: /Users/sam/.clax/remote.key\nSnapshot: /x.png`;
    const out = await rewriteClipText(text, async (a, t) => { fetched.push(`${a}/${t}`); return { path: "/c/x.png" }; });
    expect(fetched).toEqual([`${aid}/${tid}`]);
    expect(out.split("\n")).toEqual([
      "[clax] x", "Clip: /c/x.png", "Clip: withheld",
      "Clip: unavailable (the remote daemon named no clip)", "Snapshot: unavailable (on the remote daemon's machine)",
    ]);
    expect(await rewriteClipText(`Clip: clax-clip:${aid}/${tid}`, async () => ({ why: "withheld" }))).toBe("Clip: unavailable (withheld)");
  });
});

/** A connected home: `remote.json` naming `url`, and an Ed25519 key. */
function connectedHome(url: string): { home: string; publicKey: KeyObject } {
  const home = join(scratch, `home-${Math.random().toString(36).slice(2)}`);
  mkdirSync(home, { recursive: true });
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  writeFileSync(join(home, "remote.key"), privateKey.export({ format: "der", type: "pkcs8" }), { mode: 0o600 });
  writeFileSync(join(home, "remote.json"), JSON.stringify({
    version: 1, url, grant_id: "g1", access_token: "cxg_t", manage_token: "cxm_t", continue_token: "cxc_t",
    person: { public_id: "u_sam", display_name: "Sam" }, owner_name: "Alex", own_machine: false, client_name: "m", connected_at: "",
  }), { mode: 0o600 });
  return { home, publicKey };
}

/** Whether `req` (with `body`) carries a valid signature by `key`, as the
 * daemon checks it. */
function signedBy(req: IncomingMessage, body: Buffer, key: KeyObject): boolean {
  const m = /^t=(\d+), n=([A-Za-z0-9_-]{22}), s=([A-Za-z0-9_-]+)$/.exec(String(req.headers["clax-signature"] ?? ""));
  if (!m) return false;
  const auth = String(req.headers.authorization ?? "");
  if (!auth.startsWith("Clax-Grant ")) return false;
  const agent = AGENT_HEADERS.map(h => req.headers[h] as string | undefined);
  const c = canonical({ method: req.method!, pathAndQuery: req.url!, host: String(req.headers.host), t: Number(m[1]), nonce: m[2]!, body, token: auth.slice("Clax-Grant ".length), agent });
  return verify(null, Buffer.from(c), key, Buffer.from(m[3]!, "base64url"));
}

describe("remote mode against a daemon of another machine", () => {
  it("signs every request, proves its session with its secret, keeps it in its credential file, and registers again when the proof is refused", async () => {
    const seen: { what: string; signed: boolean; session?: string; secret?: string }[] = [];
    let n = 0;
    let secretOf = new Map<string, string>();
    let refuseOnce = false;
    let piggybacked = false;
    const AID = "7q3k9mzx2b4t";
    const TID = "01J9ZZZZZZZZZZZZZZZZZZZZZZ";
    let home = "";
    let publicKey!: KeyObject;
    const server = createServer((req: IncomingMessage, res: ServerResponse) => {
      const chunks: Buffer[] = [];
      req.on("data", c => chunks.push(c));
      req.on("end", () => {
        const body = Buffer.concat(chunks);
        const reply = (status: number, v: unknown) => { res.statusCode = status; res.setHeader("content-type", "application/json"); res.end(JSON.stringify(v)); };
        const session = req.headers["x-clax-session"] as string | undefined;
        const secret = req.headers["x-clax-session-secret"] as string | undefined;
        seen.push({ what: `${req.method} ${req.url}`, signed: signedBy(req, body, publicKey), session, secret });
        if (!signedBy(req, body, publicKey)) return reply(401, { error: { code: "invalid_signature", message: "no" } });
        if (session !== undefined && secretOf.get(session) !== secret) return reply(401, { error: { code: "invalid_session_proof", message: "no" } });
        if (req.method === "POST" && req.url === "/api/sessions") {
          const id = `S${++n}`;
          const s = `cxs_${"a".repeat(42)}${n}`;
          secretOf.set(id, s);
          return reply(201, { session: { id, harness: "pi", harness_session_id: "pi-remote-1", cwd: "/", pid: null, parent_pid: null, started_at: "", last_seen_at: "", ended_at: null, name: "pi" }, session_secret: s });
        }
        if (req.url === "/api/artifacts" && refuseOnce) {
          refuseOnce = false;
          return reply(401, { error: { code: "invalid_session_proof", message: "ended" } });
        }
        if (req.url === "/api/artifacts") return reply(200, { artifacts: [] });
        if (req.url?.startsWith("/api/sessions/") && req.url.includes("/feedback?tier=piggyback")) {
          const once = !piggybacked;
          piggybacked = true;
          return reply(200, once ? {
            feedback: [{ thread_id: TID, clip_path: `clax-clip:${AID}/${TID}` }], answers: [], text: `[clax] 1 comment sent to you:\nClip: clax-clip:${AID}/${TID}\nSam: "x"`,
          } : { feedback: [], answers: [], text: null });
        }
        if (req.url === `/api/artifacts/${AID}/threads/${TID}/clip`) {
          res.setHeader("content-type", "image/png");
          return res.end(PNG);
        }
        if (req.method === "PATCH") return reply(200, { session: { id: session, harness: "pi", harness_session_id: "pi-remote-1", cwd: "/", pid: null, parent_pid: null, started_at: "", last_seen_at: "", ended_at: "x" } });
        return reply(200, {});
      });
    });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    ({ home, publicKey } = connectedHome(`http://127.0.0.1:${port}`));
    try {
      const pi = new FakePi();
      claxExtension({ home, env: { ...process.env, CLAX_BIN: "/nonexistent" } })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-remote-1");
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      const file = join(home, "remote-sessions", "pi-s-pi-remote-1.json");
      expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({ session: "S1", secret: secretOf.get("S1"), harness_session_id: "pi-remote-1" });
      expect(statSync(file).mode & 0o777).toBe(0o600);
      expect(statSync(join(home, "remote-sessions")).mode & 0o777).toBe(0o700);
      expect((await pi.callTool("clax_list", {}, ctx)).isError).toBe(false);
      // The session ended under it: its proof is refused, and it registers
      // again, getting a new session and secret.
      refuseOnce = true;
      expect((await pi.callTool("clax_list", {}, ctx)).isError).toBe(false);
      expect(JSON.parse(readFileSync(file, "utf8")).session).toBe("S2");
      // A clip the payload names by reference is fetched, signed, into the
      // cache, and the agent is handed that file.
      const withClip = await pi.callToolAsPi("clax_list", {}, ctx);
      const cached = join(home, "remote-cache", "clips", `${AID}-${TID}.png`);
      expect(readFileSync(cached).equals(PNG)).toBe(true);
      expect(withClip.content.at(-1)!.text).toContain(`\nClip: ${cached}\n`);
      expect(JSON.parse(withClip.content[0]!.text!).feedback[0].clip_path).toBe(cached);
      for (const r of seen) expect(r.signed, r.what).toBe(true);
      for (const r of seen.filter(r => r.session !== undefined)) expect(r.secret, r.what).toBe(secretOf.get(r.session!));
      expect(seen.filter(r => r.what === "GET /api/artifacts").every(r => r.session !== undefined)).toBe(true);
      await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
      expect(existsSync(file)).toBe(false);
    } finally {
      server.closeAllConnections();
      server.close();
    }
  });
});

describe("withheld content", () => {
  it("the db tools show a withheld document as withheld and pass a listing's withheld count on, as the Rust tools do", async () => {
    const home = join(scratch, `withheld-${Math.random().toString(36).slice(2)}`);
    mkdirSync(home, { recursive: true });
    const aid = "7q3k9mzx2b4t";
    const authors = [{ public_id: "u_mia", name: "Mia" }];
    const server = createServer((req, res) => {
      const reply = (v: unknown) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(v)); };
      req.resume();
      req.on("end", () => {
        if (req.url === "/healthz") return reply({ version: "0.1.0" });
        if (req.method === "POST" && req.url === "/api/sessions") {
          return reply({ session: { id: "s1", harness: "pi", harness_session_id: "h", cwd: "/", pid: 1, parent_pid: 1, started_at: "", last_seen_at: "", ended_at: null } });
        }
        if (req.url?.startsWith(`/api/artifacts/${aid}/docs/notes/d1`)) return reply({ doc: { doc_id: "d1", version: 3, withheld: { authors } } });
        if (req.url?.startsWith(`/api/artifacts/${aid}/docs?`)) return reply({ docs: [], next_cursor: null, withheld_count: 2 });
        return reply({});
      });
    });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    writeFileSync(join(home, "daemon.json"), JSON.stringify({ port, pid: process.pid, token: "t", started_at: "2026-01-01T00:00:00Z", bind: "127.0.0.1", version: "0.1.0" }));
    try {
      const pi = new FakePi();
      claxExtension({ home, env: { ...process.env, CLAX_BIN: "/nonexistent" } })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-withheld");
      const got = JSON.parse((await pi.callTool("clax_db_get", { url_or_id: aid, collection: "notes", doc_id: "d1" }, ctx)).content[0]!.text!);
      expect(got.doc).toEqual({ id: "d1", version: 3, withheld: { authors } });
      const listed = JSON.parse((await pi.callTool("clax_db_list", { url_or_id: aid, collection: "notes" }, ctx)).content[0]!.text!);
      expect(listed.withheld_count).toBe(2);
    } finally {
      server.closeAllConnections();
      server.close();
    }
  });
});

describe("remote clips while feedback is handed over", () => {
  it("a clip that does not come in time, or is not a PNG, never holds the comments", async () => {
    const AID = "7q3k9mzx2b4t";
    const SLOW = "01J9ZZZZZZZZZZZZZZZZZZZZZY";
    const HTML = "01J9ZZZZZZZZZZZZZZZZZZZZZX";
    const held: ServerResponse[] = [];
    let handed = false;
    const server = createServer((req, res) => {
      req.resume();
      req.on("end", () => {
        const reply = (v: unknown) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(v)); };
        if (req.method === "POST" && req.url === "/api/sessions") {
          return reply({ session: { id: "S1", harness: "pi", harness_session_id: "pi-slow", cwd: "/", pid: null, parent_pid: null, started_at: "", last_seen_at: "", ended_at: null }, session_secret: `cxs_${"b".repeat(43)}` });
        }
        if (req.url?.includes("/feedback?tier=wait")) {
          const text = `[clax] 2 comments sent to you:\nClip: clax-clip:${AID}/${SLOW}\nSam: "FIRST"\n\nClip: clax-clip:${AID}/${HTML}\nSam: "SECOND"`;
          const once = !handed;
          handed = true;
          return reply(once ? { feedback: [{ thread_id: SLOW }, { thread_id: HTML }], answers: [], text, waited_s: 0 } : { feedback: [], answers: [], text: null, waited_s: 0 });
        }
        if (req.url === `/api/artifacts/${AID}/threads/${SLOW}/clip`) return void held.push(res);
        if (req.url === `/api/artifacts/${AID}/threads/${HTML}/clip`) {
          res.setHeader("content-type", "text/html");
          return res.end("<html>");
        }
        return reply({});
      });
    });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    const { home } = connectedHome(`http://127.0.0.1:${port}`);
    try {
      const pi = new FakePi();
      claxExtension({ home, env: { ...process.env, CLAX_BIN: "/nonexistent" }, clipFetchMs: 200 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-slow");
      const out = await pi.callTool("clax_wait_for_feedback", { timeout_s: 1 }, ctx);
      const trailing = out.content.at(-1)!.text!;
      expect(trailing).toContain('Sam: "FIRST"');
      expect(trailing).toContain('Sam: "SECOND"');
      expect(trailing).toContain("Clip: unavailable (timed out)");
      expect(trailing).toContain("Clip: unavailable (not a PNG)");
    } finally {
      for (const r of held) r.destroy();
      server.closeAllConnections();
      server.close();
    }
  });
});

describe("remote clips while the injection loop hands feedback over", () => {
  it("sends nothing when the session ended while the feedback's clips were fetched", async () => {
    const AID = "7q3k9mzx2b4t";
    const TID = "01J9ZZZZZZZZZZZZZZZZZZZZZW";
    let polls = 0;
    let clip: ServerResponse | undefined;
    let clipAsked!: () => void;
    const asked = new Promise<void>(r => { clipAsked = r; });
    const server = createServer((req, res) => {
      req.resume();
      req.on("end", () => {
        const reply = (v: unknown) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(v)); };
        if (req.method === "POST" && req.url === "/api/sessions") {
          return reply({ session: { id: "S1", harness: "pi", harness_session_id: "pi-inject-clip", cwd: "/", pid: null, parent_pid: null, started_at: "", last_seen_at: "", ended_at: null }, session_secret: `cxs_${"c".repeat(43)}` });
        }
        if (req.url?.includes("/feedback?tier=inject")) {
          // The first poll hands over feedback naming a clip; later ones are parked.
          if (++polls === 1) return reply({ feedback: [{ thread_id: TID }], answers: [], text: `[clax] 1 comment sent to you:\nClip: clax-clip:${AID}/${TID}\nSam: "x"`, waited_s: 0 });
          return;
        }
        if (req.url === `/api/artifacts/${AID}/threads/${TID}/clip`) { clip = res; clipAsked(); return; }
        return reply({});
      });
    });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    const { home } = connectedHome(`http://127.0.0.1:${port}`);
    try {
      const pi = new FakePi();
      // A clip fetch that cannot time out within the test: only the held answer ends it.
      claxExtension({ home, env: { ...process.env, CLAX_BIN: "/nonexistent" }, clipFetchMs: 600_000 })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-inject-clip");
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      await asked;
      await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
      clip!.setHeader("content-type", "image/png");
      clip!.end(PNG);
      // The clip is stored before the text is rewritten; what follows is promise
      // continuations only, all run before the next macrotask.
      const cached = join(home, "remote-cache", "clips", `${AID}-${TID}.png`);
      await expect.poll(() => existsSync(cached), { timeout: 15_000 }).toBe(true);
      await new Promise(r => setImmediate(r));
      expect(pi.sent).toEqual([]);
    } finally {
      server.closeAllConnections();
      server.close();
    }
  });
});

describe("ending a session that already ended", () => {
  it("registers no session just to end it, and removes the credential file", async () => {
    const seen: string[] = [];
    const secretOf = new Map<string, string>();
    let n = 0;
    let home = "";
    let publicKey!: KeyObject;
    const server = createServer((req: IncomingMessage, res: ServerResponse) => {
      const chunks: Buffer[] = [];
      req.on("data", c => chunks.push(c));
      req.on("end", () => {
        const body = Buffer.concat(chunks);
        const reply = (status: number, v: unknown) => { res.statusCode = status; res.setHeader("content-type", "application/json"); res.end(JSON.stringify(v)); };
        const session = req.headers["x-clax-session"] as string | undefined;
        const secret = req.headers["x-clax-session-secret"] as string | undefined;
        seen.push(`${req.method} ${req.url}`);
        if (!signedBy(req, body, publicKey)) return reply(401, { error: { code: "invalid_signature", message: "no" } });
        if (session !== undefined && secretOf.get(session) !== secret) return reply(401, { error: { code: "invalid_session_proof", message: "ended" } });
        if (req.method === "POST" && req.url === "/api/sessions") {
          const id = `S${++n}`;
          const s = `cxs_${"a".repeat(42)}${n}`;
          secretOf.set(id, s);
          return reply(201, { session: { id, harness: "pi", harness_session_id: "pi-ended-1", cwd: "/", pid: null, parent_pid: null, started_at: "", last_seen_at: "", ended_at: null, name: "pi" }, session_secret: s });
        }
        return reply(200, {});
      });
    });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    ({ home, publicKey } = connectedHome(`http://127.0.0.1:${port}`));
    try {
      const pi = new FakePi();
      claxExtension({ home, env: { ...process.env, CLAX_BIN: "/nonexistent" } })(pi.api);
      const { ctx } = fakeContext(scratch, "pi-ended-1");
      await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
      const file = join(home, "remote-sessions", "pi-s-pi-ended-1.json");
      expect(existsSync(file)).toBe(true);
      // Ended elsewhere (in People): its secret proves nothing any more.
      secretOf.delete("S1");
      await pi.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
      expect(seen.filter(r => r === "POST /api/sessions")).toHaveLength(1);
      expect(existsSync(file)).toBe(false);
    } finally {
      server.closeAllConnections();
      server.close();
    }
  });
});

describe("remote retries", () => {
  /** A daemon answering each request with `answer(n)` (n from 1). */
  async function daemonAnswering(answer: (n: number, res: ServerResponse) => void) {
    let n = 0;
    const server = createServer((req, res) => { req.resume(); req.on("end", () => answer(++n, res)); });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    return { url: `http://127.0.0.1:${port}`, count: () => n, close: () => { server.closeAllConnections(); server.close(); } };
  }
  const error = (res: ServerResponse, status: number, code: string, date?: string) => {
    res.statusCode = status;
    if (date) res.setHeader("date", date);
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ error: { code, message: code } }));
  };

  it("takes the daemon's URL as the Rust clients do, and sends each path as given whatever form the URL has", async () => {
    const hostOf = (url: string) => loadRemote(connectedHome(url).home)!.config;
    expect(hostOf("http://192.168.1.5")).toMatchObject({ url: "http://192.168.1.5", host: "192.168.1.5" });
    expect(hostOf("http://1.2.3.4:80/")).toMatchObject({ url: "http://1.2.3.4", host: "1.2.3.4" });
    expect(hostOf("http://[0:0:0:0:0:0:0:1]:7480")).toMatchObject({ url: "http://[::1]:7480", host: "[::1]:7480" });
    for (const bad of ["http://clax.local:7480", "https://1.2.3.4:7480", "http://1.2.3.4:7480/x", "http://1.2.3.4:7480/?a", "http://1.2.3.4:7480#f", "http://u:p@1.2.3.4:7480", "1.2.3.4:7480"]) {
      expect(() => loadRemote(connectedHome(bad).home), bad).toThrow(/the daemon URL is not http:\/\/<IP address>:<port>/);
    }
    const seen: { url?: string; host?: string }[] = [];
    const server = createServer((req, res) => { seen.push({ url: req.url, host: req.headers.host }); req.resume(); req.on("end", () => res.end("{}")); });
    await new Promise<void>(r => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    try {
      const r = loadRemote(connectedHome(`http://127.000.000.001:${port}/`).home)!;
      expect(r.config.host).toBe(`127.0.0.1:${port}`);
      expect((await sendRemote(r, "/api/artifacts?limit=2", { method: "GET" })).status).toBe(200);
      expect(seen).toEqual([{ url: "/api/artifacts?limit=2", host: `127.0.0.1:${port}` }]);
    } finally {
      server.closeAllConnections();
      server.close();
    }
  });

  it("retries busy and limit_reached three times, after 1, 2 and 4 s with up to a quarter more, then gives up", async () => {
    for (const [status, code] of [[503, "busy"], [429, "limit_reached"]] as const) {
      const d = await daemonAnswering((_, res) => error(res, status, code));
      try {
        const r = loadRemote(connectedHome(d.url).home)!;
        const slept: number[] = [];
        r.sleep = async ms => { slept.push(ms); };
        const res = await sendRemote(r, "/api/artifacts", { method: "GET" });
        expect(res.status).toBe(status);
        expect(d.count()).toBe(4);
        expect(slept).toHaveLength(3);
        for (const [i, base] of [1000, 2000, 4000].entries()) {
          expect(slept[i]).toBeGreaterThanOrEqual(base);
          expect(slept[i]).toBeLessThanOrEqual(base * 1.25);
        }
      } finally {
        d.close();
      }
    }
  });

  it("cuts a busy backoff short when the request's signal aborts, failing it cancelled", async () => {
    const d = await daemonAnswering((_, res) => error(res, 503, "busy"));
    try {
      const r = loadRemote(connectedHome(d.url).home)!;
      let slept!: () => void;
      const asleep = new Promise<void>(done => { slept = done; });
      // A backoff that never ends on its own.
      r.sleep = () => { slept(); return new Promise<void>(() => {}); };
      const ac = new AbortController();
      const res = sendRemote(r, "/api/artifacts", { method: "GET", signal: ac.signal });
      await asleep;
      ac.abort();
      await expect(res).rejects.toMatchObject({ error: { kind: "unreachable", message: "request cancelled" } });
      expect(d.count()).toBe(1);
    } finally {
      d.close();
    }
  });

  it("retries a stale request once, with the daemon's clock, and only once", async () => {
    const ahead = new Date(Date.now() + 100_000).toUTCString();
    const once = await daemonAnswering((n, res) => (n === 1 ? error(res, 401, "stale_request", ahead) : res.end("{}")));
    const always = await daemonAnswering((_, res) => error(res, 401, "stale_request", ahead));
    try {
      const r = loadRemote(connectedHome(once.url).home)!;
      expect((await sendRemote(r, "/api/artifacts", { method: "GET" })).status).toBe(200);
      expect(once.count()).toBe(2);
      expect(Math.abs(r.offset - 100)).toBeLessThanOrEqual(2);
      const r2 = loadRemote(connectedHome(always.url).home)!;
      expect((await sendRemote(r2, "/api/artifacts", { method: "GET" })).status).toBe(401);
      expect(always.count()).toBe(2);
    } finally {
      once.close();
      always.close();
    }
  });

  it("says the clocks differ when the daemon's is too far off to learn, as the Rust clients do", async () => {
    const far = new Date(Date.now() + 3600_000).toUTCString();
    const d = await daemonAnswering((_, res) => error(res, 401, "stale_request", far));
    try {
      const r = loadRemote(connectedHome(d.url).home)!;
      const res = await sendRemote(r, "/api/artifacts", { method: "GET" });
      expect(res.status).toBe(401);
      const e = JSON.parse(res.body.toString("utf8")).error;
      expect(e.code).toBe("clock_skew");
      expect(e.message).toMatch(/^the daemon's clock is \+36\d\d s from this machine's, more than the 600 s a signed request may be moved by: set this machine's clock \(nothing was signed for that time\)$/);
      expect(d.count()).toBe(1);
      expect(r.offset).toBe(0);
    } finally {
      d.close();
    }
  });

  it("reads at most MAX_RESPONSE_BYTES of an answer, as the Rust clients do", async () => {
    const d = await daemonAnswering((_, res) => {
      res.setHeader("content-type", "application/json");
      res.write("[");
      res.end(Buffer.alloc(MAX_RESPONSE_BYTES, 0x20));
    });
    try {
      const r = loadRemote(connectedHome(d.url).home)!;
      await expect(sendRemote(r, "/api/artifacts", { method: "GET" })).rejects.toMatchObject({ error: { kind: "bad_response" } });
    } finally {
      d.close();
    }
  });

  it("counts an agent header's bytes, not its UTF-16 units", () => {
    expect(encodeAgentHeaders(["é", undefined, undefined, undefined, undefined]).toString()).toBe("+2:é\n-\n-\n-\n-\n");
  });
});
