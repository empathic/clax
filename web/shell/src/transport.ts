// The shell's transport in the Clax app. The app runs Clax's core in its own
// process, with no port: its WebView answers the shell's documents and the
// artifacts' content from a scheme, and the shell's API calls go over the
// `ClaxHost` channel. The core marks every shell document it writes in the
// app with `<meta name="clax-embed" content='{"transport":"host",…}'>`;
// `installTransport` reads it at boot and, when it is there, replaces the
// document's `fetch` with `hostFetch`. A browser's shell has no such meta and
// keeps its own `fetch`.
//
// The channel's calls (each posted with the load's nonce, `app/host.ts`):
// - `call {id, method, path, headers, body, body64, chunked}`: a request to
//   the shell's origin under `/api/`. A text or JSON body that is valid UTF-8
//   goes as `body`, any other as base64 in `body64`; a body over
//   `CHUNKED_OVER` goes in neither (`chunked: true`) but in `callChunk {id,
//   n, last, body64}` messages that follow, `n` from 0, each one `CHUNK`
//   bytes of the body (the last one the rest) in its own base64. The host
//   answers with `claxHost.callResult(id, {status, headers, body | body64})`.
// - `callAbort {id}`: the call's signal aborted; its answer is dropped.
// - `subscribe {id}`: `GET /api/stream`. The host pushes the event stream's
//   text by `claxHost.streamBlock(id, text)` and ends it by
//   `claxHost.streamEnd(id)`.
// - `unsubscribe {id}`: the stream's reader cancelled it or its signal
//   aborted.
// Calls made before the app's `init` wait for it: the channel needs the
// nonce.

import { connNotices } from "./conn-notice";

/** The app's embed settings, from the shell document's
 * `<meta name="clax-embed" content='{"transport":"host","shell":…,"content":…}'>`
 * (written by the core when it runs in the app; absent in a browser).
 * `shell` is the shell's origin, `content` an artifact's content origin
 * with `{id}` in place of its ID. */
export type Embed = { transport: "host"; shell: string; content: string };

/** The embed settings of `doc`, or null without a well-formed meta. */
export function readEmbed(doc: Document = document): Embed | null {
  const raw = doc.querySelector('meta[name="clax-embed"]')?.getAttribute("content");
  if (!raw) return null;
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof v !== "object" || v === null) return null;
  const { transport, shell, content } = v as Record<string, unknown>;
  if (transport !== "host" || typeof shell !== "string" || typeof content !== "string") return null;
  return { transport, shell, content };
}

/** What the host side of the channel looks like to the transport. */
export type HostLink = {
  /** Posts `msg` on the channel with the load's nonce. */
  post(msg: Record<string, unknown>): void;
  /** Resolves once the channel has its nonce (the app's `init`). */
  ready: Promise<void>;
};

/** A body's bytes a `callChunk` carries. */
export const CHUNK = 1 << 20;
/** A body over this many bytes goes in chunks. */
export const CHUNKED_OVER = 8 << 20;

/** The host's answer to a `call`. */
export type CallResult = { status: number; headers: [string, string][]; body?: string | null; body64?: string | null };

type Pending = { resolve(r: Response): void; reject(e: unknown): void };

// The calls waiting for their answer and the open streams, by ID: one set
// per document, which `claxHost` answers into.
const calls = new Map<number, Pending>();
const streams = new Map<number, ReadableStreamDefaultController<Uint8Array>>();
let lastId = 0;

const NULL_BODY = new Set([101, 103, 204, 205, 304]);

/** A URL's scheme and host. `URL.origin` is "null" for a scheme the URL
 * standard does not know (`clax://app`), so origins are compared by these. */
function originOf(u: URL): string {
  return `${u.protocol}//${u.host}`;
}

function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

function fromBase64(text: string): Uint8Array {
  const s = atob(text);
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

const TEXTUAL = /^(text\/|application\/([\w.+-]*\+)?json\b|application\/x-www-form-urlencoded\b)/i;

function abortError(signal: AbortSignal): unknown {
  return signal.reason ?? new DOMException("The operation was aborted.", "AbortError");
}

/** `fetch` for app mode: a request to `origin` under `/api/` goes over
 * `link`, anything else to `fallback`. `GET /api/stream` answers a
 * response whose body the host's pushed blocks fill. */
export function hostFetch(link: HostLink, fallback: typeof fetch, origin: string): typeof fetch {
  const home = originOf(new URL(origin));
  return async (input, init) => {
    const href = input instanceof Request ? input.url : input instanceof URL ? input.href : String(input);
    const url = new URL(href, origin);
    if (originOf(url) !== home || !url.pathname.startsWith("/api/")) return fallback(input, init);
    // The signal stays out of the Request: it is watched here.
    const signal = init?.signal ?? (input instanceof Request ? input.signal : null);
    const req = new Request(input instanceof Request ? input : url.href, { ...init, signal: null });
    if (signal?.aborted) throw abortError(signal);
    if (req.method === "GET" && url.pathname === "/api/stream") return subscribe(link, signal);
    const bytes = new Uint8Array(await req.arrayBuffer());
    await link.ready;
    if (signal?.aborted) throw abortError(signal);
    const id = ++lastId;
    const answer = new Promise<Response>((resolve, reject) => calls.set(id, { resolve, reject }));
    const msg = { type: "call", id, method: req.method, path: url.pathname + url.search, headers: [...req.headers], body: null as string | null, body64: null as string | null };
    if (bytes.length > CHUNKED_OVER) {
      link.post({ ...msg, chunked: true });
      const n = Math.ceil(bytes.length / CHUNK);
      for (let i = 0; i < n; i++) link.post({ type: "callChunk", id, n: i, last: i === n - 1, body64: toBase64(bytes.subarray(i * CHUNK, (i + 1) * CHUNK)) });
    } else {
      const text = bytes.length > 0 && TEXTUAL.test(req.headers.get("content-type") ?? "") ? utf8(bytes) : null;
      if (text !== null) msg.body = text;
      else if (bytes.length > 0) msg.body64 = toBase64(bytes);
      link.post(msg);
    }
    if (signal) {
      const onAbort = () => {
        const p = calls.get(id);
        if (!p) return;
        calls.delete(id);
        link.post({ type: "callAbort", id });
        p.reject(abortError(signal));
      };
      signal.addEventListener("abort", onAbort, { once: true });
      void answer.finally(() => signal.removeEventListener("abort", onAbort)).catch(() => {});
    }
    return answer;
  };
}

function utf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

async function subscribe(link: HostLink, signal: AbortSignal | null): Promise<Response> {
  await link.ready;
  if (signal?.aborted) throw abortError(signal);
  const id = ++lastId;
  const stop = () => {
    if (!streams.delete(id)) return;
    link.post({ type: "unsubscribe", id });
  };
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      streams.set(id, c);
      if (signal) signal.addEventListener("abort", () => {
        if (!streams.has(id)) return;
        stop();
        c.error(abortError(signal));
      }, { once: true });
    },
    cancel: stop,
  });
  link.post({ type: "subscribe", id });
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}

/** Answers call `id` with the host's result. An answer to no waiting call
 * (aborted, or never made) does nothing. */
export function callResult(id: number, r: CallResult): void {
  const p = calls.get(id);
  if (!p) return;
  calls.delete(id);
  try {
    const status = r.status;
    const bytes = typeof r.body64 === "string" ? fromBase64(r.body64) : null;
    const body = NULL_BODY.has(status) ? null : (bytes ?? r.body ?? null);
    p.resolve(new Response(body as BodyInit | null, { status, headers: r.headers }));
  } catch (e) {
    p.reject(new TypeError(`the app answered a malformed result: ${String(e)}`));
  }
}

/** Adds `text` to stream `id`. */
export function streamBlock(id: number, text: string): void {
  const c = streams.get(id);
  if (c && typeof text === "string") c.enqueue(new TextEncoder().encode(text));
}

/** Ends stream `id` (the core closed it); its reader sees the end. */
export function streamEnd(id: number): void {
  const c = streams.get(id);
  if (!c) return;
  streams.delete(id);
  c.close();
}

let nonceGiven: (n: string) => void = () => {};
const nonceSet = new Promise<string>(resolve => { nonceGiven = resolve; });

/** Gives the channel this load's nonce: `claxHost.init` (`app/host.ts`)
 * calls it once. */
export function channelNonce(n: string): void {
  nonceGiven(n);
}

type HostWindow = Window & { ClaxHost?: { postMessage(m: string): void } };

/** At boot: in app mode (the document carries the `clax-embed` meta)
 * replaces `win.fetch` with `hostFetch` over `window.ClaxHost` and turns
 * connection notices off. `nonce` resolves to the load's nonce (by default
 * once `channelNonce` gives it). Returns whether it did. */
export function installTransport(win: Window = window, nonce?: Promise<string>): boolean {
  const embed = readEmbed(win.document);
  if (!embed) return false;
  const w = win as HostWindow;
  let given: string | null = null;
  const ready = (nonce ?? nonceSet).then(n => { given = n; });
  const link: HostLink = {
    post: m => {
      const ch = w.ClaxHost;
      if (given !== null && ch && typeof ch.postMessage === "function") ch.postMessage(JSON.stringify({ ...m, nonce: given }));
    },
    ready,
  };
  const native = win.fetch.bind(win);
  win.fetch = hostFetch(link, native, embed.shell);
  connNotices(false, win.document);
  return true;
}
