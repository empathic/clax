import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import type { DiagLine, Report } from "../../../sdk/src/types";
import { QUEUE, report } from "./fixtures";
import ReportView from "./ReportView.svelte";

const R = "/api/queues/q1/reports/t1";
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const type = (el: Element | null, v: string) => flush(() => { const i = el as HTMLInputElement; i.value = v; i.dispatchEvent(new Event("input", { bubbles: true })); });
const choose = (el: Element | null, v: string) => flush(() => { const s = el as HTMLSelectElement; s.value = v; s.dispatchEvent(new Event("change", { bubbles: true })); });
const button = (root: Element, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent?.trim() === text);
const submit = (root: Element) => click(root.querySelector('form.af button[type="submit"]'));
const view = () => mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US" });
type Handler = (e: { type: string; topic?: string; [k: string]: unknown }) => void;
/** A stream whose events the test sends: the topics watched, and `send`. */
function fakeStream() {
  const watching: { topics: string[]; on: Handler }[] = [];
  return {
    watching,
    stream: { watch: (topics: string[], on: Handler) => { const w = { topics, on }; watching.push(w); return () => { watching.splice(watching.indexOf(w), 1); }; } },
    send: (e: Parameters<Handler>[0]) => flush(() => { for (const w of [...watching]) w.on(e); }),
  };
}
const CLAIM = { holder_name: "Claude Code", expires_at: "2026-10-29T08:30:00Z" };

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("ReportView", () => {
  it("shows the report, its reporter and conversation, and no claim or release while it is open", async () => {
    const r = report(1, {
      reporter_profile: { name: "Ana", github: "ana-k" },
      comments: [...report(1).comments, { id: "c2", author_kind: "viewer", author_name: "Mia", body: "Looking", created_at: "2026-10-29T08:00:00Z" }],
    });
    stubFetch({ [`GET ${R}`]: { report: r } });
    const m = view();
    await settle();
    expect(m.root.querySelector("h2")!.textContent).toBe("Report 1: the total is wrong");
    expect(m.root.querySelector(".by")!.textContent).toContain("Ana");
    expect(m.root.querySelector<HTMLAnchorElement>('.by a[href="https://github.com/ana-k"]')).not.toBeNull();
    expect(m.root.querySelector(".anchor")!.textContent).toBe("On Element: Total");
    expect([...m.root.querySelectorAll(".convo .cm")].map(c => c.getAttribute("data-id"))).toEqual(["c2"]);
    const actions = [...m.root.querySelectorAll(".btns button")].map(b => b.textContent);
    expect(actions).toEqual(["Propose fix", "Resolve"]);
    expect([...m.root.querySelectorAll("button")].filter(b => /claim|release/i.test(b.textContent ?? ""))).toEqual([]);
  });

  it("outlines what the render left out, in the clip's own pixels", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1) } });
    const m = view();
    await settle();
    const img = m.root.querySelector<HTMLImageElement>(".clip img")!;
    expect(img.getAttribute("src")).toBe(`${R}/clip`);
    Object.defineProperty(img, "naturalWidth", { value: 1000 });
    Object.defineProperty(img, "naturalHeight", { value: 500 });
    flush(() => img.dispatchEvent(new Event("load")));
    const o = m.root.querySelector<HTMLElement>(".omit")!;
    expect([o.style.left, o.style.top, o.style.width, o.style.height]).toEqual(["10%", "10%", "20%", "20%"]);
    expect(o.textContent).toBe("iframe");
    // At 1× the clip is its size on the reporter's screen (dpr 2), at 2× twice it.
    click(button(m.root, "1×"));
    expect(m.root.querySelector<HTMLElement>(".clipframe")!.style.width).toBe("500px");
    click(button(m.root, "2×"));
    expect(m.root.querySelector<HTMLElement>(".clipframe")!.style.width).toBe("1000px");
  });

  it("propose fix posts summary and url", async () => {
    const proposed: Report = report(1, { state: "fix_proposed", fixes: [{ n: 1, summary: "Rounds the total", url: "https://github.com/o/r/pull/9", proposer_name: "Mia", proposer_kind: "person", created_at: "2026-10-29T09:00:00Z", outcome: "pending" }] });
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1) }, [`POST ${R}/fix`]: { report: proposed } });
    const m = view();
    await settle();
    click(button(m.root, "Propose fix"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".acts .err")!.textContent).toBe("Say what the fix changes.");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    type(m.root.querySelector('textarea[name="summary"]'), " Rounds the total ");
    type(m.root.querySelector('input[name="url"]'), "https://github.com/o/r/pull/9");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/fix`, method: "POST", body: { summary: "Rounds the total", url: "https://github.com/o/r/pull/9" } });
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Fix proposed");
    expect(m.root.querySelector('.fix[data-n="1"]')!.textContent).toContain("Waiting for the reporter");
    expect([...m.root.querySelectorAll(".btns button")].map(b => b.textContent)).toEqual(["Propose fix", "Resolve", "Verify for the reporter", "Reopen for the reporter"]);
  });

  // A held fix as the daemon shows it to members: `verify_url` is what the
  // reporter was given, so a held fix has none; `preview_url` and
  // `preview_missed` say where its preview stands.
  const heldFix = { n: 1, summary: "Rounds the total", url: "https://github.com/o/r/pull/9", commit: "abc123", proposer_name: "ClaxBot", proposer_kind: "agent" as const, held: true, preview_url: null, preview_missed: false, created_at: "2026-10-29T09:00:00Z", outcome: "pending" as const };
  const PREVIEW = "https://pr-9.acme.dev/";

  it("tells members the reporter waits on a held fix waiting for its preview, and does not call it waiting for the reporter", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed", fixes: [heldFix] }) } });
    const m = view();
    await settle();
    const line = m.root.querySelector("[data-held]")!;
    expect(line.textContent).toContain("The reporter is waiting on you.");
    expect(line.textContent).toContain("Fix 1 is waiting for its preview, so they have nothing to check yet.");
    expect(line.querySelector("a")).toBeNull();
    expect(m.root.querySelector('.fix[data-n="1"]')!.textContent).toContain("Held for you");
    expect(m.root.querySelector('.fix[data-n="1"]')!.textContent).not.toContain("Waiting for the reporter");
  });

  it("gives members a held fix's preview, says the reporter has not got it, and hands it over with that link", async () => {
    const up = { ...heldFix, preview_url: PREVIEW };
    const handed = { ...heldFix, n: 2, held: false, verify_url: PREVIEW, proposer_name: "Mia" };
    const proposed = report(1, { state: "fix_proposed", fixes: [{ ...up, held: false, outcome: "superseded" }, handed] });
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed", fixes: [up] }) }, [`POST ${R}/fix`]: { report: proposed } });
    const m = view();
    await settle();
    const line = m.root.querySelector("[data-held]")!;
    expect(line.querySelector<HTMLAnchorElement>(`a[href="${PREVIEW}"]`)).not.toBeNull();
    expect(line.textContent).toContain("Fix 1 has a preview, which only this queue's people can see");
    expect(line.textContent).toContain("They hear of it when you hand it over.");
    expect(line.textContent).not.toContain("no preview");
    click(button(line as Element, "Hand it to the reporter"));
    expect(m.root.querySelector<HTMLTextAreaElement>('textarea[name="summary"]')!.value).toBe("Rounds the total");
    expect(m.root.querySelector<HTMLInputElement>('input[name="verify_url"]')!.value).toBe(PREVIEW);
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({ summary: "Rounds the total", url: "https://github.com/o/r/pull/9", commit: "abc123", verify_url: PREVIEW });
    expect(m.root.querySelector("[data-held]")).toBeNull();
  });

  it("says when the wait for a held fix's preview ended with none, and that a link of a person's own is what is left", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed", fixes: [{ ...heldFix, preview_missed: true }] }) } });
    const m = view();
    await settle();
    const line = m.root.querySelector("[data-held]")!;
    expect(line.textContent).toContain("ClaxBot stopped watching for a preview of fix 1 without seeing one up, so they have no link to check.");
    expect(line.textContent).toContain("hand it over with a link of your own");
    expect(line.textContent).not.toContain("waiting for its preview");
    expect(line.querySelector("a")).toBeNull();
  });

  it("will not hand a held fix over with no link, and sends nothing", async () => {
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed", fixes: [heldFix] }) } });
    const m = view();
    await settle();
    click(button(m.root.querySelector("[data-held]")!, "Hand it to the reporter"));
    expect(m.root.querySelector('input[name="verify_url"]')!.getAttribute("aria-required")).toBe("true");
    submit(m.root);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Say where the reporter checks it: until this fix has a link, they have nothing to check.");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    // The same goes for Propose fix, since a person's fix supersedes the held one.
    click(button(m.root.querySelector(".btns")!, "Propose fix"));
    click(button(m.root.querySelector(".btns")!, "Propose fix"));
    type(m.root.querySelector('textarea[name="summary"]'), "Rounds it");
    submit(m.root);
    await settle();
    expect(calls.some(c => c.method === "POST")).toBe(false);
  });

  it("leaves the link optional when no fix is held", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "open" }) } });
    const m = view();
    await settle();
    click(button(m.root.querySelector(".btns")!, "Propose fix"));
    expect(m.root.querySelector('input[name="verify_url"]')!.hasAttribute("aria-required")).toBe(false);
  });

  it("puts the daemon's refusal of a hand-over with no link in the form's words, as a fix may be held since this view read it", async () => {
    stubFetch({
      [`GET ${R}`]: { report: report(1, { state: "open" }) },
      [`POST ${R}/fix`]: [400, { error: { code: "verify_url_required", message: "the reporter has nothing to check until this fix has a link" } }],
    });
    const m = view();
    await settle();
    click(button(m.root.querySelector(".btns")!, "Propose fix"));
    type(m.root.querySelector('textarea[name="summary"]'), "Rounds it");
    submit(m.root);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Say where the reporter checks it: until this fix has a link, they have nothing to check.");
    expect(m.root.querySelector<HTMLTextAreaElement>('textarea[name="summary"]')!.value).toBe("Rounds it");
  });

  it("does not hand the reporter the words that told them a person reviews the fix", async () => {
    const fixed = { ...heldFix, summary: "A fix has been proposed." };
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed", fixes: [fixed] }) } });
    const m = view();
    await settle();
    click(button(m.root.querySelector("[data-held]")!, "Hand it to the reporter"));
    expect(m.root.querySelector<HTMLTextAreaElement>('textarea[name="summary"]')!.value).toBe("A fix is ready for you to check.");
  });

  it("says nothing of a fix no longer held, or one that ended", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "open", fixes: [{ ...heldFix, outcome: "superseded" }] }) } });
    const m = view();
    await settle();
    expect(m.root.querySelector("[data-held]")).toBeNull();
    expect(m.root.querySelector('.fix[data-n="1"]')!.textContent).not.toContain("Held for you");
  });

  it("resolve requires a reason", async () => {
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1) }, [`POST ${R}/resolve`]: { report: report(1, { state: "resolved", status: "resolved", resolve_reason: "duplicate" }) } });
    const m = view();
    await settle();
    click(button(m.root, "Resolve"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".acts .err")!.textContent).toBe("Choose why the report is resolved.");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    choose(m.root.querySelector('select[name="reason"]'), "duplicate");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/resolve`, method: "POST", body: { reason: "duplicate" } });
    expect(m.root.querySelector(".rh")!.textContent).toContain("Duplicate");
    expect([...m.root.querySelectorAll(".btns button")].map(b => b.textContent)).toEqual(["Reopen for the reporter"]);
  });

  it("verify on behalf posts verify", async () => {
    const waiting = report(1, { state: "fix_proposed" });
    const calls = stubFetch({ [`GET ${R}`]: { report: waiting }, [`POST ${R}/verify`]: { report: report(1, { state: "verified", status: "resolved" }) } });
    const m = view();
    await settle();
    click(button(m.root, "Verify for the reporter"));
    type(m.root.querySelector('form.af textarea[name="note"]'), "Checked on 0.6.1");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/verify`, method: "POST", body: { note: "Checked on 0.6.1" } });
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Verified");
    expect(m.root.querySelector(".acts .stat")!.textContent).toBe("Marked verified for the reporter.");
  });

  it("shows a refusal from the daemon", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed" }) }, [`POST ${R}/verify`]: [409, { error: { code: "not_fix_proposed", message: "no fix waits for a verdict" } }] });
    const m = view();
    await settle();
    click(button(m.root, "Verify for the reporter"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".acts .err")!.textContent).toBe("No fix waits for a verdict.");
  });

  it("diagnostics filter by level", async () => {
    const lines: DiagLine[] = [
      { at: "2026-10-29T07:12:01.000Z", level: "info", kind: "console", text: "booted" },
      { at: "2026-10-29T07:12:02.000Z", level: "error", kind: "error", text: "TypeError: total is undefined" },
      { at: "2026-10-29T07:12:02.500Z", level: "warn", kind: "request", text: "GET /api/total 500" },
    ];
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1, { diagnostics_lines: 3, filing_diagnostics_lines: 3 }) }, [`GET ${R}/diagnostics?capture=0`]: { lines } });
    const m = view();
    await settle();
    const d = m.root.querySelector<HTMLDetailsElement>("details.diag")!;
    expect(d.open).toBe(false);
    expect(d.querySelector("summary")!.textContent).toContain("3 lines");
    expect(calls.some(c => c.url.includes("diagnostics"))).toBe(false);
    flush(() => { d.open = true; d.dispatchEvent(new Event("toggle")); });
    await settle();
    expect([...d.querySelectorAll(".line")].map(l => l.getAttribute("data-level"))).toEqual(["info", "error", "warn"]);
    click(d.querySelector('input[type="radio"][value="error"]'));
    expect([...d.querySelectorAll(".line .text")].map(l => l.textContent)).toEqual(["TypeError: total is undefined"]);
    click(d.querySelector('input[type="radio"][value="debug"]'));
    expect(d.querySelector(".line")).toBeNull();
    expect(d.textContent).toContain("No debug lines.");
  });

  it("loads the filing's diagnostics as capture 0 and a reopen's as its own capture", async () => {
    const line = (text: string): DiagLine => ({ at: "2026-10-29T07:12:01.000Z", level: "error", kind: "console", text });
    const reopened = report(1, {
      diagnostics_lines: 2, filing_diagnostics_lines: 1, reopen_count: 1,
      comments: [...report(1).comments, { id: "c9", author_kind: "reporter", author_name: "emp-1", body: "Still broken", created_at: "2026-10-29T09:00:00Z", capture: 1, has_clip: false, diagnostics_lines: 2 }],
    });
    const calls = stubFetch({
      [`GET ${R}`]: { report: reopened },
      [`GET ${R}/diagnostics?capture=0`]: { lines: [line("at filing")] },
      [`GET ${R}/diagnostics?capture=1`]: { lines: [line("at reopen"), line("again")] },
    });
    const m = view();
    await settle();
    const [filing, again] = [...m.root.querySelectorAll<HTMLDetailsElement>("details.diag")];
    expect(filing.querySelector("summary")!.textContent).toContain("1 line");
    expect(again.closest('[data-id="c9"]')).not.toBeNull();
    for (const d of [filing, again]) flush(() => { d.open = true; d.dispatchEvent(new Event("toggle")); });
    await settle();
    expect([...filing.querySelectorAll(".line .text")].map(l => l.textContent)).toEqual(["at filing"]);
    expect([...again.querySelectorAll(".line .text")].map(l => l.textContent)).toEqual(["at reopen", "again"]);
    expect(calls.map(c => c.url).filter(u => u.includes("diagnostics")).sort()).toEqual([`${R}/diagnostics?capture=0`, `${R}/diagnostics?capture=1`]);
  });

  it("shows no filing diagnostics when only a reopen has some", async () => {
    stubFetch({ [`GET ${R}`]: { report: report(1, { diagnostics_lines: 2 }) } });
    const m = view();
    await settle();
    expect(m.root.querySelector("details.diag")).toBeNull();
  });

  it("retries diagnostics after a failed load", async () => {
    let fail = true;
    stubFetch({
      [`GET ${R}`]: { report: report(1, { diagnostics_lines: 1, filing_diagnostics_lines: 1 }) },
      [`GET ${R}/diagnostics?capture=0`]: () => (fail ? [500, { error: { code: "internal", message: "store unavailable" } }] : { lines: [{ at: "2026-10-29T07:12:01Z", level: "info", kind: "console", text: "ok" }] }),
    });
    const m = view();
    await settle();
    const d = m.root.querySelector<HTMLDetailsElement>("details.diag")!;
    flush(() => { d.open = true; d.dispatchEvent(new Event("toggle")); });
    await settle();
    expect(d.querySelector(".err")!.textContent).toContain("Store unavailable.");
    fail = false;
    click(button(d, "Try again"));
    await settle();
    expect(d.querySelector(".line .text")!.textContent).toBe("ok");
  });

  it("replies with the box's text and clears it", async () => {
    const replied = report(1, { comments: [...report(1).comments, { id: "c2", author_kind: "viewer", author_name: "Mia", body: "On it", created_at: "2026-10-29T08:00:00Z" }] });
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1) }, [`POST ${R}/comments`]: { report: replied } });
    const m = view();
    await settle();
    const box = m.root.querySelector<HTMLTextAreaElement>("form.reply textarea")!;
    type(box, "  On it ");
    click(m.root.querySelector('form.reply button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/comments`, method: "POST", body: { body: "On it" } });
    expect(box.value).toBe("");
    expect([...m.root.querySelectorAll(".convo .cm .body")].map(b => b.textContent)).toEqual(["On it"]);
  });

  it("keeps verify's note and reopen's text apart", async () => {
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1, { state: "fix_proposed" }) }, [`POST ${R}/reopen`]: { report: report(1) } });
    const m = view();
    await settle();
    click(button(m.root, "Verify for the reporter"));
    type(m.root.querySelector('form.af textarea[name="note"]'), "looks fine");
    click(button(m.root, "Reopen for the reporter"));
    const body = m.root.querySelector<HTMLTextAreaElement>('form.af textarea[name="body"]')!;
    expect(body.value).toBe("");
    type(body, "Still 21");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({ body: "Still 21" });
  });

  it("links back to the queue on the tab it came from", async () => {
    history.replaceState(null, "", "/queues/q1/t1?state=resolved");
    stubFetch({ [`GET ${R}`]: { report: report(1) } });
    const m = view();
    await settle();
    expect(m.root.querySelector(".back a")!.getAttribute("href")).toBe("/queues/q1?state=resolved");
    history.replaceState(null, "", "/queues/q1/t1");
  });

  it("refetches when its queue's topic says it changed, keeping a reply draft and saying what changed", async () => {
    let current: Report = report(1);
    const calls = stubFetch({ [`GET ${R}`]: () => ({ report: current }) });
    const s = fakeStream();
    const m = mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US", stream: s.stream as never });
    s.send({ type: "ready", topic: "queue:q1" });
    await settle();
    expect(s.watching.map(w => w.topics)).toEqual([["queue:q1"]]);
    const box = m.root.querySelector<HTMLTextAreaElement>("form.reply textarea")!;
    type(box, "Half a reply");
    const gets = () => calls.filter(c => c.method === "GET" && c.url === R).length;
    const before = gets();
    // Another report's change is not this one's.
    s.send({ type: "report", topic: "queue:q1", report_id: "t2", state: "claimed" });
    await settle();
    expect(gets()).toBe(before);
    // An agent claims it elsewhere: the view shows it without a reload.
    current = report(1, { state: "claimed", claim: CLAIM });
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "claimed" });
    await settle();
    expect(gets()).toBe(before + 1);
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Claimed");
    expect(m.root.querySelector(".by .claim")!.textContent).toContain("claimed by Claude Code");
    expect(box.isConnected && box.value).toBe("Half a reply");
    const live = m.root.querySelector('[aria-live="polite"].sr')!;
    expect(live.textContent).toBe("Now claimed by Claude Code.");
    // A reply from someone else is said too; the draft stays.
    current = report(1, { state: "claimed", claim: CLAIM, comments: [...report(1).comments, { id: "c2", author_kind: "reporter", author_name: "emp-1", body: "It is order 7", created_at: "2026-10-29T08:00:00Z" }] });
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "claimed" });
    await settle();
    expect([...m.root.querySelectorAll(".convo .cm .body")].map(b => b.textContent)).toEqual(["It is order 7"]);
    expect(live.textContent).toBe("New reply from emp-1.");
    expect(box.value).toBe("Half a reply");
    // The stream coming back refetches too.
    s.send({ type: "ready", topic: "queue:q1" });
    await settle();
    expect(gets()).toBe(before + 3);
  });

  it("releases a claim after asking, with a note, and the report is open again", async () => {
    const calls = stubFetch({
      [`GET ${R}`]: { report: report(1, { state: "claimed", claim: CLAIM }) },
      [`POST ${R}/release`]: { report: report(1, { comments: [...report(1).comments, { id: "c3", author_kind: "viewer", author_name: "Mia", body: "Stuck since Monday", created_at: "2026-10-29T09:00:00Z", triage_only: true }] }) },
    });
    const m = view();
    await settle();
    click(button(m.root, "Release claim"));
    const f = m.root.querySelector("form.af")!;
    expect(f.textContent).toContain("Ends Claude Code's claim now");
    expect(calls.some(c => c.method === "POST")).toBe(false);
    type(f.querySelector('textarea[name="note"]'), " Stuck since Monday ");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${R}/release`, method: "POST", body: { note: "Stuck since Monday" } });
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Open");
    expect(m.root.querySelector(".acts .stat")!.textContent).toBe("Released the claim: the report is open again.");
    expect(button(m.root, "Release claim")).toBeUndefined();
    const note = m.root.querySelector('.cm[data-id="c3"]')!;
    expect(note.querySelector(".only")!.textContent).toBe("Triage only");
  });

  it("says the claim had already ended when there was nothing to release", async () => {
    stubFetch({
      [`GET ${R}`]: { report: report(1, { state: "claimed", claim: CLAIM }) },
      [`POST ${R}/release`]: { report: report(1), released: false },
    });
    const m = view();
    await settle();
    click(button(m.root, "Release claim"));
    submit(m.root);
    await settle();
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Open");
    expect(m.root.querySelector(".acts .stat")!.textContent).toBe("The claim had already ended.");
  });

  it("sends no note when none is written, and a form its state no longer allows closes", async () => {
    let current: Report = report(1, { state: "claimed", claim: CLAIM });
    const calls = stubFetch({ [`GET ${R}`]: () => ({ report: current }), [`POST ${R}/release`]: { report: report(1) } });
    const s = fakeStream();
    const m = mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US", stream: s.stream as never });
    await settle();
    click(button(m.root, "Release claim"));
    // The agent lets it go first: the confirmation is no longer offered.
    current = report(1);
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "open" });
    await settle();
    expect(m.root.querySelector("form.af")).toBeNull();
    current = report(1, { state: "claimed", claim: CLAIM });
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "claimed" });
    await settle();
    click(button(m.root, "Release claim"));
    submit(m.root);
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({});
  });

  it("fetches once on open: its topic going live does not fetch again", async () => {
    const calls = stubFetch({ [`GET ${R}`]: { report: report(1) } });
    const s = fakeStream();
    const m = mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US", stream: s.stream as never });
    s.send({ type: "ready", topic: "queue:q1" });
    await settle();
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Open");
    expect(calls.filter(c => c.method === "GET" && c.url === R).length).toBe(1);
  });

  it("closes the fix form when a change elsewhere no longer allows it, keeping what was typed", async () => {
    let current: Report = report(1);
    stubFetch({ [`GET ${R}`]: () => ({ report: current }) });
    const s = fakeStream();
    const m = mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US", stream: s.stream as never });
    await settle();
    click(button(m.root, "Propose fix"));
    type(m.root.querySelector('textarea[name="summary"]'), "Rounds the total");
    current = report(1, { state: "resolved", status: "resolved" });
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "resolved" });
    await settle();
    expect(m.root.querySelector("form.af")).toBeNull();
    current = report(1);
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "open" });
    await settle();
    click(button(m.root, "Propose fix"));
    expect(m.root.querySelector<HTMLTextAreaElement>('textarea[name="summary"]')!.value).toBe("Rounds the total");
  });

  it("does not announce the person's own action as someone else's change", async () => {
    let current: Report = report(1, { state: "claimed", claim: CLAIM });
    let answer!: () => void;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      if ((init?.method ?? "GET") === "POST") {
        await new Promise<void>(r => { answer = r; });
        return new Response(JSON.stringify({ report: report(1), released: true }));
      }
      return new Response(JSON.stringify({ report: current }));
    }));
    const s = fakeStream();
    const m = mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US", stream: s.stream as never });
    await settle();
    click(button(m.root, "Release claim"));
    submit(m.root);
    await settle();
    // The release's own event refetches before its answer comes.
    current = report(1);
    s.send({ type: "report", topic: "queue:q1", report_id: "t1", state: "open" });
    await settle();
    answer();
    await settle();
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Open");
    expect(m.root.querySelector(".acts .stat")!.textContent).toBe("Released the claim: the report is open again.");
    expect(m.root.querySelector('[aria-live="polite"].sr')!.textContent).toBe("");
  });

  it("keeps the latest load when an earlier one answers after it", async () => {
    let answer!: (r: Response) => void;
    let n = 0;
    vi.stubGlobal("fetch", vi.fn(() => (++n === 1 ? new Promise<Response>(r => { answer = r; }) : Promise.resolve(new Response(JSON.stringify({ report: report(1, { state: "resolved" }) }))))));
    const stream = { watch: (_t: string[], f: (e: { type: string }) => void) => { f({ type: "resync" }); return () => {}; } };
    const m = mount(ReportView, { qid: "q1", tid: "t1", queue: QUEUE, nav: () => {}, zone: "UTC", locale: "en-US", stream: stream as never });
    await settle();
    answer(new Response(JSON.stringify({ report: report(1) })));
    await settle();
    expect(m.root.querySelector(".rh .state")!.textContent).toBe("Resolved");
  });
});
