import type { Thread } from "../threads";

export type ThreadChange = (ts: Thread[]) => Thread[];

/** Keeps a thread list loaded from the daemon consistent with changes that
 * happen while the load is in flight (events, this shell's own writes): its
 * answer may predate them, so they are replayed on top of it. Only the latest
 * load's answer is applied; once it answers or fails, nothing more is kept. */
export class ThreadSync {
  private n = 0;
  private since: ThreadChange[] | null = null;

  constructor(private readonly apply: (f: ThreadChange) => void) {}

  change(f: ThreadChange): void {
    this.since?.push(f);
    this.apply(f);
  }

  /** Starts a load; call the result with its answer, or undefined when it failed. */
  begin(): (answer: Thread[] | undefined) => void {
    const n = ++this.n;
    const since: ThreadChange[] = [];
    this.since = since;
    return answer => {
      if (n !== this.n) return;
      this.since = null;
      if (answer) this.apply(() => since.reduce((acc, f) => f(acc), answer));
    };
  }
}
