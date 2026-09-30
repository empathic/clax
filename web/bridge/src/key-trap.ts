// The hand-off from a pick to its composer. The shell opens the composer at
// the pick's start and focuses its textarea a moment after the click or
// release that picks (16–30 ms on a light page; on a page whose screenshot
// work holds the main thread, once that work yields). Until focus leaves the
// page, every key and text event the viewer makes there is kept from the
// page and from comment mode: those keys were meant for the comment, so the
// page neither acts on them nor learns them. They are dropped, never handed
// on: nothing typed in the page reaches the composer.
//
// The trap ends when the page loses focus (the composer took it), when the
// shell says it refused the pick (`end`), or `TRAP_MS` after the pick.
//
// Its listeners are on the window, capturing, and added when the bridge
// loads, before any page script runs, so no listener of the page's hears a
// trapped event; the timer functions it uses are the ones in place then.
// Events the page dispatched itself are left alone.

/** The longest the trap holds the page's keys after a pick, when neither
 * focus leaves the page nor the shell answers (longer than the main-thread
 * stall a heavy page's screenshot work can cause). */
export const TRAP_MS = 2_000;

/** Every key and text event type the trap keeps from the page. */
export const TRAPPED_EVENTS = ["keydown", "keypress", "keyup", "beforeinput", "input", "textInput", "compositionstart", "compositionupdate", "compositionend", "paste", "cut", "copy"] as const;

export class KeyTrap {
  private pickId: string | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private readonly trustedOnly: boolean;
  private readonly ms: number;
  private readonly setTimer: typeof setTimeout;
  private readonly clearTimer: typeof clearTimeout;

  /** `opts.trustedOnly` (default true) ignores events the page dispatched;
   * tests that synthesise input turn it off. */
  constructor(private readonly win: Window, opts: { trustedOnly?: boolean; ms?: number } = {}) {
    this.trustedOnly = opts.trustedOnly ?? true;
    this.ms = opts.ms ?? TRAP_MS;
    this.setTimer = win.setTimeout.bind(win) as typeof setTimeout;
    this.clearTimer = win.clearTimeout.bind(win) as typeof clearTimeout;
    for (const t of TRAPPED_EVENTS) win.addEventListener(t, this.onEvent, true);
    win.addEventListener("blur", () => this.stop());
  }

  /** Keeps the keys typed from now on for the pick `pickId` from the page
   * (ending any earlier trap). A page without focus gets no keys: nothing is
   * trapped. */
  start(pickId: string): void {
    this.stop();
    if (!this.win.document.hasFocus()) return;
    this.pickId = pickId;
    this.timer = this.setTimer(() => this.stop(), this.ms);
  }

  /** Ends the trap for `pickId` (the shell refused that pick). */
  end(pickId: string): void {
    if (this.pickId === pickId) this.stop();
  }

  /** Whether keys are being kept from the page. */
  get active(): boolean {
    return this.pickId !== null;
  }

  private stop(): void {
    this.pickId = null;
    this.clearTimer(this.timer);
  }

  private onEvent = (e: Event) => {
    if (this.pickId === null || (this.trustedOnly && !e.isTrusted)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
  };
}
