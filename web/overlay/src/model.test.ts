import { describe, expect, it } from "vitest";
import type { Anchor } from "../../bridge/src/protocol";
import type { Report } from "../../sdk/src/types";
import { canSend, clipStatus, externalIdOf, failureText, newDraft, Poll, POLL_MS, quoteOf, repliesOf, reportOf, summaryOf, takeClip, type Timers } from "./model";

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

describe("the composer's draft", () => {
  it("is sendable with text once, and not while a send is under way", () => {
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

  it("takes a clip as a PNG, and a render that throws or comes out empty as no clip with its reason", async () => {
    const ok = await takeClip(async () => new Uint8Array([137, 80, 78, 71]).buffer);
    expect(ok.state).toBe("taken");
    if (ok.state === "taken") expect(ok.png.type).toBe("image/png");
    expect(clipStatus(ok)).toBeNull();
    const failed = await takeClip(async () => { throw new Error("a cross-origin image tainted the canvas"); });
    expect(failed).toEqual({ state: "failed", error: "a cross-origin image tainted the canvas" });
    expect(clipStatus(failed)).toMatch(/^No screenshot: a cross-origin image tainted the canvas\. The report is sent without one\.$/);
    expect(await takeClip(async () => new ArrayBuffer(0))).toEqual({ state: "failed", error: "the screenshot came out empty" });
  });

  it("files the clip only when it was taken, and the options' artifact and metadata", async () => {
    const page = { url: "https://app.example/orders/7#tab", title: "Orders" };
    const d = { ...newDraft(anchor), body: "  Wrong total \n" };
    const failed = reportOf({ ...d, clip: { state: "failed", error: "x" } }, page, {});
    expect(failed).toEqual({ url: page.url, title: "Orders", anchor, body: "Wrong total" });
    expect("clip" in failed).toBe(false);
    expect("externalId" in failed).toBe(false);
    const png = new Blob([new Uint8Array([1])], { type: "image/png" });
    const taken = reportOf({ ...d, clip: { state: "taken", png } }, page, { externalId: "order-7", metadata: { build: "abc" } });
    expect(taken.clip).toBe(png);
    expect(taken.externalId).toBe("order-7");
    expect(taken.metadata).toEqual({ build: "abc" });
    expect(reportOf({ ...d, clip: { state: "taking" } }, page, {}).clip).toBeUndefined();
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
