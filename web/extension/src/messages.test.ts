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
    expect(isFromOverlay(JSON.parse(`{"__proto__": {"t": "ping"}}`))).toBe(false);
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
  });
});
