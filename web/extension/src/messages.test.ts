import { describe, expect, it } from "vitest";
import { MAX_BODY, MAX_SNAPSHOT_CHARS, MAX_URL, isAnchor, isComposerNote, isFromComposer, isFromOverlay, isFromPanel, isFromWorker, isToComposer, isToPanel } from "./messages";

const anchor = { kind: "element", selector: "main > button", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const ULID = "01J9ZQ3V7K8M2N4P6R8T0V2X4Y";
const ot = (extra: Record<string, unknown> = {}) => ({ id: ULID, status: "open", anchor, addressed_pending: false, ...extra });

describe("messages", () => {
  it("takes the overlay's well-formed messages", () => {
    expect(isFromOverlay({ t: "hello", url: "http://localhost:5173/" })).toBe(false);
    expect(isFromOverlay({ t: "capture", pickId: "b".repeat(32), anchor, rect: { x: 1, y: 2, w: 3, h: 4 }, dpr: 2 })).toBe(true);
    expect(isFromOverlay({ t: "pick", pickId: "a".repeat(32), url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null })).toBe(true);
    expect(isFromOverlay({ t: "quiet", url: "https://x/", title: "", snapshot: "<p>", pending: [ULID] })).toBe(true);
    expect(isFromOverlay({ t: "resolved", results: [{ id: ULID, found: true, method: null, rect: null }] })).toBe(true);
    expect(isFromOverlay({ t: "cancel", pickId: "c".repeat(32) })).toBe(true);
    expect(isFromOverlay({ t: "cancel", pickId: null })).toBe(false);
    // An address over MAX_URL goes as null, which the worker tells the person of.
    expect(isFromOverlay({ t: "route", url: null })).toBe(true);
    expect(isFromOverlay({ t: "route", url: null })).toBe(true);
    expect(isFromOverlay({ t: "pick", pickId: "a".repeat(32), url: null, title: "T", snapshot: "<p>", snapshotError: null })).toBe(true);
    expect(isFromOverlay({ t: "quiet", url: null, title: "", snapshot: "<p>", pending: [] })).toBe(false);
    expect(isFromOverlay({ t: "pin", threadId: ULID })).toBe(true);
    expect(isFromOverlay({ t: "ping" })).toBe(true);
  });

  it("drops anything else", () => {
    expect(isFromOverlay(null)).toBe(false);
    expect(isFromOverlay({ t: "route" })).toBe(false);
    expect(isFromOverlay({ t: "route", url: "x".repeat(MAX_URL + 1) })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: Number.NaN, y: 0, w: 1, h: 1 }, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId: "short", url: "http://x/", title: "T", snapshot: null, snapshotError: "too_large" })).toBe(false);
    expect(isFromOverlay({ t: "steal", url: "http://x/" })).toBe(false);
    expect(isFromComposer({ t: "post", body: "x".repeat(MAX_BODY + 1) })).toBe(false);
    expect(isFromPanel({ t: "send", threadId: "not a ulid" })).toBe(false);
  });

  it("refuses hostile shapes from the page's side", () => {
    expect(isFromOverlay([{ t: "ping" }])).toBe(false);
    expect(isFromOverlay("ping")).toBe(false);
    expect(isFromOverlay({ t: "route", url: "javascript:alert(1)" })).toBe(false);
    expect(isFromOverlay({ t: "route", url: "chrome-extension://abc/x" })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: -1, h: 1 }, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: 1, h: 1 }, dpr: 0 })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: 1, h: 1 }, dpr: Infinity })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: "0", y: 0, w: 1, h: 1 }, dpr: 1 })).toBe(false);
    // A pick needs a snapshot or the reason it has none.
    expect(isFromOverlay({ t: "pick", pickId: "a".repeat(32), url: "http://x/", title: "T", snapshot: null, snapshotError: null })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId: "A".repeat(32), url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null })).toBe(false);
    expect(isFromOverlay({ t: "quiet", url: "http://x/", title: "T", snapshot: "x".repeat(MAX_SNAPSHOT_CHARS + 1) })).toBe(false);
    expect(isFromOverlay({ t: "quiet", url: "http://x/", title: 7, snapshot: "<p>" })).toBe(false);
    expect(isFromOverlay({ t: "resolved", results: new Array(501).fill({ id: ULID, found: true }) })).toBe(false);
    expect(isFromOverlay({ t: "resolved", results: [{ id: "../x", found: true }] })).toBe(false);
    expect(isFromOverlay({ t: "comment-mode", on: "yes" })).toBe(false);
    expect(isFromOverlay({ t: "pin", threadId: ULID.toLowerCase() })).toBe(false);
  });

  it("refuses a message whose type or fields come through its prototype", () => {
    expect(isFromOverlay(JSON.parse(`{"t": "ping", "__proto__": {"t": "ping"}}`))).toBe(false);
    expect(isFromOverlay(Object.create({ t: "ping" }))).toBe(false);
    expect(isFromOverlay(Object.assign(Object.create({ url: "http://x/" }), { t: "route" }))).toBe(false);
    expect(isFromOverlay(Object.assign(Object.create(null), { t: "ping" }))).toBe(true);
    expect(isFromOverlay(new (class { t = "ping"; })())).toBe(false);
    expect(isFromOverlay({ t: "ping", [Symbol("x")]: 1 })).toBe(false);
  });

  it("checks each result the overlay resolved, field by field", () => {
    const ok = { id: ULID, found: true, method: "selector", rect: { x: 1, y: 2, w: 3, h: 4 } };
    expect(isFromOverlay({ t: "resolved", results: [ok, { id: ULID, found: false, method: null, rect: null }] })).toBe(true);
    for (const r of [
      { ...ok, method: "guess" },
      { ...ok, method: 1 },
      { ...ok, rect: { x: 1, y: 2, w: 3 } },
      { ...ok, rect: { x: 1, y: 2, w: 3, h: Number.NaN } },
      { ...ok, rect: { x: 1, y: 2, w: 3, h: 4, z: 0 } },
      { ...ok, rect: "1,2,3,4" },
      { id: ULID, found: true },
      { ...ok, html: "<script>" },
    ]) expect(isFromOverlay({ t: "resolved", results: [r] }), JSON.stringify(r)).toBe(false);
  });

  it("takes a snapshot error only as a known reason", () => {
    const pick = { t: "pick", pickId: "a".repeat(32), url: "http://x/", title: "T", snapshot: "<p>placeholder</p>", snapshotError: "too_large" };
    expect(isFromOverlay(pick)).toBe(true);
    expect(isFromOverlay({ ...pick, snapshot: null })).toBe(true);
    expect(isFromOverlay({ ...pick, snapshotError: "<img onerror>" })).toBe(false);
    expect(isFromOverlay({ ...pick, snapshotError: "" })).toBe(false);
  });

  it("checks the page and route of the worker's state", () => {
    const page = { artifact_id: "0123456789ab", origin: "http://localhost:5173", path: "/settings", page_url: "http://localhost:5173/settings", title: "Settings", current_version: 3, url: "http://127.0.0.1:7481/a/0123456789ab" };
    const state = { t: "state", page, route: "?tab=2", threads: [ot(), ot({ status: "resolved", addressed_pending: true })], commentMode: true, pending: false };
    expect(isFromWorker(state)).toBe(true);
    expect(isFromWorker({ ...state, page: null, route: null })).toBe(true);
    for (const bad of [
      { ...state, route: "x".repeat(513) },
      { ...state, route: 5 },
      { ...state, page: { ...page, artifact_id: "../../etc" } },
      { ...state, page: { ...page, url: "javascript:alert(1)" } },
      { ...state, page: { ...page, page_url: "file:///etc/passwd" } },
      { ...state, page: { ...page, origin: "http://x/path" } },
      { ...state, page: { ...page, path: "settings" } },
      { ...state, page: { ...page, current_version: -1 } },
      { ...state, page: { ...page, current_version: 1.5 } },
      { ...state, page: { ...page, title: "x".repeat(1001) } },
      { ...state, page: { ...page, extra: 1 } },
      { ...state, page: { ...page, url: undefined } },
      { ...state, threads: [ot({ id: "x" })] },
      { ...state, threads: [null] },
      { ...state, threads: new Array(1001).fill(ot()) },
      // The overlay hears no thread text: a thread with any other field is refused.
      { ...state, threads: [{ id: ULID }] },
      { ...state, threads: [ot({ comments: [{ body: "secret" }] })] },
      { ...state, threads: [ot({ author: "Mia" })] },
      { ...state, threads: [ot({ addressed_pending: { harness: "claude", at: "t" } })] },
      { ...state, threads: [ot({ status: "deleted" })] },
      { ...state, threads: [ot({ anchor: null })] },
    ]) expect(isFromWorker(bad), JSON.stringify(bad).slice(0, 200)).toBe(false);
  });

  it("takes a merged page, and a thread of another page with the path it was left at, and nothing more", () => {
    const page = { artifact_id: "0123456789ab", origin: "http://localhost:5173", path: "/users/:id", page_url: "http://localhost:5173/users/:id", title: "Users", current_version: 1, url: "http://127.0.0.1:7481/a/0123456789ab", merged: true, pattern: "/users/:id" };
    const state = { t: "state", page, route: null, threads: [ot(), ot({ from: "/users/7" })], commentMode: false, pending: false };
    expect(isFromWorker(state)).toBe(true);
    expect(isFromWorker({ ...state, page: { ...page, merged: false, pattern: null } })).toBe(true);
    for (const bad of [
      { ...state, page: { ...page, merged: "yes" } },
      { ...state, page: { ...page, pattern: 7 } },
      { ...state, page: { ...page, pattern: "x".repeat(257) } },
      { ...state, threads: [ot({ from: "users/7" })] },
      { ...state, threads: [ot({ from: null })] },
      { ...state, threads: [ot({ from: `/${"x".repeat(MAX_URL)}` })] },
      { ...state, threads: [ot({ from: "/a", page_url: "http://localhost:5173/a" })] },
    ]) expect(isFromWorker(bad), JSON.stringify(bad).slice(0, 200)).toBe(false);
  });

  it("checks the panel's site-wide requests and the worker's answers", () => {
    expect(isFromPanel({ t: "open-thread", threadId: ULID })).toBe(true);
    expect(isFromPanel({ t: "open-thread", threadId: "x" })).toBe(false);
    expect(isFromPanel({ t: "open-thread", threadId: ULID, url: "http://localhost:5173/" })).toBe(false);
    expect(isFromPanel({ t: "move", threadId: ULID, pageUrl: "http://localhost:5173/users/7" })).toBe(true);
    for (const pageUrl of ["javascript:alert(1)", "/users/7", `http://x/${"a".repeat(MAX_URL)}`, null]) expect(isFromPanel({ t: "move", threadId: ULID, pageUrl })).toBe(false);
    const O = "http://localhost:5173";
    expect(isFromPanel({ t: "rule", req: 1, origin: O, pattern: "/users/:id" })).toBe(true);
    // 256 bytes at most: printable ASCII only, so no character counts for more than one.
    for (const pattern of ["users/:id", "", `/${"x".repeat(256)}`, 5, "/ü/:id", "/a b/:id", `/${"é".repeat(200)}`]) expect(isFromPanel({ t: "rule", req: 1, origin: O, pattern }), String(pattern)).toBe(false);
    expect(isFromPanel({ t: "rule", req: -1, origin: O, pattern: "/users/:id" })).toBe(false);
    expect(isFromPanel({ t: "rule", req: 1, pattern: "/users/:id" })).toBe(false);
    for (const origin of ["http://localhost:5173/", "javascript:x", "localhost"]) expect(isFromPanel({ t: "rule", req: 1, origin, pattern: "/users/:id" })).toBe(false);
    expect(isFromPanel({ t: "unrule", req: 2, origin: O, ruleId: ULID })).toBe(true);
    expect(isFromPanel({ t: "unrule", req: 2, origin: O, ruleId: "../x" })).toBe(false);
    expect(isFromPanel({ t: "unrule", req: 2, ruleId: ULID })).toBe(false);
    expect(isToPanel({ t: "site", site: null })).toBe(true);
    expect(isToPanel({ t: "site", site: { origin: "http://localhost:5173", rules: [], pages: [] } })).toBe(true);
    expect(isToPanel({ t: "site", site: [] })).toBe(false);
    expect(isToPanel({ t: "step", req: 1, moved: 200, remaining: 3 })).toBe(true);
    expect(isToPanel({ t: "step", req: 1, moved: -1, remaining: 3 })).toBe(false);
    expect(isToPanel({ t: "step", req: 1, moved: 0 })).toBe(false);
    expect(isToPanel({ t: "failed", code: "invalid_pattern", message: "m", req: 4 })).toBe(true);
    expect(isToPanel({ t: "failed", code: "x", message: "m", req: "4" })).toBe(false);
  });

  it("checks the panel's joined-site requests and the worker's answers, refusing anything else", () => {
    const O = "http://localhost:5173", A = "http://localhost:7702";
    expect(isFromPanel({ t: "join", req: 1, origin: O, with: A })).toBe(true);
    expect(isFromPanel({ t: "join", req: 1, origin: O, with: O })).toBe(false);
    expect(isFromPanel({ t: "join", req: 1, origin: O, with: `${A}/x` })).toBe(false);
    expect(isFromPanel({ t: "join", origin: O, with: A })).toBe(false);
    expect(isFromPanel({ t: "join", req: 1, origin: O, with: A, extra: 1 })).toBe(false);
    expect(isFromPanel({ t: "split", req: 2, origin: A })).toBe(true);
    expect(isFromPanel({ t: "split", req: 2, origin: "javascript:x" })).toBe(false);
    expect(isFromPanel({ t: "suggest" })).toBe(true);
    expect(isFromPanel({ t: "suggest", url: O })).toBe(false);
    expect(isFromPanel({ t: "list-sites" })).toBe(true);
    expect(isFromPanel({ t: "answer", with: A, answer: "never" })).toBe(true);
    expect(isFromPanel({ t: "answer", with: A, answer: "later" })).toBe(true);
    expect(isFromPanel({ t: "answer", with: A, answer: "always" })).toBe(false);
    expect(isFromPanel({ t: "answer", with: "localhost", answer: "never" })).toBe(false);
    const sug = { origin: A, origins: [A], reason: "path", path: "/settings" };
    expect(isToPanel({ t: "suggestion", origin: O, suggestion: sug })).toBe(true);
    expect(isToPanel({ t: "suggestion", origin: O, suggestion: null })).toBe(true);
    for (const bad of [{ ...sug, reason: "port" }, { ...sug, origins: [] }, { ...sug, path: "settings" }, { ...sug, extra: 1 }, { ...sug, origins: Array(17).fill(A) }]) {
      expect(isToPanel({ t: "suggestion", origin: O, suggestion: bad }), JSON.stringify(bad)).toBe(false);
    }
    expect(isToPanel({ t: "sites", sites: [{ key: A, name: O, origins: [O, A] }] })).toBe(true);
    expect(isToPanel({ t: "sites", sites: [{ key: A, name: O, origins: [O, A], pages: 2 }] })).toBe(false);
    expect(isToPanel({ t: "sites", sites: [{ key: "x", name: O, origins: [O] }] })).toBe(false);
    expect(isToPanel({ t: "sites", sites: {} })).toBe(false);
  });

  it("checks what the worker sends the overlay", () => {
    expect(isFromWorker({ t: "state", page: null, route: null, threads: [], commentMode: false, pending: false })).toBe(true);
    expect(isFromWorker({ t: "pick-lost", pickId: "a".repeat(32) })).toBe(true);
    expect(isFromWorker({ t: "pick-lost", pickId: "x" })).toBe(false);
    // Types no part sends.
    for (const t of ["snapshot-now", "comment-mode", "stream-status"]) expect(isFromWorker({ t, on: true, up: true })).toBe(false);
    expect(isFromWorker({ t: "snapshot-now" })).toBe(false);
    expect(isFromWorker({ t: "resend" })).toBe(true);
    expect(isFromWorker({ t: "off" })).toBe(true);
    expect(isFromWorker({ t: "focus", threadId: null })).toBe(true);
    expect(isFromWorker({ t: "state", threads: "none", commentMode: false, pending: false })).toBe(false);
    expect(isFromWorker({ t: "scroll-to", threadId: "x" })).toBe(false);
    expect(isFromWorker({ t: "post", body: "hi" })).toBe(false);
  });

  it("checks the stream's status as the worker tells the panels", () => {
    expect(isFromWorker({ t: "stream-status", up: false })).toBe(false);
    expect(isToPanel({ t: "stream-status", up: true })).toBe(true);
    expect(isToPanel({ t: "stream-status" })).toBe(false);
    expect(isToPanel({ t: "failed", code: "x", message: "y" })).toBe(true);
    expect(isToPanel({ t: "tab", state: { tabId: 1 } })).toBe(true);
    expect(isToPanel({ t: "tab", state: null })).toBe(false);
    expect(isToPanel({ t: "ping" })).toBe(false);
    expect(isToPanel({ t: "state" })).toBe(false);
  });

  it("checks the composer's and the side panel's messages", () => {
    expect(isFromComposer({ t: "post", body: "Make this bigger" })).toBe(true);
    expect(isFromComposer({ t: "post", body: "   \n" })).toBe(false);
    expect(isFromComposer({ t: "ready" })).toBe(true);
    expect(isFromPanel({ t: "send", threadId: ULID, to: null })).toBe(true);
    expect(isFromPanel({ t: "send", threadId: ULID, to: "a_0123456789abcdef012345" })).toBe(true);
    expect(isFromPanel({ t: "send", threadId: ULID, to: "someone" })).toBe(false);
    expect(isFromPanel({ t: "send-batch", threadIds: [], note: null, to: null })).toBe(false);
    expect(isFromPanel({ t: "send-batch", threadIds: new Array(21).fill(ULID), note: null, to: null })).toBe(false);
    expect(isFromPanel({ t: "send-batch", threadIds: [ULID], note: "x".repeat(281), to: null })).toBe(false);
    expect(isFromPanel({ t: "reply", threadId: ULID, body: "" })).toBe(false);
    expect(isFromPanel({ t: "set-name", name: "x".repeat(65) })).toBe(false);
    expect(isFromPanel({ t: "navigate", route: "x".repeat(513), artifactId: "7q3k9mzx2b4t" })).toBe(false);
    expect(isFromPanel({ t: "navigate", route: null, artifactId: "NOT-AN-ID" })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: Number.NaN })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: 1.5 })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: -1 })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: 7 })).toBe(true);
    expect(isFromPanel({ t: "turn-off", tabId: 3 })).toBe(true);
    expect(isFromPanel({ t: "turn-off" })).toBe(false);
    expect(isFromPanel({ t: "turn-off", tabId: -1 })).toBe(false);
    expect(isFromPanel({ t: "turn-off", origin: "http://localhost:5173" })).toBe(false);
    expect(isFromPanel({ t: "visible", on: false })).toBe(true);
    expect(isFromPanel({ t: "visible", on: "yes" })).toBe(false);
    expect(isFromPanel({ t: "delete", threadId: ULID })).toBe(false);
  });

  it("checks a composer page's one-off messages", () => {
    expect(isComposerNote({ t: "lost", pickId: "a".repeat(32) })).toBe(true);
    expect(isComposerNote({ t: "dismiss", pickId: "a".repeat(32) })).toBe(true);
    expect(isComposerNote({ t: "dismiss", pickId: "a".repeat(32), extra: 1 })).toBe(false);
    expect(isComposerNote({ t: "lost" })).toBe(false);
    expect(isComposerNote({ t: "cancel", pickId: "a".repeat(32) })).toBe(false);
  });

  it("refuses an unknown field on every message", () => {
    const pickId = "a".repeat(32);
    const overlay = [
      { t: "route", url: "http://x/#/a" }, { t: "capture", pickId, anchor, rect: { x: 0, y: 0, w: 1, h: 1 }, dpr: 1 },
      { t: "pick", pickId, url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null },
      { t: "quiet", url: "http://x/", title: "T", snapshot: "<p>", pending: [] }, { t: "resolved", results: [] }, { t: "comment-mode", on: true },
      { t: "cancel", pickId }, { t: "pin", threadId: ULID }, { t: "removed" }, { t: "ping" },
    ];
    const worker = [
      { t: "state", page: null, route: null, threads: [ot()], commentMode: false, pending: false }, { t: "pick-lost", pickId },
      { t: "close-composer", pickId, posted: true }, { t: "scroll-to", threadId: ULID }, { t: "focus", threadId: null },
      { t: "resend" }, { t: "off" }, { t: "captured", pickId, ok: true }, { t: "captured", pickId, ok: false, error: "no_capture_permission" },
      { t: "open-composer", pickId, rect: { x: 0, y: 0, w: 1, h: 1 } }, { t: "composer-ready", pickId },
    ];
    const composer = [{ t: "ready" }, { t: "post", body: "hi" }, { t: "cancel" }];
    const panel = [
      { t: "watch-tab", tabId: 1 }, { t: "send", threadId: ULID, to: null }, { t: "send-batch", threadIds: [ULID], note: null, to: null },
      { t: "reply", threadId: ULID, body: "ok" }, { t: "resolve", threadId: ULID }, { t: "reopen", threadId: ULID },
      { t: "looked", threadIds: [ULID] }, { t: "set-name", name: "Ana" }, { t: "select", threadId: null }, { t: "comment-mode", on: true },
      { t: "navigate", route: null, artifactId: "7q3k9mzx2b4t" }, { t: "turn-off", tabId: 3 }, { t: "retry" }, { t: "ping" }, { t: "visible", on: true },
    ];
    const toComposer = [
      { t: "draft", anchor, clipUrl: "data:image/png;base64,iVBORw0KGgo=", clipError: null, capturing: false },
      { t: "draft", anchor, clipUrl: null, clipError: "no_capture_permission", capturing: false },
      { t: "posted", threadId: ULID }, { t: "failed", message: "The daemon is not running." },
    ];
    for (const [check, list] of [[isFromOverlay, overlay], [isFromWorker, worker], [isFromComposer, composer], [isFromPanel, panel], [isToComposer, toComposer]] as const) {
      for (const m of list) {
        expect(check(m), JSON.stringify(m)).toBe(true);
        expect(check({ ...m, extra: 1 }), `${JSON.stringify(m)} + extra`).toBe(false);
      }
    }
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: 1, h: 1, z: 1 }, dpr: 1 })).toBe(false);
    // A required field that is absent is refused, not read as undefined.
    expect(isFromPanel({ t: "send", threadId: ULID })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId, url: "http://x/", title: "T", snapshot: "<p>" })).toBe(false);
  });

  it("checks the pick's messages field by field", () => {
    const pickId = "c".repeat(32);
    const rect = { x: 0, y: 0, w: 1, h: 1 };
    // The capture names the pick it is for (spec §9.4).
    expect(isFromOverlay({ t: "capture", rect, dpr: 1 })).toBe(false);
    // The capture carries the pick's anchor, so the draft does not wait for the snapshot.
    expect(isFromOverlay({ t: "capture", pickId, rect, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "capture", pickId, anchor: { ...anchor, kind: "script" }, rect, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId, anchor, url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null })).toBe(false);
    // A serializer that failed sends no snapshot, and says so.
    expect(isFromOverlay({ t: "pick", pickId, url: "http://x/", title: "T", snapshot: null, snapshotError: "failed" })).toBe(true);
    expect(isFromWorker({ t: "composer-ready", pickId: "short" })).toBe(false);
    expect(isFromWorker({ t: "close-composer", pickId, posted: false, reason: "timeout" })).toBe(true);
    expect(isFromWorker({ t: "close-composer", pickId, posted: false, reason: "other" })).toBe(false);
    expect(isFromWorker({ t: "composer-ready" })).toBe(false);
    expect(isFromOverlay({ t: "capture", pickId: "C".repeat(32), rect, dpr: 1 })).toBe(false);
    // A quiet snapshot names the pending threads it covers.
    expect(isFromOverlay({ t: "quiet", url: "http://x/", title: "T", snapshot: "<p>" })).toBe(false);
    expect(isFromOverlay({ t: "quiet", url: "http://x/", title: "T", snapshot: "<p>", pending: ["x"] })).toBe(false);
    expect(isFromOverlay({ t: "quiet", url: "http://x/", title: "T", snapshot: "<p>", pending: new Array(1001).fill(ULID) })).toBe(false);
    expect(isFromWorker({ t: "captured", pickId, ok: "yes" })).toBe(false);
    expect(isFromWorker({ t: "captured", pickId, ok: false, error: "x".repeat(65) })).toBe(false);
    expect(isFromWorker({ t: "captured", pickId: "short", ok: true })).toBe(false);
    expect(isFromWorker({ t: "open-composer", pickId, rect: { ...rect, w: -1 } })).toBe(false);
    expect(isFromWorker({ t: "open-composer", pickId })).toBe(false);
    expect(isToComposer({ t: "draft", anchor: { ...anchor, kind: "script" }, clipUrl: null, clipError: null, capturing: false })).toBe(false);
    expect(isToComposer({ t: "draft", anchor, clipUrl: "https://x/a.png", clipError: null, capturing: false })).toBe(false);
    expect(isToComposer({ t: "draft", anchor, clipUrl: null, clipError: null })).toBe(false);
    expect(isToComposer({ t: "posted", threadId: "x" })).toBe(false);
    expect(isToComposer({ t: "failed", message: 5 })).toBe(false);
    expect(isToComposer({ t: "ready" })).toBe(false);
  });

  it("checks anchors field by field", () => {
    expect(isAnchor(anchor)).toBe(true);
    expect(isAnchor({ ...anchor, kind: "area", area: { x: 0.1, y: 0.2, w: 0.3, h: 0.4 }, rect: { x: 1, y: 2, w: 3, h: 4, scrollX: 0, scrollY: 0, viewportW: 800 } })).toBe(true);
    expect(isAnchor({ ...anchor, kind: "script" })).toBe(false);
    expect(isAnchor({ ...anchor, selector: "x".repeat(1025) })).toBe(false);
    expect(isAnchor({ ...anchor, file: "../etc" })).toBe(false);
    expect(isAnchor({ ...anchor, route: "?a" })).toBe(false); // the daemon sets routes
    expect(isAnchor({ ...anchor, custom_name: "mine" })).toBe(false);
    expect(isAnchor({ ...anchor, rect: { x: 1, y: 2, w: 3, h: 4 } })).toBe(false);
    expect(isAnchor({ ...anchor, quote: 5 })).toBe(false);
    expect(isAnchor({ ...anchor, prefix: "x".repeat(65) })).toBe(false);
    expect(isAnchor({ ...anchor, onclick: "x" })).toBe(false);
    expect(isAnchor({ ...anchor, rect: { x: 1, y: 2, w: 3, h: 4, scrollX: 0, scrollY: 0, viewportW: 800, z: 1 } })).toBe(false);
    const area = { x: 0.1, y: 0.2, w: 0.3, h: 0.4, tag: "section", text: "Quarterly goals", children: 3 };
    expect(isAnchor({ ...anchor, kind: "area", area })).toBe(true);
    for (const bad of [
      { ...area, x: 1.5 }, { ...area, w: 0 }, { ...area, x: 0.9, w: 0.5 }, { ...area, h: -0.1 },
      { ...area, tag: "x".repeat(65) }, { ...area, text: "a\nb" }, { ...area, text: "a\u2028b" }, { ...area, tag: 5 },
      { ...area, children: -1 }, { ...area, children: 1.5 }, { ...area, extra: 1 },
    ]) expect(isAnchor({ ...anchor, kind: "area", area: bad }), JSON.stringify(bad)).toBe(false);
  });
});
