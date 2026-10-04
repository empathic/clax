// This viewer's presence reports (spec §10, "Presence"), loaded after the
// first paint: a report on start, on visibility changes, when the selection
// or the composer's anchor changes, on the first input after an away period,
// and every 30 s; away as the page is left; and the artifact's presence, fetched once and on each
// stream (re)connect.
import { getPresence, putPresence } from "../api";
import type { ViewState } from "./artifact-controller";
import { type PresenceView, stateFor, whereLabel } from "./presence-model";

const INPUTS = ["pointerdown", "pointermove", "keydown", "wheel", "touchstart"] as const;

export class PresenceReporter {
  /** When the viewer last pressed, typed, moved the pointer or scrolled in the shell. */
  private lastInput = Date.now();
  /** The last report sent: its state, location and time. */
  private reported: { state: "here" | "away"; where: string | null; at: number } | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private readonly beat: ReturnType<typeof setInterval>;
  private readonly onVisible = () => this.report();
  private readonly onInput = () => {
    this.lastInput = Date.now();
    if (this.reported?.state === "away" && document.visibilityState === "visible") this.report();
  };
  /** Leaving the page: away at once, rather than here until the report lapses. */
  readonly leave = () => {
    if (this.done || !this.may(this.state())) return;
    this.reported = { state: "away", where: null, at: Date.now() };
    void putPresence(this.id, "away", null, true);
  };
  private done = false;

  /** `may(s)`: whether `s` may be reported (a viewer cookie exists and the
   * artifact is loaded and not deleted); `take` receives the artifact's presence. */
  constructor(private readonly id: string, private readonly state: () => ViewState, private readonly may: (s: ViewState) => boolean, private readonly take: (people: PresenceView[]) => void) {
    document.addEventListener("visibilitychange", this.onVisible);
    addEventListener("pagehide", this.leave);
    for (const t of INPUTS) addEventListener(t, this.onInput, { passive: true, capture: true });
    this.beat = setInterval(() => this.report(true), 30_000);
  }

  fetch(): void {
    void getPresence(this.id).then(people => { if (people && !this.done) this.take(people); });
  }

  /** Reports this viewer's presence. A report that says nothing new is
   * skipped unless `force`; reports go out at most once per 2 s (a later one
   * is sent when the 2 s are up), unless the state flips or `reset` ran. */
  report(force = false): void {
    const s = this.state();
    if (this.done || !this.may(s)) return;
    const state = stateFor(document.visibilityState === "visible", Date.now() - this.lastInput);
    const where = state === "here" && s.shareWhere ? whereLabel(s) : null;
    const last = this.reported;
    if (!force && last && last.state === state && last.where === where) return;
    const wait = last && last.state === state ? last.at + 2000 - Date.now() : 0;
    if (wait > 0) {
      this.timer ??= setTimeout(() => { this.timer = undefined; this.report(true); }, wait);
      return;
    }
    clearTimeout(this.timer);
    this.timer = undefined;
    this.reported = { state, where, at: Date.now() };
    void putPresence(this.id, state, where).then(people => { if (people && !this.done) this.take(people); });
  }

  /** Lets the next report go out at once (the share switch changed). */
  reset(): void { this.reported = null; }

  dispose(): void {
    this.done = true;
    document.removeEventListener("visibilitychange", this.onVisible);
    removeEventListener("pagehide", this.leave);
    for (const t of INPUTS) removeEventListener(t, this.onInput, { capture: true });
    clearInterval(this.beat);
    clearTimeout(this.timer);
  }
}
