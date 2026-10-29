// The Clax app's scheme and host, played against a test daemon: the stand-in
// for what the app does in its WebViews when it runs Clax's core in its own
// process, so the shell's app transport (web/shell/src/transport.ts) runs end
// to end in a browser.
//
// The daemon runs with `--embed-origins` (`startEmbedDaemon`), so it serves
// as the core an app embeds does: its shell documents carry the
// `clax-embed` meta and the transport script, its content policy names the
// shell's origin, and its bridge tags name it as `data-origin`. Playwright
// answers the app's hosts in Node, so the daemon gets ordinary HTTP; each
// request is mapped as the app's scheme handler maps it to a frame, and
// carries what the core would add to that frame:
//
// - `https://app.clax.invalid/…` (the shell, `as: shell-page`): `GET` and
//   `HEAD` only (else 405). A main-frame navigation only to a shell
//   document (`/a/…`; else 403 `text/plain` `refused`, as Android's host
//   answers it without asking Dart), and a shell document or a gallery
//   document (`/`, `/inbox`, `/people`) only to one (else the answerer's
//   403 `refused`). Only `/a/` and `/_clax/` reach the core (else 404
//   `not_in_app`). `Host: localhost`; the shell document alone carries the
//   owner cookie.
// - `https://<id>.c.clax.invalid/…` (an artifact's site, `as: content`):
//   `GET` and `HEAD` only, `Host: <id>.localhost`, no credential of the
//   owner's: only the site's own cookies, which the core keeps for it
//   (`clax_embed::site_jar`): what an answer of the site sets, sent with
//   every later request of the site, and never given to the WebView.
// - Every request carries only its `Range`, `Accept` and `Service-Worker`
//   (never `If-None-Match` or `If-Modified-Since`: no WebView here takes the
//   304 they would bring), and a site's request also where it came from as
//   the WebView says (`Origin`, `Sec-Fetch-Site`, `-Mode`, `-Dest`), which
//   the site's API checks a read by; every answer loses its
//   `Access-Control-*` headers and any `Set-Cookie`.
// - No answer is a 3xx, which no WebView here takes from its host: a
//   redirect within the request's origin and directory (`…/index.html` to
//   `…/`) is followed in the core and its target answered under the
//   request's URL; a document request for `/_clax/v/<n>` answered 308, or
//   for a site's handshake (`/_clax/session`) answered 3xx to its own
//   origin, is a page that refreshes to its `Location`; any other 3xx is a
//   404.
// - Each main-frame request takes a number, which replaces the kept one
//   (with its URL); a shell document answered 200 to it gets a fresh
//   nonce, written as `<meta name="clax-nonce">` right after its `<head>`
//   (unless `nonceMeta: false`, the app over loopback), which joins that
//   number's set. The nonces are kept as the app's surface keeps them
//   (`ShellLoads`, apps/clax_app/lib/core/shell_link.dart): a document
//   that commits in the main frame (CDP's Page.frameNavigated; a
//   same-document navigation is Page.navigatedWithinDocument and changes
//   nothing) at the kept URL commits the kept number, whose set is the
//   load's candidates, and the sets of every number up to it are dropped;
//   any other commit, or a number with no set, leaves the load without a
//   channel. The first candidate the main frame presents pins the load: a
//   message under it, or, once the document has loaded with nothing
//   pinned, the document's `clax-nonce` meta, which the host reads (as
//   the app reads it with runJavaScriptReturningResult; here outside the
//   queue). Without the meta the host names the nonce itself, so its one
//   candidate is pinned at the commit. Once the load is pinned and loaded,
//   the host gives the nonce to `claxTransport.init` (without the meta)
//   and to `claxHost.init`.
//
// The channel (`window.ClaxHost`) is injected only into frames of the
// shell's origin, as Android's web message listener is; it posts to a
// binding every frame can reach, as Apple's script message handler can be,
// and the host drops a message unless it comes from the main frame on the
// shell's origin under the load's nonce. The relay answers `call`,
// `callChunk`, `callAbort`, `subscribe` and `unsubscribe`:
//
// - `call` naming `site` (an artifact ID) is a framed page's call to its own
//   site's API, which the shell relays (web/shell/src/caps/site.ts): it goes
//   to the daemon as an `as: site` frame, any method, only under
//   `/_clax/api/` but the room (else 404 `not_in_app`), with only its
//   `Content-Type` and `Accept`, `Host: <id>.localhost`, the site's
//   `Origin` (its content origin) and the site's own cookies, whose answer's
//   cookies the core keeps as a content request's.
// - Any other `call` goes to the daemon as an `as: owner` frame: the message's
//   headers (but `Host`), `Authorization: Bearer <token>` unless it names
//   one, `Host: localhost`; a held-open route (`/api/stream`, `/api/events`,
//   `/api/artifacts/{aid}/room`, `/api/artifacts/{aid}/live`, `/mcp`) or
//   `/api/admin/shutdown` answers 404 `not_in_app` unasked. A `chunked`
//   call waits for its `callChunk`s. The answer is `claxTransport.callResult
//   (nonce, id, {status, headers, body | body64})`: `body` for text or JSON
//   that is valid UTF-8, else `body64`. An answer of more than `PART` bytes
//   goes in parts as the app's relay sends it (`answerScripts`, checked
//   against the relay's own scripts in apps/clax_app/test/fixtures/
//   relay_answer_parts.json): `callResult(nonce, id, {body64, more: true})`
//   for each part but the last, each run before the next is queued, then
//   the ordinary `callResult` with the rest. `callAbort` cancels the
//   request and drops its answer, or the rest of its parts.
// - `subscribe {id, lastEventId?}` opens `GET /api/stream` as the owner,
//   from `Last-Event-ID` when it names one, and pushes each SSE block of
//   its body by `claxTransport.streamBlock(nonce, id, text)`, then the end
//   block the core's subscription closes with (`event: end` with
//   `{"reason": "stream_ended"}`, or `{"reason": "stream_refused",
//   "status": …}` for a stream the daemon refused), then
//   `claxTransport.streamEnd(nonce, id)`. `unsubscribe` closes it.
//
// Everything the host runs in the shell goes through one queue, in order,
// as the app's `runJavaScript` calls do; `hold()` stops it (an answer
// already sent but not yet run), `release()` runs what waited, into
// whichever document is loaded by then. A new load's commit closes the
// previous load's streams and drops its unanswered calls; what is queued
// already still runs, under the previous load's nonce.
import http from "node:http";
import { randomBytes } from "node:crypto";
import type { CDPSession, Page, Request, Route } from "@playwright/test";
import { expect, ownerSessionOf, startDaemon, type Daemon } from "./fixtures";

/** The app's shell origin (Android's), and an artifact's content origin. */
export const SHELL = "https://app.clax.invalid";
export const CONTENT = "https://{id}.c.clax.invalid";
export const content = (id: string) => CONTENT.replace("{id}", id);

/** A daemon that serves as the core an app with these origins embeds. */
export const startEmbedDaemon = () => startDaemon({ args: ["--embed-origins", SHELL, CONTENT] });

const SHELL_HOST = "app.clax.invalid";
const CONTENT_HOST = /^([a-z0-9]+)\.c\.clax\.invalid$/;
const PAGE_HEADERS = ["range", "accept", "service-worker"];
/** What a site's request also carries: where it came from. */
const CONTENT_HEADERS = ["origin", "sec-fetch-site", "sec-fetch-mode", "sec-fetch-dest"];
/** What a site call the shell relays carries of its own headers. */
const SITE_HEADERS = ["content-type", "accept"];
const ARTIFACT_ID = /^[0-9a-hjkmnp-tv-z]{12}$/;
const SHELL_PATHS = ["/a/", "/_clax/"];
/** The gallery's documents, which the app never loads. */
const GALLERY = new Set(["/", "/inbox", "/people"]);
const HELD_OPEN = [/^\/api\/events$/, /^\/api\/stream$/, /^\/api\/artifacts\/[^/]+\/room$/, /^\/api\/artifacts\/[^/]+\/live$/, /^\/mcp$/, /^\/api\/admin\/shutdown$/];
const TEXTUAL = /^(text\/|application\/([\w.+-]*\+)?json\b|application\/x-www-form-urlencoded\b)/i;
/** The binding `window.ClaxHost` posts through, which every frame can reach. */
export const BINDING = "claxAppChannel";

export type HostMessage = { type: string; nonce?: unknown; id?: number; [k: string]: unknown };

/** A message the host dropped, and why. */
export type Dropped = { why: "frame" | "nonce" | "malformed"; message: unknown };

/** The owner browser credential the core gives the shell document's frame
 * (it marks the request as an owner browser session in process,
 * `clax_http::local_trust::OwnerSession`): here, over HTTP, the owner
 * cookie of the session the worker's browsers hold on `d`, named for
 * `Host: localhost`. */
function ownerCookie(d: Daemon): string {
  return `clax_owner_80=${ownerSessionOf(d.base)}`;
}

export type Answer = { status: number; headers: [string, string][]; body: Buffer };

/** Sends one request to the daemon, as the router gets it from a frame. */
function send(d: Daemon, o: { method: string; path: string; headers: [string, string][]; body?: Buffer; signal?: AbortSignal }): Promise<http.IncomingMessage> {
  const headers: Record<string, string> = {};
  for (const [k, v] of o.headers) {
    const n = k.toLowerCase();
    headers[n] = n in headers ? `${headers[n]}${n === "cookie" ? "; " : ", "}${v}` : v;
  }
  if (o.body?.length) headers["content-length"] = String(o.body.length);
  else delete headers["content-length"];
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port: d.port, method: o.method, path: o.path, headers, signal: o.signal }, resolve);
    req.on("error", reject);
    req.end(o.body?.length ? o.body : undefined);
  });
}

/** The headers the daemon's HTTP server adds on the wire, which the
 * router's answer through the core (`crates/clax-embed/src/core.rs`
 * `respond`) does not carry. */
const WIRE_ONLY = new Set(["connection", "keep-alive", "transfer-encoding", "date"]);

/** The daemon's answer, with the headers the core's answer would have. */
async function readAll(res: http.IncomingMessage): Promise<Answer> {
  const chunks: Buffer[] = [];
  for await (const c of res) chunks.push(c as Buffer);
  const headers: [string, string][] = [];
  for (let i = 0; i < res.rawHeaders.length; i += 2) {
    const name = res.rawHeaders[i].toLowerCase();
    if (!WIRE_ONLY.has(name)) headers.push([name, res.rawHeaders[i + 1]]);
  }
  return { status: res.statusCode ?? 502, headers, body: Buffer.concat(chunks) };
}

const errorAnswer = (status: number, code: string, message: string): Answer =>
  ({ status, headers: [["content-type", "application/json"]], body: Buffer.from(JSON.stringify({ error: { code, message } })) });

const textAnswer = (status: number, text: string): Answer =>
  ({ status, headers: [["content-type", "text/plain; charset=utf-8"]], body: Buffer.from(text) });

function utf8(b: Buffer): string | null {
  try { return new TextDecoder("utf-8", { fatal: true }).decode(b); } catch { return null; }
}

/** A call the relay is answering. */
type Call = { nonce: string; id: number; method: string; path: string; headers: [string, string][]; body: Buffer[]; chunked: boolean; ac: AbortController; site: string | null };
/** A subscription the relay is pumping. */
type Stream = { nonce: string; id: number; lastEventId: string | null; ac: AbortController };
/** One load of the shell document: its candidates until one is pinned,
 * then its nonce. */
type Load = { url: string; candidates: Set<string>; nonce: string | null; loaded: boolean; inited: boolean; calls: Map<number, Call>; streams: Map<number, Stream> };

/** A call the relay sent, and what it answered (`answered` once it has). */
export type CallRecord = { nonce: string; id: number; method: string; path: string; chunked: boolean; site?: string; body?: string | null; answered?: { status: number; text: string | null }; aborted?: boolean };
/** A subscription the relay opened. */
export type SubscriptionRecord = { nonce: string; id: number; lastEventId: string | null; ended?: string };

export type AppHostOptions = {
  /** Write the `clax-nonce` meta into each shell document (default true);
   * without it the shell waits for `claxTransport.init`. */
  nonceMeta?: boolean;
  /** Call `claxTransport.init` (without the meta) and `claxHost.init` once
   * each load has loaded and is pinned (default true); else `init()` does. */
  autoInit?: boolean;
  /** Hold every message the shell posts until `releasePosts()`: a shell
   * that posts nothing before its load. */
  holdPosts?: boolean;
  /** Read the document's `clax-nonce` meta at its load when nothing is
   * pinned (default true), as the app does. */
  readMeta?: boolean;
  /** `claxHost.init`'s other fields (`native`, `platform`). */
  initWith?: Record<string, unknown>;
};

/** The app's side of the shell in one page: the scheme, the channel and
 * the relay, against daemon `d`. */
export class AppHost {
  /** Every message the host took (the main frame, the load's nonce), oldest first. */
  readonly accepted: HostMessage[] = [];
  readonly dropped: Dropped[] = [];
  readonly calls: CallRecord[] = [];
  readonly subscriptions: SubscriptionRecord[] = [];
  /** The nonces of the loads that were pinned, oldest first. */
  readonly loads: string[] = [];
  /** How each load was pinned (a message, or its meta read at the load), oldest first. */
  readonly pins: ("message" | "meta" | "host")[] = [];
  /** What the scheme answered, by URL, newest last. */
  readonly scheme: { url: string; status: number; mainFrame: boolean }[] = [];
  /** The expressions the host ran in the shell, oldest first. */
  readonly ran: string[] = [];
  /** The URLs of the main frame's same-document navigations, oldest first. */
  readonly sameDocument: string[] = [];
  private load: Load | null = null;
  /** The latest main-frame request's number and URL. */
  private kept: { n: number; url: string } | null = null;
  /** The nonces minted, by navigation number. */
  private sets = new Map<number, Set<string>>();
  private navigations = 0;
  private queue: { e: string; ran: () => void }[] = [];
  private held = false;
  private pumping = false;
  private idleWaiters: (() => void)[] = [];
  private stalls: { match: (c: CallRecord) => boolean; open: Promise<void> }[] = [];
  private partStalls: { match: (c: CallRecord) => boolean; open: Promise<void> }[] = [];
  private callFaults: { match: (c: CallRecord) => boolean; answer: Answer }[] = [];
  private schemeFaults: { match: (u: URL) => boolean; answer: Answer }[] = [];
  /** Each site's cookies (by artifact ID), as the core keeps them. */
  private jar = new Map<string, Map<string, string>>();
  private streamFaults: { reason: string; status?: number }[] = [];
  private cdp: Promise<CDPSession>;

  constructor(readonly page: Page, readonly d: Daemon, readonly opts: AppHostOptions = {}) {
    this.cdp = page.context().newCDPSession(page);
  }

  /** Routes the app's hosts of `page` and installs the channel; before the first `goto`. */
  async install(): Promise<this> {
    const page = this.page;
    await page.exposeBinding(BINDING, ({ frame }, text: unknown) => this.receive(frame === page.mainFrame(), frame.url(), text));
    await page.addInitScript(({ shell, binding, held }) => {
      if (location.origin !== shell) return;
      const deliver = (m: string) => (window as unknown as Record<string, (m: string) => void>)[binding](String(m));
      // With holdPosts the main frame's posts wait for claxReleasePosts().
      const waiting: string[] = [];
      let holding = held && window.top === window;
      if (holding) Object.defineProperty(window, "claxReleasePosts", { value: () => { holding = false; for (const m of waiting.splice(0)) deliver(m); } });
      Object.defineProperty(window, "ClaxHost", { value: Object.freeze({ postMessage: (m: string) => { if (holding) waiting.push(String(m)); else deliver(m); } }) });
    }, { shell: SHELL, binding: BINDING, held: this.opts.holdPosts === true });
    await page.route(u => u.hostname === SHELL_HOST || CONTENT_HOST.test(u.hostname), route => this.answerScheme(route));
    // The main frame's commits, as the WebView reports them: CDP tells a
    // document's commit (Page.frameNavigated) from a navigation within one
    // (Page.navigatedWithinDocument), which Playwright's framenavigated
    // does not.
    const cdp = await this.cdp;
    await cdp.send("Page.enable");
    let main: string | null = null;
    cdp.on("Page.frameNavigated", e => {
      if (e.frame.parentId) return;
      main = e.frame.id;
      this.committed(e.frame.url);
    });
    cdp.on("Page.navigatedWithinDocument", e => { if (e.frameId === main) this.sameDocument.push(e.url); });
    page.on("load", () => { void this.loaded(); });
    return this;
  }

  /** The load's nonce, once it is pinned. */
  get nonce(): string | null { return this.load?.nonce ?? null; }

  /** The load's nonce, once the main frame has presented it. */
  async pinned(): Promise<string> {
    await expect.poll(() => this.nonce).not.toBeNull();
    return this.nonce!;
  }

  /** Sends what the shell posted while `holdPosts` held it, and every post after. */
  async releasePosts(): Promise<void> { await this.evalNow("window.claxReleasePosts(); 0"); }

  /** The messages of `type` of this load (or of `nonce`'s), oldest first, without their nonce. */
  ofType(type: string, nonce = this.nonce): HostMessage[] {
    return this.accepted.filter(m => m.type === type && m.nonce === nonce).map(m => { const out = { ...m }; delete out.nonce; return out; });
  }

  /** The calls of this load to `path` (a string: exactly; a RegExp: matching), oldest first. */
  callsTo(method: string, path: string | RegExp, nonce = this.nonce): CallRecord[] {
    return this.calls.filter(c => c.nonce === nonce && c.method === method && (typeof path === "string" ? c.path === path : path.test(c.path)));
  }

  /** This load's subscriptions, oldest first. */
  streamsOf(nonce = this.nonce): SubscriptionRecord[] { return this.subscriptions.filter(s => s.nonce === nonce); }

  /** What waits in the queue, oldest first. */
  get waiting(): readonly string[] { return this.queue.map(q => q.e); }

  /** Stops running anything in the shell; what the relay answers waits. */
  hold(): void { this.held = true; }

  /** Runs what waited, in order, and resolves once the queue is empty. */
  async release(): Promise<void> {
    this.held = false;
    void this.pump();
    await this.idle();
  }

  /** Resolves once the queue is empty and nothing runs. */
  idle(): Promise<void> {
    if (!this.pumping && (this.held || !this.queue.length)) return Promise.resolve();
    return new Promise(r => this.idleWaiters.push(r));
  }

  /** Waits until every message the shell posted before now has reached the
   * host: the main frame posts a marker under the load's nonce, which
   * arrives after them. */
  async barrier(): Promise<void> {
    const mark = `barrier-${randomBytes(6).toString("hex")}`;
    await this.evalNow(`window.ClaxHost.postMessage(JSON.stringify({ type: "barrier", mark: ${JSON.stringify(mark)}, nonce: ${JSON.stringify(this.nonce)} }))`);
    await expect.poll(() => this.accepted.some(m => m.type === "barrier" && m.mark === mark)).toBe(true);
  }

  /** Keeps every call `match` names from the daemon until the returned
   * function is called. */
  stall(match: (c: CallRecord) => boolean): () => void {
    let open = () => {};
    const s = { match, open: new Promise<void>(r => { open = r; }) };
    this.stalls.push(s);
    return () => { this.stalls.splice(this.stalls.indexOf(s), 1); open(); };
  }

  /** Keeps every call `match` names, once the first part of its answer
   * has run, from sending the rest until the returned function is called. */
  stallParts(match: (c: CallRecord) => boolean): () => void {
    let open = () => {};
    const s = { match, open: new Promise<void>(r => { open = r; }) };
    this.partStalls.push(s);
    return () => { this.partStalls.splice(this.partStalls.indexOf(s), 1); open(); };
  }

  /** Answers the next call `match` names with `status` and `code`, unasked. */
  failNext(match: (c: CallRecord) => boolean, status: number, code = "unavailable"): void {
    this.callFaults.push({ match, answer: errorAnswer(status, code, "refused by the test") });
  }

  /** Has the core answer the next scheme request `match` names with
   * `status` and `code`, unasked. */
  failNextScheme(match: (u: URL) => boolean, status: number, code = "unavailable"): void {
    this.schemeFaults.push({ match, answer: errorAnswer(status, code, "refused by the test") });
  }

  /** Ends the next subscription at once with this end block, as the core
   * does when its stream is refused or the core is closed. */
  endNextStream(end: { reason: string; status?: number }): void { this.streamFaults.push(end); }

  /** Ends every open subscription of this load with `end`, as the core's
   * subscription ends. */
  endStreams(end: { reason: string; status?: number }): void {
    const load = this.load;
    if (!load) return;
    for (const s of [...load.streams.values()]) this.endStream(load, s, end);
  }

  /** Gives `claxTransport.init` (without the meta) and `claxHost.init` this
   * load's nonce, once the shell's host module has loaded (app mode). */
  async init(): Promise<void> {
    const load = this.load;
    if (!load) throw new Error("no load committed");
    const nonce = await this.pinned();
    if (this.load !== load || load.inited) return;
    load.inited = true;
    if (this.opts.nonceMeta === false) void this.run(`window.claxTransport.init(${JSON.stringify(nonce)})`);
    if (!new URL(load.url).searchParams.has("app")) return;
    await expect.poll(() => this.evalNow<string>("typeof window.claxHost")).toBe("object");
    if (this.load !== load) return;
    void this.run(`window.claxHost.init(${JSON.stringify({ nonce, ...this.opts.initWith })})`);
  }

  /** Runs `expression` in the shell now, outside the queue, with no user gesture. */
  async evalNow<T>(expression: string): Promise<T> {
    const r = await (await this.cdp).send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: false });
    if (r.exceptionDetails) throw new Error(`${expression}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value as T;
  }

  // ---- the scheme ----

  private async answerScheme(route: Route): Promise<void> {
    const req = route.request();
    const url = new URL(req.url());
    const shell = url.hostname === SHELL_HOST;
    const mainNav = req.isNavigationRequest() && frameOf(req) === this.page.mainFrame();
    const document = shell && url.pathname.startsWith("/a/");
    // Every main-frame request the answerer gets takes the next number and
    // replaces the kept one, as Android's host numbers each
    // isForMainFrame() request it hands Dart (it refuses one for the
    // shell's host outside /a/ itself, unnumbered); a nonce is minted only
    // for a shell document answered 200.
    const numbered = mainNav && (!shell || document) ? ++this.navigations : null;
    if (numbered !== null) this.kept = { n: numbered, url: req.url() };
    const answer = await this.schemeAnswer(req, url, shell, mainNav, document);
    const headers = answer.headers.filter(([k]) => !k.toLowerCase().startsWith("access-control-"));
    let body = answer.body;
    if (numbered !== null && document && answer.status === 200) {
      const nonce = randomBytes(18).toString("base64url");
      const html = body.toString("utf8");
      // A document with no <head> gets no nonce, as the app's answerer.
      if (html.includes("<head>")) {
        let set = this.sets.get(numbered);
        if (!set) this.sets.set(numbered, set = new Set());
        set.add(nonce);
        if (this.opts.nonceMeta !== false) body = Buffer.from(html.replace("<head>", `<head><meta name="clax-nonce" content="${nonce}">`));
      }
    }
    this.scheme.push({ url: req.url(), status: answer.status, mainFrame: mainNav });
    const out: Record<string, string> = {};
    for (const [k, v] of headers) {
      const n = k.toLowerCase();
      if (n === "content-length" || n === "set-cookie") continue;
      out[n] = n in out ? `${out[n]}${n === "set-cookie" ? "\n" : ", "}${v}` : v;
    }
    await route.fulfill({ status: answer.status, headers: out, body: req.method() === "HEAD" ? Buffer.alloc(0) : body }).catch(() => {});
  }

  private async schemeAnswer(req: Request, url: URL, shell: boolean, mainNav: boolean, document: boolean): Promise<Answer> {
    if (req.method() !== "GET" && req.method() !== "HEAD") return textAnswer(405, "the app's scheme answers GET and HEAD only");
    if (shell && mainNav && !document) return { status: 403, headers: [["content-type", "text/plain"]], body: Buffer.from("refused") };
    if (shell && !mainNav && (document || GALLERY.has(url.pathname))) return textAnswer(403, "refused");
    const id = shell ? null : CONTENT_HOST.exec(url.hostname)![1];
    if (shell && !SHELL_PATHS.some(p => url.pathname.startsWith(p))) return errorAnswer(404, "not_in_app", `${url.pathname} is not served in the app`);
    const all = await req.allHeaders();
    const headers: [string, string][] = [...PAGE_HEADERS, ...(id === null ? [] : CONTENT_HEADERS)].filter(h => all[h] !== undefined).map(h => [h, all[h]]);
    headers.push(["host", id ? `${id}.localhost` : "localhost"]);
    // The core adds the owner cookie for the shell document's path alone,
    // and a site's own cookies to its requests.
    const asked = (target: URL): [string, string][] => {
      if (target.pathname.startsWith("/a/") && shell) return [...headers, ["cookie", ownerCookie(this.d)]];
      const kept = id === null ? undefined : this.jar.get(id);
      return kept?.size ? [...headers, ["cookie", [...kept].map(([k, v]) => `${k}=${v}`).join("; ")]] : headers;
    };
    // No WebView here takes a 3xx from its host, so none leaves the answerer
    // (plan Task 15, `SchemeAnswerer`): one whose Location keeps the
    // request's origin and directory is followed in the core, at most
    // MAX_HOPS times, and its target's answer is the request's; a document
    // request for /v/<n> answered 308 is a page that refreshes to its
    // Location; any other 3xx is a 404.
    let target = url;
    for (let hop = 0; ; hop++) {
      let answer: Answer;
      const fault = this.schemeFaults.findIndex(f => f.match(target));
      if (fault >= 0) answer = this.schemeFaults.splice(fault, 1)[0].answer;
      else try {
        answer = await readAll(await send(this.d, { method: req.method(), path: target.pathname + target.search, headers: asked(target) }));
      } catch (e) {
        return textAnswer(502, `the core did not answer: ${String(e)}`);
      }
      if (id !== null) this.keep(id, answer);
      if (answer.status < 300 || answer.status >= 400) return answer;
      const location = answer.headers.find(([k]) => k === "location")?.[1] ?? null;
      const next = location === null ? null : resolveLocation(target, location);
      if (next && hop < MAX_HOPS && sameBase(url, next)) { target = next; continue; }
      const asDocument = location !== null && hop === 0 && (mainNav || /text\/html/i.test(all.accept ?? ""));
      const versionRoot = answer.status === 308 && /^\/_clax\/v\/[0-9]+$/.test(url.pathname);
      const handshake = !shell && url.pathname === "/_clax/session" && next !== null && next.origin === url.origin;
      if (asDocument && (versionRoot || handshake)) {
        return { status: 200, headers: [["content-type", "text/html; charset=utf-8"]], body: Buffer.from(`<meta http-equiv="refresh" content="0;url=${escapeAttr(location!)}">`) };
      }
      return textAnswer(404, "not found");
    }
  }

  /** Keeps the cookies an answer of site `id` sets, and drops those it
   * ends (`Max-Age` 0 or less, or an empty value). */
  private keep(id: string, answer: Answer): void {
    for (const [k, v] of answer.headers) {
      if (k.toLowerCase() !== "set-cookie") continue;
      const [pair, ...attrs] = v.split(";");
      const eq = pair.indexOf("=");
      if (eq <= 0) continue;
      const name = pair.slice(0, eq).trim();
      const value = pair.slice(eq + 1).trim();
      const ended = !value || attrs.some(a => /^\s*max-age\s*=\s*(-|0+\s*$)/i.test(a));
      let kept = this.jar.get(id);
      if (!kept) this.jar.set(id, kept = new Map());
      if (ended) kept.delete(name);
      else kept.set(name, value);
    }
  }

  // ---- loads ----

  /** A document committed in the main frame (never a same-document
   * navigation): the kept number when its URL is the kept one's, whose set
   * is the new load's candidates; the sets of every number up to it are
   * dropped. A commit with no number, or a number with no set, leaves the
   * load without a channel (spec §4.4). The previous load's calls and
   * streams end. */
  private committed(url: string): void {
    const k = this.kept;
    const n = k && k.url === url && new URL(url).origin === SHELL ? k.n : null;
    if (n !== null) this.kept = null;
    const candidates = n === null ? undefined : this.sets.get(n);
    if (n !== null) for (const m of [...this.sets.keys()]) if (m <= n) this.sets.delete(m);
    if (!candidates?.size) return this.switchTo(null);
    const load: Load = { url, candidates, nonce: null, loaded: false, inited: false, calls: new Map(), streams: new Map() };
    this.switchTo(load);
    if (this.opts.nonceMeta === false && candidates.size === 1) this.pin(load, [...candidates][0], "host");
  }

  /** Whether a message under `nonce` is `load`'s: its pinned nonce, or
   * before the pin one of its candidates, which pins it. */
  private present(load: Load, nonce: unknown, by: "message" | "meta"): boolean {
    if (typeof nonce !== "string") return false;
    if (load.nonce !== null) return nonce === load.nonce;
    if (!load.candidates.has(nonce)) return false;
    this.pin(load, nonce, by);
    return true;
  }

  private pin(load: Load, nonce: string, by: "message" | "meta" | "host"): void {
    load.nonce = nonce;
    load.candidates = new Set();
    this.loads.push(nonce);
    this.pins.push(by);
    if (load.loaded) this.autoInit(load);
  }

  private switchTo(load: Load | null): void {
    const old = this.load;
    this.load = load;
    if (old) {
      for (const c of old.calls.values()) c.ac.abort();
      old.calls.clear();
      for (const s of old.streams.values()) {
        s.ac.abort();
        this.subscriptions.find(r => r.nonce === s.nonce && r.id === s.id)!.ended ??= "unloaded";
      }
      old.streams.clear();
    }
  }

  /** The main frame's document loaded: with nothing pinned the host reads
   * its `clax-nonce` meta and presents it; once pinned, it inits. */
  private async loaded(): Promise<void> {
    const load = this.load;
    if (!load) return;
    load.loaded = true;
    if (load.nonce === null && this.opts.readMeta !== false) {
      const meta = await this.evalNow<string>(`document.querySelector('meta[name="clax-nonce"]')?.content ?? ""`).catch(() => "");
      if (this.load === load) this.present(load, meta, "meta");
    }
    if (load.nonce !== null) this.autoInit(load);
  }

  private autoInit(load: Load): void {
    if (this.opts.autoInit === false || load.inited || this.load !== load) return;
    void this.init().catch(() => { /* the document went away */ });
  }

  // ---- the channel ----

  private receive(mainFrame: boolean, frameUrl: string, text: unknown): void {
    let m: HostMessage;
    try {
      m = JSON.parse(String(text));
      if (!m || typeof m !== "object" || typeof m.type !== "string") throw new Error("not a message");
    } catch {
      this.dropped.push({ why: "malformed", message: text });
      return;
    }
    if (!mainFrame || new URL(frameUrl).origin !== SHELL) { this.dropped.push({ why: "frame", message: m }); return; }
    const load = this.load;
    if (!load || !this.present(load, m.nonce, "message")) { this.dropped.push({ why: "nonce", message: m }); return; }
    this.accepted.push(m);
    const nonce = load.nonce!;
    const id = Number(m.id);
    switch (m.type) {
      case "call": {
        const headers = Array.isArray(m.headers) ? (m.headers as [string, string][]).filter(([k]) => k.toLowerCase() !== "host") : [];
        const body = typeof m.body64 === "string" ? Buffer.from(m.body64, "base64") : typeof m.body === "string" ? Buffer.from(m.body) : Buffer.alloc(0);
        if (m.site !== undefined && (typeof m.site !== "string" || !ARTIFACT_ID.test(m.site))) return;
        const site = typeof m.site === "string" ? m.site : null;
        const call: Call = { nonce, id, method: String(m.method), path: String(m.path), headers, body: m.chunked ? [] : [body], chunked: m.chunked === true, ac: new AbortController(), site };
        load.calls.set(id, call);
        this.calls.push({ nonce: call.nonce, id, method: call.method, path: call.path, chunked: call.chunked, ...(site ? { site } : {}) });
        if (!call.chunked) void this.answerCall(load, call);
        return;
      }
      case "callChunk": {
        const call = load.calls.get(id);
        if (!call || !call.chunked || m.n !== call.body.length) return;
        call.body.push(Buffer.from(String(m.body64), "base64"));
        if (m.last === true) void this.answerCall(load, call);
        return;
      }
      case "callAbort": {
        const call = load.calls.get(id);
        if (!call) return;
        load.calls.delete(id);
        call.ac.abort();
        this.recordOf(call).aborted = true;
        return;
      }
      case "subscribe":
        void this.subscribe(load, id, typeof m.lastEventId === "string" ? m.lastEventId : null);
        return;
      case "unsubscribe": {
        const s = load.streams.get(id);
        if (!s) return;
        load.streams.delete(id);
        s.ac.abort();
        this.subscriptions.find(r => r.nonce === s.nonce && r.id === s.id)!.ended = "unsubscribed";
        return;
      }
    }
  }

  private recordOf(c: Call): CallRecord {
    for (let i = this.calls.length - 1; i >= 0; i--) if (this.calls[i].nonce === c.nonce && this.calls[i].id === c.id) return this.calls[i];
    throw new Error(`no record of call ${c.id}`);
  }

  private async answerCall(load: Load, call: Call): Promise<void> {
    const record = this.recordOf(call);
    const sent = Buffer.concat(call.body);
    record.body = sent.length <= 1 << 16 ? utf8(sent) : null;
    const stalled = this.stalls.filter(s => s.match(record));
    if (stalled.length) {
      const aborted = new Promise<void>(r => call.ac.signal.addEventListener("abort", () => r(), { once: true }));
      await Promise.race([Promise.all(stalled.map(s => s.open)), aborted]);
    }
    if (load.calls.get(call.id) !== call) return;
    const path = call.path.split(/[?#]/)[0];
    const fault = this.callFaults.findIndex(f => f.match(record));
    let answer: Answer;
    if (fault >= 0) answer = this.callFaults.splice(fault, 1)[0].answer;
    else if (call.site !== null) {
      const site = call.site;
      const admitted = path.startsWith("/_clax/api/") && path !== "/_clax/api/room" && !path.split("/").some(g => g === "." || g === "..");
      if (!admitted) answer = errorAnswer(404, "not_in_app", `${path} is not served in the app`);
      else {
        const headers: [string, string][] = call.headers.filter(([k]) => SITE_HEADERS.includes(k.toLowerCase()));
        headers.push(["host", `${site}.localhost`], ["origin", content(site)]);
        const kept = this.jar.get(site);
        if (kept?.size) headers.push(["cookie", [...kept].map(([k, v]) => `${k}=${v}`).join("; ")]);
        try {
          answer = await readAll(await send(this.d, { method: call.method, path: call.path, headers, body: sent, signal: call.ac.signal }));
        } catch (e) {
          if (call.ac.signal.aborted) return;
          answer = errorAnswer(502, "response_failed", String(e));
        }
        this.keep(site, answer);
        answer = { ...answer, headers: answer.headers.filter(([k]) => k.toLowerCase() !== "set-cookie") };
      }
    } else if (HELD_OPEN.some(r => r.test(path))) answer = errorAnswer(404, "not_in_app", `${path} is not served in the app`);
    else {
      const headers = [...call.headers];
      if (!headers.some(([k]) => k.toLowerCase() === "authorization")) headers.push(["authorization", `Bearer ${this.d.token}`]);
      headers.push(["host", "localhost"]);
      try {
        answer = await readAll(await send(this.d, { method: call.method, path: call.path, headers, body: sent, signal: call.ac.signal }));
      } catch (e) {
        if (call.ac.signal.aborted) return;
        answer = errorAnswer(502, "response_failed", String(e));
      }
    }
    if (load.calls.get(call.id) !== call) return;
    const type = answer.headers.find(([k]) => k === "content-type")?.[1] ?? "";
    record.answered = { status: answer.status, text: TEXTUAL.test(type) ? utf8(answer.body) : null };
    const scripts = answerScripts(call.nonce, call.id, answer);
    const last = scripts.pop()!;
    for (const [i, js] of scripts.entries()) {
      // Each part runs before the next goes, as the relay awaits it.
      await this.run(js);
      if (load.calls.get(call.id) !== call) return;
      const paused = i === 0 ? this.partStalls.filter(s => s.match(record)) : [];
      if (paused.length) {
        const aborted = new Promise<void>(r => call.ac.signal.addEventListener("abort", () => r(), { once: true }));
        await Promise.race([Promise.all(paused.map(s => s.open)), aborted]);
        if (load.calls.get(call.id) !== call) return;
      }
    }
    load.calls.delete(call.id);
    void this.run(last);
  }

  private async subscribe(load: Load, id: number, lastEventId: string | null): Promise<void> {
    const s: Stream = { nonce: load.nonce!, id, lastEventId, ac: new AbortController() };
    load.streams.set(id, s);
    this.subscriptions.push({ nonce: s.nonce, id, lastEventId });
    const fault = this.streamFaults.shift();
    if (fault) return this.endStream(load, s, fault);
    const headers: [string, string][] = [["authorization", `Bearer ${this.d.token}`], ["host", "localhost"], ["accept", "text/event-stream"]];
    if (lastEventId) headers.push(["last-event-id", lastEventId]);
    let res: http.IncomingMessage;
    try {
      res = await send(this.d, { method: "GET", path: "/api/stream", headers, signal: s.ac.signal });
    } catch {
      return this.endStream(load, s, { reason: "stream_ended" });
    }
    if ((res.statusCode ?? 500) >= 300) {
      res.resume();
      return this.endStream(load, s, { reason: "stream_refused", status: res.statusCode });
    }
    // One message per SSE block, its blank line included, as the core
    // posts each block the hub's connection yields (spec §2.4).
    res.setEncoding("utf8");
    const sse = new SseBlocks();
    const push = (block: string) => {
      if (load.streams.get(id) === s) void this.run(`window.claxTransport.streamBlock(${JSON.stringify(s.nonce)}, ${id}, ${JSON.stringify(block)})`);
    };
    res.on("data", (text: string) => { for (const b of sse.push(text)) push(b); });
    const ended = () => {
      const rest = sse.rest();
      if (rest) push(rest);
      this.endStream(load, s, { reason: "stream_ended" });
    };
    res.on("end", ended);
    res.on("error", ended);
  }

  /** Ends stream `s` with its end block, unless it ended already. */
  private endStream(load: Load, s: Stream, end: { reason: string; status?: number }): void {
    if (load.streams.get(s.id) !== s) return;
    load.streams.delete(s.id);
    s.ac.abort();
    this.subscriptions.find(r => r.nonce === s.nonce && r.id === s.id)!.ended = end.reason;
    void this.run(`window.claxTransport.streamBlock(${JSON.stringify(s.nonce)}, ${s.id}, ${JSON.stringify(`event: end\ndata: ${JSON.stringify(end)}\n\n`)})`);
    void this.run(`window.claxTransport.streamEnd(${JSON.stringify(s.nonce)}, ${s.id})`);
  }

  // ---- the queue ----

  /** Queues `expression` to run in the shell, as `runJavaScript` does;
   * resolves once it has run (or the document could not run it). */
  private run(expression: string): Promise<void> {
    return new Promise(ran => {
      this.queue.push({ e: expression, ran });
      void this.pump();
    });
  }

  private async pump(): Promise<void> {
    if (this.pumping) return;
    this.pumping = true;
    try {
      while (!this.held && this.queue.length) {
        const { e, ran } = this.queue.shift()!;
        this.ran.push(e);
        // A document that went away takes nothing, as runJavaScript into it.
        await this.evalNow(e).catch(() => {});
        ran();
      }
    } finally {
      this.pumping = false;
      if (this.held || !this.queue.length) for (const w of this.idleWaiters.splice(0)) w();
    }
  }
}

/** The most body bytes one part of an answer carries, as the app's relay
 * (`ChannelRelay.partBytes`): a multiple of 3, so the parts' base64 texts
 * join, and exactly 1 MiB of base64. */
export const PART = 3 << 18;

/** The scripts the app's relay runs to answer call `id` of the load under
 * `nonce` with `a`, byte for byte as ChannelRelay writes them: one
 * `callResult` with `body` (text or JSON that is valid UTF-8) or `body64`,
 * or for a body of more than `PART` bytes a `{body64, more: true}` part for
 * each `PART` bytes but the last, then the `callResult` with the rest as
 * `body64`. */
export function answerScripts(nonce: string, id: number, a: Answer): string[] {
  const call = (r: object) => `window.claxTransport.callResult(${JSON.stringify(nonce)}, ${id}, ${JSON.stringify(r)})`;
  const head = { status: a.status, headers: a.headers };
  if (a.body.length <= PART) {
    const type = a.headers.find(([k]) => k.toLowerCase() === "content-type")?.[1] ?? "";
    const text = TEXTUAL.test(type) ? utf8(a.body) : null;
    return [call({ ...head, ...(text !== null ? { body: text } : { body64: a.body.toString("base64") }) })];
  }
  const out: string[] = [];
  let at = 0;
  for (; a.body.length - at > PART; at += PART) out.push(call({ body64: a.body.subarray(at, at + PART).toString("base64"), more: true }));
  return [...out, call({ ...head, body64: a.body.subarray(at).toString("base64") })];
}

/** An SSE body cut into its blocks, each with its blank line, however
 * the body's chunks fall. */
export class SseBlocks {
  private pending = "";
  /** The blocks `text` completes, oldest first. */
  push(text: string): string[] {
    this.pending += text;
    const out: string[] = [];
    for (let at = this.pending.indexOf("\n\n"); at >= 0; at = this.pending.indexOf("\n\n")) {
      out.push(this.pending.slice(0, at + 2));
      this.pending = this.pending.slice(at + 2);
    }
    return out;
  }
  /** What no blank line has ended yet; taken. */
  rest(): string {
    const r = this.pending;
    this.pending = "";
    return r;
  }
}

/** The most redirects the answerer follows for one request. */
const MAX_HOPS = 5;

function resolveLocation(base: URL, location: string): URL | null {
  try { return new URL(location, base); } catch { return null; }
}

/** Whether `to` is `from`'s origin and directory (its path up to its last
 * `/`), so the target's relative links resolve as the request's. */
function sameBase(from: URL, to: URL): boolean {
  const dir = (p: string) => p.slice(0, p.lastIndexOf("/") + 1);
  return from.origin === to.origin && dir(from.pathname) === dir(to.pathname);
}

const escapeAttr = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function frameOf(req: Request) {
  try { return req.frame(); } catch { return null; }
}

/** Routes the app's hosts of `page` to daemon `d` and plays the app's host
 * there; before the first `goto`. */
export async function appHost(page: Page, d: Daemon, opts: AppHostOptions = {}): Promise<AppHost> {
  return new AppHost(page, d, opts).install();
}
