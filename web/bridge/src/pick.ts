// A pick's way from the viewer's click or release to its screenshot. The
// bridge posts the pick's start with its anchor, and the shell opens the
// composer on it and focuses it. The clip is rendered only once the shell
// says the composer has focus (`clax:composer-ready`): its work can hold the
// main thread the page may share with the shell, and keys typed meanwhile
// then wait for it and reach the focused composer. A start the shell refused
// (`clax:pick-refused`), or one with no answer within `READY_MS`, renders
// nothing; the bridge then posts the pick with `NOT_TAKEN` in place of a
// clip, so a composer still waiting for it stops waiting.
//
// The timer functions are the ones in place when the flow is made (the bridge
// makes it as it loads, before any page script), so a page replacing them
// later cannot hold a pick open or end it.

import type { Anchor, BridgeToShell } from "./protocol";

/** How long a pick's start waits for the shell's answer. */
export const READY_MS = 5_000;
/** The clip error a pick carries when no clip was rendered for it. */
export const NOT_TAKEN = "it was not taken";

type Post = (m: BridgeToShell, transfer?: Transferable[]) => void;

export class PickFlow {
  private waiting: { pickId: string; answer(go: boolean): void; timer: number } | null = null;
  private readonly setTimer: (f: () => void, ms: number) => number;
  private readonly clearTimer: (t: number) => void;
  private readonly ms: number;
  private readonly newId: () => string;

  constructor(win: Window, private readonly post: Post, private readonly version: number, opts: { ms?: number; newId?: () => string } = {}) {
    const set = win.setTimeout.bind(win);
    const clear = win.clearTimeout.bind(win);
    this.setTimer = (f, ms) => set(f, ms) as unknown as number;
    this.clearTimer = t => clear(t);
    this.ms = opts.ms ?? READY_MS;
    this.newId = opts.newId ?? (() => `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`);
  }

  /** The shell's answer for `pickId`: `go` when its composer has focus,
   * false when the start was refused. Any other ID is ignored. */
  answer(pickId: string, go: boolean): void {
    const w = this.waiting;
    if (!w || w.pickId !== pickId) return;
    this.waiting = null;
    this.clearTimer(w.timer);
    w.answer(go);
  }

  /** Posts the start of a pick of `anchor`, and once the shell's composer
   * for it has focus, renders `clip` and posts the pick with it. A pick
   * still waiting for its answer is dropped first. */
  async start(anchor: Anchor, clip: () => Promise<ArrayBuffer>): Promise<void> {
    const pickId = this.newId();
    if (this.waiting) this.answer(this.waiting.pickId, false);
    const go = new Promise<boolean>(resolve => {
      this.waiting = { pickId, answer: resolve, timer: this.setTimer(() => this.answer(pickId, false), this.ms) };
    });
    this.post({ type: "clax:pick-start", pickId, version: this.version, anchor });
    if (!(await go)) {
      this.post({ type: "clax:pick", pickId, version: this.version, anchor, clipError: NOT_TAKEN });
      return;
    }
    let clipPng: ArrayBuffer | undefined;
    let clipError: string | undefined;
    try { clipPng = await clip(); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
    this.post({ type: "clax:pick", pickId, version: this.version, anchor, clipPng, clipError }, clipPng ? [clipPng] : []);
  }
}
