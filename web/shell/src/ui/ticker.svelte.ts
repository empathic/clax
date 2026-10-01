/** A clock for elapsed-time labels: it ticks each second while `active()`
 * holds, unless `fixed()` pins it (tests). Call during component setup. */
export function ticker(active: () => boolean, fixed: () => Date | undefined): { readonly now: Date } {
  let tick = $state(new Date());
  $effect(() => {
    if (fixed() !== undefined || !active()) return;
    tick = new Date();
    const timer = setInterval(() => { tick = new Date(); }, 1000);
    return () => clearInterval(timer);
  });
  return { get now() { return fixed() ?? tick; } };
}
