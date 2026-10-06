// Every message between the extension's parts (spec 2026-10-05 §9.4), and
// one validator per receiver. A receiver drops anything its validator
// refuses: content scripts share the page's DOM, so the worker treats what
// they send as checked input, never as trusted. The validators deny by
// default: a message is a plain object whose own fields are exactly its
// type's (optional ones aside), each checked for type and bounds; an
// unknown field, a field read through a prototype, or a symbol key refuses
// it.
import type { Anchor, AnchorResult } from "../../bridge/src/protocol";
import type { Participants, Version } from "../../shell/src/api";
import type { Thread } from "../../shell/src/threads";
import type { PresenceView } from "../../shell/src/view/presence-model";
import type { Working } from "../../shell/src/view/working-model";

export const MAX_URL = 4096;
export const MAX_BODY = 10_000;
export const MAX_TITLE = 1000;
/** A transport bound in UTF-16 code units; the daemon enforces the 8 MiB cap on the snapshot's bytes. */
export const MAX_SNAPSHOT_CHARS = 8 * 1024 * 1024;
export const MAX_ROUTE = 512;
/** Why a pick carries a placeholder or no snapshot: the serializer's
 * `error`, or `failed` when the serializer threw. */
export type SnapshotError = "too_large" | "failed";
export const PICK_ID = /^[0-9a-f]{32}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const HANDLE = /^a_[0-9a-f]{22}$/;
const ARTIFACT_ID = /^[0-9a-hjkmnp-tv-z]{12}$/;

export type Rect = { x: number; y: number; w: number; h: number };

/** Whether a thread is open and waits for a snapshot to link its address
 * to (spec L11): what both the overlay and the worker count as pending. */
export const waitsForSnapshot = (t: { status: string }) => t.status === "open" && !!(t as { addressed_pending?: unknown }).addressed_pending;
export type PageView = { artifact_id: string; origin: string; path: string; page_url: string; title: string; current_version: number; url: string };

export type OverlayToWorker =
  | { t: "hello"; url: string }
  | { t: "route"; url: string }
  /** The overlay names the pick (128 random bits); the worker takes it as
   * the tab's pick. The anchor comes with it, so the composer's draft does
   * not wait for the page's snapshot. */
  | { t: "capture"; pickId: string; anchor: Anchor; rect: Rect; dpr: number }
  /** The page's snapshot for the pick, serialized once its composer is shown. */
  | { t: "pick"; pickId: string; url: string; title: string; snapshot: string | null; snapshotError: SnapshotError | null }
  /** `pending`: the threads the overlay's state showed waiting for a snapshot when it serialized the page. */
  | { t: "quiet"; url: string; title: string; snapshot: string; pending: string[] }
  | { t: "resolved"; results: AnchorResult[] }
  | { t: "comment-mode"; on: boolean }
  | { t: "cancel"; pickId: string | null }
  | { t: "pin"; threadId: string }
  | { t: "removed" }
  | { t: "ping" };

export type WorkerToOverlay =
  | { t: "state"; page: PageView | null; route: string | null; threads: Thread[]; commentMode: boolean; pending: boolean }
  | { t: "comment-mode"; on: boolean }
  /** The answer to `capture`: the screenshot was taken (`ok`), or why not. */
  | { t: "captured"; pickId: string; ok: boolean; error?: string }
  /** The worker took the pick: the overlay frames its composer beside `rect`, then serializes the page. */
  | { t: "open-composer"; pickId: string; rect: Rect }
  /** The pick's composer page connected to the worker (after its own load): only now is its frame shown and focused. */
  | { t: "composer-ready"; pickId: string }
  /** `reason: "timeout"`: the composer page never connected, which the overlay tells the person. */
  | { t: "close-composer"; pickId: string; posted: boolean; reason?: "timeout" }
  | { t: "scroll-to"; threadId: string }
  | { t: "focus"; threadId: string | null }
  | { t: "snapshot-now" }
  /** The worker has no results for the threads it shows (it restarted): the overlay sends its current `resolved` again. */
  | { t: "resend" }
  /** Whether the worker's event stream is up; while it is down, what the overlay shows may be stale. */
  | { t: "stream-status"; up: boolean };

export type ComposerToWorker = { t: "ready" } | { t: "post"; body: string } | { t: "cancel" };
/** `clipUrl` is a `data:image/png` URL (a service worker cannot make object URLs). */
export type WorkerToComposer =
  | { t: "draft"; anchor: Anchor; clipUrl: string | null; clipError: string | null; capturing: boolean }
  | { t: "posted"; threadId: string }
  | { t: "failed"; message: string };

/** What the side panel shows for its window's active tab. */
export type PanelState = {
  tabId: number | null;
  url: string | null;
  page: PageView | null;
  route: string | null;
  threads: Thread[];
  resolved: Record<string, AnchorResult>;
  versions: Version[];
  working: Working[];
  participants: Participants | null;
  viewer: { public_id: string; display_name: string | null } | null;
  commentMode: boolean;
  enabled: boolean;
  selected: string | null;
  error: { code: string; message: string } | null;
  /** Who is on the page now (its `presence:<aid>` topic, which the worker follows for this panel). */
  presence?: PresenceView[];
};
export type WorkerToPanel =
  | { t: "tab"; state: PanelState }
  | { t: "failed"; code: string; message: string }
  /** Whether the worker's event stream is up (told on `watch-tab` and on every change). */
  | { t: "stream-status"; up: boolean }
  | { t: "ping" };

export type PanelToWorker =
  | { t: "watch-tab"; tabId: number }
  | { t: "send"; threadId: string; to: string | null }
  | { t: "send-batch"; threadIds: string[]; note: string | null; to: string | null }
  | { t: "reply"; threadId: string; body: string }
  | { t: "resolve"; threadId: string }
  | { t: "reopen"; threadId: string }
  | { t: "delete"; threadId: string }
  | { t: "looked"; threadIds: string[] }
  | { t: "set-name"; name: string }
  | { t: "select"; threadId: string | null }
  | { t: "comment-mode"; on: boolean }
  /** `artifactId`: the live page the panel showed; the worker refuses it for another. */
  | { t: "navigate"; route: string | null; artifactId: string }
  /** `origin`: the site the panel named; the worker refuses it for another. */
  | { t: "turn-off"; origin: string }
  | { t: "retry" }
  /** Whether the panel's document is visible: the worker reports the owner here only while it is. */
  | { t: "visible"; on: boolean }
  | { t: "ping" };

type Obj = Record<string, unknown>;
const obj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
/** A plain object whose own string keys are all `required` (each present) and some of `optional`, and nothing else. */
function shape(v: unknown, required: readonly string[], optional: readonly string[] = []): v is Obj {
  if (!obj(v)) return false;
  const proto = Object.getPrototypeOf(v);
  if (proto !== Object.prototype && proto !== null) return false;
  return Reflect.ownKeys(v).every(k => typeof k === "string" && (required.includes(k) || optional.includes(k)))
    && required.every(k => Object.hasOwn(v, k));
}
const str = (v: unknown, max: number): v is string => typeof v === "string" && v.length <= max;
const strOrNull = (v: unknown, max: number) => v === null || str(v, max);
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const count = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const bool = (v: unknown): v is boolean => typeof v === "boolean";
const url = (v: unknown) => str(v, MAX_URL) && /^https?:\/\//.test(v);
const ulid = (v: unknown) => typeof v === "string" && ULID.test(v);
const pickId = (v: unknown) => typeof v === "string" && PICK_ID.test(v);
const text = (v: unknown, max: number) => str(v, max) && v.trim().length > 0;
/** The most pending thread IDs a quiet snapshot names (the threads a state carries). */
const MAX_PENDING = 1000;
/** A failure code: lowercase words joined by underscores. */
const code = (v: unknown) => typeof v === "string" && /^[a-z_]{1,64}$/.test(v);
const BOX = ["x", "y", "w", "h"];
const box = (v: unknown): v is Rect => shape(v, BOX) && BOX.every(k => num(v[k])) && (v.w as number) >= 0 && (v.h as number) >= 0;

const KINDS = new Set(["element", "range", "area"]);
const ANCHOR = ["kind", "selector", "quote", "prefix", "suffix", "html_hash", "rect", "custom_name", "file"];
const ANCHOR_RECT = ["x", "y", "w", "h", "scrollX", "scrollY", "viewportW"];
/** At most 64 characters, none of them a control character or a line separator (the daemon's fingerprint rule). */
const fingerprint = (v: unknown) => v === undefined || (typeof v === "string" && [...v].length <= 64 && !/[\p{Cc}\u2028\u2029]/u.test(v));
/** A drawn rectangle as fractions of its element's box, inside it, with some width and height. */
function area(v: unknown): boolean {
  if (!shape(v, BOX, ["tag", "text", "children"]) || !BOX.every(k => num(v[k]) && (v[k] as number) >= 0 && (v[k] as number) <= 1)) return false;
  const { x, y, w, h } = v as Rect;
  return w > 0 && h > 0 && x + w <= 1 + 1e-4 && y + h <= 1 + 1e-4
    && fingerprint(v.tag) && fingerprint(v.text) && (v.children === undefined || (count(v.children) && v.children <= 0xffff_ffff));
}
/** An anchor as the overlay builds one (spec main §9 "Anchors"); the daemon validates it again. */
export function isAnchor(v: unknown): v is Anchor {
  if (!shape(v, ANCHOR, ["area"]) || typeof v.kind !== "string" || !KINDS.has(v.kind) || v.file !== "index.html") return false;
  if (!strOrNull(v.selector, 1024) || !strOrNull(v.quote, 2000) || !strOrNull(v.prefix, 64) || !strOrNull(v.suffix, 64)) return false;
  if (!strOrNull(v.html_hash, 80) || v.custom_name !== null) return false;
  if (v.rect !== null && !(shape(v.rect, ANCHOR_RECT) && ANCHOR_RECT.every(k => num((v.rect as Obj)[k])))) return false;
  if (Object.hasOwn(v, "area") && !area(v.area)) return false;
  return true;
}

const METHODS = new Set(["exact", "selector", "quote", "custom"]);
const result = (r: unknown) => shape(r, ["id", "found", "method", "rect"]) && ulid(r.id) && bool(r.found)
  && (r.method === null || (typeof r.method === "string" && METHODS.has(r.method))) && (r.rect === null || box(r.rect));

const PAGE = ["artifact_id", "origin", "path", "page_url", "title", "current_version", "url"];
/** A live page as the daemon describes it (`PageView`). */
const page = (v: unknown): v is PageView => shape(v, PAGE) && typeof v.artifact_id === "string" && ARTIFACT_ID.test(v.artifact_id)
  && str(v.origin, MAX_URL) && /^https?:\/\/[^/?#]+$/.test(v.origin) && str(v.path, MAX_URL) && v.path.startsWith("/")
  && url(v.page_url) && str(v.title, MAX_TITLE) && count(v.current_version) && url(v.url);

export function isFromOverlay(m: unknown): m is OverlayToWorker {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "hello": case "route": return has("url") && url(m.url);
    case "capture": return has("pickId", "anchor", "rect", "dpr") && pickId(m.pickId) && isAnchor(m.anchor) && box(m.rect) && num(m.dpr) && m.dpr > 0 && m.dpr <= 8;
    case "pick": return has("pickId", "url", "title", "snapshot", "snapshotError") && pickId(m.pickId) && url(m.url)
      && str(m.title, MAX_TITLE) && strOrNull(m.snapshot, MAX_SNAPSHOT_CHARS) && (m.snapshotError === null || m.snapshotError === "too_large" || m.snapshotError === "failed")
      && (m.snapshot !== null || m.snapshotError !== null);
    case "quiet": return has("url", "title", "snapshot", "pending") && url(m.url) && str(m.title, MAX_TITLE) && str(m.snapshot, MAX_SNAPSHOT_CHARS)
      && Array.isArray(m.pending) && m.pending.length <= MAX_PENDING && m.pending.every(ulid);
    case "resolved": return has("results") && Array.isArray(m.results) && m.results.length <= 500 && m.results.every(result);
    case "comment-mode": return has("on") && bool(m.on);
    case "cancel": return has("pickId") && (m.pickId === null || pickId(m.pickId));
    case "pin": return has("threadId") && ulid(m.threadId);
    case "removed": case "ping": return has();
    default: return false;
  }
}

/** The overlay's view of a thread is the daemon's; the worker relays it, so only its ID is checked here. */
const thread = (v: unknown) => obj(v) && ulid(v.id);

export function isFromWorker(m: unknown): m is WorkerToOverlay {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "state": return has("page", "route", "threads", "commentMode", "pending") && (m.page === null || page(m.page)) && strOrNull(m.route, MAX_ROUTE)
      && Array.isArray(m.threads) && m.threads.length <= 1000 && m.threads.every(thread) && bool(m.commentMode) && bool(m.pending);
    case "comment-mode": return has("on") && bool(m.on);
    case "captured": return shape(m, ["t", "pickId", "ok"], ["error"]) && pickId(m.pickId) && bool(m.ok) && (m.error === undefined || code(m.error));
    case "open-composer": return has("pickId", "rect") && pickId(m.pickId) && box(m.rect);
    case "composer-ready": return has("pickId") && pickId(m.pickId);
    case "close-composer": return shape(m, ["t", "pickId", "posted"], ["reason"]) && pickId(m.pickId) && bool(m.posted)
      && (m.reason === undefined || m.reason === "timeout");
    case "scroll-to": return has("threadId") && ulid(m.threadId);
    case "focus": return has("threadId") && (m.threadId === null || ulid(m.threadId));
    case "snapshot-now": case "resend": return has();
    case "stream-status": return has("up") && bool(m.up);
    default: return false;
  }
}

/** What a side panel takes from the worker. The worker is trusted; this
 * keeps the panel to the messages it knows, of the right shape. */
export function isToPanel(m: unknown): m is WorkerToPanel {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "tab": return has("state") && obj(m.state);
    case "failed": return has("code", "message") && str(m.code, 64) && str(m.message, MAX_BODY);
    case "stream-status": return has("up") && bool(m.up);
    case "ping": return has();
    default: return false;
  }
}

export function isFromComposer(m: unknown): m is ComposerToWorker {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "ready": case "cancel": return has();
    case "post": return has("body") && text(m.body, MAX_BODY);
    default: return false;
  }
}

/** What the composer page takes from the worker over its port: the worker
 * is trusted; this keeps the composer to the messages it knows, of the
 * right shape (a clip is a PNG `data:` URL). */
export function isToComposer(m: unknown): m is WorkerToComposer {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "draft": return has("anchor", "clipUrl", "clipError", "capturing") && isAnchor(m.anchor)
      && (m.clipUrl === null || (typeof m.clipUrl === "string" && m.clipUrl.startsWith("data:image/png;base64,")))
      && (m.clipError === null || code(m.clipError)) && bool(m.capturing);
    case "posted": return has("threadId") && ulid(m.threadId);
    case "failed": return has("message") && str(m.message, MAX_BODY);
    default: return false;
  }
}

export function isFromPanel(m: unknown): m is PanelToWorker {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  const to = (v: unknown) => v === null || (typeof v === "string" && HANDLE.test(v));
  switch (m.t) {
    case "watch-tab": return has("tabId") && count(m.tabId);
    case "send": return has("threadId", "to") && ulid(m.threadId) && to(m.to);
    case "send-batch": return has("threadIds", "note", "to") && Array.isArray(m.threadIds) && m.threadIds.length >= 1 && m.threadIds.length <= 20
      && m.threadIds.every(ulid) && strOrNull(m.note, 280) && to(m.to);
    case "reply": return has("threadId", "body") && ulid(m.threadId) && text(m.body, MAX_BODY);
    case "resolve": case "reopen": case "delete": return has("threadId") && ulid(m.threadId);
    case "looked": return has("threadIds") && Array.isArray(m.threadIds) && m.threadIds.length <= 50 && m.threadIds.every(ulid);
    case "set-name": return has("name") && str(m.name, 64);
    case "select": return has("threadId") && (m.threadId === null || ulid(m.threadId));
    case "comment-mode": case "visible": return has("on") && bool(m.on);
    case "navigate": return has("route", "artifactId") && (m.route === null || str(m.route, MAX_ROUTE))
      && typeof m.artifactId === "string" && ARTIFACT_ID.test(m.artifactId);
    case "turn-off": return has("origin") && str(m.origin, MAX_URL) && /^https?:\/\/[^/?#]+$/.test(m.origin);
    case "retry": case "ping": return has();
    default: return false;
  }
}

/** Failures a new pairing can fix: the native host, the credential, or a
 * pairing whose daemon is gone (restarted on another port, or stopped; the
 * native host answers the running one, starting it if need be). The panel
 * offers Retry for these, and Retry pairs again. */
export const RETRYABLE = /* @__PURE__ */ new Set(["host_missing", "host_failed", "daemon_unavailable", "bad_reply", "unknown_credential", "http_401", "paired_recently", "daemon_unreachable"]);
