// One run as its panel follows it (spec 2026-10-21-claxbot §10): the run
// with its steps (`GET /api/bot/runs/<id>`), then its events (`…/events`,
// SSE: `step_started`, `turn_delta`, `script`, `call`, `step_done`,
// `run_end`), each applied once as it arrives (`applyEvent`). The events
// are opened after the run's `last_event_id`, and opened again after the
// last event applied when the stream ends before the run does, so none is
// missed and a replay at or below the last ID is dropped (`RunEvent`): at
// most `RETRIES` times in a row, each after `again` (injected for tests).
// A run that has ended needs no stream. `onStep` hears each step and the
// run's end (the panel's live line, the week's meter). `stop` ends it all.
import { readSse } from "../sse";
import { type BotRun, type RunEvent, getRun, runEvents } from "./api";
import { after, applyEvent, ended, parseEvent } from "./model";
import { refusal } from "../people/refusal";

const RETRIES = 5;

export class RunFeed {
  run = $state<BotRun | null>(null);
  error = $state<string | null>(null);
  /** The ID of the last event applied (or the run's own `last_event_id`). */
  last: string | null = null;
  private abort = new AbortController();
  private tries = 0;

  constructor(
    readonly id: string,
    private readonly again: (f: () => void) => void = f => { setTimeout(f, 1000); },
    private readonly onStep: (ev: RunEvent, run: BotRun) => void = () => {},
  ) {}

  start(): void { void this.follow(); }

  stop(): void { this.abort.abort(); }

  get stopped(): boolean { return this.abort.signal.aborted; }

  private async follow(): Promise<void> {
    if (this.stopped) return;
    try {
      if (!this.run) {
        const r = await getRun(this.id);
        if (this.stopped) return;
        this.run = r;
        this.last = r.last_event_id ?? null;
        if (ended(r)) return;
      }
      this.error = null;
      const res = await runEvents(this.id, this.abort.signal, this.last);
      if (!res.ok || !res.body) throw new Error(`the run's events did not open (HTTP ${res.status})`);
      for await (const m of readSse(res.body)) {
        const ev = parseEvent(m);
        if (!ev || !this.run) continue;
        if (!after(m.id, this.last)) continue;
        if (m.id) this.last = m.id;
        this.tries = 0;
        this.run = applyEvent(this.run, ev);
        if (ev.event === "step_done" || ev.event === "run_end") this.onStep(ev, this.run);
        if (ev.event === "run_end") { this.abort.abort(); return; }
      }
    } catch (e) {
      if (this.stopped) return;
      this.error = refusal(e);
    }
    if (this.stopped || (this.run && ended(this.run))) return;
    if (++this.tries > RETRIES) { this.error ??= "The run stopped updating. Reload to follow it again."; return; }
    this.again(() => { void this.follow(); });
  }
}
