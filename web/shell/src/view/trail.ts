// Whether the viewer's keyboard trail in the shell can be trusted to be
// theirs (spec §8, "Keys"). The page decides when the viewer's Tab leaves its
// frame (it can run out of fields, or push focus out with `parent.focus()`),
// and it opens the composer and the consent prompt, so focus that entered the
// shell from the frame or from <body>, with no press of the viewer's on a
// shell control, may carry keys the viewer typed for the page. While the
// trail is tainted, a consequential action (Send, Resolve, Reply, batch Send,
// Send N unsent, the composer's Post, the prompt's Allow) runs only on a
// trusted pointer's click, and says so to anything else. No key clears it:
// only a trusted press that puts focus on the shell control it targets
// (`ArtifactController.listen`), never one on the gesture shield or the
// prompt's backdrop. Every load starts tainted.

let tainted = false;
const listeners = new Set<() => void>();

/** The shell's keyboard trail; the controller taints and clears it (`listen`). */
export const keyboardTrail = {
  get tainted(): boolean { return tainted; },
  taint(): void { tainted = true; },
  clear(): void {
    if (!tainted) return;
    tainted = false;
    for (const f of [...listeners]) f();
  },
  /** Hears each time the trail goes from tainted to clear; the result stops it. */
  onClear(f: () => void): () => void { listeners.add(f); return () => { listeners.delete(f); }; },
};

/** Whether `e` is a trusted pointer's click: a click whose `detail` (the
 * click count) is 1 or more. Enter or Space on a button, a script's
 * `click()`, and an assistive technology's activation all click with 0. */
export function fromPointer(e: Event): boolean {
  return e.isTrusted && e.type === "click" && (e as MouseEvent).detail > 0;
}

/** The hint for a consequential action refused to anything but a click:
 * `verb` is the action's own ("send", "resolve", "reply", "post", "allow"). */
export const trailHint = (verb: string): string => `Click to ${verb}`;

/** Runs `act` for activation `e` of a consequential action, unless the trail
 * is tainted (or `steered` says so: a composer or prompt the page
 * opened) and `e` is not a trusted pointer's click: then nothing runs, and
 * the result is the hint to show in the action's live region. Null when
 * `act` ran. */
export function guardedAction(e: Event, verb: string, act: () => void, steered = keyboardTrail.tainted): string | null {
  if (steered && !fromPointer(e)) return trailHint(verb);
  act();
  return null;
}
