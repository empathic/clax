// What the side panel's site tools (SiteTools, under Addresses) and the
// Sites tab share: how an address is named, and a join run to its end in
// batches as a merge is, repeated while the daemon says threads remain.

export type Step = { moved: number; remaining: number };
/** The most batches one join sends (200 threads each). */
export const MAX_BATCHES = 1000;
/** `origin` without its scheme, as the panel names an address. */
export const host = (o: string) => o.replace(/^\w+:\/\//, "");
export const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** Runs a join to its end: `step` sends one batch, `left` hears
 * how many threads are left after each one that leaves some. `allowed` is
 * Chrome's answer for `all`, the site's origins, asked under the click
 * before any await; without it only a join already made (`continuing`)
 * goes on. Answers the threads merged and whether Chrome refused, or null
 * once `alive` says the panel went; throws what to tell the person when
 * Chrome refused a new join, or a batch merged nothing, left no fewer, or
 * was past MAX_BATCHES. */
export async function runJoin(o: { all: string[]; allowed: Promise<boolean>; continuing: boolean;
  step(): Promise<Step>; left(remaining: number): void; alive(): boolean }): Promise<{ moved: number; denied: boolean } | null> {
  const denied = !(await o.allowed);
  if (denied && !o.continuing) throw new Error(`Chrome was not allowed access to ${o.all.map(host).join(", ")}, so Clax cannot follow the site there.`);
  let moved = 0;
  let left = Infinity;
  for (let i = 0; ; i++) {
    if (!o.alive()) return null;
    const r = await o.step();
    moved += r.moved;
    if (!r.remaining) return { moved, denied };
    o.left(r.remaining);
    if (!r.moved || r.remaining >= left || i + 1 >= MAX_BATCHES) throw new Error(`Stopped with ${plural(r.remaining, "thread")} left to merge. Join again later to finish.`);
    left = r.remaining;
  }
}
