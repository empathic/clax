import { afterEach, describe, expect, it, vi } from "vitest";
import type { Anchor } from "../../bridge/src/protocol";
import type { Recorder } from "../../sdk/src/diagnostics";
import type { Badge, DiagLine, Report } from "../../sdk/src/types";
import type { Captured } from "./capture";
import {
  BADGE_POLL_MS, BadgeWatch, CONTEXT_TIMEOUT_MS, MAX_CONTEXT_BYTES, PAGE_ANCHOR, Poll, POLL_MS, Session, type Timers, type Visibility,
  buildContext, canSend, clipOf, clipStatus, contextPreview, defaultShapes, externalIdOf, failureText, newDraft, quoteOf, reopenOf, repliesOf, reportOf,
  safeLink, stateLabel, summaryOf, waitingFix,
} from "./model";

const anchor: Anchor = { kind: "element", selector: "#save", quote: "Save   the\nform", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };

/** Timers that run only when the test says. */
function manualTimers() {
  const due: { fn: () => void; ms: number; id: number }[] = [];
  let next = 0;
  const t: Timers & { fire(): void; pending(): number[] } = {
    set: (fn, ms) => { const id = ++next; due.push({ fn, ms, id }); return id; },
    clear: h => { const i = due.findIndex(d => d.id === h); if (i >= 0) due.splice(i, 1); },
    fire: () => { const d = due.shift(); d?.fn(); },
    pending: () => due.map(d => d.ms),
  };
  return t;
}
const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };
const at = new Date("2026-10-29T07:12:03.120Z");
const now = () => at;
const lines: DiagLine[] = [{ at: "2026-10-29T07:12:00Z", level: "error", kind: "error", text: "TypeError: x is undefined" }];
const recorder = (l = lines): Recorder => ({ lines: () => l.slice(), count: () => l.length, stop: () => {} });
const captured = (over: Partial<Captured> = {}): Captured => ({
  png: new Blob([new Uint8Array([1])], { type: "image/png" }), info: { mode: "element", method: "dom", omitted: [] }, error: null, note: null, suggestExact: false, pick: { x: 4, y: 6, w: 20, h: 10 }, scale: 2, ...over,
});

afterEach(() => { vi.useRealTimers(); });

describe("the composer's draft", () => {
  it("is sendable with its note once, and not while a send is under way", () => {
    const d = newDraft(anchor);
    expect(canSend(d)).toBe(false);
    d.body = "  \n";
    expect(canSend(d)).toBe(false);
    d.body = "Make it green";
    expect(canSend(d)).toBe(true);
    d.sending = true;
    expect(canSend(d)).toBe(false);
  });

  it("a clip still being taken does not hold Send", () => {
    const d = { ...newDraft(anchor), body: "x" };
    expect(d.clip.state).toBe("taking");
    expect(canSend(d)).toBe(true);
    expect(clipStatus(d.clip)).toMatch(/Taking/);
  });

  it("takes a capture's PNG as its clip, and a capture without one as no clip with its reason", () => {
    const ok = clipOf(captured());
    expect(ok.state).toBe("taken");
    expect(clipStatus(ok)).toBeNull();
    const failed = clipOf(captured({ png: null, error: "a cross-origin image tainted the canvas" }));
    expect(failed).toEqual({ state: "failed", error: "a cross-origin image tainted the canvas" });
    expect(clipStatus(failed)).toMatch(/^No screenshot: a cross-origin image tainted the canvas\. The report is sent without one\.$/);
    expect(clipOf(captured({ png: new Blob([]) }))).toEqual({ state: "failed", error: "the screenshot came out empty" });
  });

  it("starts an Element pick's marks with a box around the pick, and Area and Screen captures with none", () => {
    expect(defaultShapes(captured())).toEqual([{ kind: "box", from: { x: 4, y: 6 }, to: { x: 24, y: 16 } }]);
    expect(defaultShapes(captured({ info: { mode: "range", method: "dom" } }))).toHaveLength(1);
    expect(defaultShapes(captured({ info: { mode: "area", method: "dom" } }))).toEqual([]);
    expect(defaultShapes(captured({ info: { mode: "screen", method: "dom" }, pick: null }))).toEqual([]);
  });

  it("files the clip only when it was taken and included, and the draft's metadata and context", () => {
    const page = { url: "https://app.example/orders/7#tab", title: "Orders" };
    const d = { ...newDraft(anchor), body: "  Wrong total \n" };
    const failed = reportOf({ ...d, clip: { state: "failed", error: "x" } }, page, {});
    expect(failed).toEqual({ url: page.url, title: "Orders", anchor, body: "Wrong total" });
    expect("clip" in failed).toBe(false);
    expect("externalId" in failed).toBe(false);
    const png = new Blob([new Uint8Array([1])], { type: "image/png" });
    const ctx = { v: 1 as const, captured_at: at.toISOString() };
    const taken = reportOf({ ...d, clip: { state: "taken", png }, metadata: { build: "abc" }, context: ctx }, page, { externalId: "order-7" });
    expect(taken.clip).toBe(png);
    expect(taken.externalId).toBe("order-7");
    expect(taken.metadata).toEqual({ build: "abc" });
    expect(taken.context).toBe(ctx);
    const marked = new Blob([new Uint8Array([2])], { type: "image/png" });
    expect(reportOf({ ...d, clip: { state: "taken", png } }, page, {}, marked).clip).toBe(marked);
    expect(reportOf({ ...d, clip: { state: "taken", png }, includeClip: false }, page, {}, marked).clip).toBeUndefined();
    expect(reportOf({ ...d, clip: { state: "taking" } }, page, {}).clip).toBeUndefined();
    // A Screen capture points at the whole page.
    expect(reportOf({ ...d, anchor: null }, page, {}).anchor).toEqual(PAGE_ANCHOR);
    // Text sent is well-formed.
    expect(reportOf({ ...d, body: "bad \uD800 half" }, page, {}).body).toBe("bad � half");
  });

  it("diagnostics excluded unless ticked", () => {
    const page = { url: "https://app.example/", title: "App" };
    const session = new Session({ recorder: recorder() }, window);
    const begun = session.begin({ mode: "screen", method: "dom" });
    const d = { ...newDraft(null, { kind: "new" }, session.diagnosticsDefault), body: "x", diagnostics: begun.diagnostics };
    expect(d.includeDiagnostics).toBe(false);
    expect(d.diagnostics).toEqual(lines);
    expect("diagnostics" in reportOf(d, page, {})).toBe(false);
    expect("diagnostics" in reopenOf(d)).toBe(false);
    d.includeDiagnostics = true;
    expect(reportOf(d, page, {}).diagnostics).toEqual(lines);
    expect(reopenOf(d).diagnostics).toEqual(lines);
    // Ticked with none recorded, none is sent.
    expect("diagnostics" in reportOf({ ...d, diagnostics: [] }, page, {})).toBe(false);
    // The host may start it ticked.
    expect(new Session({ diagnostics: { default: true } }, window).diagnosticsDefault).toBe(true);
  });

  it("sends a reopen's note with its new clip and context", () => {
    const png = new Blob([new Uint8Array([3])], { type: "image/png" });
    const ctx = { v: 1 as const, captured_at: at.toISOString() };
    const d = { ...newDraft(null, { kind: "reopen", report: {} as Report }), body: " Still wrong ", clip: { state: "taken" as const, png }, context: ctx };
    expect(reopenOf(d)).toEqual({ body: "Still wrong", clip: png, context: ctx });
    expect(newDraft(null, { kind: "reopen", report: {} as Report }).clip).toEqual({ state: "none" });
  });

  it("titles a report by the page, cut, or by its URL when the page has no title", () => {
    const d = { ...newDraft(anchor), body: "x" };
    expect(reportOf(d, { url: "https://a.example/", title: "" }, {}).title).toBe("https://a.example/");
    expect(reportOf(d, { url: "u", title: "t".repeat(400) }, {}).title).toHaveLength(300);
  });

  it("quotes the pick by its text, collapsed and cut, else its name, else its selector", () => {
    expect(quoteOf(anchor)).toBe("«Save the form»");
    expect(quoteOf({ ...anchor, quote: "q".repeat(200) })).toBe(`«${"q".repeat(160)}…»`);
    expect(quoteOf({ ...anchor, quote: null, name: "Save button" })).toBe("Save button");
    expect(quoteOf({ ...anchor, quote: null })).toBe("#save");
    expect(quoteOf({ ...anchor, kind: "area", quote: null })).toBe("An area of the page");
  });
});

describe("the page's artifact", () => {
  it("is the option, else the clax-artifact meta, else none", () => {
    const doc = document.implementation.createHTMLDocument("t");
    expect(externalIdOf(doc)).toBeUndefined();
    const meta = doc.createElement("meta");
    meta.name = "clax-artifact";
    meta.content = " checkout-v2 ";
    doc.head.append(meta);
    expect(externalIdOf(doc)).toBe("checkout-v2");
    expect(externalIdOf(doc, "given")).toBe("given");
    meta.content = "";
    expect(externalIdOf(doc)).toBeUndefined();
  });
});

describe("failures", () => {
  it("read as the reporter would", () => {
    expect(failureText({ code: "network", message: "x" })).toMatch(/could not be reached/);
    expect(failureText({ code: "wrong_origin" })).toMatch(/may not report/);
    expect(failureText(new Error("too big"))).toBe("too big");
    expect(failureText({ code: "unsupported_browser" })).toBe("This browser can't send reports (it lacks Ed25519 signing). Use a current Chrome, Firefox or Safari.");
  });
});

describe("a report's replies", () => {
  const report = (bodies: string[]): Report => ({
    id: "t1", queue_id: "q", artifact: { id: "a", external_id: "e", title: "Page", url: "u", metadata: null, metadata_version: 0, revision: null },
    anchor, status: "open", state: "open", reporter_id: "r", metadata: null, metadata_version: 0, has_clip: false, claim: null, created_at: "",
    comments: bodies.map((body, i) => ({ id: `c${i}`, author_kind: i ? "agent" : "reporter", author_name: i ? "Agent" : "R", body, created_at: "" })),
  });
  it("are the comments after the report's own", () => {
    const r = report(["The total is wrong", "Fixed in 1.2"]);
    expect(repliesOf(r).map(c => c.body)).toEqual(["Fixed in 1.2"]);
    expect(summaryOf(r)).toBe("The total is wrong");
    expect(summaryOf(report([]))).toBe("Page");
  });
});

describe("the reports poll", () => {
  it("loads at once, then POLL_MS after each load settles, never two at once, and none after stop", async () => {
    const t = manualTimers();
    let calls = 0;
    let release: () => void = () => {};
    const poll = new Poll(() => { calls++; return new Promise<void>(r => { release = r; }); }, t);
    poll.start();
    poll.start();
    expect(calls).toBe(1);
    expect(t.pending()).toEqual([]);
    release();
    await settle();
    expect(t.pending()).toEqual([POLL_MS]);
    t.fire();
    expect(calls).toBe(2);
    // A load asked for while one runs follows it, once.
    poll.now();
    poll.now();
    expect(calls).toBe(2);
    release();
    await settle();
    expect(calls).toBe(3);
    release();
    await settle();
    expect(t.pending()).toEqual([POLL_MS]);
    poll.stop();
    expect(t.pending()).toEqual([]);
    poll.now();
    expect(calls).toBe(3);
  });

  it("keeps going after a failed load, which it reports", async () => {
    const t = manualTimers();
    const errors: unknown[] = [];
    const poll = new Poll(() => Promise.reject(new Error("down")), t, POLL_MS, e => errors.push(e));
    poll.start();
    await settle();
    expect(errors).toHaveLength(1);
    expect(t.pending()).toEqual([POLL_MS]);
    poll.stop();
  });

  it("drops what a load in flight at stop brings", async () => {
    const t = manualTimers();
    const errors: unknown[] = [];
    let fail: (e: Error) => void = () => {};
    const poll = new Poll(() => new Promise<void>((_, f) => { fail = f; }), t, POLL_MS, e => errors.push(e));
    poll.start();
    poll.stop();
    fail(new Error("late"));
    await settle();
    expect(errors).toEqual([]);
    expect(t.pending()).toEqual([]);
  });
});

describe("a capture's context", () => {
  it("context_failure_still_files", async () => {
    const info = { mode: "screen" as const, method: "dom" as const };
    const threw = await buildContext({ context: async () => { throw new Error("flags service down"); } }, window, info, now);
    expect(threw.app).toEqual({ context_error: "flags service down" });
    expect(threw).toMatchObject({ v: 1, captured_at: at.toISOString(), capture: info });
    expect(threw.environment?.url).toBeTruthy();
    // A context that never answers is given up on after CONTEXT_TIMEOUT_MS, on the fake clock.
    vi.useFakeTimers();
    let done = false;
    const hung = buildContext({ context: () => new Promise(() => {}) }, window, info, now).then(c => { done = true; return c; });
    await vi.advanceTimersByTimeAsync(CONTEXT_TIMEOUT_MS - 1);
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(done).toBe(true);
    expect((await hung).app).toEqual({ context_error: `no answer within ${CONTEXT_TIMEOUT_MS} ms` });
    expect(CONTEXT_TIMEOUT_MS).toBe(2000);
    // And the report still files, with that context.
    const d = { ...newDraft(null), body: "x", context: await hung };
    expect(reportOf(d, { url: "https://a.example/", title: "A" }, {}).context?.app).toEqual({ context_error: `no answer within ${CONTEXT_TIMEOUT_MS} ms` });
  });

  it("takes only release, flags and app from the host, in their shapes, and scrubs app", async () => {
    const ctx = await buildContext({
      context: {
        release: { version: "0.6.0", commit: "f7e862ab0f7e862ab0f7e862ab0f7e862ab0aaaa", channel: 7 } as never,
        flags: { claxbot: true, editor: "b", ratio: 0.5, broken: Number.NaN, nested: {} } as never,
        app: { view: "artifact", note: "mail ines@example.com", when: undefined },
        environment: { url: "https://forged.example/" },
        captured_at: "1999-01-01T00:00:00Z",
        v: 2,
      } as never,
    }, window, { mode: "element", method: "dom" }, now);
    expect(Object.keys(ctx)).toEqual(["v", "captured_at", "release", "flags", "environment", "capture", "app"]);
    expect(ctx.v).toBe(1);
    expect(ctx.captured_at).toBe(at.toISOString());
    expect(ctx.release).toEqual({ version: "0.6.0", commit: "f7e862ab0f7e862ab0f7e862ab0f7e862ab0aaaa" });
    expect(ctx.flags).toEqual({ claxbot: true, editor: "b", ratio: 0.5 });
    expect(ctx.environment?.url).not.toBe("https://forged.example/");
    expect(ctx.app).toEqual({ view: "artifact", note: "mail ‹redacted›" });
  });

  it("stays within the 16 KiB Clax takes, leaving out the host's app first", async () => {
    const ctx = await buildContext({ context: { app: { blob: "word ".repeat(5000) }, flags: { a: true } } }, window, null, now);
    expect(new TextEncoder().encode(JSON.stringify(ctx)).length).toBeLessThanOrEqual(MAX_CONTEXT_BYTES);
    expect((ctx.app as { context_error: string }).context_error).toMatch(/app was left out/);
    expect(ctx.flags).toEqual({ a: true });
  });

  it("context is evaluated at capture start, not at load", async () => {
    let calls = 0;
    const session = new Session({ context: () => { calls++; return { release: { version: `v${calls}` } }; }, now }, window);
    await settle();
    expect(calls).toBe(0);
    const first = await session.begin({ mode: "screen", method: "dom" }).context;
    expect(calls).toBe(1);
    expect(first.release).toEqual({ version: "v1" });
    expect((await session.begin({ mode: "area", method: "dom" }).context).release).toEqual({ version: "v2" });
    const verdict = await session.verdictContext();
    expect(calls).toBe(3);
    expect("capture" in verdict).toBe(false);
  });

  it("metadata function is called per report", async () => {
    let calls = 0;
    const session = new Session({ metadata: () => ({ n: ++calls }) }, window);
    expect(calls).toBe(0);
    expect(await session.begin(null).metadata).toEqual({ n: 1 });
    expect(await session.begin(null).metadata).toEqual({ n: 2 });
    expect(await new Session({ metadata: { build: "7" } }, window).begin(null).metadata).toEqual({ build: "7" });
    // One that throws leaves the report without metadata.
    expect(await new Session({ metadata: () => { throw new Error("no"); } }, window).begin(null).metadata).toBeUndefined();
  });

  it("is previewed as exactly the JSON sent", () => {
    const ctx = { v: 1 as const, captured_at: at.toISOString(), flags: { a: Number.POSITIVE_INFINITY, b: 1 } };
    expect(JSON.parse(contextPreview(ctx))).toEqual({ v: 1, captured_at: at.toISOString(), flags: { b: 1 } });
    expect(contextPreview(ctx)).toContain('\n  "v": 1,');
  });
});

describe("the badge", () => {
  /** A page whose visibility the test sets, and whose events it fires. */
  function page() {
    const on: Record<string, (() => void)[]> = {};
    const add = (t: string, fn: () => void) => { (on[t] ??= []).push(fn); };
    const remove = (t: string, fn: () => void) => { on[t] = (on[t] ?? []).filter(f => f !== fn); };
    const p = {
      document: { visibilityState: "visible" as DocumentVisibilityState, addEventListener: add, removeEventListener: remove },
      addEventListener: add, removeEventListener: remove,
      fire: (t: string) => { for (const f of on[t] ?? []) f(); },
      listeners: () => Object.values(on).flat().length,
    };
    return p satisfies Visibility & Record<string, unknown>;
  }

  it("badge rise emits badge and pulses once", async () => {
    const t = manualTimers();
    const answers: Badge[] = [
      { waiting: 1, report_ids: ["r1"] },
      { waiting: 1, report_ids: ["r1"] },
      { waiting: 3, report_ids: ["r1", "r2", "r3"] },
      { waiting: 0, report_ids: [] },
    ];
    let asked = 0;
    const badges: number[] = [];
    const rises: string[][] = [];
    const p = page();
    const watch = new BadgeWatch(async () => answers[Math.min(asked++, answers.length - 1)], p, { changed: n => badges.push(n), rose: ids => rises.push(ids) }, t);
    watch.start();
    await settle();
    expect(badges).toEqual([1]);
    expect(rises).toEqual([]);
    expect(t.pending()).toEqual([BADGE_POLL_MS]);
    t.fire();
    await settle();
    expect(badges).toEqual([1]);
    t.fire();
    await settle();
    expect(badges).toEqual([1, 3]);
    expect(rises).toEqual([["r2", "r3"]]);
    // A fall is a change, never a pulse.
    p.fire("focus");
    await settle();
    expect(badges).toEqual([1, 3, 0]);
    expect(rises).toHaveLength(1);
    expect(watch.count).toBe(0);
    watch.stop();
    expect(p.listeners()).toBe(0);
    expect(t.pending()).toEqual([]);
  });

  it("is asked for only while the page is visible, and at once when it shows again", async () => {
    const t = manualTimers();
    let asked = 0;
    const p = page();
    p.document.visibilityState = "hidden";
    const watch = new BadgeWatch(async () => { asked++; return { waiting: 0, report_ids: [] }; }, p, { changed: () => {}, rose: () => {} }, t);
    watch.start();
    await settle();
    expect(asked).toBe(0);
    p.document.visibilityState = "visible";
    p.fire("visibilitychange");
    await settle();
    expect(asked).toBe(1);
    expect(t.pending()).toEqual([BADGE_POLL_MS]);
    p.document.visibilityState = "hidden";
    p.fire("visibilitychange");
    expect(t.pending()).toEqual([]);
    watch.stop();
  });
});

describe("a report as the reporter reads it", () => {
  const base = { state: "open", fixes: [] } as unknown as Report;
  it("names every state, and a resolved one by its reason", () => {
    expect(stateLabel({ state: "open" })).toBe("Open");
    expect(stateLabel({ state: "claimed" })).toBe("Being worked on");
    expect(stateLabel({ state: "fix_proposed" })).toBe("Fix ready for you to check");
    expect(stateLabel({ state: "verified" })).toBe("Fixed: you confirmed it");
    expect(stateLabel({ state: "resolved" })).toBe("Resolved");
    expect(stateLabel({ state: "resolved", resolve_reason: "duplicate" })).toBe("Resolved: duplicate");
    expect(stateLabel({ state: "resolved", resolve_reason: "wontfix" })).toBe("Resolved: won't fix");
  });

  it("waits on the latest fix only while a fix is proposed", () => {
    const fix = { n: 2, summary: "Fixed the end date", proposer_name: "agent", created_at: "", outcome: "pending" as const };
    expect(waitingFix({ ...base, state: "fix_proposed", fixes: [{ ...fix, n: 1, outcome: "rejected" }, fix] })).toEqual(fix);
    expect(waitingFix({ ...base, state: "open", fixes: [fix] })).toBeNull();
    expect(waitingFix({ ...base, state: "fix_proposed", fixes: undefined })).toBeNull();
  });

  it("links only http and https URLs", () => {
    expect(safeLink("https://github.com/o/r/pull/1")).toBe("https://github.com/o/r/pull/1");
    expect(safeLink("javascript:alert(1)")).toBeNull();
    expect(safeLink("not a url")).toBeNull();
    expect(safeLink(undefined)).toBeNull();
  });
});
