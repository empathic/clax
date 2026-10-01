// How the eager bridge loads one lazy part: on need, with a time limit, and,
// after a failure, again on a later need once a backoff has passed (a
// network blip, or a daemon restarted under an open tab), never in a loop.
// Every failed attempt is reported; a need inside the backoff fails at once,
// with the last error, and reports nothing.

/** How long one attempt may take before it counts as failed. */
export const PART_TIMEOUT_MS = 15_000;
/** The wait after the first failure before a need tries again; it doubles
 * with each further failure, up to `RETRY_MAX_MS`. */
export const RETRY_MS = 2_000;
export const RETRY_MAX_MS = 60_000;

export type Clock = { now(): number; setTimeout(f: () => void, ms: number): unknown; clearTimeout(t: unknown): void };

/** A loader for one part: `load(attempt)` starts attempt number `attempt`
 * (0 first); `onFail` hears each failed attempt. */
export function retrying<T>(name: string, load: (attempt: number) => Promise<T>, onFail: (e: unknown) => void, clock: Clock, timeoutMs = PART_TIMEOUT_MS): () => Promise<T> {
  let current: Promise<T> | null = null;
  let attempts = 0;
  let failedAt = 0;
  let lastError: unknown = null;
  let failed = false;
  return () => {
    if (current && !failed) return current;
    if (failed && clock.now() - failedAt < Math.min(RETRY_MS * 2 ** (attempts - 1), RETRY_MAX_MS)) return Promise.reject(lastError);
    failed = false;
    const attempt = attempts;
    const p = new Promise<T>((resolve, reject) => {
      const timer = clock.setTimeout(() => reject(new Error(`the ${name} part did not load within ${timeoutMs / 1000} s`)), timeoutMs);
      load(attempt).then(
        v => { clock.clearTimeout(timer); resolve(v); },
        e => { clock.clearTimeout(timer); reject(e); },
      );
    });
    current = p;
    p.catch(e => {
      if (current !== p) return;
      failed = true;
      attempts = attempt + 1;
      failedAt = clock.now();
      lastError = e;
      onFail(e);
    });
    return p;
  };
}
