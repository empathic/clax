import { describe, expect, it } from "vitest";
import { type Clock, PART_TIMEOUT_MS, RETRY_MS, retrying } from "../src/part-loader";

/** A clock the test moves by hand. */
function manualClock() {
  let t = 0;
  const timers = new Map<number, { at: number; f: () => void }>();
  let seq = 0;
  const clock: Clock = {
    now: () => t,
    setTimeout: (f, ms) => { timers.set(++seq, { at: t + ms, f }); return seq; },
    clearTimeout: id => { timers.delete(id as number); },
  };
  const advance = (ms: number) => {
    t += ms;
    for (const [id, x] of [...timers]) if (x.at <= t) { timers.delete(id); x.f(); }
  };
  return { clock, advance };
}
const tick = () => new Promise(r => setTimeout(r, 0));

describe("a part's loader", () => {
  it("loads once on success, and every later need shares that load", async () => {
    const { clock } = manualClock();
    const attempts: number[] = [];
    const next = retrying("comment", async a => { attempts.push(a); return "part"; }, () => {}, clock);
    await expect(next()).resolves.toBe("part");
    await expect(next()).resolves.toBe("part");
    expect(attempts).toEqual([0]);
  });

  it("reports each failed attempt, fails at once within the backoff, and tries again after it", async () => {
    const { clock, advance } = manualClock();
    const attempts: number[] = [];
    const fails: unknown[] = [];
    let ok = false;
    const next = retrying("clip", async a => { attempts.push(a); if (!ok) throw new Error(`blocked ${a}`); return "part"; }, e => fails.push(e), clock);
    await expect(next()).rejects.toThrow("blocked 0");
    await tick();
    expect(fails).toHaveLength(1);
    // Inside the backoff: the same error, no new attempt, nothing reported.
    advance(RETRY_MS - 1);
    await expect(next()).rejects.toThrow("blocked 0");
    await tick();
    expect(attempts).toEqual([0]);
    expect(fails).toHaveLength(1);
    // After it: a new attempt, numbered, whose failure doubles the backoff.
    advance(1);
    await expect(next()).rejects.toThrow("blocked 1");
    await tick();
    expect(fails).toHaveLength(2);
    advance(RETRY_MS);
    await expect(next()).rejects.toThrow("blocked 1");
    ok = true;
    advance(RETRY_MS);
    await expect(next()).resolves.toBe("part");
    expect(attempts).toEqual([0, 1, 2]);
    expect(fails).toHaveLength(2);
  });

  it("fails an attempt that does not finish in time, and reports it", async () => {
    const { clock, advance } = manualClock();
    const fails: unknown[] = [];
    const next = retrying("caps", () => new Promise<string>(() => {}), e => fails.push(e), clock);
    const p = next();
    advance(PART_TIMEOUT_MS);
    await expect(p).rejects.toThrow(`the caps part did not load within ${PART_TIMEOUT_MS / 1000} s`);
    await tick();
    expect(fails).toHaveLength(1);
  });
});
