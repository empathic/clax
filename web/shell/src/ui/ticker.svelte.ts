/** A clock for elapsed-time labels: it ticks each second while `active()`
 * holds, else every `idleMs` when given (labels such as "5 min ago"), unless
 * `fixed()` pins it (tests). Call during component setup. */
export function ticker(active: () => boolean, fixed: () => Date | undefined, idleMs = 0): { readonly now: Date } {
  let tick = $state(new Date());
  $effect(() => {
    const ms = active() ? 1000 : idleMs;
    if (fixed() !== undefined || !ms) return;
    tick = new Date();
    const timer = setInterval(() => { tick = new Date(); }, ms);
    return () => clearInterval(timer);
  });
  return { get now() { return fixed() ?? tick; } };
}
