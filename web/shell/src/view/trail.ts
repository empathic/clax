// Whether the viewer's keyboard trail in the shell can be trusted to be
// theirs (spec §8, "Keys"). The page decides when the viewer's Tab leaves its
// frame (it can run out of fields, or push focus out with `parent.focus()`),
// so focus that entered the shell from the frame or from <body>, with no press
// of the viewer's in the shell, may carry keys the viewer typed for the page.
// While the trail is tainted, a card action that has consequences (Send,
// Resolve, and later ones such as batch send) ignores keyboard activation and
// says how to act instead. A trusted press in the shell, or a trusted Escape
// on a shell control (a page cannot send the shell a key), clears it.

let tainted = false;

/** The shell's keyboard trail; the controller taints and clears it (`listen`). */
export const keyboardTrail = {
  get tainted(): boolean { return tainted; },
  taint(): void { tainted = true; },
  clear(): void { tainted = false; },
};

/** Whether `e` activates a control from the keyboard: a key event, or the
 * click a key (Enter, Space) makes on a button, whose `detail` is 0. */
export function fromKeyboard(e: Event): boolean {
  return e.type.startsWith("key") || (e.type === "click" && (e as MouseEvent).detail === 0);
}

/** The hint for a consequential action refused from the keyboard: `verb` is
 * the action's own ("send", "resolve", "delete"). */
export const trailHint = (verb: string): string => `Click to ${verb}, or press Esc first`;

/** Runs `act` for activation `e` of a consequential action, unless the trail
 * is tainted and `e` came from the keyboard: then nothing runs and the result
 * is the hint to show (politely announced, beside the action). Null when
 * `act` ran. */
export function guardedAction(e: Event, verb: string, act: () => void): string | null {
  if (keyboardTrail.tainted && fromKeyboard(e)) return trailHint(verb);
  act();
  return null;
}
