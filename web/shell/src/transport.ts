// The shell's transport in the Clax app. The app runs Clax's core in its own
// process, with no port: its WebView answers the shell's documents and the
// artifacts' content from a scheme, and the shell's API calls go over the
// `ClaxHost` channel. The core marks every shell document it writes in the
// app with `<meta name="clax-embed" content='{"transport":"host",…}'>`,
// followed by `<script src="/_clax/transport.js">`: this module, built on its
// own as a classic script (`vite.transport.config.ts`, `transport-main.ts`),
// so it runs before any of the shell's modules, and a browser's shell, which
// the core never marks, loads none of it.
//
// `installTransport` reads the meta and, when it is there:
// - replaces the document's `fetch` with the transport's;
// - hides the connection notices (`.conn-notice`, `conn-notice.ts`) with a
//   style of its own: the app owns liveness, and the core runs in its
//   process;
// - installs `window.claxTransport`, where the app answers: `init(nonce)`,
//   `callResult(nonce, id, r)`, `callResultChunk(nonce, id, n, last,
//   body64)`, `streamBlock(nonce, id, text)` and `streamEnd(nonce, id)`.
//   Every shell document has it, whichever entry (gallery or artifact
//   view) it loads. It also carries the meta's `embed`
//   settings, which the shell's entries read there (`hostEmbed`), so no
//   browser entry carries the meta's parser.
//
// The load's nonce comes with the document: the app's scheme handler writes
// a fresh `<meta name="clax-nonce" content="…">` into each shell document it
// serves, so the shell's calls go out from boot, before the document's load
// event. A document without a well-formed one (the app over loopback, until
// its scheme handler writes it) waits for the app's `claxTransport.init`.
//
// The channel's calls, each posted with the load's nonce; every answer names it again as its first
// argument, and one naming another nonce (a late answer to the WebView's
// previous load, whose call IDs this load reuses) is dropped:
// - `call {id, method, path, headers, body, body64, chunked}`: a request to
//   the shell's origin under `/api/`. A text or JSON body that is valid UTF-8
//   goes as `body`, any other as base64 in `body64`; a body over
//   `CHUNKED_OVER` goes in neither (`chunked: true`) but in `callChunk {id,
//   n, last, body64}` messages that follow, `n` from 0, each one `CHUNK`
//   bytes of the body (the last one the rest) in its own base64. The app
//   answers with `claxTransport.callResult(nonce, id, {status, headers, body
//   | body64})`; an answer of more than 1 MiB with `callResult(nonce, id,
//   {status, headers, chunked: true})`, then its body in
//   `callResultChunk(nonce, id, n, last, body64)` calls, `n` from 0.
// - `callAbort {id}`: the call's signal aborted; its answer is dropped.
// - `subscribe {id, lastEventId?}`: `GET /api/stream`, from the resume point
//   the request names (its `Last-Event-ID`, or its `last_event_id` query),
//   from which the core's subscription resumes as the SSE route does. The
//   app pushes the event stream's text by `claxTransport.streamBlock(nonce,
//   id, text)` and ends it by `claxTransport.streamEnd(nonce, id)`.
// - `unsubscribe {id}`: the stream's reader cancelled it or its signal
//   aborted.
// Calls made before the nonce is known wait for it. One
// whose signal aborts meanwhile rejects with an `AbortError` and posts
// nothing.

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
  const { transport: kind, shell, content } = v as Record<string, unknown>;
  if (kind !== "host" || typeof shell !== "string" || typeof content !== "string") return null;
  return { transport: kind, shell, content };
}

/** A body's bytes a `callChunk` carries. */
export const CHUNK = 1 << 20;
/** A body over this many bytes goes in chunks. */
export const CHUNKED_OVER = 8 << 20;

/** The app's answer to a `call`. */
export type CallResult = { status: number; headers: [string, string][]; body?: string | null; body64?: string | null; chunked?: boolean };

/** The app's channel, `window.ClaxHost`. */
export type Channel = { postMessage(m: string): void };

/** One document's transport: its `fetch`, and the receivers the app calls
 * (`window.claxTransport`). */
export type Transport = {
  fetch: typeof fetch;
  /** Gives the channel this load's nonce; only the first well-formed one counts. */
  init(nonce: string): void;
  /** Answers call `id` of the load whose nonce is `nonce`. An answer to no
   * waiting call (aborted, or never made), or for another load, does
   * nothing. */
  callResult(nonce: string, id: number, r: CallResult): void;
  /** Adds part `n` (from 0) of a `chunked` result's body to call `id`;
   * the `last` one answers it. One out of order fails the call. */
  callResultChunk(nonce: string, id: number, n: number, last: boolean, body64: string): void;
  /** Adds `text` to stream `id` of the load whose nonce is `nonce`. */
  streamBlock(nonce: string, id: number, text: string): void;
  /** Ends stream `id` of the load whose nonce is `nonce` (the core closed
   * it); its reader sees the end. */
  streamEnd(nonce: string, id: number): void;
};

type Pending = { resolve(r: Response): void; reject(e: unknown): void };

const NULL_BODY = new Set([101, 103, 204, 205, 304]);
const NONCE = /^[A-Za-z0-9_-]{16,128}$/;
const TEXTUAL = /^(text\/|application\/([\w.+-]*\+)?json\b|application\/x-www-form-urlencoded\b)/i;

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

/** The bytes of `text`, base64, or of its parts each in its own base64. */
function fromBase64(text: string | string[]): Uint8Array {
  const s = [text].flat().map(t => atob(t)).join("");
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

function utf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

function abortError(signal: AbortSignal): unknown {
  return signal.reason ?? new DOMException("The operation was aborted.", "AbortError");
}

/** A transport over `channel` (read at each post): a request to `origin`
 * under `/api/` goes over it, anything else to `fallback`. `GET
 * /api/stream` answers a response whose body the app's pushed blocks fill.
 * `given`, when well formed, is the load's nonce from the start; else
 * `init` gives it. */
export function transport(channel: () => Channel | undefined, fallback: typeof fetch, origin: string, given: string | null = null): Transport {
  const home = originOf(new URL(origin));
  // The calls waiting for their answer and the open streams, by ID.
  const calls = new Map<number, Pending>();
  const streams = new Map<number, ReadableStreamDefaultController<Uint8Array>>();
  // The chunked results being received: their head and their parts' base64 so far.
  const parts = new Map<number, [CallResult, string[]]>();
  let lastId = 0;
  let nonce: string | null = null;
  let opened = () => {};
  const ready = new Promise<void>(resolve => { opened = resolve; });

  /** Resolves once the nonce is given, or rejects once `signal` aborts. */
  const untilOpen = (signal: AbortSignal | null) => !signal ? ready : new Promise<void>((resolve, reject) => {
    const stop = () => reject(abortError(signal));
    if (signal.aborted) return stop();
    signal.addEventListener("abort", stop, { once: true });
    void ready.then(() => { signal.removeEventListener("abort", stop); resolve(); });
  });

  // An answer is this load's only under its nonce: IDs start at 1 in every
  // document, and the app's answer to a previous load's call can reach this
  // one.
  const ours = (n: unknown) => nonce !== null && n === nonce;

  const post = (m: Record<string, unknown>) => {
    const ch = channel();
    if (nonce !== null && ch && typeof ch.postMessage === "function") ch.postMessage(JSON.stringify({ ...m, nonce }));
  };

  const subscribe = async (from: string | null, signal: AbortSignal | null): Promise<Response> => {
    await untilOpen(signal);
    if (signal?.aborted) throw abortError(signal);
    const id = ++lastId;
    const stop = () => {
      if (!streams.delete(id)) return;
      post({ type: "unsubscribe", id });
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
    post({ type: "subscribe", id, ...(from ? { lastEventId: from } : {}) });
    return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
  };

  const hostFetch: typeof fetch = async (input, init) => {
    const href = input instanceof Request ? input.url : input instanceof URL ? input.href : String(input);
    const url = new URL(href, origin);
    if (originOf(url) !== home || !url.pathname.startsWith("/api/")) return fallback(input, init);
    // The signal stays out of the Request: it is watched here.
    const signal = init?.signal ?? (input instanceof Request ? input.signal : null);
    const req = new Request(input instanceof Request ? input : url.href, { ...init, signal: null });
    if (signal?.aborted) throw abortError(signal);
    if (req.method === "GET" && url.pathname === "/api/stream") return subscribe(req.headers.get("last-event-id") ?? url.searchParams.get("last_event_id"), signal);
    const bytes = new Uint8Array(await req.arrayBuffer());
    await untilOpen(signal);
    if (signal?.aborted) throw abortError(signal);
    const id = ++lastId;
    const answer = new Promise<Response>((resolve, reject) => calls.set(id, { resolve, reject }));
    const msg = { type: "call", id, method: req.method, path: url.pathname + url.search, headers: [...req.headers], body: null as string | null, body64: null as string | null };
    if (bytes.length > CHUNKED_OVER) {
      post({ ...msg, chunked: true });
      const n = Math.ceil(bytes.length / CHUNK);
      for (let i = 0; i < n; i++) post({ type: "callChunk", id, n: i, last: i === n - 1, body64: toBase64(bytes.subarray(i * CHUNK, (i + 1) * CHUNK)) });
    } else {
      const text = bytes.length > 0 && TEXTUAL.test(req.headers.get("content-type") ?? "") ? utf8(bytes) : null;
      if (text !== null) msg.body = text;
      else if (bytes.length > 0) msg.body64 = toBase64(bytes);
      post(msg);
    }
    if (signal) {
      const onAbort = () => {
        const p = calls.get(id);
        if (!p) return;
        calls.delete(id);
        parts.delete(id);
        post({ type: "callAbort", id });
        p.reject(abortError(signal));
      };
      signal.addEventListener("abort", onAbort, { once: true });
      void answer.finally(() => signal.removeEventListener("abort", onAbort)).catch(() => {});
    }
    return answer;
  };

  /** Answers call `id` with what `r()` gives: a result, and its body's
   * bytes when they came apart from it. Fails the call when `r()` throws. */
  const settle = (id: number, r: () => [CallResult, Uint8Array | null]) => {
    const p = calls.get(id)!;
    calls.delete(id);
    parts.delete(id);
    try {
      const [{ status, headers, body64, body: text }, joined] = r();
      const bytes = joined ?? (typeof body64 === "string" ? fromBase64(body64) : null);
      const body = NULL_BODY.has(status) ? null : (bytes ?? text ?? null);
      p.resolve(new Response(body as BodyInit | null, { status, headers }));
    } catch (e) {
      p.reject(new TypeError(`the app answered a malformed result: ${String(e)}`));
    }
  };

  const init = (n: unknown) => {
    if (nonce !== null || typeof n !== "string" || !NONCE.test(n)) return;
    nonce = n;
    opened();
  };
  init(given);

  return {
    fetch: hostFetch,
    init,
    callResult(n, id, r) {
      if (!ours(n) || !calls.has(id)) return;
      if (r?.chunked) parts.set(id, [r, []]);
      else settle(id, () => [r, null]);
    },
    callResultChunk(n, id, i, last, body64) {
      const c = ours(n) && calls.has(id) ? parts.get(id) : undefined;
      if (!c) return;
      const [r, got] = c;
      if (i !== got.push(body64) - 1) settle(id, () => { throw new Error(`part ${i} out of order`); });
      else if (last) settle(id, () => [r, fromBase64(got)]);
    },
    streamBlock(n, id, text) {
      const c = ours(n) ? streams.get(id) : undefined;
      if (c && typeof text === "string") c.enqueue(new TextEncoder().encode(text));
    },
    streamEnd(n, id) {
      const c = ours(n) ? streams.get(id) : undefined;
      if (!c) return;
      streams.delete(id);
      c.close();
    },
  };
}

/** What `installTransport` leaves on the window: the app's receivers and
 * the document's embed settings. */
export type HostReceivers = Omit<Transport, "fetch"> & { embed: Embed };

type HostWindow = Window & { ClaxHost?: Channel; claxTransport?: HostReceivers };

/** The embed settings of `win`'s document when it runs on the app's
 * transport (`installTransport` ran there: its `fetch` reaches the core over
 * the app's channel), else null. */
export function hostEmbed(win: Window = window): Embed | null {
  return (win as HostWindow).claxTransport?.embed ?? null;
}

/** Whether `win`'s document runs on the app's transport. */
export function onHost(win: Window = window): boolean {
  return hostEmbed(win) !== null;
}

/** At boot: in app mode (the document carries the `clax-embed` meta)
 * replaces `win.fetch` with a transport over `window.ClaxHost`, hides the
 * connection notices and installs `window.claxTransport` with the embed
 * settings. Returns whether it did. */
export function installTransport(win: Window = window): boolean {
  const doc = win.document;
  const embed = readEmbed(doc);
  if (!embed) return false;
  const w = win as HostWindow;
  const nonce = doc.querySelector('meta[name="clax-nonce"]')?.getAttribute("content") ?? null;
  const { fetch: f, ...receivers } = transport(() => w.ClaxHost, win.fetch.bind(win), embed.shell, nonce);
  win.fetch = f;
  const style = doc.createElement("style");
  style.textContent = ".conn-notice{display:none!important}";
  doc.head.append(style);
  Object.defineProperty(w, "claxTransport", { value: Object.freeze({ ...receivers, embed: Object.freeze(embed) }) });
  return true;
}
