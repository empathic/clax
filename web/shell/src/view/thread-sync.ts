import { type Thread, upsert } from "../threads";

/** A change to the thread list. It must be idempotent and keyed by thread
 * ID (an upsert, a removal, a field set on one thread): changes are replayed
 * on top of a load's answer, and onto a list that may already hold them. */
export type ThreadChange = (ts: Thread[]) => Thread[];

type Logged = { at: number; f: ThreadChange };

/** `ts` with each thread ID once, the last copy of a repeated ID kept (a
 * paged list can repeat a thread that moved between pages). */
export const dedupe = (ts: Thread[]): Thread[] => [...new Map(ts.map(t => [t.id, t])).values()];

/** Keeps a thread list loaded from the daemon consistent with changes that
 * happen while a load is in flight (stream deltas, this shell's own
 * writes): a load's answer may predate them, so they are replayed on top of
 * it, in the order they happened.
 *
 * Every load and every change takes a ticket from one clock. A whole-list
 * load (`begin`) applies only when no newer list load began; a one-thread
 * load (`beginThread`) applies only when no newer load of that thread began
 * and no list load that began after it has applied (that list is newer). An
 * answer applies as of its ticket: the changes made since are replayed on
 * it, so an answer never undoes a newer change, and a thread is held once
 * whatever the interleaving. Once no load is in flight, nothing is kept. */
export class ThreadSync {
  private clock = 0;
  /** Changes (and applied answers) since the oldest load in flight began, by ticket. */
  private log: Logged[] = [];
  private inflight = new Set<number>();
  /** The newest load begun and not yet answered, by thread ID ("" for the list). */
  private newest = new Map<string, number>();
  /** The ticket of the newest list answer applied. */
  private listApplied = 0;

  constructor(private readonly apply: (f: ThreadChange) => void) {}

  change(f: ThreadChange): void {
    const at = ++this.clock;
    if (this.inflight.size) this.log.push({ at, f });
    this.apply(f);
  }

  /** Starts a load of every thread; call the result with its answer, or
   * undefined when it failed. */
  begin(): (answer: Thread[] | undefined) => void {
    return this.load("", (a: Thread[]) => () => dedupe(a));
  }

  /** Starts a load of thread `tid` alone; call the result with its answer,
   * or undefined when it failed. */
  beginThread(tid: string): (answer: Thread | undefined) => void {
    return this.load(tid, (t: Thread) => ts => upsert(ts, t));
  }

  private load<T>(key: string, change: (answer: T) => ThreadChange): (answer: T | undefined) => void {
    const n = ++this.clock;
    this.inflight.add(n);
    this.newest.set(key, n);
    return answer => {
      if (!this.inflight.delete(n)) return;
      const newest = this.newest.get(key) === n;
      if (newest) this.newest.delete(key);
      if (newest && answer && n > this.listApplied) {
        if (!key) this.listApplied = n;
        const f = change(answer);
        // A load still in flight that began before this one replays it in its place.
        const i = this.log.findIndex(e => e.at > n);
        this.log.splice(i < 0 ? this.log.length : i, 0, { at: n, f });
        const since = this.log.filter(e => e.at > n).map(e => e.f);
        this.apply(ts => since.reduce((acc, g) => g(acc), f(ts)));
      }
      // Drops the changes no load in flight needs.
      const oldest = Math.min(...this.inflight);
      this.log = this.log.filter(e => e.at > oldest);
    };
  }
}
