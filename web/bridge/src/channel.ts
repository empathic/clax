// Which window and origins the bridge takes orders from (spec §9: the bridge
// never trusts messages that do not come from its shell's window and origin).

import { SHELL_TYPES, type ShellToBridge } from "./protocol";

/** The origins the shell can have, from the origins the bridge tag names
 * (`data-origin`, the daemon's public origin; a trusted host's frame names
 * that host on `https` and `http`, separated by a space): each origin, and
 * for a local daemon's (`localhost` or `127.0.0.1`) each of those names
 * at its port, as the shell may be reached by any. None without one, or
 * for one that does not parse: the bridge then takes no orders from it. */
export function shellOrigins(origin: string | undefined): string[] {
  return (origin ?? "").split(" ").filter(Boolean).flatMap(o => {
    try {
      const u = new URL(o), loopback = ["localhost", "127.0.0.1"];
      return loopback.includes(u.hostname) ? loopback.map(h => `${u.protocol}//${h}${u.port && ":" + u.port}`) : [u.origin];
    } catch { return []; }
  });
}

/** The keys the shell forwards to comment mode (`clax:key`). */
const FORWARDED_KEYS: ReadonlySet<string> = new Set(["Alt", "ArrowUp", "ArrowDown", "Escape"]);

/** The key and direction of a forwarded `clax:key`, or null for any other
 * key or a malformed message. */
export function forwardedKey(m: { key?: unknown; down?: unknown }): { key: string; down: boolean } | null {
  return typeof m.key === "string" && FORWARDED_KEYS.has(m.key) && typeof m.down === "boolean" ? { key: m.key, down: m.down } : null;
}

/** The shell keys a press in the page stands for (`clax:shortcut`): only C,
 * comment mode. `?` and every other key stay the page's. */
const PAGE_SHORTCUTS: ReadonlySet<string> = new Set(["c"]);

/** What a key is typed into: a form field, editable content, or a custom
 * text field or combo box. */
const TYPING = "input, textarea, select, [contenteditable]:not([contenteditable=false]), [role=textbox], [role=combobox]";

/** The shell key that keydown `e` in the page stands for, or null. It is C
 * (either case, so Caps Lock does not matter) when the browser delivered it
 * (`isTrusted`: a page's own `dispatchEvent` never counts), the page did not
 * cancel it (`defaultPrevented`, which the bridge reads again once every
 * listener has run), it has no modifier (Shift, Control, Option or Command), it is not a
 * repeat or an input method's composition, and it was not typed into an
 * input, textarea, select, contenteditable, or anything with the role
 * textbox or combobox, or inside one (the event's innermost target, through
 * open shadow roots, so it is called while the event is dispatched). */
export function pageShortcut(e: KeyboardEvent): "c" | null {
  if (!e.isTrusted || e.defaultPrevented || e.shiftKey || e.ctrlKey || e.altKey || e.metaKey || e.repeat || e.isComposing) return null;
  if (typeof e.key !== "string" || !PAGE_SHORTCUTS.has(e.key.toLowerCase())) return null;
  const t = e.composedPath()[0] ?? e.target;
  if (t instanceof Element && ((t as HTMLElement).isContentEditable || t.closest(TYPING))) return null;
  return "c";
}

/** The message when the browser delivered it (a page's own `dispatchEvent`
 * of a made-up `MessageEvent`, which can name any source and origin, is not
 * trusted), from `parent` at one of `origins`, with a shell message type. */
export function acceptFromShell(e: MessageEvent, parent: Window | null, origins: string[]): ShellToBridge | null {
  if (!e.isTrusted || !parent || e.source !== parent || !origins.includes(e.origin)) return null;
  const d = e.data;
  if (!d || typeof d !== "object" || !SHELL_TYPES.has(d.type)) return null;
  return d as ShellToBridge;
}
