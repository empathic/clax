// Per-tab rate budgets for capability calls that act on the viewer's behalf
// (a publish, a download prompt). Kept in sessionStorage so a budget survives
// the page's own reload; where storage is unavailable, in memory for this load.

export type Limits = { gapMs?: number; perWindow: { n: number; ms: number } };

const memory = new Map<string, number[]>();

function load(key: string): number[] {
  try {
    const raw = sessionStorage.getItem(key);
    if (raw !== null) {
      const v: unknown = JSON.parse(raw);
      return Array.isArray(v) ? v.filter((x): x is number => typeof x === "number" && Number.isFinite(x)) : [];
    }
  } catch {
    // Unavailable or corrupt: fall back to this load's memory.
  }
  return memory.get(key) ?? [];
}

function store(key: string, times: number[]): void {
  memory.set(key, times);
  try {
    sessionStorage.setItem(key, JSON.stringify(times));
  } catch {
    // Storage unavailable: the in-memory copy holds for this page load.
  }
}

/** Takes one slot of `key`'s budget at `now`: 0 when taken, else the
 * milliseconds to wait before the next slot (nothing is recorded then). */
export function takeSlot(key: string, limits: Limits, now: number = Date.now()): number {
  const { n, ms } = limits.perWindow;
  const times = load(key).filter(t => t <= now && now - t < ms);
  let wait = 0;
  const last = times.at(-1);
  if (limits.gapMs !== undefined && last !== undefined && now - last < limits.gapMs) wait = limits.gapMs - (now - last);
  if (times.length >= n) wait = Math.max(wait, times[times.length - n] + ms - now);
  if (wait > 0) return wait;
  times.push(now);
  store(key, times);
  return 0;
}

/** Drops the in-memory budgets (tests). */
export function forgetBudgets(): void {
  memory.clear();
}

/** `ms` as whole seconds for a message, at least 1. */
export const seconds = (ms: number) => Math.max(1, Math.ceil(ms / 1000));
