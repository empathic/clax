// The keys of the headers' menu buttons (the feedback menu, the account
// menu): a menu's items are its `[role="menuitem"]` elements. Up, Down, Home
// and End move between them; Escape closes the menu (`close` puts focus back
// on its button); Tab leaves it as usual. Any other printable key typed in
// an open menu stops there, so it never reaches the shell's keys.

/** The first item of `menu`, which opening the menu focuses. */
export const firstItem = (menu: HTMLElement | undefined): HTMLElement | null => menu?.querySelector<HTMLElement>('[role="menuitem"]') ?? null;

/** Handles `e`, a key pressed in the open menu `menu`. */
export function menuKey(e: KeyboardEvent, menu: HTMLElement | undefined, close: () => void): void {
  if (e.key === "Tab") return;
  const items = Array.from(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
  const i = items.indexOf(document.activeElement as HTMLElement);
  const to = ({ ArrowDown: (i + 1) % items.length, ArrowUp: (i - 1 + items.length) % items.length, Home: 0, End: items.length - 1 } as Record<string, number>)[e.key];
  if (e.key !== "Escape" && to === undefined) {
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  if (e.key === "Escape") close(); else items[to]?.focus();
}
