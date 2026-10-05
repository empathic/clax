// Every message between the extension's parts (spec 2026-10-05 §9.4), and
// one validator per receiver. A receiver drops anything its validator
// refuses: content scripts share the page's DOM, so the worker treats what
// they send as checked input, never as trusted.
import type { Anchor, AnchorResult } from "../../bridge/src/protocol";
import type { Participants, Version } from "../../shell/src/api";
import type { Thread } from "../../shell/src/threads";
import type { Working } from "../../shell/src/view/working-model";

export const MAX_URL = 4096;
export const MAX_BODY = 10_000;
export const MAX_TITLE = 1000;
export const MAX_SNAPSHOT_CHARS = 8 * 1024 * 1024;
export const PICK_ID = /^[0-9a-f]{32}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const HANDLE = /^a_[0-9a-f]{22}$/;

export type Rect = { x: number; y: number; w: number; h: number };
export type PageView = { artifact_id: string; origin: string; path: string; page_url: string; title: string; current_version: number; url: string };

export type OverlayToWorker =
  | { t: "hello"; url: string }
  | { t: "route"; url: string }
  | { t: "capture"; rect: Rect; dpr: number }
  | { t: "pick"; pickId: string; anchor: Anchor; url: string; title: string; snapshot: string | null; snapshotError: string | null }
  | { t: "quiet"; url: string; title: string; snapshot: string }
  | { t: "resolved"; results: AnchorResult[] }
  | { t: "comment-mode"; on: boolean }
  | { t: "cancel"; pickId: string | null }
  | { t: "pin"; threadId: string }
  | { t: "removed" }
  | { t: "ping" };

export type WorkerToOverlay =
  | { t: "state"; page: PageView | null; route: string | null; threads: Thread[]; commentMode: boolean; pending: boolean }
  | { t: "comment-mode"; on: boolean }
  | { t: "close-composer"; pickId: string; posted: boolean }
  | { t: "scroll-to"; threadId: string }
  | { t: "focus"; threadId: string | null }
  | { t: "snapshot-now" };

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
};
export type WorkerToPanel =
  | { t: "tab"; state: PanelState }
  | { t: "failed"; code: string; message: string }
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
  | { t: "navigate"; route: string | null }
  | { t: "turn-off" }
  | { t: "retry" }
  | { t: "ping" };

type Obj = Record<string, unknown>;
const obj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown, max: number): v is string => typeof v === "string" && v.length <= max;
const strOrNull = (v: unknown, max: number) => v === null || str(v, max);
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const bool = (v: unknown): v is boolean => typeof v === "boolean";
const url = (v: unknown) => str(v, MAX_URL) && /^https?:\/\//.test(v);
const ulid = (v: unknown) => typeof v === "string" && ULID.test(v);
const pickId = (v: unknown) => typeof v === "string" && PICK_ID.test(v);
const text = (v: unknown, max: number) => str(v, max) && v.trim().length > 0;
const rect = (v: unknown): v is Rect => obj(v) && num(v.x) && num(v.y) && num(v.w) && num(v.h) && v.w >= 0 && v.h >= 0;
const numbers = (v: unknown, keys: string[]) => obj(v) && keys.every(k => num(v[k]));

const KINDS = new Set(["element", "range", "area"]);
/** An anchor as the overlay builds one (spec main §9 "Anchors"); the daemon validates it again. */
export function isAnchor(v: unknown): v is Anchor {
  if (!obj(v) || typeof v.kind !== "string" || !KINDS.has(v.kind) || v.file !== "index.html" || "route" in v) return false;
  if (!strOrNull(v.selector, 1024) || !strOrNull(v.quote, 2000) || !strOrNull(v.prefix, 64) || !strOrNull(v.suffix, 64)) return false;
  if (!strOrNull(v.html_hash, 80) || v.custom_name !== null) return false;
  if (v.rect !== null && !numbers(v.rect, ["x", "y", "w", "h", "scrollX", "scrollY", "viewportW"])) return false;
  if (v.area !== undefined && !numbers(v.area, ["x", "y", "w", "h"])) return false;
  return true;
}

export function isFromOverlay(m: unknown): m is OverlayToWorker {
  if (!obj(m)) return false;
  switch (m.t) {
    case "hello": case "route": return url(m.url);
    case "capture": return rect(m.rect) && num(m.dpr) && m.dpr > 0 && m.dpr <= 8;
    case "pick": return pickId(m.pickId) && isAnchor(m.anchor) && url(m.url) && str(m.title, MAX_TITLE)
      && strOrNull(m.snapshot, MAX_SNAPSHOT_CHARS) && strOrNull(m.snapshotError, 40) && (m.snapshot !== null || m.snapshotError !== null);
    case "quiet": return url(m.url) && str(m.title, MAX_TITLE) && str(m.snapshot, MAX_SNAPSHOT_CHARS);
    case "resolved": return Array.isArray(m.results) && m.results.length <= 500 && m.results.every(r => obj(r) && ulid(r.id) && bool(r.found));
    case "comment-mode": return bool(m.on);
    case "cancel": return m.pickId === null || pickId(m.pickId);
    case "pin": return ulid(m.threadId);
    case "removed": case "ping": return true;
    default: return false;
  }
}

export function isFromWorker(m: unknown): m is WorkerToOverlay {
  if (!obj(m)) return false;
  switch (m.t) {
    case "state": return Array.isArray(m.threads) && bool(m.commentMode) && bool(m.pending);
    case "comment-mode": return bool(m.on);
    case "close-composer": return pickId(m.pickId) && bool(m.posted);
    case "scroll-to": return ulid(m.threadId);
    case "focus": return m.threadId === null || ulid(m.threadId);
    case "snapshot-now": return true;
    default: return false;
  }
}

export function isFromComposer(m: unknown): m is ComposerToWorker {
  if (!obj(m)) return false;
  switch (m.t) {
    case "ready": case "cancel": return true;
    case "post": return text(m.body, MAX_BODY);
    default: return false;
  }
}

export function isFromPanel(m: unknown): m is PanelToWorker {
  if (!obj(m)) return false;
  const to = (v: unknown) => v === null || (typeof v === "string" && HANDLE.test(v));
  switch (m.t) {
    case "watch-tab": return Number.isSafeInteger(m.tabId) && (m.tabId as number) >= 0;
    case "send": return ulid(m.threadId) && to(m.to);
    case "send-batch": return Array.isArray(m.threadIds) && m.threadIds.length >= 1 && m.threadIds.length <= 20 && m.threadIds.every(ulid) && strOrNull(m.note, 280) && to(m.to);
    case "reply": return ulid(m.threadId) && text(m.body, MAX_BODY);
    case "resolve": case "reopen": case "delete": return ulid(m.threadId);
    case "looked": return Array.isArray(m.threadIds) && m.threadIds.length <= 50 && m.threadIds.every(ulid);
    case "set-name": return str(m.name, 64);
    case "select": return m.threadId === null || ulid(m.threadId);
    case "comment-mode": return bool(m.on);
    case "navigate": return m.route === null || str(m.route, 512);
    case "turn-off": case "retry": case "ping": return true;
    default: return false;
  }
}
