// The on-demand parts (`where`), once loaded: a surface that needs one
// calls `need()` and shows the part once `later.parts` holds it.
type Parts = typeof import("./where");

export const later: { parts: Parts | null } = $state({ parts: null });
let loading: Promise<Parts | null> | null = null;

/** Loads the on-demand parts (once); resolves to them, or to null when
 * they did not load, and the next call tries again. */
export function need(): Promise<Parts | null> {
  return (loading ??= import("./where").then(m => (later.parts = m), () => { loading = null; return null; }));
}
