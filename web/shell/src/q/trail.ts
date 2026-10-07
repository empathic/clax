// The keyboard trail the question card follows (`view/trail.ts`, spec §8
// "Keys"). The artifact view hands over its own (`useTrail`), since its
// frame may steer focus into the shell. Elsewhere (the gallery, `/inbox`) no
// frame taints the trail, so every action runs, as `guardedAction` does on a
// clean trail. The question module does not import `view/trail.ts` itself:
// that module belongs to the artifact entry, and importing it here would
// split it out of the entry's chunk.
import type { guardedAction, keyboardTrail } from "../view/trail";

export type Trail = { keyboardTrail: Pick<typeof keyboardTrail, "onClear">; guardedAction: typeof guardedAction };

const clean: Trail = { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _verb, act) => { act(); return null; } };
let current = clean;

/** The trail the cards follow now. */
export const trail = (): Trail => current;

/** Makes the cards follow `t` (the artifact view's trail). */
export function useTrail(t: Trail): void { current = t; }
