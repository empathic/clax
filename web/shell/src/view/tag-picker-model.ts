// The tag picker's rules (`ui/TagPicker.svelte`): which tags a search
// shows, where the arrow keys move, and what a filter's button says. Pure.
import type { TagRef } from "../api";

/** The tags whose name contains `query` (trimmed, any case), in their order; all of them for none. */
export function matchTags<T extends TagRef>(all: readonly T[], query: string): T[] {
  const q = query.trim().toLowerCase();
  return q ? all.filter(t => t.name.toLowerCase().includes(q)) : [...all];
}

/** Where a key moves focus among the search field (-1) and `n` options
 * (0 … n-1) from `at`: Down and Up step (Up from the first option is the
 * field), Home and End jump; null for any other key, or with no option. */
export function step(at: number, n: number, key: string): number | null {
  if (!n) return null;
  switch (key) {
    case "ArrowDown": return Math.min(at + 1, n - 1);
    case "ArrowUp": return Math.max(at - 1, -1);
    case "Home": return at < 0 ? null : 0;
    case "End": return at < 0 ? null : n - 1;
    default: return null;
  }
}

/** What a tag filter's button says: "Any tag", the one tag, or the first and how many more. */
export function filterLabel(sel: readonly string[]): string {
  if (!sel.length) return "Any tag";
  return sel.length === 1 ? sel[0] : `${sel[0]} +${sel.length - 1}`;
}
