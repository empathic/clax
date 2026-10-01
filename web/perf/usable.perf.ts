import { test, expect, type Browser } from "@playwright/test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { arch, platform } from "node:os";
import { fileURLToPath } from "node:url";
import { type FrameMode, contentFrame, publish, startDaemon } from "../e2e/fixtures";

/** What the harness measures, in milliseconds, per frame mode:
 * - `firstPaint`: link → the frame's first contentful paint, in the tab
 *   whose harness presses Comment;
 * - `commentReady`: link → comment mode on in the frame, in that tab;
 * - `framePaint`: link → the frame's first contentful paint in a tab where
 *   the harness does nothing after opening the link (`firstPaint` moves with
 *   the harness's own input, this does not);
 * - `readyLatency`: the viewer's click on Comment (the click event's own
 *   timestamp) → comment mode on in the frame: the shell's and the bridge's
 *   share of comment ready, without the harness's time to find and press the
 *   button. */
type Metrics = { firstPaint: number; commentReady: number };
type Shell = { framePaint: number; readyLatency: number };
type Modes<T> = { subdomain: T; sandbox: T };
/** `enforceTargets` turns on the checks of the `Shell` metrics against their
 * budgets; `Metrics` budgets are always checked. A platform recorded before
 * the `Shell` metrics existed has none of them until they are recorded. */
type Budget = { baseline: Modes<Metrics & Partial<Shell> & { control: number }>; budget: Modes<Metrics & Partial<Shell>>; control: Modes<number>; enforceTargets: boolean };
/** budget.json: one Budget per platform, keyed by `PLATFORM`. Times differ
 * too much between machines for one platform's budget to judge another's. */
type Budgets = Record<string, Budget>;
/** `paintSource` says which clock gave `firstPaint`: the frame's paint
 * timing, or the two-animation-frame fallback where paint timing is missing. */
type Sample = Metrics & Shell & { control: number; paintSource: "paint" | "raf"; framePaintSource: "paint" | "raf" };

const BUDGET = fileURLToPath(new URL("./budget.json", import.meta.url));
const RESULTS = fileURLToPath(new URL("./results.json", import.meta.url));
const WARMUPS = 2;
const SAMPLES = 9;
const MODES: FrameMode[] = ["subdomain", "sandbox"];
/** What `CLAX_PERF_RECORD` asks for: nothing, the first baseline, or lower budgets. */
const RECORD = process.env.CLAX_PERF_RECORD ?? "";
/** The key of this machine's entry in budget.json, e.g. `darwin-arm64`. */
const PLATFORM = `${platform()}-${arch()}`;

/** A readable page of about 60 KB: headings, paragraphs, a table. */
const PAGE = "<!doctype html><title>Perf</title><style>body{font:16px/1.5 system-ui;margin:2rem}</style>"
  + Array.from({ length: 40 }, (_, i) => `<h2>Section ${i + 1}</h2><p>${"Clax keeps the conversation next to the page it is about. ".repeat(24)}</p>`).join("")
  + `<table>${Array.from({ length: 30 }, (_, r) => `<tr>${Array.from({ length: 6 }, (_cell, c) => `<td>r${r}c${c}</td>`).join("")}</tr>`).join("")}</table>`;

/** Runs in every frame before its scripts: records the document's time
 * origin, its first contentful paint (or, where paint timing is missing, two
 * animation frames after DOMContentLoaded), when comment mode's crosshair
 * first shows, and the first trusted click (by its event timestamp), all in
 * epoch milliseconds. */
function recorder() {
  const rec = { origin: performance.timeOrigin, paint: null as number | null, raf: null as number | null, crosshair: null as number | null, click: null as number | null };
  Object.defineProperty(window, "claxPerf", { value: rec });
  const at = () => performance.timeOrigin + performance.now();
  try {
    new PerformanceObserver(list => {
      for (const e of list.getEntries()) if (e.name === "first-contentful-paint" && rec.paint === null) rec.paint = performance.timeOrigin + e.startTime;
    }).observe({ type: "paint", buffered: true });
  } catch { /* no paint timing here */ }
  // The first click the browser delivers (the harness's press on Comment), by
  // the event's own timestamp.
  addEventListener("click", e => { if (e.isTrusted && rec.click === null) rec.click = performance.timeOrigin + e.timeStamp; }, true);
  const check = () => { if (rec.crosshair === null && document.documentElement?.style.cursor === "crosshair") rec.crosshair = at(); };
  new MutationObserver(check).observe(document, { subtree: true, attributes: true, attributeFilter: ["style"] });
  const painted = () => requestAnimationFrame(() => requestAnimationFrame(() => { rec.raf ??= at(); }));
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", painted, { once: true });
  else painted();
}

type Rec = { origin: number; paint: number | null; raf: number | null; crosshair: number | null; click: number | null };
const median = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };

async function poll<T>(f: () => Promise<T | null>, what: string): Promise<T> {
  const deadline = Date.now() + 60_000;
  for (;;) {
    const v = await f();
    if (v !== null) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 20));
  }
}

/** The frame's first paint in a context of its own, set up as `sample`'s (a
 * first tab that loaded the artifact), in a fresh tab where the harness opens
 * the link and does nothing else. Paint timing is waited for; the
 * animation-frame fallback counts only where none comes within half a second
 * of it. */
async function quietPaint(browser: Browser, base: string, id: string, mode: FrameMode): Promise<{ paint: number; source: "paint" | "raf" }> {
  const ctx = await browser.newContext();
  try {
    await ctx.addInitScript(recorder);
    if (mode === "sandbox") await ctx.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    const first = await ctx.newPage();
    await first.goto(`${base}/a/${id}`);
    await contentFrame(first, id, 1);
    await first.getByRole("button", { name: "Comment" }).waitFor();
    await first.close();

    const quiet = await ctx.newPage();
    await quiet.goto(`${base}/a/${id}`, { waitUntil: "commit" });
    const frame = await contentFrame(quiet, id, 1);
    const shell = await poll(() => quiet.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf?.origin ?? null), "the quiet tab's time origin");
    let rafSeen = 0;
    const r = await poll(async () => {
      const x = await frame.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf ?? null);
      if (!x) return null;
      if (x.paint !== null) return x;
      if (x.raf !== null) { rafSeen ||= Date.now(); if (Date.now() - rafSeen > 500) return x; }
      return null;
    }, "the quiet tab's first paint");
    return { paint: (r.paint ?? r.raf)! - shell, source: r.paint !== null ? "paint" : "raf" };
  } finally {
    await ctx.close();
  }
}

/** One sample: a context that has seen the artifact once (a first tab that
 * loaded it and pressed nothing), then a fresh tab opening the link. */
async function sample(browser: Browser, base: string, port: string, id: string, mode: FrameMode): Promise<Sample> {
  const ctx = await browser.newContext();
  try {
    await ctx.addInitScript(recorder);
    if (mode === "sandbox") await ctx.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    const first = await ctx.newPage();
    await first.goto(`${base}/a/${id}`);
    await contentFrame(first, id, 1);
    await first.getByRole("button", { name: "Comment" }).waitFor();
    await first.close();

    const page = await ctx.newPage();
    await page.goto(`${base}/a/${id}`, { waitUntil: "commit" });
    // Press Comment the moment it exists and is enabled. A plain `click()`
    // first waits for the button to hold still over two animation frames,
    // which adds about 46 ms of Playwright to the app's time. `force` skips
    // those checks but still sends real input through Chromium, so the shell
    // sees a trusted click, as a person's; `dispatchEvent` would be synthetic.
    const comment = page.getByRole("button", { name: "Comment", disabled: false });
    await comment.waitFor({ state: "attached" });
    await comment.click({ force: true });
    const frame = await contentFrame(page, id, 1);
    const shell = await poll(() => page.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf?.origin ?? null), "the shell's time origin");
    const click = await poll(() => page.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf?.click ?? null), "the click on Comment");
    const rec = await poll(async () => {
      const r = await frame.evaluate(() => (window as unknown as { claxPerf?: Rec }).claxPerf ?? null);
      return r && (r.paint ?? r.raf) !== null && r.crosshair !== null ? r : null;
    }, "first paint and comment mode in the frame");

    // The control, in both modes, is a direct navigation to the artifact's
    // own host. Leaving the shell's origin for it is a cross-site move that
    // starts a new renderer process, so it slows down with the machine (44 ms
    // quiet, 72 ms under load here). A same-site `/c/` navigation takes about
    // 16 ms, under the noise floor, and barely moves under load, so it could
    // not scale the sandbox budgets.
    await page.goto(`http://${id}.localhost:${port}/v/1/`);
    const ctl = await poll(() => page.evaluate(() => {
      const r = (window as unknown as { claxPerf?: Rec }).claxPerf;
      const p = r ? r.paint ?? r.raf : null;
      return r && p !== null ? p - r.origin : null;
    }), "the control page's first paint");


    const q = await quietPaint(browser, base, id, mode);
    return {
      firstPaint: (rec.paint ?? rec.raf)! - shell,
      commentReady: rec.crosshair! - shell,
      framePaint: q.paint,
      readyLatency: rec.crosshair! - click,
      control: ctl,
      paintSource: rec.paint !== null ? "paint" : "raf",
      framePaintSource: q.source,
    };
  } finally {
    await ctx.close();
  }
}

function loadAll(): Budgets {
  return existsSync(BUDGET) ? JSON.parse(readFileSync(BUDGET, "utf8")) as Budgets : {};
}

function save(b: Budget): void {
  const all = loadAll();
  all[PLATFORM] = b;
  writeFileSync(BUDGET, JSON.stringify(all, null, 2) + "\n");
}

/** Chromium reports these times in 4 ms steps, and the medians are only
 * tens of milliseconds, so a relative margin alone is a step or two wide.
 * Each budget is therefore at least 30 ms above its median, and the control
 * counts as no less than 30 ms when it scales the budgets, so one 4 ms step
 * of a fast control cannot move a limit by 25%. */
const NOISE_FLOOR_MS = 30;
const budgetFor = (mid: number) => Math.ceil(Math.max(mid * 1.25, mid + NOISE_FLOOR_MS));
const floored = (control: number) => Math.max(control, NOISE_FLOOR_MS);

function judge(mode: FrameMode, m: Metrics & Shell & { control: number }): void {
  const round = (x: number) => Math.ceil(x);
  const b = loadAll()[PLATFORM] ?? null;
  if (RECORD === "baseline") {
    // Each metric's baseline is recorded once per platform: a platform that
    // already has the `Metrics` baselines gains only the missing `Shell` ones.
    if (b?.baseline[mode].firstPaint) {
      if (b.baseline[mode].framePaint) throw new Error(`budget.json already holds a ${PLATFORM} baseline; it is recorded once per platform`);
      b.baseline[mode] = { ...b.baseline[mode], framePaint: round(m.framePaint), readyLatency: round(m.readyLatency) };
      b.budget[mode] = { ...b.budget[mode], framePaint: budgetFor(m.framePaint), readyLatency: budgetFor(m.readyLatency) };
      save(b);
      return;
    }
    const next: Budget = b ?? { baseline: { subdomain: { firstPaint: 0, commentReady: 0, control: 0 }, sandbox: { firstPaint: 0, commentReady: 0, control: 0 } }, budget: { subdomain: { firstPaint: 0, commentReady: 0 }, sandbox: { firstPaint: 0, commentReady: 0 } }, control: { subdomain: 0, sandbox: 0 }, enforceTargets: true };
    next.baseline[mode] = { firstPaint: round(m.firstPaint), commentReady: round(m.commentReady), framePaint: round(m.framePaint), readyLatency: round(m.readyLatency), control: round(m.control) };
    next.budget[mode] = { firstPaint: budgetFor(m.firstPaint), commentReady: budgetFor(m.commentReady), framePaint: budgetFor(m.framePaint), readyLatency: budgetFor(m.readyLatency) };
    next.control[mode] = round(m.control);
    save(next);
    return;
  }
  const measured = `first paint ${m.firstPaint.toFixed(0)} ms, comment ready ${m.commentReady.toFixed(0)} ms, frame paint ${m.framePaint.toFixed(0)} ms, ready latency ${m.readyLatency.toFixed(0)} ms, control ${m.control.toFixed(0)} ms`;
  if (!b) {
    // No baseline for this platform (a CI runner, say): report, never fail.
    console.log(`${mode}: NOT GATED: web/perf/budget.json has no baseline for ${PLATFORM}, so this run only reports: ${measured}. Record one on this platform with CLAX_PERF_RECORD=baseline.`);
    if (RECORD === "budget") throw new Error(`no ${PLATFORM} baseline to lower budgets from; record one with CLAX_PERF_RECORD=baseline`);
    return;
  }
  const scale = Math.min(3, Math.max(1, floored(m.control) / floored(b.control[mode])));
  const limit = (x: number) => x * scale;
  const bud = b.budget[mode];
  const of = (x: number | undefined) => x === undefined ? "no budget" : `budget ${x}, limit ${limit(x).toFixed(0)}`;
  const report = `${mode}: first paint ${m.firstPaint.toFixed(0)} ms (${of(bud.firstPaint)}), comment ready ${m.commentReady.toFixed(0)} ms (${of(bud.commentReady)}), frame paint ${m.framePaint.toFixed(0)} ms (${of(bud.framePaint)}), ready latency ${m.readyLatency.toFixed(0)} ms (${of(bud.readyLatency)}), control ${m.control.toFixed(0)} ms (scale ${scale.toFixed(2)})`;
  console.log(report);
  expect(m.firstPaint, report).toBeLessThanOrEqual(limit(bud.firstPaint));
  expect(m.commentReady, report).toBeLessThanOrEqual(limit(bud.commentReady));
  if (b.enforceTargets) {
    if (bud.framePaint === undefined || bud.readyLatency === undefined) throw new Error(`${report}: enforceTargets is on, but ${PLATFORM} has no frame paint or ready latency budget; record them with CLAX_PERF_RECORD=baseline`);
    expect(m.framePaint, report).toBeLessThanOrEqual(limit(bud.framePaint));
    expect(m.readyLatency, report).toBeLessThanOrEqual(limit(bud.readyLatency));
  }
  if (RECORD === "budget") {
    const low = (next: number, cur: number | undefined) => Math.min(next, cur ?? Infinity);
    b.budget[mode] = {
      firstPaint: low(budgetFor(m.firstPaint), bud.firstPaint),
      commentReady: low(budgetFor(m.commentReady), bud.commentReady),
      ...(bud.framePaint !== undefined ? { framePaint: low(budgetFor(m.framePaint), bud.framePaint) } : {}),
      ...(bud.readyLatency !== undefined ? { readyLatency: low(budgetFor(m.readyLatency), bud.readyLatency) } : {}),
    };
    b.control[mode] = round(m.control);
    save(b);
  }
}

let d: Awaited<ReturnType<typeof startDaemon>>;
let id = "";
test.beforeAll(async () => {
  test.setTimeout(300_000);
  d = await startDaemon();
  id = (await publish(d.base, d.token, "Perf", { "index.html": PAGE })).artifact.id;
});
test.afterAll(async () => { await d?.stop(); });

for (const mode of MODES) {
  test(`time to usable (${mode})`, async ({ browser }) => {
    const port = new URL(d.base).port;
    for (let i = 0; i < WARMUPS; i++) await sample(browser, d.base, port, id, mode);
    const xs: Sample[] = [];
    for (let i = 0; i < SAMPLES; i++) xs.push(await sample(browser, d.base, port, id, mode));
    const m: Metrics & Shell & { control: number } = { firstPaint: median(xs.map(x => x.firstPaint)), commentReady: median(xs.map(x => x.commentReady)), framePaint: median(xs.map(x => x.framePaint)), readyLatency: median(xs.map(x => x.readyLatency)), control: median(xs.map(x => x.control)) };
    const results = existsSync(RESULTS) ? JSON.parse(readFileSync(RESULTS, "utf8")) : {};
    results[mode] = { platform: PLATFORM, median: m, samples: xs };
    writeFileSync(RESULTS, JSON.stringify(results, null, 2) + "\n");
    judge(mode, m);
  });
}
