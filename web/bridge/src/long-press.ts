// A touch long-press for comment mode: one finger held `HOLD_MS` within
// `SLOP_PX` of where it went down arms (the caller outlines the target), and
// lifting it there picks. The pick waits for the release because the browser
// grants user activation on `touchend`, not while a finger rests, and the
// shell takes a pick only with the frame's activation. Moving beyond the
// slop, a second finger, or `touchcancel` abandons it; a release before the
// hold is an ordinary tap, left to the browser (its synthesized click).

export const HOLD_MS = 450;
export const SLOP_PX = 10;

/** Timers as the long-press uses them; injected by tests. */
export type Timers = { set(fn: () => void, ms: number): number; clear(id: number): void };

export type LongPressHooks = {
  /** The hold elapsed: outline what is under (x, y). */
  armed(x: number, y: number): void;
  /** Released after `armed`, without moving beyond the slop: pick what is under (x, y). */
  pick(x: number, y: number): void;
  /** Abandoned after `armed` (moved, second finger, cancel). */
  abandoned(): void;
};

/** The window's own timers, as they are when this is called. */
export function windowTimers(win: Window): Timers {
  const set = win.setTimeout.bind(win);
  const clear = win.clearTimeout.bind(win);
  return { set: (fn, ms) => set(fn, ms), clear: id => clear(id) };
}

/** One finger down: where it went down and its touch's identifier. */
type Hold = { id: number; x: number; y: number; timer: number | null; armed: boolean };

export class LongPress {
  private on = false;
  private hold: Hold | null = null;
  private readonly timers: Timers;
  private readonly trustedOnly: boolean;

  /** `opts.trustedOnly` (default true) ignores touches the page dispatched
   * itself; tests that synthesise input turn it off. */
  constructor(private readonly doc: Document, private readonly hooks: LongPressHooks, timers?: Timers, opts: { trustedOnly?: boolean } = {}) {
    this.timers = timers ?? windowTimers(doc.defaultView!);
    this.trustedOnly = opts.trustedOnly ?? true;
  }

  /** Whether a finger is down on a hold that has not ended or been
   * abandoned (armed or not). */
  get holding(): boolean {
    return this.hold !== null;
  }

  /** Listens to touches (capture, not passive) while on; off drops a hold
   * in progress without reporting it. */
  set(on: boolean): void {
    if (on === this.on) return;
    this.on = on;
    const listeners: [string, EventListener][] = [
      ["touchstart", this.onStart as EventListener],
      ["touchmove", this.onMove as EventListener],
      ["touchend", this.onEnd as EventListener],
      ["touchcancel", this.onCancel as EventListener],
    ];
    for (const [type, fn] of listeners) {
      if (on) this.doc.addEventListener(type, fn, { capture: true, passive: false });
      else this.doc.removeEventListener(type, fn, { capture: true });
    }
    if (!on) this.drop();
  }

  private real(e: Event): boolean {
    return !this.trustedOnly || e.isTrusted;
  }

  /** Forgets the hold, clearing its timer. */
  private drop(): Hold | null {
    const h = this.hold;
    if (h?.timer != null) this.timers.clear(h.timer);
    this.hold = null;
    return h;
  }

  /** Ends the hold; an armed one is reported abandoned. */
  private abandon(): void {
    if (this.drop()?.armed) this.hooks.abandoned();
  }

  /** The hold's touch in `list`, if it is there. */
  private find(list: TouchList): Touch | null {
    const h = this.hold;
    if (!h) return null;
    for (let i = 0; i < list.length; i++) if (list[i].identifier === h.id) return list[i];
    return null;
  }

  private onStart = (e: TouchEvent) => {
    if (!this.real(e)) return;
    // A second finger (or two at once) is a gesture of its own; a hold whose
    // finger is no longer down (its release never reached the document) ends.
    this.abandon();
    if (e.touches.length !== 1) return;
    const t = e.touches[0];
    const h: Hold = { id: t.identifier, x: t.clientX, y: t.clientY, timer: null, armed: false };
    h.timer = this.timers.set(() => {
      if (this.hold !== h) return;
      h.timer = null;
      h.armed = true;
      this.hooks.armed(h.x, h.y);
    }, HOLD_MS);
    this.hold = h;
  };

  private onMove = (e: TouchEvent) => {
    if (!this.real(e) || !this.hold) return;
    if (e.touches.length > 1) { this.abandon(); return; }
    const t = this.find(e.touches) ?? this.find(e.changedTouches);
    if (!t) return;
    if (Math.hypot(t.clientX - this.hold.x, t.clientY - this.hold.y) > SLOP_PX) { this.abandon(); return; }
    // An armed finger resting within the slop does not scroll the page.
    if (this.hold.armed) e.preventDefault();
  };

  private onEnd = (e: TouchEvent) => {
    if (!this.real(e) || !this.hold) return;
    const t = this.find(e.changedTouches);
    if (!t) return;
    const h = this.drop()!;
    if (!h.armed) return; // a tap: the browser's click takes it
    if (Math.hypot(t.clientX - h.x, t.clientY - h.y) > SLOP_PX) { this.hooks.abandoned(); return; }
    // No synthesized mouse events or click follow a cancelled release.
    e.preventDefault();
    this.hooks.pick(t.clientX, t.clientY);
  };

  private onCancel = (e: TouchEvent) => {
    if (!this.real(e)) return;
    this.abandon();
  };
}
