// The diagnostics recorder (spec 2026-10-29-feedback-loop-design §4.4,
// §5.2): a ring buffer of a page's recent trouble — `console.warn` and
// `console.error`, uncaught errors, unhandled rejections and failed
// requests — each line scrubbed (§5.3) before it is kept. Nothing leaves
// the page from here; a report sends the lines it is given.
import type { DiagLine } from "./types";
import { cut, scrub, scrubUrl, wellFormed } from "./scrub";

export type { DiagLine } from "./types";

/** The most lines a report's diagnostics hold, and the recorder's default size. */
export const MAX_DIAG_LINES = 200;
/** The largest diagnostics part, in bytes. */
export const MAX_DIAG_BYTES = 256 * 1024;
/** The longest `text` the recorder keeps, in UTF-16 code units; a longer one ends in `…`. */
export const MAX_TEXT_CHARS = 1000;
/** The most of a logged value the recorder reads before scrubbing it, in UTF-16 code units. */
export const MAX_READ_CHARS = 8 * MAX_TEXT_CHARS;

/** The response headers a failed request's ID is read from, in order. */
export const REQUEST_ID_HEADERS = ["x-request-id", "cf-ray", "x-clax-request"];

/** A window's recorder. */
export interface Recorder {
  /** The lines kept, oldest first (a copy). */
  lines(): DiagLine[];
  /** How many lines are kept. */
  count(): number;
  /** Restores what the recorder wrapped, stops listening and forgets the
   * lines; the next `installDiagnostics` on the window starts afresh. */
  stop(): void;
}

export interface DiagnosticsOptions {
  /** Milliseconds since the Unix epoch. Default: `Date.now`. */
  now?: () => number;
  /** The most lines kept. Default and ceiling: `MAX_DIAG_LINES`. */
  max?: number;
  /** A Clax instance's base URL: the SDK's own requests to it (its
   * `/_clax/` files and its `/api/queues/<qid>/reporter…` routes) are not
   * recorded; the page's other requests to it are. A signed request (one
   * with a `Clax-Signature` header) is never recorded, wherever it goes. */
  instance?: string;
}

/** What the recorder touches on a window. */
interface DiagWindow {
  console: Console;
  fetch?: typeof fetch;
  XMLHttpRequest?: typeof XMLHttpRequest;
  location: { href: string };
  addEventListener: Window["addEventListener"];
  removeEventListener: Window["removeEventListener"];
}

interface Installed extends Recorder {
  ignore(origin: string): void;
}

/** The paths of an instance's that are the SDK's own traffic. */
const SDK_PATH = /^\/(_clax\/|api\/queues\/[^/]+\/reporter)/;

/** Where a window's recorder is kept, shared by every copy of the SDK on the page. */
const KEY = Symbol.for("clax.diagnostics");

/** Starts recording `win`'s diagnostics, or returns the recorder already
 * recording it (whose options stay; a new `instance` is added to the
 * origins it skips).
 *
 * It wraps `console.warn` and `console.error` (each still logs), listens to
 * `error` and `unhandledrejection`, and wraps `fetch` and `XMLHttpRequest`
 * to record an answer of status 400 or more, or no answer, as `"{METHOD}
 * {path} → {status | failed} in {ms} ms [req {id}]"`: the path is the
 * URL's path and query for the page's own origin, and its origin, path
 * and query for another (never its fragment), with every query value
 * replaced (`scrubUrl`); the request ID is the first of
 * `REQUEST_ID_HEADERS` the answer carries. A request cancelled by its
 * caller is not recorded.
 *
 * Each line's text is read to at most `MAX_READ_CHARS`, made well-formed
 * (`wellFormed`), scrubbed, and cut to `MAX_TEXT_CHARS`. */
export function installDiagnostics(win: Window, opts: DiagnosticsOptions = {}): Recorder {
  const w = win as unknown as DiagWindow & { [KEY]?: Installed };
  const existing = w[KEY];
  if (existing) {
    if (opts.instance) existing.ignore(opts.instance);
    return existing;
  }
  const now = opts.now ?? Date.now;
  const max = Math.max(1, Math.min(opts.max ?? MAX_DIAG_LINES, MAX_DIAG_LINES));
  const ignored = new Set<string>();
  const ignore = (u: string) => {
    const o = originOf(u, w.location.href);
    if (o) ignored.add(o);
  };
  if (opts.instance) ignore(opts.instance);
  let kept: DiagLine[] = [];
  let stopped = false;

  const record = (level: DiagLine["level"], kind: DiagLine["kind"], text: string) => {
    if (stopped) return;
    try {
      const t = cut(scrub(wellFormed(cut(text, MAX_READ_CHARS))), MAX_TEXT_CHARS);
      kept.push({ at: new Date(now()).toISOString(), level, kind, text: t });
      if (kept.length > max) kept = kept.slice(kept.length - max);
    } catch {
      // Recording never breaks the page.
    }
  };

  /** Whether a request to `url` is the SDK's own: signed, or to an
   * instance's overlay files or reporter routes. Any other request to the
   * instance is the page's (on an instance that is its own product, every
   * request of the page goes there). */
  const skipped = (url: string, signed: boolean) => {
    if (signed) return true;
    try {
      const u = new URL(url, w.location.href);
      return ignored.has(u.origin) && SDK_PATH.test(u.pathname);
    } catch {
      return false;
    }
  };
  /** `url` as a line names it. */
  const target = (url: string) => {
    try {
      const u = new URL(url, w.location.href);
      const own = u.origin === new URL(w.location.href).origin;
      return scrubUrl(own ? u.pathname + u.search : u.origin + u.pathname + u.search);
    } catch {
      return scrubUrl(url);
    }
  };
  const failed = (method: string, url: string, started: number, status: number, id: string | null) => {
    const ms = Math.max(0, Math.round(now() - started));
    const what = status === 0 ? "failed" : String(status);
    record(status === 0 || status >= 500 ? "error" : "warn", "request",
      `${method.toUpperCase()} ${target(url)} → ${what} in ${ms} ms${id ? ` [req ${id}]` : ""}`);
  };

  const con = w.console;
  const origWarn = con.warn;
  const origError = con.error;
  con.warn = function (...args: unknown[]) {
    origWarn.apply(this, args);
    record("warn", "console", formatArgs(args));
  };
  con.error = function (...args: unknown[]) {
    origError.apply(this, args);
    record("error", "console", formatArgs(args));
  };
  const wrappedWarn = con.warn;
  const wrappedError = con.error;

  const onError = (ev: Event) => {
    const e = ev as ErrorEvent;
    const where = e.filename ? ` (${scrubUrl(e.filename)}:${e.lineno}:${e.colno})` : "";
    record("error", "error", e.error !== undefined && e.error !== null ? describe(e.error) : `${e.message ?? "error"}${where}`);
  };
  const onRejection = (ev: Event) => {
    record("error", "rejection", describe((ev as PromiseRejectionEvent).reason));
  };
  w.addEventListener("error", onError);
  w.addEventListener("unhandledrejection", onRejection);

  const origFetch = w.fetch;
  let wrappedFetch: typeof fetch | undefined;
  if (origFetch) {
    wrappedFetch = async function (this: unknown, input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
      // A Request of any realm (an iframe's too) has a string `url`.
      const req = typeof input === "object" && !(input instanceof URL) && typeof (input as Request).url === "string" ? input as Request : null;
      const url = req ? req.url : String(input);
      const method = init?.method ?? req?.method ?? "GET";
      const signed = hasHeader(init?.headers, "clax-signature") || (req !== null && req.headers.has("clax-signature"));
      if (skipped(url, signed)) return origFetch.call(w, input, init);
      const started = now();
      let res: Response;
      try {
        res = await origFetch.call(w, input, init);
      } catch (e) {
        if ((e as { name?: unknown } | null)?.name !== "AbortError") failed(method, url, started, 0, null);
        throw e;
      }
      if (res.status >= 400) failed(method, url, started, res.status, requestId(h => res.headers.get(h)));
      return res;
    } as typeof fetch;
    w.fetch = wrappedFetch;
  }

  const proto = w.XMLHttpRequest?.prototype;
  const origOpen = proto?.open;
  const origSetHeader = proto?.setRequestHeader;
  const origSend = proto?.send;
  /** An XMLHttpRequest's current request; `watched` while one is in flight
   * that is recorded. Its listeners are added once, on its first recorded
   * send, and read whichever request it carries then. */
  interface XhrState { method: string; url: string; signed: boolean; watched: boolean; started: number; broke: boolean; listening: boolean }
  const xhrs = new WeakMap<XMLHttpRequest, XhrState>();
  type Proto = typeof XMLHttpRequest.prototype;
  let ourOpen: Proto["open"] | undefined;
  let ourSetHeader: Proto["setRequestHeader"] | undefined;
  let ourSend: Proto["send"] | undefined;
  if (proto && origOpen && origSetHeader && origSend) {
    ourOpen = function (this: XMLHttpRequest, ...args: unknown[]) {
      const next = { method: String(args[0]), url: String(args[1]), signed: false, watched: false, started: 0, broke: false };
      const x = xhrs.get(this);
      if (x) Object.assign(x, next);
      else xhrs.set(this, { ...next, listening: false });
      return (origOpen as (...a: unknown[]) => void).apply(this, args);
    } as Proto["open"];
    ourSetHeader = function (this: XMLHttpRequest, name: string, value: string) {
      const x = xhrs.get(this);
      if (x && name.toLowerCase() === "clax-signature") x.signed = true;
      return origSetHeader.call(this, name, value);
    };
    ourSend = function (this: XMLHttpRequest, body?: Document | XMLHttpRequestBodyInit | null) {
      const x = xhrs.get(this);
      if (x) {
        x.watched = !skipped(x.url, x.signed);
        x.started = now();
        x.broke = false;
        if (x.watched && !x.listening) {
          x.listening = true;
          const onBroke = () => { x.broke = true; };
          this.addEventListener("error", onBroke);
          this.addEventListener("timeout", onBroke);
          this.addEventListener("loadend", () => {
            if (!x.watched) return;
            x.watched = false;
            if (x.broke) failed(x.method, x.url, x.started, 0, null);
            else if (this.status >= 400) failed(x.method, x.url, x.started, this.status, requestId(h => this.getResponseHeader(h)));
          });
        }
      }
      return origSend.call(this, body);
    };
    proto.open = ourOpen;
    proto.setRequestHeader = ourSetHeader;
    proto.send = ourSend;
  }

  const recorder: Installed = {
    lines: () => kept.slice(),
    count: () => kept.length,
    ignore,
    stop() {
      // A wrapper another script put over one of ours stays, calling
      // through ours, which then records nothing.
      stopped = true;
      if (con.warn === wrappedWarn) con.warn = origWarn;
      if (con.error === wrappedError) con.error = origError;
      w.removeEventListener("error", onError);
      w.removeEventListener("unhandledrejection", onRejection);
      if (wrappedFetch && w.fetch === wrappedFetch) w.fetch = origFetch;
      if (proto && origOpen && origSetHeader && origSend) {
        if (proto.open === ourOpen) proto.open = origOpen;
        if (proto.setRequestHeader === ourSetHeader) proto.setRequestHeader = origSetHeader;
        if (proto.send === ourSend) proto.send = origSend;
      }
      kept = [];
      if (w[KEY] === recorder) delete w[KEY];
    },
  };
  w[KEY] = recorder;
  return recorder;
}

/** The diagnostics part a report sends: `lines` as JSON lines, the newest
 * that fit in `MAX_DIAG_LINES` lines and `MAX_DIAG_BYTES` bytes. */
export function encodeDiagnostics(lines: DiagLine[]): Uint8Array {
  const utf8 = new TextEncoder();
  const out: string[] = [];
  let bytes = 0;
  for (let i = lines.length - 1; i >= 0 && out.length < MAX_DIAG_LINES; i--) {
    const { at, level, kind, text } = lines[i];
    const line = JSON.stringify({ at: wellFormed(at), level, kind, text: wellFormed(text) });
    const n = utf8.encode(line).length + 1;
    if (bytes + n > MAX_DIAG_BYTES) break;
    bytes += n;
    out.push(line);
  }
  return utf8.encode(out.reverse().map(l => `${l}\n`).join(""));
}

/** The origin of `url` read against `base`, or `null`. */
function originOf(url: string, base: string): string | null {
  try {
    return new URL(url, base).origin;
  } catch {
    return null;
  }
}

function hasHeader(h: HeadersInit | undefined, name: string): boolean {
  if (!h) return false;
  if (h instanceof Headers) return h.has(name);
  const keys = Array.isArray(h) ? h.map(([k]) => k) : Object.keys(h);
  return keys.some(k => k.toLowerCase() === name);
}

function requestId(get: (header: string) => string | null): string | null {
  for (const h of REQUEST_ID_HEADERS) {
    try {
      const v = get(h);
      if (v) return v;
    } catch {
      // A header the page may not read.
    }
  }
  return null;
}

/** A thrown or rejected value as a line's text: an error's stack (or its
 * name and message), a string as it is, anything else as JSON. */
function describe(v: unknown): string {
  if (v instanceof Error || (typeof v === "object" && v !== null && "message" in v && "name" in v)) {
    const e = v as Error;
    const head = `${e.name}: ${e.message}`;
    return typeof e.stack === "string" && e.stack ? (e.stack.startsWith(head) ? e.stack : `${head}\n${e.stack}`) : head;
  }
  if (typeof v === "string") return cut(v, MAX_READ_CHARS);
  try {
    return cut(JSON.stringify(v) ?? String(v), MAX_READ_CHARS);
  } catch {
    return String(v);
  }
}

function formatArgs(args: unknown[]): string {
  return cut(args.map(describe).join(" "), MAX_READ_CHARS);
}
