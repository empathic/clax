import { test as playwrightTest, expect as playwrightExpect, type Browser, type BrowserContext, type Frame, type Locator, type Page, type Response } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { request as httpRequest } from "node:http";
import { createServer } from "node:net";
import { mkdtempSync, readFileSync, existsSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { advance, quietEval } from "./time";

/** Playwright's `expect`, whose `poll` checks again after 10, 20 and 50 ms,
 * then every 100 ms (Playwright's own waits 100, 250, 500 ms, then every
 * second), so a test moves on as soon as the state it waits for holds. */
const POLL_INTERVALS = [10, 20, 50, 100];
export const expect = new Proxy(playwrightExpect, {
  get(target, prop, receiver) {
    if (prop === "poll") {
      const poll: typeof target.poll = (fn, opts) => target.poll(fn, { intervals: POLL_INTERVALS, ...(typeof opts === "string" ? { message: opts } : opts) });
      return poll;
    }
    return Reflect.get(target, prop, receiver);
  },
});

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

/** The default config.toml: the sample key comes from a variable nobody sets, so no test reaches a real provider. */
export const NO_KEY_CONFIG = '[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n';
/** The stub provider: canned, deterministic answers (see crates/clax-sample/src/stub.rs). */
export const STUB_CONFIG = '[sample]\nprovider = "stub"\nstub_delay_ms = 150\n';

export type Daemon = Awaited<ReturnType<typeof startDaemon>>;

/** The daemons this worker runs, each with the owner browser session its
 * browsers hold (its cookie value and its proof), and the browser contexts
 * it opened. */
type Live = { base: string; port: number; token: string; session: string | null; proof: string | null; browser?: Promise<string> };
const liveDaemons = new Set<Live>();
const liveContexts = new Set<BrowserContext>();

/** The `clax_viewer` cookie value of a new browser of this machine's with
 * no name on daemon `d`, minted by its debug-build test route (reaching the
 * port alone mints no viewer). */
async function mintViewer(d: { base: string; token: string }): Promise<string> {
  const r = await plainFetch(`${d.base}/api/_test/viewers`, { method: "POST", headers: { authorization: `Bearer ${d.token}` } });
  if (!r.ok) throw new Error(`a viewer for the tests: ${r.status} ${await r.text()}`);
  return ((await r.json()) as { cookie: string }).cookie;
}

/** The `clax_viewer` cookie (`name=value`) of the browser of this machine's
 * with no name on daemon `d` that the tests' own requests are, minted once. */
function localBrowser(d: Live): Promise<string> {
  d.browser ??= mintViewer(d).then(c => `clax_viewer=${c}`);
  return d.browser;
}

/** The header a local owner browser session's proof rides in
 * (`clax_http::local_trust::PROOF_HEADER`). */
export const PROOF_HEADER = "x-clax-owner-proof";

/** The tests' own requests to a daemon of this worker (`fetch` from Node)
 * that carry no credential are a browser of this machine's with no name
 * ([`localBrowser`]): the anonymous caller of before, whom a local daemon
 * now refuses. A request with `Authorization` or a cookie of its own goes
 * as it is, except that one carrying the owner browser session's cookie
 * carries its proof too, as the owner's shell sends it. */
const plainFetch = globalThis.fetch.bind(globalThis);
globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
  if (input instanceof Request) return plainFetch(input, init);
  const url = new URL(String(input));
  const d = [...liveDaemons].find(x => String(x.port) === url.port && (url.hostname === "localhost" || url.hostname === "127.0.0.1"));
  const headers = new Headers(init?.headers);
  if (!d || headers.has("authorization")) return plainFetch(input, init);
  const cookie = headers.get("cookie");
  if (cookie !== null) {
    if (d.session && d.proof && !headers.has(PROOF_HEADER) && cookie.includes(d.session)) {
      headers.set(PROOF_HEADER, d.proof);
      return plainFetch(input, { ...init, headers });
    }
    return plainFetch(input, init);
  }
  headers.set("cookie", await localBrowser(d));
  return plainFetch(input, { ...init, headers });
}) as typeof fetch;

/** A browser login on the local daemon at `base` with `token`, as `clax
 * open` makes one: the owner browser session's cookie value it set and the
 * proof its page writes into the origin's storage, or nulls for a daemon
 * that has none (a hosted one). */
async function ownerSession(base: string, token: string, port: number): Promise<{ session: string | null; proof: string | null }> {
  const none = { session: null, proof: null };
  const made = await plainFetch(`${base}/api/browser-login`, { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify({ next: "/" }) });
  if (!made.ok) return none;
  const { path } = (await made.json()) as { path: string };
  const res = await plainFetch(`${base}${path}`, { redirect: "manual" });
  const kv = res.headers.getSetCookie().map(c => c.split(";")[0]).find(c => c.startsWith(`clax_owner_${port}=`));
  const proof = /data-proof="([^"]+)"/.exec(await res.text())?.[1] ?? null;
  return kv ? { session: kv.slice(kv.indexOf("=") + 1), proof } : none;
}

function liveAt(base: string): Live | undefined {
  return [...liveDaemons].find(x => x.base === base || x.base === base.replace("127.0.0.1", "localhost"));
}

/** The owner browser session (cookie value and proof) each home's daemon
 * started with: a daemon started again on the same home keeps it, as a
 * restart keeps owner browser sessions, so open pages stay signed in. */
const homeSessions = new Map<string, { session: string | null; proof: string | null }>();

/** The owner browser session the worker's browsers hold on the daemon at
 * `base` (a login made it when the daemon started). */
export function ownerSessionOf(base: string): string {
  const d = liveAt(base);
  if (!d?.session) throw new Error(`no owner browser session for ${base}`);
  return d.session;
}

/** The proof header a browser of the owner's shell sends to `origin`, for
 * a test's own request from a browser context (`page.request`, whose
 * requests carry the context's cookies but none of the page's storage):
 * the session's proof on a daemon of the worker reached by `localhost` or
 * `127.0.0.1`, else nothing. */
export function proofHeaders(origin: string): Record<string, string> {
  const proof = liveAt(origin)?.proof;
  return proof ? { [PROOF_HEADER]: proof } : {};
}

/** The proof of that session, which a browser of the owner's sends beside
 * its cookie ([`PROOF_HEADER`]). */
export function ownerProofOf(base: string): string {
  const d = liveAt(base);
  if (!d?.proof) throw new Error(`no owner browser session proof for ${base}`);
  return d.proof;
}

/** Makes `context` a browser of the owner's on daemon `d`: it holds the
 * owner and events cookies of `d`'s session, as a login sets them, and the
 * session's proof in the storage of each of `d`'s origins, as the login's
 * page writes it.
 *
 * That session is shared by every context the worker signs in. A real
 * login's page (`GET /login`) presents every proof the storage holds to
 * `POST /api/browser-login/retire`, which ends the sessions of all but the
 * new one: had it seen the shared proof, every other context of the worker
 * would go cold. So on `/login` the init script takes the shared proof out
 * of the storage instead of writing it, and once a login wrote a proof of
 * its own (a key beginning `clax.owner_proof.`) it writes it no more. */
async function signIn(context: BrowserContext, d: { port: number; session: string | null; proof: string | null }): Promise<void> {
  if (!d.session) return;
  const cookie = (name: string, path: string, sameSite: "Lax" | "Strict", domain: string) => ({ name, value: d.session!, domain, path, httpOnly: true, sameSite });
  // For each name a test reaches this machine by: `localhost` and `127.0.0.1`.
  await context.addCookies(["localhost", "127.0.0.1"].flatMap(domain => [
    cookie(`clax_owner_${d.port}`, "/", "Lax", domain),
    ...["/api/events", "/api/stream"].map(p => cookie(`clax_events_${d.port}`, p, "Strict", domain)),
  ])).catch((e: unknown) => { if (!String(e).includes("closed")) throw e; });
  if (d.proof) {
    // Only where the origin holds no proof: a login in the context (its
    // page writes a new session's proof, under a key of its own, beside the
    // new cookie) is not undone, and never on the login's page itself.
    await context.addInitScript(({ port, proof }) => {
      if (location.port === port && (location.hostname === "localhost" || location.hostname === "127.0.0.1")) {
        try {
          if (location.pathname === "/login") {
            if (localStorage.getItem("clax.owner_proof") === proof) localStorage.removeItem("clax.owner_proof");
            return;
          }
          const held = Object.keys(localStorage).some(k => k === "clax.owner_proof" || k.startsWith("clax.owner_proof."));
          if (!held) localStorage.setItem("clax.owner_proof", proof);
        } catch { /* storage refused */ }
      }
    }, { port: String(d.port), proof: d.proof }).catch((e: unknown) => { if (!String(e).includes("closed")) throw e; });
  }
}

/** Makes `context` a browser of this machine's that holds no owner browser
 * session, for tests of what a viewer may do: it forgets every cookie, holds
 * instead a `clax_viewer` cookie the daemon `d` minted (its debug-build test
 * route) for `host` (`localhost`, or the machine's LAN address where the
 * test reaches `d` there), and is signed in to no daemon the worker starts
 * later. */
export async function asLocalViewer(context: BrowserContext, d: { base: string; token: string }, host = "localhost"): Promise<void> {
  liveContexts.delete(context);
  await context.clearCookies();
  await context.addCookies([{ name: "clax_viewer", value: await mintViewer(d), domain: host, path: "/", httpOnly: true, sameSite: "Lax" }]);
}

/** Makes `context` a stranger to every daemon of the worker: it forgets
 * every cookie, and is signed in to no daemon the worker starts later. */
export async function asStranger(context: BrowserContext): Promise<void> {
  liveContexts.delete(context);
  await context.clearCookies();
}

/** Makes `context` a browser of the owner's on the daemon at `base`, as a
 * login link opened in it would. */
export async function signInTo(context: BrowserContext, base: string): Promise<void> {
  const d = [...liveDaemons].find(x => x.base === base);
  if (!d) throw new Error(`no daemon of this worker at ${base}`);
  liveContexts.add(context);
  await signIn(context, d);
}

/** Counts `context` among the worker's browsers: it is a browser of the
 * owner's on every daemon the worker runs, now and later, as every browser
 * of this machine was before a browser had to log in. */
export async function trustContext(context: BrowserContext): Promise<void> {
  liveContexts.add(context);
  context.once("close", () => liveContexts.delete(context));
  for (const d of liveDaemons) await signIn(context, d);
}

/** Makes every context `browser` opens (`newContext`, `newPage`) a
 * browser of the owner's on the worker's daemons ([`trustContext`]): the
 * fixture's browser, and one a test launches itself. */
export function trustBrowser(browser: Browser): Browser {
  const newContext = browser.newContext.bind(browser);
  browser.newContext = (async (o?: Parameters<Browser["newContext"]>[0]) => {
    const c = await newContext(o);
    await trustContext(c);
    return c;
  }) as Browser["newContext"];
  browser.newPage = (async (o?: Parameters<Browser["newPage"]>[0]) => {
    const c = await browser.newContext(o);
    const p = await c.newPage();
    p.once("close", () => { c.close().catch(() => { /* its browser closed first */ }); });
    return p;
  }) as Browser["newPage"];
  return browser;
}

/** The browser tests' `test`. `daemon` is the worker's daemon, shared by every
 * test the worker runs: each test publishes its own artifacts under its own
 * titles and judges only those. `freshDaemon` is a daemon of the test's own,
 * for a test that judges the daemon's global state (its open streams). Every
 * browser context a test uses is a browser of the owner's on every daemon
 * the worker runs ([`trustContext`]). */
export const test = playwrightTest.extend<{ freshDaemon: Daemon }, { daemon: Daemon }>({
  context: async ({ context }, use) => {
    await trustContext(context);
    await use(context);
  },
  browser: [async ({ browser }, use) => {
    trustBrowser(browser);
    await use(browser);
  }, { scope: "worker" }],
  // Playwright reads a fixture's needs from its first parameter's pattern: none here.
  // oxlint-disable-next-line no-empty-pattern
  daemon: [async ({}, use) => {
    const d = await startDaemon();
    await use(d);
    await d.stop();
  }, { scope: "worker", timeout: 30_000 }],
  // oxlint-disable-next-line no-empty-pattern
  freshDaemon: async ({}, use) => {
    const d = await startDaemon();
    await use(d);
    await d.stop();
  },
});

/** Sleeps `ms` of real time; only for polling a state with no event of its own. */
const tick = (ms: number) => new Promise<void>(r => setTimeout(r, ms));

/** Starts a daemon on a fresh home whose config.toml is `opts.config` (`NO_KEY_CONFIG` by default);
 * with `opts.home`, on that home (which it does not create); with `opts.port` too, on that port (a restart);
 * with `opts.bind` (`0.0.0.0`: one that may invite people), on that address instead of loopback;
 * with `opts.env`, with those variables set in its environment too;
 * with `opts.args`, with those arguments added to `clax serve`'s.
 * `stop({ keepHome: true })` leaves the home; `ended()` says whether the daemon has exited.
 * It runs the binary daemon-setup.ts built, on a port the system picks, and
 * waits for its daemon.json and its health check until the daemon answers or
 * exits, bounded only by the test's (or fixture's) own timeout, so a loaded
 * machine's slow start is waited out; `opts.startMs` caps the wait for a
 * caller that wants its own bound. A start that fails says so with the end
 * of the daemon's stderr.
 * `localhost` resolves to `::1` first, and another process (a Bazel server)
 * may listen on `[::1]` at the port the system gave the daemon on
 * `127.0.0.1`, where every request to `http://localhost:<port>` would reach
 * it: a daemon on such a port is stopped and another started. */
export async function startDaemon(opts: StartOpts = {}) {
  for (let tries = 1; ; tries++) {
    const d = await startOnce(opts);
    if (d) return d;
    if (tries === 10) throw new Error("ten ports the system gave the daemon were each taken on [::1]");
  }
}

type StartOpts = { config?: string; home?: string; port?: number; startMs?: number; bind?: string; env?: Record<string, string>; args?: string[] };

/** Whether another process listens on `[::1]` at `port` (false on a machine without IPv6). */
export function v6Taken(port: number): Promise<boolean> {
  return new Promise(resolve => {
    const s = createServer();
    s.once("error", (e: NodeJS.ErrnoException) => resolve(e.code === "EADDRINUSE"));
    s.listen({ host: "::1", port, exclusive: true }, () => s.close(() => resolve(false)));
  });
}

/** One start of `startDaemon`'s daemon; null (the daemon stopped) when its port is taken on `[::1]`. */
async function startOnce(opts: StartOpts) {
  const bin = process.env.CLAX_E2E_BIN;
  if (!bin) throw new Error("CLAX_E2E_BIN is unset: run the tests through a Playwright config whose global setup builds the daemon (e2e/daemon-setup.ts)");
  // The browser tests' build of the web UI, when the global setup made one;
  // else the daemon serves web/dist (the time-to-usable suite).
  const dist = process.env.CLAX_E2E_WEB_DIST;
  const home = opts.home ?? mkdtempSync(join(tmpdir(), "clax-e2e-"));
  writeFileSync(join(home, "config.toml"), opts.config ?? NO_KEY_CONFIG);
  rmSync(join(home, "daemon.json"), { force: true });
  const child: ChildProcess = spawn(bin, ["serve", "--foreground", "--bind", opts.bind ?? "127.0.0.1", "--port", String(opts.port ?? 0), ...(dist ? ["--web-dist", dist] : []), ...(opts.args ?? [])],
    { cwd: repoRoot, env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_E2E_UNSET_KEY: "", CLAX_GALLERY__QUICKSTART: "false", ...opts.env }, stdio: ["ignore", "inherit", "pipe"] });
  // The daemon's stderr goes on to the test's, and its end is kept for a failed start.
  let stderr = "";
  child.stderr?.on("data", (b: Buffer) => { process.stderr.write(b); stderr = (stderr + b.toString()).slice(-4000); });
  const failed = (why: string) => new Error(`${why}${stderr ? `; the daemon's stderr ends:\n${stderr}` : " (no stderr)"}`);
  const exited = new Promise<void>(resolve => child.once("exit", () => resolve()));
  const infoPath = join(home, "daemon.json");
  let base = "";
  let token = "";
  let port = 0;
  const ended = () => child.exitCode !== null || child.signalCode !== null;
  let live: Live | null = null;
  const stop = async (o: { keepHome?: boolean } = {}) => {
    if (live) liveDaemons.delete(live);
    if (base && token) await fetch(`${base}/api/admin/shutdown`, { method: "POST", headers: { authorization: `Bearer ${token}` } }).catch(() => {});
    if (!ended()) {
      // The shutdown lets the daemon end on its own; it is sent SIGTERM if it
      // has not within 5 s, and SIGKILL if that has not ended it within 5 s more.
      await Promise.race([exited, tick(5000)]);
      if (!ended()) child.kill();
      await Promise.race([exited, tick(5000)]);
      if (!ended()) child.kill("SIGKILL");
      await Promise.race([exited, tick(5000)]);
    }
    if (!o.keepHome) {
      rmSync(home, { recursive: true, force: true });
      homeSessions.delete(home);
    }
  };
  try {
    const deadline = opts.startMs === undefined ? Infinity : Date.now() + opts.startMs;
    let gone = false;
    void exited.then(() => { gone = true; });
    while (!existsSync(infoPath)) {
      if (gone) throw failed(`the daemon exited (${child.exitCode ?? child.signalCode}) before it started`);
      if (Date.now() > deadline) throw failed("daemon did not start");
      await tick(10);
    }
    // The daemon may be writing the file still: read it until it parses.
    let info: { token: string; port: number } | null = null;
    while (!info) {
      try { info = JSON.parse(readFileSync(infoPath, "utf8")); } catch { if (Date.now() > deadline) throw failed("daemon.json never parsed"); await tick(10); }
    }
    token = info.token as string;
    port = info.port as number;
    // Stopped there, the daemon is told to shut down on 127.0.0.1, which only it holds.
    base = `http://127.0.0.1:${info.port}`;
    if (!opts.port && await v6Taken(port)) { await stop({ keepHome: !!opts.home }); return null; }
    base = `http://localhost:${info.port}`;
    for (;;) {
      try { if ((await fetch(`${base}/healthz`)).ok) break; } catch { /* retry */ }
      if (gone) throw failed(`the daemon exited (${child.exitCode ?? child.signalCode}) before it answered its health check`);
      if (Date.now() > deadline) throw failed("daemon did not become healthy");
      await tick(10);
    }
    // The worker's browsers are the owner's here too.
    // A home emptied and used again holds none of its sessions: one is
    // kept only while the daemon still counts it.
    const kept = opts.home ? homeSessions.get(home) : undefined;
    const stillLive = async (k: { session: string | null; proof: string | null }) => !!k.session && !!k.proof
      && (await plainFetch(`${base}/api/token`, { headers: { cookie: `clax_owner_${port}=${k.session}`, [PROOF_HEADER]: k.proof } })).ok;
    const session = kept && await stillLive(kept) ? kept : await ownerSession(base, token, port);
    homeSessions.set(home, session);
    live = { base, port, token, ...session };
    liveDaemons.add(live);
    for (const c of liveContexts) await signIn(c, live);
  } catch (e) { await stop({ keepHome: !!opts.home }); throw e; }
  return { base, token, stop, home, port, ended };
}

export async function publish(base: string, token: string, title: string, files: Record<string, string>, ifVersion?: number, id?: string) {
  const body = { title, if_version: ifVersion, files: Object.fromEntries(Object.entries(files).map(([k, v]) => [k, { content: v, encoding: "utf8" }])) };
  const url = id ? `${base}/api/artifacts/${id}/versions` : `${base}/api/artifacts`;
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}

/** Registers a live harness session; returns it. */
export async function registerSession(base: string, token: string, harness = "claude", hsid = `e2e-${Math.random().toString(36).slice(2)}`) {
  const res = await fetch(`${base}/api/sessions`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ harness, harness_session_id: hsid, cwd: "/tmp" }) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()).session as { id: string; harness: string; name?: string | null };
}

/** Publishes as `sessionId` (which then owns and watches the artifact). */
export async function publishAs(base: string, token: string, sessionId: string, title: string, files: Record<string, string>, ifVersion?: number, id?: string) {
  const body = { title, if_version: ifVersion, files: Object.fromEntries(Object.entries(files).map(([k, v]) => [k, { content: v, encoding: "utf8" }])) };
  const url = id ? `${base}/api/artifacts/${id}/versions` : `${base}/api/artifacts`;
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}`, "x-clax-session": sessionId }, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}

/** Publishes the next version of `aid` as session `sid`, carrying every file
 * forward (the index, which every publish names, from version `ifVersion`),
 * with a change note and the threads it addresses. */
export async function publishNext(base: string, token: string, sid: string, aid: string, ifVersion: number, extra: { note?: string; addresses?: string[]; files?: Record<string, string> }) {
  const index = extra.files?.["index.html"] ?? await (await fetch(`${base}/api/artifacts/${aid}/versions/${ifVersion}/files/index.html`)).text();
  const files = Object.fromEntries(Object.entries({ ...extra.files, "index.html": index }).map(([k, v]) => [k, { content: v, encoding: "utf8" }]));
  const res = await fetch(`${base}/api/artifacts/${aid}/versions`, {
    method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}`, "x-clax-session": sid },
    body: JSON.stringify({ if_version: ifVersion, files, note: extra.note, addresses: extra.addresses }),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}

/** This page's viewer's version seen mark on `aid`, read from inside the page (its cookie). */
export async function seenOf(page: Page, aid: string): Promise<number | null> {
  return page.evaluate(async a => (await (await fetch(`/api/viewers/me/seen?artifact=${a}`)).json()).seen as number | null, aid);
}

/** A JSON API call with the token (and, when given, the session header). */
export async function api(base: string, token: string, path: string, init: RequestInit & { session?: string } = {}) {
  const headers: Record<string, string> = { "content-type": "application/json", authorization: `Bearer ${token}` };
  if (init.session) headers["x-clax-session"] = init.session;
  const res = await fetch(`${base}${path}`, { ...init, headers });
  if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 ? {} : res.json();
}

/** Approves, as the owner by hand (the token, no session), what artifact
 * `id`'s current version declares that waits for approval: the
 * `pending_capabilities` of `GET /api/artifacts/<id>`, by their version and
 * declaration hash (`PUT /api/artifacts/<id>/capability-approval`).
 * Approving grants the `net` hosts and the `sample` that waited. Sends
 * nothing, and answers null, when nothing waits. */
export async function approve(base: string, token: string, id: string) {
  const { artifact } = await api(base, token, `/api/artifacts/${id}`) as { artifact: { pending_capabilities?: { version: number; declaration_sha256: string } | null } };
  const p = artifact.pending_capabilities;
  if (!p) return null;
  return api(base, token, `/api/artifacts/${id}/capability-approval`, { method: "PUT", body: JSON.stringify({ version: p.version, declaration_sha256: p.declaration_sha256 }) });
}

/** Sets session `sid`'s working record on artifact `aid` (`PUT /api/sessions/<sid>/working/<aid>`). */
export async function setWorking(base: string, token: string, sid: string, aid: string, body: { thread_ids?: string[]; message?: string }) {
  return api(base, token, `/api/sessions/${sid}/working/${aid}`, { method: "PUT", body: JSON.stringify(body) });
}

/** Moves the daemon's working clock `secs` forward and sweeps (debug builds only). */
export async function skewWorking(base: string, token: string, secs: number) {
  return api(base, token, "/api/_test/working/skew", { method: "POST", body: JSON.stringify({ secs }) });
}

/** A GET of `path` on artifact `id`'s site host (`<id>.localhost:<port>`),
 * sent to the daemon at `base` with that `Host`, as a browser on this
 * machine reaches it. */
export function siteGet(base: string, id: string, path: string): Promise<{ status: number; headers: Record<string, string | string[] | undefined>; text: string }> {
  const u = new URL(base);
  return new Promise((done, fail) => {
    const req = httpRequest({ host: u.hostname, port: u.port, path, headers: { host: `${id}.localhost:${u.port}` } }, res => {
      let text = "";
      res.setEncoding("utf8");
      res.on("data", c => { text += c; });
      res.on("end", () => done({ status: res.statusCode ?? 0, headers: res.headers, text }));
    });
    req.on("error", fail);
    req.end();
  });
}

/** The content frame showing version `n` of artifact `id` on its site
 * origin, once its handshake (`/_clax/session/start`, then the site code)
 * has landed on the page and its document has been parsed (a test that
 * reads or scrolls the page at once would otherwise race the parser). */
export async function contentFrame(page: Page, id: string, n: number): Promise<Frame> {
  const url = new RegExp(`${id}\\.localhost:\\d+/_clax/v/${n}/(#.*)?$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 10_000 }).toBe(true);
  const frame = page.frame({ url })!;
  await frame.waitForLoadState("domcontentloaded");
  return frame;
}

/** Waits until `page`'s view hears the daemon's events as they happen: its
 * stream's topics are live (`claxStreamLive`, shell/src/stream.ts), read
 * without granting a gesture. A view whose stream opens after an event
 * catches up by refetching, and a version a page published then is offered
 * as Reload rather than followed; a test of how open views react to an event
 * waits for this before causing it. */
export async function streamLive(page: Page): Promise<void> {
  await expect.poll(() => quietEval<number>(page, "window.claxStreamLive ?? 0")).toBeGreaterThan(0);
}

/** Waits until the bridge in `frame` is in comment mode (its crosshair):
 * the shell's toggle shows the mode before the frame has heard of it, and a
 * hover or click there before then goes to the page. */
export async function commentModeIn(frame: Frame): Promise<void> {
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
}

/** Opens the artifact's shell and waits for version `n` in its frame. */
export async function openArtifact(page: Page, base: string, id: string, n: number): Promise<Frame> {
  await page.goto(`${base}/a/${id}`);
  return contentFrame(page, id, n);
}

/** Creates an artifact whose index.html is `html`, declaring `capabilities`. */
export async function publishWith(base: string, token: string, title: string, html: string, capabilities: Record<string, unknown>) {
  const res = await fetch(`${base}/api/artifacts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ title, capabilities, files: { "index.html": { content: html, encoding: "utf8" } } }),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()) as { artifact: { id: string; current_version: number } };
}

/** Records every clax:* message the shell page receives; clips are reduced
 * to their byte length in the log and kept by pick ID in `claxClips`. */
export async function record(page: Page) {
  await page.addInitScript(() => {
    (window as any).claxMsgs = [];
    (window as any).claxClips = {};
    addEventListener("message", e => {
      const m = e.data;
      if (m && typeof m.type === "string" && m.type.startsWith("clax:")) {
        if (m.clipPng) (window as any).claxClips[m.pickId] = m.clipPng;
        (window as any).claxMsgs.push({ ...m, clipPng: undefined, clipBytes: m.clipPng ? m.clipPng.byteLength : 0 });
      }
    });
  });
}

export async function last(page: Page, type: string): Promise<any> {
  await expect.poll(() => page.evaluate(t => (window as any).claxMsgs.some((m: any) => m.type === t), type), { timeout: 10_000 }).toBe(true);
  return page.evaluate(t => (window as any).claxMsgs.filter((m: any) => m.type === t).at(-1), type);
}

/** Decodes a recorded clip: its size, the share of pixels that are not fully
 * transparent, and the count of pixels that differ from the top-left
 * (background) pixel, so blank clips fail whether transparent or opaque. */
export async function clipStats(page: Page, pickId: string) {
  return page.evaluate(async id => {
    const buf = (window as any).claxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    let opaque = 0;
    let ink = 0;
    for (let i = 0; i < px.length; i += 4) {
      if (px[i + 3] > 0) opaque++;
      if (Math.abs(px[i] - px[0]) + Math.abs(px[i + 1] - px[1]) + Math.abs(px[i + 2] - px[2]) + Math.abs(px[i + 3] - px[3]) > 96) ink++;
    }
    return { w: bmp.width, h: bmp.height, opaqueShare: opaque / (px.length / 4), ink };
  }, pickId);
}

export async function expectVisibleClip(page: Page, pickId: string) {
  const s = await clipStats(page, pickId);
  expect(s.w * s.h).toBeGreaterThan(0);
  expect(s.opaqueShare).toBeGreaterThan(0.01);
  expect(s.ink).toBeGreaterThan(20);
}

/** Pixels of a recorded clip that show `tint` (RGB) at `alpha` over the clip's
 * background (its top-left pixel), within a tolerance, and the rows they span. */
export async function tintStats(page: Page, pickId: string, rgb: [number, number, number], share: number) {
  return page.evaluate(async ({ id, tint, alpha }) => {
    const buf = (window as any).claxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    const want = [0, 1, 2].map(c => px[c] * (1 - alpha) + tint[c] * alpha);
    let count = 0;
    let top = Infinity;
    let bottom = -Infinity;
    for (let i = 0; i < px.length; i += 4) {
      if ([0, 1, 2].every(c => Math.abs(px[i + c] - want[c]) <= 10)) {
        count++;
        const row = Math.floor(i / 4 / bmp.width);
        top = Math.min(top, row);
        bottom = Math.max(bottom, row);
      }
    }
    return { count, top, bottom, h: bmp.height };
  }, { id: pickId, tint: rgb, alpha: share });
}

/** Pixels of a recorded clip within `tol` of `rgb` on every channel, counted
 * in rows `fromRow` to `toRow` (all rows when absent), and the rows they span. */
export async function colorStats(page: Page, pickId: string, rgb: [number, number, number], tol: number, fromRow = 0, toRow = Infinity) {
  return page.evaluate(async ({ id, want, near, from, to }) => {
    const buf = (window as any).claxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    let count = 0;
    let top = Infinity;
    let bottom = -Infinity;
    for (let i = 0; i < px.length; i += 4) {
      const row = Math.floor(i / 4 / bmp.width);
      if (row < from || row > to) continue;
      if ([0, 1, 2].every(c => Math.abs(px[i + c] - want[c]) <= near)) { count++; top = Math.min(top, row); bottom = Math.max(bottom, row); }
    }
    return { count, top, bottom };
  }, { id: pickId, want: rgb, near: tol, from: fromRow, to: Number.isFinite(toRow) ? toRow : 1e9 });
}

/** A viewer named `name`, made through the daemon's API; its public ID. */
export async function namedViewer(base: string, name: string): Promise<string> {
  // A viewer of its own: one minted for a browser of this machine's.
  const d = [...liveDaemons].find(x => x.base === base);
  if (!d) throw new Error(`no daemon of this worker at ${base}`);
  const cookie = `clax_viewer=${await mintViewer(d)}`;
  const res = await fetch(`${base}/api/viewers/me`, { method: "PUT", headers: { "content-type": "application/json", cookie }, body: JSON.stringify({ display_name: name }) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return ((await res.json()) as { viewer: { public_id: string } }).viewer.public_id;
}

/** Moves the mouse onto `loc` in steps, as a hand does. After the viewer's
 * input to the shell over the page, the shell covers the page with bands
 * until it sees the pointer move (web/shell/src/caps/gesture.ts); a hand
 * always moves before clicking, but Playwright's `click` and `hover` check
 * their target before moving, so a test reaches the element first. The
 * move starts a few pixels off the middle: a pointer already resting there
 * (from an earlier click on the same element) would otherwise send moves
 * with no movement, which leave the bands up, and the click lands on one. */
export async function reach(page: Page, loc: Locator) {
  const b = (await loc.boundingBox())!;
  const x = b.x + b.width / 2;
  const y = b.y + b.height / 2;
  await page.mouse.move(x - Math.min(6, b.width / 4), y);
  await page.mouse.move(x, y, { steps: 5 });
}

/** Creates a viewer thread on `aid` v1 anchored to `body > main > h2`, as the shell posts it (multipart); returns the thread. */
export async function postThread(base: string, aid: string, body: string, selector = "body > main > h2") {
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
  form.set("body", body);
  form.set("version", "1");
  const res = await fetch(`${base}/api/artifacts/${aid}/threads`, { method: "POST", body: form, headers: { origin: base } });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()).thread as { id: string };
}

/** Sets this page's viewer name through the people panel, as a person does. */
export async function setName(page: Page, name: string) {
  await page.getByRole("button", { name: "People and agents" }).click();
  const field = page.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
  await field.fill(name);
  await field.press("Enter");
  await page.keyboard.press("Escape");
}

/** The name field, in the open people panel: the shell text control the gesture tests click. */
export async function nameField(page: Page) {
  if (!(await page.getByRole("dialog", { name: "People and agents" }).isVisible())) await page.getByRole("button", { name: "People and agents" }).click();
  return page.getByRole("dialog", { name: "People and agents" }).getByRole("textbox", { name: "Your name" });
}

/** Waits until the shell has written this viewer's look marks (`PUT
 * /api/viewers/me/looked`), moving its clock on a second at a time past a
 * card's time in view and the marks' throttle. */
export async function lookedMarks(page: Page) {
  let done = false;
  const on = (r: Response) => { if (r.request().method() === "PUT" && new URL(r.url()).pathname === "/api/viewers/me/looked") done = true; };
  page.on("response", on);
  try {
    await expect.poll(async () => { if (!done) await advance(page, 1000); return done; }).toBe(true);
  } finally {
    page.off("response", on);
  }
}
