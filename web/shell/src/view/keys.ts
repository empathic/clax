// The shell's keyboard layer (spec §8, "Keys"): C toggles comment mode and ?
// opens the sheet; Escape is handled where it is heard. A key acts only when
// focus is in the shell, outside a text field and outside any dialog, with no
// modifier but Shift, and not while an input method composes. C acts whatever
// its case (Caps Lock). Keys pressed inside the artifact's frame belong to the
// page and never reach here.
export type KeyAction = "help" | "comment";
export type KeyLike = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "isComposing" | "repeat" | "target">;
export type KeyRow = { keys: string[]; what: string; action: KeyAction | "escape" };

const MAP: Record<string, KeyAction> = { "?": "help", c: "comment" };

/** The sheet's rows, in order. */
export const KEY_ROWS: KeyRow[] = [
  { keys: ["C"], what: "Comment mode: click an element or drag an area", action: "comment" },
  { keys: ["?"], what: "This sheet", action: "help" },
  { keys: ["Esc"], what: "Leave comment mode, close a menu or this sheet", action: "escape" },
];

function typing(t: EventTarget | null): boolean {
  if (!(t instanceof Element)) return false;
  const el = t as HTMLElement;
  return el.localName === "input" || el.localName === "textarea" || el.localName === "select" || el.isContentEditable || el.contentEditable === "true";
}

export function keyAction(e: KeyLike): KeyAction | null {
  if (e.metaKey || e.ctrlKey || e.altKey || e.isComposing || e.repeat || typing(e.target)) return null;
  // A dialog's keys are its own: a page's consent prompt takes focus while
  // the viewer may still be typing for the page.
  if (e.target instanceof Element && e.target.closest("[role=dialog], [aria-modal=true]")) return null;
  return MAP[e.key.length === 1 ? e.key.toLowerCase() : e.key] ?? null;
}

const HELD_KEY = "clax.keys-held";

/** Marks the shell load about to happen as the page's doing (its publish
 * reloading the view), so the next view starts with the keys held: the viewer
 * may still be typing for the page. Called just before that navigation. */
export function holdKeysAcrossLoad(): void {
  try { sessionStorage.setItem(HELD_KEY, "1"); } catch { /* read back as held: storage that throws here throws there */ }
}

/** Whether this view starts with the keys held: the load was the page's
 * doing (`holdKeysAcrossLoad`), or storage cannot say. The mark is used up. */
export function keysHeldAtLoad(): boolean {
  try {
    const held = sessionStorage.getItem(HELD_KEY) !== null;
    if (held) sessionStorage.removeItem(HELD_KEY);
    return held;
  } catch {
    return true;
  }
}
