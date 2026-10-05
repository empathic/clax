import { describe, expect, it } from "vitest";
import { MAX_BODY, MAX_SNAPSHOT_CHARS, MAX_URL, isAnchor, isFromComposer, isFromOverlay, isFromPanel, isFromWorker } from "./messages";

const anchor = { kind: "element", selector: "main > button", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const ULID = "01J9ZQ3V7K8M2N4P6R8T0V2X4Y";

describe("messages", () => {
  it("takes the overlay's well-formed messages", () => {
    expect(isFromOverlay({ t: "hello", url: "http://localhost:5173/" })).toBe(true);
    expect(isFromOverlay({ t: "capture", rect: { x: 1, y: 2, w: 3, h: 4 }, dpr: 2 })).toBe(true);
    expect(isFromOverlay({ t: "pick", pickId: "a".repeat(32), anchor, url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null })).toBe(true);
    expect(isFromOverlay({ t: "quiet", url: "https://x/", title: "", snapshot: "<p>" })).toBe(true);
    expect(isFromOverlay({ t: "resolved", results: [{ id: ULID, found: true, method: null, rect: null }] })).toBe(true);
    expect(isFromOverlay({ t: "cancel", pickId: null })).toBe(true);
    expect(isFromOverlay({ t: "pin", threadId: ULID })).toBe(true);
    expect(isFromOverlay({ t: "ping" })).toBe(true);
  });

  it("drops anything else", () => {
    expect(isFromOverlay(null)).toBe(false);
    expect(isFromOverlay({ t: "hello" })).toBe(false);
    expect(isFromOverlay({ t: "hello", url: "x".repeat(MAX_URL + 1) })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: Number.NaN, y: 0, w: 1, h: 1 }, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId: "short", anchor, url: "http://x/", title: "T", snapshot: null, snapshotError: "too_large" })).toBe(false);
    expect(isFromOverlay({ t: "steal", url: "http://x/" })).toBe(false);
    expect(isFromComposer({ t: "post", body: "x".repeat(MAX_BODY + 1) })).toBe(false);
    expect(isFromPanel({ t: "send", threadId: "not a ulid" })).toBe(false);
  });

  it("refuses hostile shapes from the page's side", () => {
    expect(isFromOverlay([{ t: "ping" }])).toBe(false);
    expect(isFromOverlay("ping")).toBe(false);
    expect(isFromOverlay({ t: "hello", url: "javascript:alert(1)" })).toBe(false);
    expect(isFromOverlay({ t: "hello", url: "chrome-extension://abc/x" })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: -1, h: 1 }, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: 1, h: 1 }, dpr: 0 })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: 1, h: 1 }, dpr: Infinity })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: "0", y: 0, w: 1, h: 1 }, dpr: 1 })).toBe(false);
    // A pick needs a snapshot or the reason it has none.
    expect(isFromOverlay({ t: "pick", pickId: "a".repeat(32), anchor, url: "http://x/", title: "T", snapshot: null, snapshotError: null })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId: "A".repeat(32), anchor, url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null })).toBe(false);
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
    expect(isFromOverlay(Object.assign(Object.create({ url: "http://x/" }), { t: "hello" }))).toBe(false);
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
    const pick = { t: "pick", pickId: "a".repeat(32), anchor, url: "http://x/", title: "T", snapshot: "<p>placeholder</p>", snapshotError: "too_large" };
    expect(isFromOverlay(pick)).toBe(true);
    expect(isFromOverlay({ ...pick, snapshot: null })).toBe(true);
    expect(isFromOverlay({ ...pick, snapshotError: "<img onerror>" })).toBe(false);
    expect(isFromOverlay({ ...pick, snapshotError: "" })).toBe(false);
  });

  it("checks the page and route of the worker's state", () => {
    const page = { artifact_id: "0123456789ab", origin: "http://localhost:5173", path: "/settings", page_url: "http://localhost:5173/settings", title: "Settings", current_version: 3, url: "http://127.0.0.1:7481/a/0123456789ab" };
    const state = { t: "state", page, route: "?tab=2", threads: [{ id: ULID }], commentMode: true, pending: false };
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
      { ...state, threads: [{ id: "x" }] },
      { ...state, threads: [null] },
      { ...state, threads: new Array(1001).fill({ id: ULID }) },
    ]) expect(isFromWorker(bad), JSON.stringify(bad).slice(0, 200)).toBe(false);
  });

  it("checks what the worker sends the overlay", () => {
    expect(isFromWorker({ t: "state", page: null, route: null, threads: [], commentMode: false, pending: false })).toBe(true);
    expect(isFromWorker({ t: "snapshot-now" })).toBe(true);
    expect(isFromWorker({ t: "focus", threadId: null })).toBe(true);
    expect(isFromWorker({ t: "state", threads: "none", commentMode: false, pending: false })).toBe(false);
    expect(isFromWorker({ t: "scroll-to", threadId: "x" })).toBe(false);
    expect(isFromWorker({ t: "post", body: "hi" })).toBe(false);
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
    expect(isFromPanel({ t: "navigate", route: "x".repeat(513) })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: Number.NaN })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: 1.5 })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: -1 })).toBe(false);
    expect(isFromPanel({ t: "watch-tab", tabId: 7 })).toBe(true);
    expect(isFromPanel({ t: "turn-off" })).toBe(true);
  });

  it("refuses an unknown field on every message", () => {
    const pickId = "a".repeat(32);
    const overlay = [
      { t: "hello", url: "http://x/" }, { t: "route", url: "http://x/#/a" }, { t: "capture", rect: { x: 0, y: 0, w: 1, h: 1 }, dpr: 1 },
      { t: "pick", pickId, anchor, url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null },
      { t: "quiet", url: "http://x/", title: "T", snapshot: "<p>" }, { t: "resolved", results: [] }, { t: "comment-mode", on: true },
      { t: "cancel", pickId }, { t: "pin", threadId: ULID }, { t: "removed" }, { t: "ping" },
    ];
    const worker = [
      { t: "state", page: null, route: null, threads: [], commentMode: false, pending: false }, { t: "comment-mode", on: false },
      { t: "close-composer", pickId, posted: true }, { t: "scroll-to", threadId: ULID }, { t: "focus", threadId: null }, { t: "snapshot-now" },
    ];
    const composer = [{ t: "ready" }, { t: "post", body: "hi" }, { t: "cancel" }];
    const panel = [
      { t: "watch-tab", tabId: 1 }, { t: "send", threadId: ULID, to: null }, { t: "send-batch", threadIds: [ULID], note: null, to: null },
      { t: "reply", threadId: ULID, body: "ok" }, { t: "resolve", threadId: ULID }, { t: "reopen", threadId: ULID }, { t: "delete", threadId: ULID },
      { t: "looked", threadIds: [ULID] }, { t: "set-name", name: "Ana" }, { t: "select", threadId: null }, { t: "comment-mode", on: true },
      { t: "navigate", route: null }, { t: "turn-off" }, { t: "retry" }, { t: "ping" },
    ];
    for (const [check, list] of [[isFromOverlay, overlay], [isFromWorker, worker], [isFromComposer, composer], [isFromPanel, panel]] as const) {
      for (const m of list) {
        expect(check(m), JSON.stringify(m)).toBe(true);
        expect(check({ ...m, extra: 1 }), `${JSON.stringify(m)} + extra`).toBe(false);
      }
    }
    expect(isFromOverlay({ t: "capture", rect: { x: 0, y: 0, w: 1, h: 1, z: 1 }, dpr: 1 })).toBe(false);
    // A required field that is absent is refused, not read as undefined.
    expect(isFromPanel({ t: "send", threadId: ULID })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId, anchor, url: "http://x/", title: "T", snapshot: "<p>" })).toBe(false);
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
