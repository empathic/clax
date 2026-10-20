// One owner for everything a view starts: listeners, timers, requests and
// streams are registered with the view's Lifecycle, and its `dispose` ends
// them all at once, so nothing outlives the view. Requests take its `signal`.

/** The lifecycle of one view (or one part of it, through `child`). */
export class Lifecycle {
  private readonly ac = new AbortController();
  private undo: (() => void)[] = [];

  /** With `parent`, this one is disposed when the parent is (or at once when
   * it already was), and may be disposed earlier by itself. */
  constructor(parent?: Lifecycle) {
    if (parent) this.defer(parent.defer(() => this.dispose()));
  }

  /** Aborts when the lifecycle ends; pass it to requests. */
  get signal(): AbortSignal { return this.ac.signal; }
  get disposed(): boolean { return this.ac.signal.aborted; }

  /** Runs `fn` when the lifecycle ends (at once when it already has).
   * Returns a function that runs nothing and forgets `fn`. */
  defer(fn: () => void): () => void {
    if (this.disposed) { fn(); return () => {}; }
    this.undo.push(fn);
    return () => { this.undo = this.undo.filter(f => f !== fn); };
  }

  /** Adds a listener that is removed when the lifecycle ends. */
  listen<E extends Event = Event>(target: EventTarget, type: string, fn: (e: E) => void, opts?: AddEventListenerOptions): void {
    if (this.disposed) return;
    const l = fn as EventListener;
    target.addEventListener(type, l, opts);
    this.defer(() => target.removeEventListener(type, l, opts));
  }

  /** A timeout cleared when the lifecycle ends; returns its own clear. */
  timeout(fn: () => void, ms: number): () => void {
    if (this.disposed) return () => {};
    let forget = () => {};
    const id = setTimeout(() => { forget(); fn(); }, ms);
    forget = this.defer(() => clearTimeout(id));
    return () => { clearTimeout(id); forget(); };
  }

  /** An interval cleared when the lifecycle ends; returns its own clear. */
  interval(fn: () => void, ms: number): () => void {
    if (this.disposed) return () => {};
    const id = setInterval(fn, ms);
    const forget = this.defer(() => clearInterval(id));
    return () => { clearInterval(id); forget(); };
  }

  /** A lifecycle that ends with this one, or earlier. */
  child(): Lifecycle { return new Lifecycle(this); }

  /** Ends the lifecycle: the signal aborts, then what was deferred runs,
   * latest first. A throwing teardown does not stop the others; the first
   * error is rethrown once all have run. Idempotent. */
  dispose(): void {
    if (this.disposed) return;
    this.ac.abort();
    const undo = this.undo;
    this.undo = [];
    const errors: unknown[] = [];
    for (let i = undo.length - 1; i >= 0; i--) {
      try { undo[i](); } catch (e) { errors.push(e); }
    }
    if (errors.length) throw errors[0];
  }
}

/** Calls `hide` as the page is hidden (`pagehide`: a navigation away, a
 * reload, a close, or entering the back/forward cache, `persisted`), and
 * `show` when the back/forward cache restores it; until `life` ends. */
export function onPageCache(life: Lifecycle, hide: (persisted: boolean) => void, show: () => void, win: Window = window): void {
  life.listen<PageTransitionEvent>(win, "pagehide", e => hide(!!e.persisted));
  life.listen<PageTransitionEvent>(win, "pageshow", e => { if (e.persisted) show(); });
}

/** The wait before retry number `attempt` (0 first): doubling from `base`
 * up to `cap`, each spread by ±20% so pages do not retry in step. */
export function backoff(attempt: number, random: () => number = Math.random, base = 500, cap = 30_000): number {
  const ms = Math.min(cap, base * 2 ** Math.min(attempt, 30));
  return Math.round(ms * (0.8 + 0.4 * random()));
}

/** A request is stuck when it has not answered in this long; it is then
 * abandoned and tried again. */
export const STUCK_MS = 8000;

/** Runs `attempt` until it succeeds, retrying with `backoff` after a failure
 * `retry` accepts (by default any), or after it was stuck `timeoutMs`
 * (aborted through its signal). `trouble(true)` is called while it is
 * failing or stuck, `trouble(false)` once it is settled. Rejects with the
 * failure `retry` refused, or with an `AbortError` once `life` ends. */
export async function retrying<T>(life: Lifecycle, attempt: (signal: AbortSignal) => Promise<T>, opts: { timeoutMs?: number; retry?: (e: unknown) => boolean; trouble?: (on: boolean) => void } = {}): Promise<T> {
  const { timeoutMs = STUCK_MS, retry = () => true, trouble = () => {} } = opts;
  for (let n = 0; ; n++) {
    const one = life.child();
    let stuck = false;
    one.timeout(() => { stuck = true; trouble(true); one.dispose(); }, timeoutMs);
    try {
      const v = await attempt(one.signal);
      one.dispose();
      trouble(false);
      return v;
    } catch (e) {
      one.dispose();
      if (life.disposed) { trouble(false); throw new DOMException("the view ended", "AbortError"); }
      if (!stuck && !retry(e)) { trouble(false); throw e; }
      trouble(true);
    }
    let forget = () => {};
    await new Promise<void>(resolve => { life.timeout(resolve, backoff(n)); forget = life.defer(resolve); });
    forget();
    if (life.disposed) { trouble(false); throw new DOMException("the view ended", "AbortError"); }
  }
}
