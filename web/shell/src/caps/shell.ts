// The `shell` capability: a page points at Clax's own controls
// (clax-extensions.d.ts `Shell`), in the owner's and editors' views
// (availability.ts).
//
// - `highlight(control)` rings the first shown element marked for that
//   control (ui/shell-controls.ts) with a coach mark (ui/coach-mark.ts) and
//   resolves `true`; `false`, drawing nothing and clearing any earlier ring, when none is shown or the ring cannot be drawn. One ring
//   at a time; the ring goes with the frame's document (`leave`, `reset`,
//   `dispose`). A `send` control is scrolled into its sidebar first, only when it
//   is not wholly in view there.
// - `can(control)` answers the same without drawing.
// - `commentMode(on)` enters or leaves comment mode through the view
//   (`CapEnv.commentMode`), only on the viewer's gesture in the page (the
//   composer tier, judged when the call arrives: registry.ts); `unavailable`
//   where the view cannot comment.
// This module is a lazy chunk (registry.ts), loaded on the page's first call.
import { showCoachMark } from "../ui/coach-mark";
import { SHELL_CONTROLS, type ShellControl, findControl, isShellControl, revealIfOutside } from "../ui/shell-controls";
import { CapError } from "./errors";
import { frameGesture } from "./gesture";
import type { HandlerFactory } from "./host";

/** Where the handler finds a shown control, and how it rings one (returning its dismiss, or `null` when nothing was drawn). */
export type ShellDeps = { find(c: ShellControl): HTMLElement | null; mark(el: HTMLElement): (() => void) | null };

function control(v: unknown): ShellControl {
  if (!isShellControl(v)) throw new CapError("invalid_argument", `control is one of ${SHELL_CONTROLS.map(c => `"${c}"`).join(", ")}`);
  return v;
}

export function makeShellHandler(deps: ShellDeps): HandlerFactory {
  return env => {
    let dismiss: (() => void) | null = null;
    const clear = () => { dismiss?.(); dismiss = null; };
    return {
      async call(method, args, gesture) {
        switch (method) {
          case "highlight": {
            const el = deps.find(control(args[0]));
            clear();
            if (!el) return false;
            dismiss = deps.mark(el);
            return dismiss !== null;
          }
          case "can":
            return deps.find(control(args[0])) !== null;
          case "commentMode": {
            const on = args[0];
            if (typeof on !== "boolean") throw new CapError("invalid_argument", "commentMode takes true or false");
            if (!(gesture?.composer ?? frameGesture())) throw new CapError("no_gesture", "shell.commentMode needs the person's click or key press in the page; while comment mode is on, a click in the page picks what to comment on and never reaches the page, so leave it from a key press; the viewer's Escape or Comment button also leaves it");
            const now = env.commentMode?.(on) ?? null;
            if (now === null) throw new CapError("unavailable", "comments cannot be made in this view");
            return now;
          }
          default:
            throw new CapError("capability_removed", `shell.${String(method)} is not part of this runtime`);
        }
      },
      leave: clear,
      reset: clear,
      dispose: clear,
    };
  };
}

export const shellHandler: HandlerFactory = makeShellHandler({
  find: c => findControl(c, document),
  mark: el => {
    if (el.getAttribute("data-shell-control") === "send") revealIfOutside(el);
    return showCoachMark(el);
  },
});
