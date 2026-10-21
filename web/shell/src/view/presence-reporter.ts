// This viewer's presence reports (spec §10, "Presence"), loaded after the
// first paint: a report on start, on visibility changes, when the selection
// or the composer's anchor changes, on the first input after an away period,
// and every 30 s; away as the page is left; and the artifact's presence, fetched once and on each
// stream (re)connect. Each report says whether the tab has focus and how
// long ago the viewer last gave input in it (never a time, so the browser's
// clock does not matter); the daemon keeps both for the viewer's own agents
// and shows them to no one. Input there is the viewer's own act: a pointer
// moving over a window without focus (on its way to another app) is not,
// and a page opened in the background has none until the viewer acts, so
// its reports leave `idle_ms` out until then. Gaining or losing focus reports at once; while
// focus sits in the artifact's frame, where the shell window hears no blur
// or focus as the viewer leaves the browser or comes back, focus is read
// again every second.
import { after, wall } from "../clock";
import type { ViewState } from "./artifact-controller";
import { type PresenceView, stateFor, whereLabel } from "./presence-model";

const INPUTS = ["pointerdown", "pointermove", "keydown", "wheel", "touchstart"] as const;
/** How often focus is read again while it sits in a frame. */
const FRAME_FOCUS_MS = 1000;

export class PresenceReporter {
  /** This page's name in its reports: one viewer (the owner's browsers are
   * one) may have the artifact open in several tabs, which the daemon combines. */
  private readonly tab = Math.random().toString(36).slice(2, 12);
  /** When the viewer last pressed, typed, moved the pointer or scrolled in
   * the shell, counted from the page's start: whether they are here or away. */
  private lastInput = wall();
  /** When the viewer last gave input the reports' `idle_ms` describes: as
   * `lastInput`, but a pointer move only while the window has focus, and
   * unset until the first. */
  private engagedInput: number | null = null;
  /** The last report sent: its state, location, focus and time. */
  private reported: { state: "here" | "away"; where: string | null; focused: boolean; at: number } | null = null;
  private timer: (() => void) | undefined;
  private readonly beat: ReturnType<typeof setInterval>;
  /** Cancels the pending re-read of focus while it sits in a frame. */
  private frameWatch: (() => void) | undefined;
  private readonly onVisible = () => { this.report(); this.watchFrame(); };
  private readonly onFocus = () => { this.report(); this.watchFrame(); };
  private readonly onInput = (e: Event) => {
    this.lastInput = wall();
    if (e.type !== "pointermove" || document.hasFocus()) this.engagedInput = this.lastInput;
    if (this.reported?.state === "away" && document.visibilityState === "visible") this.report();
  };
  /** Leaving the page: away at once, rather than here until the report lapses. */
  readonly leave = () => {
    if (this.done || !this.may(this.state())) return;
    this.reported = { state: "away", where: null, focused: false, at: wall() };
    void putPresence(this.id, "away", null, this.tab, true, { focused: false, ...this.idle() });
  };
  private done = false;

  /** `may(s)`: whether `s` may be reported (a viewer cookie exists and the
   * artifact is loaded and not deleted); `take` receives the artifact's
   * presence with the ticket `ticket` gave as its request started, so the
   * view can tell an answer from before an event it heard since. */
  constructor(private readonly id: string, private readonly state: () => ViewState, private readonly may: (s: ViewState) => boolean, private readonly take: (p: Presence, ticket: number) => void, private readonly ticket: () => number = () => 0) {
    document.addEventListener("visibilitychange", this.onVisible);
    addEventListener("focus", this.onFocus);
    addEventListener("blur", this.onFocus);
    addEventListener("pagehide", this.leave);
    for (const t of INPUTS) addEventListener(t, this.onInput, { passive: true, capture: true });
    this.beat = setInterval(() => this.report(true), 30_000);
    // Focus may already sit in a frame, where no blur will say so.
    if (document.activeElement?.tagName === "IFRAME") this.watchFrame();
  }

  fetch(): void {
    const t = this.ticket();
    void getPresence(this.id).then(p => { if (p && !this.done) this.take(p, t); });
  }

  /** Reports this viewer's presence. A report that says nothing new is
   * skipped unless `force`; reports go out at most once per 2 s (a later one
   * is sent when the 2 s are up), unless the state or focus flips or `reset` ran. */
  report(force = false): void {
    const s = this.state();
    if (this.done || !this.may(s)) return;
    const state = stateFor(document.visibilityState === "visible", wall() - this.lastInput);
    const where = state === "here" && s.shareWhere ? whereLabel(s) : null;
    const focused = document.visibilityState === "visible" && document.hasFocus();
    const last = this.reported;
    if (!force && last && last.state === state && last.where === where && last.focused === focused) return;
    // A flip of state or focus goes out at once; anything else at most every 2 s.
    const flip = !last || last.state !== state || last.focused !== focused;
    const wait = flip ? 0 : last.at + 2000 - wall();
    if (wait > 0) {
      this.timer ??= after(wait, () => { this.timer = undefined; this.report(true); });
      return;
    }
    this.timer?.();
    this.timer = undefined;
    this.reported = { state, where, focused, at: wall() };
    const t = this.ticket();
    void putPresence(this.id, state, where, this.tab, false, { focused, ...this.idle() }).then(p => { if (p && !this.done) this.take(p, t); });
  }

  /** Re-reads focus every second while the tab is visible and focus is in a
   * frame (a blur toward the frame is followed by one re-read, since focus
   * lands there after the blur), reporting when it changed. */
  private watchFrame(): void {
    if (this.done || this.frameWatch) return;
    this.frameWatch = after(FRAME_FOCUS_MS, () => {
      this.frameWatch = undefined;
      if (this.done) return;
      this.report();
      if (document.visibilityState === "visible" && document.activeElement?.tagName === "IFRAME") this.watchFrame();
    });
  }

  /** `idle_ms`, how long ago in whole milliseconds the viewer last gave
   * input in this tab; nothing before their first. */
  private idle(): { idle_ms?: number } {
    return this.engagedInput === null ? {} : { idle_ms: Math.max(0, Math.round(wall() - this.engagedInput)) };
  }

  /** Lets the next report go out at once (the share switch changed). */
  reset(): void { this.reported = null; }

  dispose(): void {
    this.done = true;
    document.removeEventListener("visibilitychange", this.onVisible);
    removeEventListener("focus", this.onFocus);
    removeEventListener("blur", this.onFocus);
    removeEventListener("pagehide", this.leave);
    for (const t of INPUTS) removeEventListener(t, this.onInput, { capture: true });
    clearInterval(this.beat);
    this.timer?.();
    this.frameWatch?.();
  }
}

/** Reports this viewer here or away on `aid` from tab `tab`, with where they look and, when given,
 * whether the tab has focus and how long ago they last gave input in it (left out before any); answers the artifact's
 * presence, or null on failure (the next report writes again). */
export async function putPresence(aid: string, state: "here" | "away", where: string | null, tab: string, keepalive = false, engaged?: { focused: boolean; idle_ms?: number }): Promise<Presence | null> {
  try {
    const r = await fetch("/api/viewers/me/presence", { method: "PUT", keepalive, headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, state, tab, ...where && { where }, ...engaged }) });
    return r.ok ? presenceOf(await r.json()) : null;
  } catch { return null; }
}
/** The artifact's presence, or null on failure. */
export async function getPresence(aid: string): Promise<Presence | null> {
  try {
    const r = await fetch(`/api/artifacts/${aid}/presence`);
    return r.ok ? presenceOf(await r.json()) : null;
  } catch { return null; }
}
/** An artifact's presence: the people listed (seated ones, spectators never,
 * spec 2026-10-11-clax-dev-hosted-design §16.3) and how many are viewing,
 * spectators included (null when the daemon does not say). */
export type Presence = { people: PresenceView[]; viewing: number | null };
function presenceOf(v: { people?: unknown; viewing?: unknown }): Presence | null {
  if (!Array.isArray(v.people)) return null;
  return { people: v.people as PresenceView[], viewing: typeof v.viewing === "number" ? v.viewing : null };
}
