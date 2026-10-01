import { test, expect, type Browser } from "@playwright/test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { arch, platform } from "node:os";
import { fileURLToPath } from "node:url";
import { type FrameMode, contentFrame, publish, startDaemon } from "../e2e/fixtures";

type Metrics = { firstPaint: number; commentReady: number };
type Modes<T> = { subdomain: T; sandbox: T };
type Budget = { baseline: Modes<Metrics & { control: number }>; budget: Modes<Metrics>; control: Modes<number>; enforceTargets: boolean };
/** budget.json: one Budget per platform, keyed by `PLATFORM`. Times differ
 * too much between machines for one platform's budget to judge another's. */
type Budgets = Record<string, Budget>;
/** `paintSource` says which clock gave `firstPaint`: the frame's paint
 * timing, or the two-animation-frame fallback where paint timing is missing. */
type Sample = Metrics & { control: number; paintSource: "paint" | "raf" };

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
 * animation frames after DOMContentLoaded), and when comment mode's crosshair
 * first shows, all in epoch milliseconds. */
function recorder() {
  const rec = { origin: performance.timeOrigin, paint: null as number | null, raf: null as number | null, crosshair: null as number | null };
  Object.defineProperty(window, "claxPerf", { value: rec });
  const at = () => performance.timeOrigin + performance.now();
  try {
    new PerformanceObserver(list => {
      for (const e of list.getEntries()) if (e.name === "first-contentful-paint" && rec.paint === null) rec.paint = performance.timeOrigin + e.startTime;
    }).observe({ type: "paint", buffered: true });
  } catch { /* no paint timing here */ }
  const check = () => { if (rec.crosshair === null && document.documentElement?.style.cursor === "crosshair") rec.crosshair = at(); };
  new MutationObserver(check).observe(document, { subtree: true, attributes: true, attributeFilter: ["style"] });
  const painted = () => requestAnimationFrame(() => requestAnimationFrame(() => { rec.raf ??= at(); }));
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", painted, { once: true });
  else painted();
}

type Rec = { origin: number; paint: number | null; raf: number | null; crosshair: number | null };
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
    return { firstPaint: (rec.paint ?? rec.raf)! - shell, commentReady: rec.crosshair! - shell, control: ctl, paintSource: rec.paint !== null ? "paint" : "raf" };
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

function judge(mode: FrameMode, m: Metrics & { control: number }): void {
  const round = (x: number) => Math.ceil(x);
  const b = loadAll()[PLATFORM] ?? null;
  if (RECORD === "baseline") {
    if (b?.baseline[mode].firstPaint) throw new Error(`budget.json already holds a ${PLATFORM} baseline; it is recorded once per platform`);
    const next: Budget = b ?? { baseline: { subdomain: { firstPaint: 0, commentReady: 0, control: 0 }, sandbox: { firstPaint: 0, commentReady: 0, control: 0 } }, budget: { subdomain: { firstPaint: 0, commentReady: 0 }, sandbox: { firstPaint: 0, commentReady: 0 } }, control: { subdomain: 0, sandbox: 0 }, enforceTargets: false };
    next.baseline[mode] = { firstPaint: round(m.firstPaint), commentReady: round(m.commentReady), control: round(m.control) };
    next.budget[mode] = { firstPaint: budgetFor(m.firstPaint), commentReady: budgetFor(m.commentReady) };
    next.control[mode] = round(m.control);
    save(next);
    return;
  }
  if (!b) {
    // No baseline for this platform (a CI runner, say): report, never fail.
    console.log(`${mode}: NOT GATED: web/perf/budget.json has no baseline for ${PLATFORM}, so this run only reports: first paint ${m.firstPaint.toFixed(0)} ms, comment ready ${m.commentReady.toFixed(0)} ms, control ${m.control.toFixed(0)} ms. Record one on this platform with CLAX_PERF_RECORD=baseline.`);
    if (RECORD === "budget") throw new Error(`no ${PLATFORM} baseline to lower budgets from; record one with CLAX_PERF_RECORD=baseline`);
    return;
  }
  const scale = Math.min(3, Math.max(1, floored(m.control) / floored(b.control[mode])));
  const limit = (x: number) => x * scale;
  const report = `${mode}: first paint ${m.firstPaint.toFixed(0)} ms (budget ${b.budget[mode].firstPaint}, limit ${limit(b.budget[mode].firstPaint).toFixed(0)}), comment ready ${m.commentReady.toFixed(0)} ms (budget ${b.budget[mode].commentReady}, limit ${limit(b.budget[mode].commentReady).toFixed(0)}), control ${m.control.toFixed(0)} ms (scale ${scale.toFixed(2)})`;
  console.log(report);
  expect(m.firstPaint, report).toBeLessThanOrEqual(limit(b.budget[mode].firstPaint));
  expect(m.commentReady, report).toBeLessThanOrEqual(limit(b.budget[mode].commentReady));
  if (b.enforceTargets) {
    expect(m.firstPaint, `${report}; target ≤ 50% of the baseline ${b.baseline[mode].firstPaint}`).toBeLessThanOrEqual(limit(b.baseline[mode].firstPaint * 0.5));
    expect(m.commentReady, `${report}; target ≤ 70% of the baseline ${b.baseline[mode].commentReady}`).toBeLessThanOrEqual(limit(b.baseline[mode].commentReady * 0.7));
  }
  if (RECORD === "budget") {
    const next = { firstPaint: budgetFor(m.firstPaint), commentReady: budgetFor(m.commentReady) };
    b.budget[mode] = { firstPaint: Math.min(next.firstPaint, b.budget[mode].firstPaint), commentReady: Math.min(next.commentReady, b.budget[mode].commentReady) };
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
    const m: Metrics & { control: number } = { firstPaint: median(xs.map(x => x.firstPaint)), commentReady: median(xs.map(x => x.commentReady)), control: median(xs.map(x => x.control)) };
    const results = existsSync(RESULTS) ? JSON.parse(readFileSync(RESULTS, "utf8")) : {};
    results[mode] = { platform: PLATFORM, median: m, samples: xs };
    writeFileSync(RESULTS, JSON.stringify(results, null, 2) + "\n");
    judge(mode, m);
  });
}
