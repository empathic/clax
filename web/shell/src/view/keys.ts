// The shell's keyboard layer (spec §8, "Keys"). A key acts only when focus is
// in the shell, outside a text field and outside any dialog, with no modifier
// but Shift, and not while an input method composes. Letters act whatever
// their case (Caps Lock), except that Shift+S is its own key. Keys pressed inside the artifact's frame
// belong to the page and never reach here.
export type KeyAction = "help" | "comment" | "threads" | "next" | "prev" | "reply" | "send" | "resolve" | "versions" | "tick" | "sendTicked" | "people";
export type KeyLike = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "isComposing" | "repeat" | "target">;
export type KeyRow = { keys: string[]; what: string; action: KeyAction | "escape" };

const MAP: Record<string, KeyAction> = {
  "?": "help", c: "comment", t: "threads", j: "next", k: "prev", Enter: "reply", s: "send", r: "resolve",
};

/** The sheet's rows, in order. Tasks that add a key add its row and its MAP entry. */
export const KEY_ROWS: KeyRow[] = [
  { keys: ["C"], what: "Comment mode: click an element or drag an area", action: "comment" },
  { keys: ["Esc"], what: "Leave comment mode, close a menu", action: "escape" },
  { keys: ["T"], what: "Show or hide threads", action: "threads" },
  { keys: ["J", "K"], what: "Next and previous thread; the page scrolls to its pin", action: "next" },
  { keys: ["↵"], what: "Reply to the selected thread", action: "reply" },
  { keys: ["S"], what: "Send the selected thread to an agent", action: "send" },
  { keys: ["R"], what: "Resolve the selected thread", action: "resolve" },
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
  if (e.key === "Enter" && e.target instanceof Element && e.target.closest("button, a, summary, [role=button]")) return null;
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (key === "s" && e.shiftKey) return "sendTicked";
  return MAP[key] ?? null;
}
