/** An immutable-snapshot store. `subscribe` follows the Svelte store
 * contract, so a Svelte component reads it with `fromStore`. */
export class Store<S extends object> {
  private value: S;
  private readonly subs = new Set<(s: S) => void>();

  constructor(initial: S) {
    this.value = initial;
  }

  get(): S {
    return this.value;
  }

  /** Merges `patch` into a new snapshot and tells every subscriber; a patch
   * whose every field is already equal (`Object.is`) changes nothing. */
  set(patch: Partial<S> | ((s: S) => Partial<S>)): void {
    const p = typeof patch === "function" ? patch(this.value) : patch;
    const keys = Object.keys(p) as (keyof S)[];
    if (keys.every(k => Object.is(p[k], this.value[k]))) return;
    this.value = { ...this.value, ...p };
    for (const fn of [...this.subs]) fn(this.value);
  }

  /** Calls `fn` with the snapshot at once and after every change, until the
   * returned function is called. */
  subscribe(fn: (s: S) => void): () => void {
    this.subs.add(fn);
    fn(this.value);
    return () => { this.subs.delete(fn); };
  }
}
