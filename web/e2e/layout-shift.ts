// What a page load moves and rewrites, for tests that hold a page to
// loading in its final shape. Both recorders are init scripts, installed
// before any of the page's own scripts run, in the top frame only:
//
// - `recordLayoutShifts`: every `layout-shift` entry of the Layout
//   Instability API (`buffered: true`), with its value, `hadRecentInput`,
//   and each source's node (tag, ID, classes, a snippet of its text) and its
//   rectangles before and after, so a failure says what moved and where.
// - `recordTexts`: for each selector given, every distinct `textContent` the
//   first element it matches held, in order (null while none matches): the
//   flicker the Layout Instability API does not see, text rendered and then
//   replaced in a box that keeps its size. A MutationObserver reads the
//   selectors at each microtask checkpoint after the DOM changed, so a value
//   is recorded once per task or flush that left it in the DOM, whatever the
//   machine's speed.
//
// Each record carries the stage it happened in: the last label `markStage`
// set before it (`load-stages.ts` marks one per released request), or null
// before the first.
import type { Page } from "@playwright/test";
import { quietEval } from "./time";

export type Rect = { x: number; y: number; w: number; h: number };
export type ShiftSource = { node: string; previous: Rect; current: Rect };
export type Shift = { value: number; hadRecentInput: boolean; t: number; stage: string | null; sources: ShiftSource[] };
export type TextValue = { value: string | null; t: number; stage: string | null };

/** Installs the layout-shift recorder in `page`'s top frame, before any of its scripts. */
export async function recordLayoutShifts(page: Page): Promise<void> {
  await page.addInitScript(() => {
    if (window !== window.top) return;
    const w = window as unknown as { claxStages?: { label: string; t: number }[]; claxShiftLog?: unknown[]; claxShiftFlush?: () => void };
    w.claxStages ??= [];
    w.claxShiftLog = [];
    const stageAt = (t: number) => { let s: string | null = null; for (const m of w.claxStages!) if (m.t <= t) s = m.label; return s; };
    const snip = (s: string | null) => (s ?? "").replace(/\s+/g, " ").trim().slice(0, 48);
    const describe = (n: Node | null | undefined): string => {
      if (!n) return "(a node since removed)";
      if (n.nodeType !== 1) return `text "${snip(n.textContent)}" in ${n.parentElement ? describe(n.parentElement) : "(detached)"}`;
      const e = n as Element;
      const cls = typeof e.className === "string" && e.className.trim() ? `.${e.className.trim().split(/\s+/).join(".")}` : "";
      return `${e.localName}${e.id ? `#${e.id}` : ""}${cls} "${snip(e.textContent)}"`;
    };
    const rect = (r: DOMRectReadOnly) => ({ x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) });
    type Entry = PerformanceEntry & { value: number; hadRecentInput: boolean; sources: { node: Node | null; previousRect: DOMRectReadOnly; currentRect: DOMRectReadOnly }[] };
    const take = (list: PerformanceEntryList) => {
      for (const e of list as Entry[]) {
        w.claxShiftLog!.push({ value: e.value, hadRecentInput: e.hadRecentInput, t: Math.round(e.startTime), stage: stageAt(e.startTime),
          sources: e.sources.map(s => ({ node: describe(s.node), previous: rect(s.previousRect), current: rect(s.currentRect) })) });
      }
    };
    try {
      const o = new PerformanceObserver(l => take(l.getEntries()));
      o.observe({ type: "layout-shift", buffered: true });
      w.claxShiftFlush = () => take(o.takeRecords());
    } catch { /* no Layout Instability API: nothing is recorded */ }
  });
}

/** The layout shifts recorded so far in `page`, those after recent input
 * left out (the API's own rule: a shift the person caused is expected). It
 * takes the entries the observer has not yet been handed too. */
export async function layoutShifts(page: Page): Promise<Shift[]> {
  const all = await quietEval<Shift[]>(page, `(() => {
    if (!window.claxShiftLog) throw new Error("recordLayoutShifts was not installed before this page loaded");
    window.claxShiftFlush?.();
    return window.claxShiftLog;
  })()`);
  return all.filter(s => !s.hadRecentInput);
}

/** The sum of the shifts' values (the page's cumulative layout shift for the load). */
export function shiftTotal(shifts: Shift[]): number {
  return shifts.reduce((n, s) => n + s.value, 0);
}

/** A failure message listing each shift: its value, its stage, and each node that moved, from where to where. */
export function formatShifts(shifts: Shift[]): string {
  if (!shifts.length) return "no layout shifts";
  const r = (x: Rect) => `(${x.x},${x.y} ${x.w}x${x.h})`;
  return [`${shifts.length} layout shift(s), total ${shiftTotal(shifts).toFixed(4)}:`, ...shifts.flatMap((s, i) => [
    `  #${i + 1} ${s.value.toFixed(4)} at ${s.t} ms, stage ${s.stage === null ? "(before the first release)" : `after ${s.stage}`}`,
    ...s.sources.map(x => `      ${x.node}: ${r(x.previous)} -> ${r(x.current)}`),
  ])].join("\n");
}

/** Installs the text recorder for `selectors` in `page`'s top frame, before any of its scripts. */
export async function recordTexts(page: Page, selectors: string[]): Promise<void> {
  await page.addInitScript((sels: string[]) => {
    if (window !== window.top) return;
    const w = window as unknown as { claxStages?: { label: string; t: number }[]; claxTexts?: Record<string, { value: string | null; t: number; stage: string | null }[]> };
    w.claxStages ??= [];
    const log: Record<string, { value: string | null; t: number; stage: string | null }[]> = Object.fromEntries(sels.map(s => [s, []]));
    w.claxTexts = log;
    const read = () => {
      const t = Math.round(performance.now());
      const stage = w.claxStages!.at(-1)?.label ?? null;
      for (const s of sels) {
        const v = document.querySelector(s)?.textContent ?? null;
        const l = log[s];
        if (!l.length || l[l.length - 1].value !== v) l.push({ value: v, t, stage });
      }
    };
    read();
    new MutationObserver(read).observe(document, { childList: true, subtree: true, characterData: true });
  }, selectors);
}

/** Each distinct text recorded for each selector, in order. */
export async function textHistory(page: Page): Promise<Record<string, TextValue[]>> {
  return quietEval(page, `(() => { if (!window.claxTexts) throw new Error("recordTexts was not installed before this page loaded"); return window.claxTexts; })()`);
}

/** The non-empty values a selector's text took, in order, each distinct from the one before (absence and empty text left out). */
export function shownTexts(history: TextValue[]): string[] {
  const out: string[] = [];
  for (const h of history) {
    const v = h.value?.trim();
    if (v && out.at(-1) !== v) out.push(v);
  }
  return out;
}

/** A failure message listing a selector's texts, with the stage each appeared in. */
export function formatTexts(selector: string, history: TextValue[]): string {
  return [`${selector} took ${history.length} value(s):`, ...history.map(h => `  ${JSON.stringify(h.value)} at ${h.t} ms, stage ${h.stage === null ? "(before the first release)" : `after ${h.stage}`}`)].join("\n");
}

/** Marks the start of a stage in `page`: the records that follow carry `label`. */
export async function markStage(page: Page, label: string): Promise<void> {
  await quietEval(page, `(() => { (window.claxStages ??= []).push({ label: ${JSON.stringify(label)}, t: performance.now() }); })()`);
}
