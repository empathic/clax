// Every message between the extension's parts (spec 2026-10-05 §9.4), and
// one validator per receiver. A receiver drops anything its validator
// refuses: content scripts share the page's DOM, so the worker treats what
// they send as checked input, never as trusted. The validators deny by
// default: a message is a plain object whose own fields are exactly its
// type's (optional ones aside), each checked for type and bounds; an
// unknown field, a field read through a prototype, or a symbol key refuses
// it.
import type { Anchor, AnchorResult } from "../../bridge/src/protocol";
import type { AnswerBody, InboxFilter, InboxItem, InboxKind, InboxPage, Participants, QuestionView, Version } from "../../shell/src/api";
import type { Thread } from "../../shell/src/threads";
import type { PresenceView } from "../../shell/src/view/presence-model";
import type { Working } from "../../shell/src/view/working-model";
import type { VersionTag } from "../../shell/src/view/history-model";

export const MAX_URL = 4096;
export const MAX_BODY = 10_000;
export const MAX_TITLE = 1000;
/** A transport bound in UTF-16 code units; the daemon enforces the 8 MiB cap on the snapshot's bytes. */
export const MAX_SNAPSHOT_CHARS = 8 * 1024 * 1024;
export const MAX_ROUTE = 512;
/** The longest clip the worker hands a panel, as a `data:` URL: a 5 MiB PNG (the daemon's bound) in base64. */
export const MAX_CLIP_URL = 7_000_000;
/** Why a pick carries a placeholder or no snapshot: the serializer's
 * `error`, or `failed` when the serializer threw. */
export type SnapshotError = "too_large" | "failed";
export const PICK_ID = /^[0-9a-f]{32}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const HANDLE = /^a_[0-9a-f]{22}$/;
const ARTIFACT_ID = /^[0-9a-hjkmnp-tv-z]{12}$/;
/** An inbox item's ID: a ULID, or `b` and 24 hex digits for an item filled in from the history. */
export const ITEM_ID = /^(?:[0-9A-HJKMNP-TV-Z]{26}|b[0-9a-f]{24})$/;
/** The most item IDs one bulk mark names (the daemon's bound). */
export const MAX_MARK = 500;
/** The longest answer text (the daemon's bound). */
export const MAX_ANSWER = 10_000;

export type Rect = { x: number; y: number; w: number; h: number };

/** Whether a thread is open and waits for a snapshot to link its address
 * to (spec L11): what both the overlay and the worker count as pending. */
export const waitsForSnapshot = (t: { status: string }) => t.status === "open" && !!(t as { addressed_pending?: unknown }).addressed_pending;
/** What the overlay hears of a thread (spec L7, §10.4): where it is
 * anchored (the page's own text), whether it is open, and whether it waits
 * for a snapshot; never its comments, replies, authors or agents. */
export type OverlayThread = { id: string; status: "open" | "resolved"; anchor: Anchor; addressed_pending: boolean;
  /** A thread of another page of the site (owner decision 2026-10-06): the
   * path it was left at. The overlay pins it wherever its anchor resolves,
   * whatever the route. */
  from?: string };
/** The most threads of the site's other pages the overlay hears of (the
 * newest) and resolves, after the page's own. */
export const MAX_FAR = 200;
/** The overlay's view of `t`. */
export const overlayThread = (t: Thread): OverlayThread => ({ id: t.id, status: t.status, anchor: t.anchor, addressed_pending: !!t.addressed_pending });
/** What the person is told when a page's address is over MAX_URL (the daemon's bound; spec §7). */
export const URL_TOO_LONG = "This page's address is too long for Clax.";
/** `href` as the overlay sends it: null when it is over MAX_URL. */
export const pageUrl = (href: string): string | null => (href.length > MAX_URL ? null : href);
/** `merged`: the page is a merge rule's canonical page, whose path is the rule's `pattern`. */
export type PageView = { artifact_id: string; origin: string; path: string; page_url: string; title: string; current_version: number; url: string; merged?: boolean; pattern?: string | null;
  /** A joined origin's page whose threads a join has not re-filed yet (the site listing's only). */
  pending?: boolean };
/** A merge rule of a site (`deleting` while its un-merge is under way). */
export type SiteRule = { id: string; origin: string; pattern: string; page_url: string; created_at: string; deleting?: boolean };
/** One origin of a site, as the daemon describes it. */
export type SiteOrigin = { origin: string; joined_at: string | null; last_used_at: string | null };
/** A site (spec §7.2): its key (the origin its pages are kept under), its
 * name (its most recently used origin), whether it joins several origins,
 * and its origins, the most recently used first. */
export type SiteInfo = { key: string; name: string; joined: boolean; origins: SiteOrigin[];
  /** Threads a join not finished has still to re-file (the listing's only). */
  joining?: number };
/** `GET /api/live/site`: the live pages of the origin's site that have
 * threads, each with its threads (resolved ones too), its merge rules, and
 * the site (absent from an older daemon). */
export type SiteView = { origin: string; site?: SiteInfo; rules: SiteRule[]; pages: { page: PageView; threads: Thread[] }[] };
/** A suggested join: the site the tab's origin may be the same app as, named by its newest origin. */
export type Suggestion = { origin: string; origins: string[]; reason: "path" | "title"; path: string | null };
/** A site of the listing `GET /api/live/sites`, as the panel offers it for "Same app as…". */
export type SiteChoice = { key: string; name: string; origins: string[] };

/** A `url` the overlay sends is null when the page's address is over
 * MAX_URL: the worker then tells the person, never looks it up. */
export type OverlayToWorker =
  | { t: "route"; url: string | null }
  /** The overlay names the pick (128 random bits); the worker takes it as
   * the tab's pick. The anchor comes with it, so the composer's draft does
   * not wait for the page's snapshot. */
  | { t: "capture"; pickId: string; anchor: Anchor; rect: Rect; dpr: number }
  /** The page's snapshot for the pick, serialized once its composer is shown. */
  | { t: "pick"; pickId: string; url: string | null; title: string; snapshot: string | null; snapshotError: SnapshotError | null }
  /** `pending`: the threads the overlay's state showed waiting for a snapshot when it serialized the page. */
  | { t: "quiet"; url: string; title: string; snapshot: string; pending: string[] }
  | { t: "resolved"; results: AnchorResult[] }
  | { t: "comment-mode"; on: boolean }
  | { t: "cancel"; pickId: string }
  | { t: "pin"; threadId: string }
  | { t: "removed" }
  | { t: "ping" };

export type WorkerToOverlay =
  | { t: "state"; page: PageView | null; route: string | null; threads: OverlayThread[]; commentMode: boolean; pending: boolean }
  /** The answer to `capture`: the screenshot was taken (`ok`), or why not. */
  | { t: "captured"; pickId: string; ok: boolean; error?: string }
  /** The worker took the pick: the overlay frames its composer beside `rect`, then serializes the page. */
  | { t: "open-composer"; pickId: string; rect: Rect }
  /** The pick's composer page connected to the worker (after its own load): only now is its frame shown and focused. */
  | { t: "composer-ready"; pickId: string }
  /** The composer's content is `height` CSS pixels tall: the overlay fits its frame to it. */
  | { t: "composer-size"; pickId: string; height: number }
  /** `reason: "timeout"`: the composer page never connected, which the overlay tells the person. */
  | { t: "close-composer"; pickId: string; posted: boolean; reason?: "timeout" }
  | { t: "scroll-to"; threadId: string }
  | { t: "focus"; threadId: string | null }
  /** The worker has no results for the threads it shows (it restarted): the overlay sends its current `resolved` again. */
  | { t: "resend" }
  /** The worker no longer holds the pick whose composer is shown (it was
   * stopped and started again): the composer stays, with its text, until the
   * person closes it, and comment mode comes back. */
  | { t: "pick-lost"; pickId: string }
  /** Clax turned off in the tab: the overlay stops, its pins and comment mode with it. */
  | { t: "off" };

/** `size`: the composer's content height in CSS pixels, which the worker relays to the overlay. */
export type ComposerToWorker = { t: "ready" } | { t: "post"; body: string } | { t: "cancel" } | { t: "size"; height: number };
/** The tallest composer content the overlay fits its frame to (it is clamped to the viewport besides). */
export const MAX_COMPOSER_HEIGHT = 2000;
const height = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 1 && v <= MAX_COMPOSER_HEIGHT;
/** What a composer page sends as a one-off runtime message once its port
 * is gone: `lost`, when the worker let go of the port unasked; `dismiss`,
 * when the person closes such a composer. */
export type ComposerNote = { t: "lost"; pickId: string } | { t: "dismiss"; pickId: string };
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
  /** `req`: the request (`rule`, `unrule`) that failed. */
  | { t: "failed"; code: string; message: string; req?: number }
  /** Whether the worker's event stream is up (told on `watch-tab` and on every change). */
  | { t: "stream-status"; up: boolean }
  /** Every thread of the tab's site (told on `watch-tab` and on every change); null until it is loaded. */
  | { t: "site"; site: SiteView | null }
  /** One batch of a rule's merge or un-merge (`rule`, `unrule`), a join or a split is done: `remaining` threads are left. */
  | { t: "step"; req: number; moved: number; remaining: number }
  /** Whether the tab's origin may be the same app as another site (null: no suggestion). */
  | { t: "suggestion"; origin: string; suggestion: Suggestion | null }
  /** Every site Clax has live pages of, for "Same app as…". */
  | { t: "sites"; sites: SiteChoice[] }
  /** The clip `clip` asked for, as a `data:image/png` URL; null when the thread has none or it could not be fetched. */
  | { t: "clip"; req: number; url: string | null }
  /** The page `far-page` asked for: its live agents and versions; null when the thread is not in the site's listing or the page could not be read. */
  | { t: "far-page"; req: number; page: FarPage | null }
  /** The owner's inbox (spec 2026-10-06-agent-questions-and-inbox §9.6),
   * told on a panel's connect and whenever it is fetched again: whether the
   * daemon takes the extension as the owner (null until it answered), the
   * unread count, and every open question, oldest first (each panel shows
   * those about its page). */
  | { t: "inbox"; owner: boolean | null; unread: number; questions: QuestionView[] }
  /** A question as it is now (a `question` event). */
  | { t: "q-event"; question: QuestionView }
  /** An inbox item as it is now (made, marked, or its source changed);
   * null: what the panel shows must be fetched again (a bulk mark). */
  | { t: "inbox-event"; item: InboxItem | null; unread: number }
  /** The answer to `q-answer`, `q-decline` or `q-release`: the question
   * after it, or (`closed`) as something else closed it first. */
  | { t: "q-done"; req: number; question: QuestionView; closed: boolean }
  /** The answer to `inbox-page`. */
  | { t: "inbox-page"; req: number; page: InboxPage }
  /** The answer to `inbox-mark` and `inbox-mark-all`: the item when one was marked, how many were, and the unread count after. */
  | { t: "marked"; req: number; item: InboxItem | null; marked: number; unread: number };
/** Another page of the tab's site, as a thread of it opened in the panel needs it. */
export type FarPage = { artifactId: string; agents: Participants["agents"]; versions: VersionTag[] };

export type PanelToWorker =
  | { t: "watch-tab"; tabId: number }
  /** `send`, `reply`, `resolve` and `reopen` act on a thread of the tab's
   * page, or of another page of its site (spec §7.1), at that thread's own page. */
  | { t: "send"; threadId: string; to: string | null }
  | { t: "send-batch"; threadIds: string[]; note: string | null; to: string | null }
  | { t: "reply"; threadId: string; body: string }
  | { t: "resolve"; threadId: string }
  | { t: "reopen"; threadId: string }
  | { t: "looked"; threadIds: string[] }
  | { t: "set-name"; name: string }
  | { t: "select"; threadId: string | null }
  | { t: "comment-mode"; on: boolean }
  /** `artifactId`: the live page the panel showed; the worker refuses it for another. */
  | { t: "navigate"; route: string | null; artifactId: string }
  /** Turns Clax off in the panel's tab, `tabId`; the worker refuses it for another. */
  | { t: "turn-off"; tabId: number }
  /** A thread of another page of the site: the tab goes to its page, where the overlay highlights it once found. */
  | { t: "open-thread"; threadId: string }
  /** A thread's clip (the tab's page's or another page's of its site), which the panel cannot fetch itself: answered by `clip`. */
  | { t: "clip"; req: number; threadId: string }
  /** The live agents and versions of the page of a thread of the site's listing: answered by `far-page`. */
  | { t: "far-page"; req: number; threadId: string }
  /** Moves a thread of the site to the page `pageUrl` names (of the tab's origin). */
  | { t: "move"; threadId: string; pageUrl: string }
  /** One batch of a new merge rule for `origin`, which must be the origin
   * Clax is on for in the panel's tab; the panel repeats it while `step`
   * says some remain. */
  | { t: "rule"; req: number; origin: string; pattern: string }
  /** One batch of deleting a merge rule of `origin` (un-merging); repeated as `rule`. */
  | { t: "unrule"; req: number; origin: string; ruleId: string }
  /** One batch of joining `origin` (the tab's) to the site of `with` (spec §7.2); repeated as `rule`. */
  | { t: "join"; req: number; origin: string; with: string }
  /** Splits `origin`, an origin of the tab's site, off it. */
  | { t: "split"; req: number; origin: string }
  /** Asks whether the tab's origin may be the same app as another site: answered by `suggestion`. */
  | { t: "suggest" }
  /** The owner's answer to the suggestion that the tab's origin is the same app as `with`. */
  | { t: "answer"; with: string; answer: "never" | "later" }
  /** Asks for every site Clax has live pages of: answered by `sites`. */
  | { t: "list-sites" }
  | { t: "retry" }
  /** The owner's questions and inbox (spec 2026-10-06-agent-questions-and-inbox
   * §9.6), on no tab: each is a request, answered by its `q-done`,
   * `inbox-page` or `marked`, or a `failed` with its `req`. */
  | { t: "q-answer"; req: number; questionId: string; body: AnswerBody }
  | { t: "q-decline"; req: number; questionId: string }
  /** "Answer in the terminal" (mirrored questions). */
  | { t: "q-release"; req: number; questionId: string }
  /** One page of the inbox matching `filter`, after cursor `before`. */
  | { t: "inbox-page"; req: number; filter: InboxFilter; before: string | null }
  /** Marks items read, or one item unread (`ids` names one then). */
  | { t: "inbox-mark"; req: number; ids: string[]; read: boolean }
  /** Marks every unread item matching `filter` read, up to `upto` (the newest `seq` shown). */
  | { t: "inbox-mark-all"; req: number; filter: InboxFilter | null; upto: number | null }
  /** Opens a daemon path (an item's `url`): a live page's item focuses a
   * tab showing that page when there is one (one Clax is on for it, else
   * one at `pageUrl`, the item's live page), else a new tab opens it on the
   * paired daemon. */
  | { t: "open-url"; url: string; pageUrl: string | null }
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
/** An origin as the daemon writes it: a scheme and a host, nothing after. */
const origin = (v: unknown) => str(v, MAX_URL) && /^https?:\/\/[^/?#]+$/.test(v);
const url = (v: unknown) => str(v, MAX_URL) && /^https?:\/\//.test(v);
const ulid = (v: unknown) => typeof v === "string" && ULID.test(v);
const pickId = (v: unknown) => typeof v === "string" && PICK_ID.test(v);
const text = (v: unknown, max: number) => str(v, max) && v.trim().length > 0;
/** The longest merge rule pattern, in bytes (the daemon's bound); a pattern is printable ASCII, so in characters too. */
export const MAX_PATTERN = 256;
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
const page = (v: unknown): v is PageView => shape(v, PAGE, ["merged", "pattern", "pending"])
  && (v.merged === undefined || bool(v.merged)) && (v.pending === undefined || bool(v.pending)) && (v.pattern === undefined || strOrNull(v.pattern, MAX_PATTERN)) && typeof v.artifact_id === "string" && ARTIFACT_ID.test(v.artifact_id)
  && str(v.origin, MAX_URL) && /^https?:\/\/[^/?#]+$/.test(v.origin) && str(v.path, MAX_URL) && v.path.startsWith("/")
  && url(v.page_url) && str(v.title, MAX_TITLE) && count(v.current_version) && url(v.url);

export function isFromOverlay(m: unknown): m is OverlayToWorker {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "route": return has("url") && (m.url === null || url(m.url));
    case "capture": return has("pickId", "anchor", "rect", "dpr") && pickId(m.pickId) && isAnchor(m.anchor) && box(m.rect) && num(m.dpr) && m.dpr > 0 && m.dpr <= 8;
    case "pick": return has("pickId", "url", "title", "snapshot", "snapshotError") && pickId(m.pickId) && (m.url === null || url(m.url))
      && str(m.title, MAX_TITLE) && strOrNull(m.snapshot, MAX_SNAPSHOT_CHARS) && (m.snapshotError === null || m.snapshotError === "too_large" || m.snapshotError === "failed")
      && (m.snapshot !== null || m.snapshotError !== null);
    case "quiet": return has("url", "title", "snapshot", "pending") && url(m.url) && str(m.title, MAX_TITLE) && str(m.snapshot, MAX_SNAPSHOT_CHARS)
      && Array.isArray(m.pending) && m.pending.length <= MAX_PENDING && m.pending.every(ulid);
    case "resolved": return has("results") && Array.isArray(m.results) && m.results.length <= 500 && m.results.every(result);
    case "comment-mode": return has("on") && bool(m.on);
    case "cancel": return has("pickId") && pickId(m.pickId);
    case "pin": return has("threadId") && ulid(m.threadId);
    case "removed": case "ping": return has();
    default: return false;
  }
}

const OVERLAY_THREAD = ["id", "status", "anchor", "addressed_pending"];
const path = (v: unknown) => str(v, MAX_URL) && v.startsWith("/");
/** An `OverlayThread`, and nothing more: no field of a thread's text reaches
 * the overlay. Its anchor is the daemon's, which the resolver reads
 * defensively, so only its kind is checked here. */
const thread = (v: unknown) => shape(v, OVERLAY_THREAD, ["from"]) && (v.from === undefined || path(v.from)) && ulid(v.id) && (v.status === "open" || v.status === "resolved")
  && obj(v.anchor) && typeof v.anchor.kind === "string" && bool(v.addressed_pending);

export function isFromWorker(m: unknown): m is WorkerToOverlay {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "state": return has("page", "route", "threads", "commentMode", "pending") && (m.page === null || page(m.page)) && strOrNull(m.route, MAX_ROUTE)
      && Array.isArray(m.threads) && m.threads.length <= 1000 && m.threads.every(thread) && bool(m.commentMode) && bool(m.pending);
    case "captured": return shape(m, ["t", "pickId", "ok"], ["error"]) && pickId(m.pickId) && bool(m.ok) && (m.error === undefined || code(m.error));
    case "open-composer": return has("pickId", "rect") && pickId(m.pickId) && box(m.rect);
    case "composer-ready": return has("pickId") && pickId(m.pickId);
    case "composer-size": return has("pickId", "height") && pickId(m.pickId) && height(m.height);
    case "close-composer": return shape(m, ["t", "pickId", "posted"], ["reason"]) && pickId(m.pickId) && bool(m.posted)
      && (m.reason === undefined || m.reason === "timeout");
    case "scroll-to": return has("threadId") && ulid(m.threadId);
    case "focus": return has("threadId") && (m.threadId === null || ulid(m.threadId));
    case "resend": case "off": return has();
    case "pick-lost": return has("pickId") && pickId(m.pickId);
    default: return false;
  }
}

/** The most sites the "Same app as…" list carries, and the most origins one site joins (the daemon's bound). */
export const MAX_SITES = 200;
export const MAX_SITE_ORIGINS = 16;
const origins = (v: unknown) => Array.isArray(v) && v.length >= 1 && v.length <= MAX_SITE_ORIGINS && v.every(origin);
const suggestion = (v: unknown) => shape(v, ["origin", "origins", "reason", "path"]) && origin(v.origin) && origins(v.origins)
  && (v.reason === "path" || v.reason === "title") && (v.path === null || path(v.path));
const siteChoice = (v: unknown) => shape(v, ["key", "name", "origins"]) && origin(v.key) && origin(v.name) && origins(v.origins);

/** What a side panel takes from the worker. The worker is trusted; this
 * keeps the panel to the messages it knows, of the right shape. */
/** A page's agents and versions, as the worker read them from the daemon: shown as text. */
const farPage = (v: unknown) => shape(v, ["artifactId", "agents", "versions"]) && typeof v.artifactId === "string" && ARTIFACT_ID.test(v.artifactId)
  && Array.isArray(v.agents) && v.agents.length <= 200 && v.agents.every(a => obj(a) && typeof a.handle === "string" && HANDLE.test(a.handle) && str(a.harness, 64) && bool(a.live))
  && Array.isArray(v.versions) && v.versions.length <= 100_000 && v.versions.every(x => shape(x, ["n", "created_at", "agent_harness"]) && count(x.n) && str(x.created_at, 64) && strOrNull(x.agent_harness, 64));
export function isToPanel(m: unknown): m is WorkerToPanel {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "tab": return has("state") && obj(m.state);
    case "failed": return shape(m, ["t", "code", "message"], ["req"]) && str(m.code, 64) && str(m.message, MAX_BODY) && (m.req === undefined || count(m.req));
    case "stream-status": return has("up") && bool(m.up);
    case "site": return has("site") && (m.site === null || obj(m.site));
    case "step": return has("req", "moved", "remaining") && count(m.req) && count(m.moved) && count(m.remaining);
    case "suggestion": return has("origin", "suggestion") && origin(m.origin) && (m.suggestion === null || suggestion(m.suggestion));
    case "sites": return has("sites") && Array.isArray(m.sites) && m.sites.length <= MAX_SITES && m.sites.every(siteChoice);
    case "clip": return has("req", "url") && count(m.req) && (m.url === null || (str(m.url, MAX_CLIP_URL) && m.url.startsWith("data:image/png;base64,")));
    case "far-page": return has("req", "page") && count(m.req) && (m.page === null || farPage(m.page));
    case "inbox": return has("owner", "unread", "questions") && (m.owner === null || bool(m.owner)) && count(m.unread) && Array.isArray(m.questions) && m.questions.every(obj);
    case "q-event": return has("question") && obj(m.question);
    case "inbox-event": return has("item", "unread") && (m.item === null || obj(m.item)) && count(m.unread);
    case "q-done": return has("req", "question", "closed") && count(m.req) && obj(m.question) && bool(m.closed);
    case "inbox-page": return has("req", "page") && count(m.req) && obj(m.page) && Array.isArray(m.page.items);
    case "marked": return has("req", "item", "marked", "unread") && count(m.req) && (m.item === null || obj(m.item)) && count(m.marked) && count(m.unread);
    default: return false;
  }
}

/** A composer page's one-off message (`ComposerNote`); the worker takes it
 * only from a composer frame of the tab (`composerTab`). */
export function isComposerNote(m: unknown): m is ComposerNote {
  return obj(m) && (m.t === "lost" || m.t === "dismiss") && shape(m, ["t", "pickId"]) && pickId(m.pickId);
}

export function isFromComposer(m: unknown): m is ComposerToWorker {
  if (!obj(m) || !Object.hasOwn(m, "t")) return false;
  const has = (...keys: string[]) => shape(m, ["t", ...keys]);
  switch (m.t) {
    case "ready": case "cancel": return has();
    case "post": return has("body") && text(m.body, MAX_BODY);
    case "size": return has("height") && height(m.height);
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

const INBOX_KINDS = new Set<string>(["reply", "version", "published", "question", "finished"] satisfies InboxKind[]);
const FILTER = ["q", "kind", "artifact", "agent", "since", "until", "read"];
/** A date (`YYYY-MM-DD`) or an RFC 3339 time; the daemon parses it. */
const when = (v: unknown) => str(v, 64) && /^\d{4}-\d{2}-\d{2}(?:[T ][0-9:.]+(?:Z|[+-]\d{2}:?\d{2})?)?$/.test(v);
/** An inbox search as the panel builds one; the daemon validates it again. */
const filter = (v: unknown): v is InboxFilter => shape(v, [], FILTER)
  && (v.q === undefined || str(v.q, 1000))
  && (v.kind === undefined || (Array.isArray(v.kind) && v.kind.length <= INBOX_KINDS.size && v.kind.every(k => typeof k === "string" && INBOX_KINDS.has(k))))
  && (v.artifact === undefined || (typeof v.artifact === "string" && ARTIFACT_ID.test(v.artifact)))
  && (v.agent === undefined || (typeof v.agent === "string" && /^[a-z0-9_-]{1,64}$/.test(v.agent)))
  && (v.since === undefined || when(v.since)) && (v.until === undefined || when(v.until))
  && (v.read === undefined || v.read === "unread" || v.read === "read" || v.read === "all");
/** One answer per question (one to four): at most four labels and a text, each within the daemon's bounds. */
const answerBody = (v: unknown): v is AnswerBody => shape(v, ["answers"]) && Array.isArray(v.answers) && v.answers.length >= 1 && v.answers.length <= 4
  && v.answers.every(a => shape(a, ["selected", "text"]) && Array.isArray(a.selected) && a.selected.length <= 4
    && a.selected.every(l => typeof l === "string" && l.length >= 1 && l.length <= 100) && strOrNull(a.text, MAX_ANSWER));
/** A path on the daemon: rooted at `/`, never `//` (another host) or a backslash, printable ASCII. */
export const daemonPath = (v: unknown): v is string => str(v, MAX_URL) && /^\/(?![/\\])[\x21-\x7e]*$/.test(v) && !v.includes("\\");
const itemId = (v: unknown) => typeof v === "string" && ITEM_ID.test(v);

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
    case "resolve": case "reopen": return has("threadId") && ulid(m.threadId);
    case "looked": return has("threadIds") && Array.isArray(m.threadIds) && m.threadIds.length <= 50 && m.threadIds.every(ulid);
    case "set-name": return has("name") && str(m.name, 64);
    case "select": return has("threadId") && (m.threadId === null || ulid(m.threadId));
    case "comment-mode": case "visible": return has("on") && bool(m.on);
    case "navigate": return has("route", "artifactId") && (m.route === null || str(m.route, MAX_ROUTE))
      && typeof m.artifactId === "string" && ARTIFACT_ID.test(m.artifactId);
    case "turn-off": return has("tabId") && count(m.tabId);
    case "open-thread": return has("threadId") && ulid(m.threadId);
    case "clip": case "far-page": return has("req", "threadId") && count(m.req) && ulid(m.threadId);
    case "move": return has("threadId", "pageUrl") && ulid(m.threadId) && url(m.pageUrl);
    case "rule": return has("req", "origin", "pattern") && count(m.req) && origin(m.origin)
      && typeof m.pattern === "string" && m.pattern.length <= MAX_PATTERN && /^\/[\x21-\x7e]*$/.test(m.pattern);
    case "unrule": return has("req", "origin", "ruleId") && count(m.req) && origin(m.origin) && ulid(m.ruleId);
    case "join": return has("req", "origin", "with") && count(m.req) && origin(m.origin) && origin(m.with) && m.origin !== m.with;
    case "split": return has("req", "origin") && count(m.req) && origin(m.origin);
    case "answer": return has("with", "answer") && origin(m.with) && (m.answer === "never" || m.answer === "later");
    case "suggest": case "list-sites": return has();
    case "q-answer": return has("req", "questionId", "body") && count(m.req) && ulid(m.questionId) && answerBody(m.body);
    case "q-decline": case "q-release": return has("req", "questionId") && count(m.req) && ulid(m.questionId);
    case "inbox-page": return has("req", "filter", "before") && count(m.req) && filter(m.filter) && (m.before === null || (typeof m.before === "string" && /^\d{1,20}$/.test(m.before)));
    case "inbox-mark": return has("req", "ids", "read") && count(m.req) && bool(m.read) && Array.isArray(m.ids) && m.ids.length >= 1
      && m.ids.length <= (m.read ? MAX_MARK : 1) && m.ids.every(itemId);
    case "inbox-mark-all": return has("req", "filter", "upto") && count(m.req) && (m.filter === null || filter(m.filter)) && (m.upto === null || count(m.upto));
    case "open-url": return has("url", "pageUrl") && daemonPath(m.url) && (m.pageUrl === null || url(m.pageUrl));
    case "retry": case "ping": return has();
    default: return false;
  }
}

/** Failures a new pairing can fix: the native host, the credential, or a
 * pairing whose daemon is gone (restarted on another port, or stopped; the
 * native host answers the running one, starting it if need be). The panel
 * offers Retry for these, and Retry pairs again. */
export const RETRYABLE = /* @__PURE__ */ new Set(["host_missing", "host_failed", "daemon_unavailable", "bad_reply", "unknown_credential", "http_401", "paired_recently", "daemon_unreachable", "host_slow"]);
