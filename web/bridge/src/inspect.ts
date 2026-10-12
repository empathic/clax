// Inspecting a page for the Clax app's agent (spec 2026-10-12-flutter-app-
// round2-design R§5.2): the app's inspect view (`?app=1&inspect=1`) asks the
// bridge, through its shell, for what the page looks like once it settles.
//
// - The error ring (`installErrorRing`) is the only part the eager bridge
//   carries: it starts with the bridge, before any page script, and keeps the
//   page's newest `RING_SIZE` errors (`error` events, unhandled rejections,
//   `console.error` calls) and failed loads, as raw events and arguments.
//   It is installed only in a frame the shell named for inspection
//   (`INSPECT_FRAME`), so other pages' `console.error` stays the browser's.
// - The rest loads with the clip part (`parts/clip.ts`) when the shell asks:
//   `settle` waits until the DOM has been quiet for `QUIET_MS` and the
//   document has loaded, at most `settleMs`, on timers only (a WebView the
//   system counts as hidden runs no animation frames); `outline` lists what
//   a reader sees, within `OUTLINE_BYTES`; `inspectPage` puts them together
//   with a screenshot of the viewport (`inspect-render.ts`), rendered again
//   at half the scale while it is over `maxBytes`.
import type { BridgeToShell, InspectError, Inspection, ShellToBridge } from "./protocol";

/** How many of the page's errors, and of its failed loads, are kept. */
export const RING_SIZE = 20;
/** How long the DOM must be still for the page to count as settled. */
export const QUIET_MS = 300;
/** How long `settle` waits at most, when the shell names no limit. */
export const SETTLE_MS = 3000;
/** The longest wait the shell may ask for. */
export const SETTLE_MAX_MS = 10_000;
/** The largest screenshot, in bytes, when the shell names no limit. */
export const MAX_BYTES = 1024 * 1024;
/** How many times a screenshot over the limit is rendered again at half scale. */
export const SHRINKS = 3;
/** The outline's largest size, in UTF-8 bytes. */
export const OUTLINE_BYTES = 3072;
/** The longest text kept of one error, or one line of the outline. */
const ERROR_CHARS = 500;
const LINE_CHARS = 120;
/** How many elements the outline looks at, at most. */
const OUTLINE_ELEMENTS = 4000;

export type InspectTimers = { setTimeout(f: () => void, ms: number): unknown; clearTimeout(t: unknown): void };
type ObserverCtor = new (cb: MutationCallback) => Pick<MutationObserver, "observe" | "disconnect">;

/** The page's errors and failed loads since the bridge started, as they
 * came, newest last: `[0]` holds each `error` event, `unhandledrejection`
 * event, or `console.error` call's arguments (an array), `[1]` the `error`
 * events of elements that failed to load. The eager bridge only captures;
 * `errorsOf` (clip part) reads and words them. */
export type ErrorRing = [errors: (Event | unknown[])[], failed: Event[]];

/** Starts keeping `win`'s errors, the newest `RING_SIZE` of each list:
 * wraps `console.error` (the page's own call still reaches the console) and
 * listens, capturing, to `error` and `unhandledrejection`. */
export function installErrorRing(win: Window): ErrorRing {
  const ring: ErrorRing = [[], []];
  const keep = <T>(list: T[], v: T) => { if (list.push(v) > RING_SIZE) list.shift(); };
  const con = (win as Window & { console: Console }).console;
  const original = con.error;
  con.error = function (this: unknown, ...v: unknown[]) {
    keep(ring[0], v);
    return original.apply(this, v);
  };
  win.addEventListener("error", e => { if ((e.target as Node | null)?.nodeType === 1) keep(ring[1], e); else keep(ring[0], e); }, true);
  win.addEventListener("unhandledrejection", e => keep(ring[0], e));
  return ring;
}

const cut = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

/** Words for a value the page logged or threw. */
function words(v: unknown): string {
  if (typeof v === "string") return v;
  if (v instanceof Error) return `${v.name}: ${v.message}`;
  try {
    const s = JSON.stringify(v);
    return s === undefined ? String(v) : s;
  } catch {
    return String(v);
  }
}

/** One captured error in words. */
function errorText(kind: InspectError["kind"], v: unknown): string {
  if (kind === "console") return (v as unknown[]).map(words).join(" ");
  if (kind === "unhandledrejection") return words((v as PromiseRejectionEvent).reason);
  const e = v as ErrorEvent;
  const at = e.filename ? ` (${e.filename}:${e.lineno}:${e.colno})` : "";
  return `${e.message || words(e.error)}${at}`;
}

/** `ring`'s errors in words, and the URLs of its failed loads, each cut
 * to `ERROR_CHARS`. */
export function errorsOf(ring: ErrorRing): { errors: InspectError[]; failed: string[] } {
  const errors = ring[0].map(v => {
    const kind: InspectError["kind"] = Array.isArray(v) ? "console" : v.type === "unhandledrejection" ? "unhandledrejection" : "error";
    let text: string;
    try { text = errorText(kind, v); } catch { text = "(an error that could not be put in words)"; }
    return { kind, text: cut(text, ERROR_CHARS) };
  });
  const failed: string[] = [];
  for (const e of ring[1]) {
    const t = e.target as { currentSrc?: string; src?: string; href?: string } | null;
    const url = t ? t.currentSrc || t.src || t.href : "";
    if (typeof url === "string" && url) failed.push(cut(url, ERROR_CHARS));
  }
  return { errors, failed };
}

export type SettleDeps = { timers: InspectTimers; Observer: ObserverCtor };

/** Resolves once `doc` has loaded and its DOM has had no mutation for
 * `QUIET_MS`, or after `maxMs` whichever comes first. */
export function settle(doc: Document, maxMs: number, deps: SettleDeps): Promise<void> {
  const { timers: t } = deps;
  const win = doc.defaultView;
  return new Promise(resolve => {
    let quiet: unknown;
    let cap: unknown;
    let done = false;
    let still = false;
    const finish = () => {
      if (done) return;
      done = true;
      obs.disconnect();
      t.clearTimeout(quiet);
      t.clearTimeout(cap);
      win?.removeEventListener("load", onLoad);
      resolve();
    };
    const loaded = () => doc.readyState === "complete";
    const onQuiet = () => { still = true; if (loaded()) finish(); };
    const onLoad = () => { if (still) finish(); };
    const restart = () => { still = false; t.clearTimeout(quiet); quiet = t.setTimeout(onQuiet, QUIET_MS); };
    const obs = new deps.Observer(restart);
    obs.observe(doc, { subtree: true, childList: true, attributes: true, characterData: true });
    win?.addEventListener("load", onLoad);
    restart();
    cap = t.setTimeout(finish, Math.max(0, maxMs));
  });
}

const HIDDEN_TAGS = /^(script|style|noscript|template|head|clax-overlay)$/i;

/** Whether a reader sees `el` (or would, scrolled to it). */
function shown(el: Element, win: Window): boolean {
  const check = (el as Element & { checkVisibility?: (o?: object) => boolean }).checkVisibility;
  if (typeof check === "function") return check.call(el, { visibilityProperty: true });
  for (let c: Element | null = el; c; c = c.parentElement) {
    if (HIDDEN_TAGS.test(c.localName) || c.hasAttribute("hidden") || c.getAttribute("aria-hidden") === "true") return false;
    const cs = win.getComputedStyle(c);
    if (cs.display === "none" || (c === el && cs.visibility === "hidden")) return false;
  }
  return true;
}

const flat = (s: string | null | undefined) => (s ?? "").replace(/\s+/g, " ").trim();

/** One outline line for `el`, or null for an element it does not list. */
function line(el: Element): string | null {
  const name = el.localName;
  const text = () => cut(flat(el.textContent), LINE_CHARS);
  const label = () => cut(flat(el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.getAttribute("name")), LINE_CHARS);
  const h = /^h([1-6])$/.exec(name);
  if (h) return `${"#".repeat(Number(h[1]))} ${text()}`;
  if (name === "button" || el.getAttribute("role") === "button") return `[button] ${text()}`;
  if (name === "label") return `label: ${text()}`;
  if (name === "input") {
    const type = (el.getAttribute("type") ?? "text").toLowerCase();
    if (type === "hidden") return null;
    if (type === "submit" || type === "button" || type === "reset") return `[button] ${cut(flat((el as HTMLInputElement).value || type), LINE_CHARS)}`;
    return `[${type === "text" ? "input" : type}] ${label()}`.trimEnd();
  }
  if (name === "textarea") return `[textarea] ${label()}`.trimEnd();
  if (name === "select") return `[select] ${label()} (${(el as HTMLSelectElement).options.length} options)`.replace("]  (", "] (");
  if (name === "ul" || name === "ol") return `list: ${el.querySelectorAll(":scope > li").length} items`;
  return null;
}

const OUTLINED = "h1,h2,h3,h4,h5,h6,button,[role=button],label,input,textarea,select,ul,ol";

/** What a reader sees on `doc`, a line per heading, button, label, input
 * (with its placeholder or label), select and list (with its length), in
 * document order; cut with "…" at `OUTLINE_BYTES` UTF-8 bytes. */
export function outline(doc: Document, win: Window = doc.defaultView ?? window): string {
  const enc = new TextEncoder();
  const lines: string[] = [];
  let bytes = 0;
  const els = doc.body ? doc.body.querySelectorAll(OUTLINED) : [];
  for (let i = 0; i < els.length && i < OUTLINE_ELEMENTS; i++) {
    const el = els[i];
    if (!shown(el, win)) continue;
    const l = line(el);
    if (!l) continue;
    const n = enc.encode(l).length + (lines.length ? 1 : 0);
    if (bytes + n > OUTLINE_BYTES - 4) {
      lines.push("…");
      break;
    }
    lines.push(l);
    bytes += n;
  }
  return lines.join("\n");
}

export type InspectDeps = SettleDeps & { render(scale: number): Promise<ArrayBuffer> };

const bounded = (v: unknown, lo: number, hi: number, dflt: number) => (typeof v === "number" && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : dflt);

/** Inspects the page in `win` once it settles (`settle`, at most `settleMs`,
 * `SETTLE_MS` when absent): its viewport rendered at scale 1, again at half
 * the scale while over `maxBytes` (`MAX_BYTES` when absent), `SHRINKS` times
 * at most; `ring`'s errors and failed loads; its outline, title and height.
 * Rejects when the render fails or stays over `maxBytes`. */
export async function inspectPage(win: Window, ring: ErrorRing, opts: { settleMs?: number; maxBytes?: number }, deps: InspectDeps): Promise<Inspection> {
  const doc = win.document;
  await settle(doc, bounded(opts.settleMs, 0, SETTLE_MAX_MS, SETTLE_MS), deps);
  const maxBytes = bounded(opts.maxBytes, 1, Number.MAX_SAFE_INTEGER, MAX_BYTES);
  let png: ArrayBuffer;
  for (let i = 0, scale = 1; ; i++, scale /= 2) {
    png = await deps.render(scale);
    if (png.byteLength <= maxBytes) break;
    if (i >= SHRINKS) throw new Error(`the screenshot is over ${maxBytes} bytes even at ${scale} scale`);
  }
  const height = doc.documentElement.scrollHeight;
  return { png, ...errorsOf(ring), text: outline(doc, win), title: cut(flat(doc.title), 200), height: Number.isFinite(height) ? height : 0 };
}

/** The bridge's `clax:inspect` order, answered: `post` gets `clax:inspection`
 * with the order's `requestId`, the inspection (its PNG transferred) or why
 * there is none. An order without a `requestId` is dropped. */
export async function answerInspect(win: Window, ring: ErrorRing, m: Extract<ShellToBridge, { type: "clax:inspect" }>, post: (m: BridgeToShell, transfer?: Transferable[]) => void, deps: InspectDeps): Promise<void> {
  const requestId = m.requestId;
  if (typeof requestId !== "string") return;
  try {
    const r = await inspectPage(win, ring, m, deps);
    post({ type: "clax:inspection", requestId, ...r }, [r.png]);
  } catch (e) {
    post({ type: "clax:inspection", requestId, error: e instanceof Error ? e.message : String(e) });
  }
}
