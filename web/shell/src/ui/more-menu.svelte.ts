// The top bar's more menu is not needed for the first paint: the artifact
// entry loads its code (and its styles) once the page has painted, and until
// then the bar keeps a slot of its size.
import type { Component } from "svelte";

type More = Component<{ rawHref: string | null; canCopy: boolean; onCopy(): void }>;

let loaded: More | null = $state(null);

/** The more menu's component once `loadMoreMenu` has loaded it, else null. */
export const moreMenu = { get current(): More | null { return loaded; } };

/** Loads the more menu's code. A failed load leaves the slot empty. */
export function loadMoreMenu(): void {
  import("./MoreMenu.svelte").then(m => { loaded = m.default; }, () => {});
}
