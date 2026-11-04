import { WAIT_MS } from "./timeouts";

/** Polls `check` until it answers something truthy, and answers that; fails
 * naming `what` only once the hang guard (WAIT_MS) has passed, so the
 * condition decides and the deadline only ends a stuck test. */
export async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    // oxlint-disable-next-line clax-timing/timer-delay -- the polling helper's pause between checks: the condition decides, not the delay
    await new Promise(r => setTimeout(r, 10));
  }
}
