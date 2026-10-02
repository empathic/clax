// The top bar's more menu, roster and working summary are not needed for the
// first paint: the artifact entry loads their code (and their styles) once
// the page has painted. Until then the bar keeps a slot of the menu's size
// and shows no roster.
import type { Component } from "svelte";
import type WhoT from "./Who.svelte";

type More = Component<{ rawHref: string | null; canCopy: boolean; onCopy(): void }>;

let loaded: More | null = $state(null);
let whoLoaded: typeof WhoT | null = $state(null);

/** The more menu's component once `loadMoreMenu` has loaded it, else null. */
export const moreMenu = { get current(): More | null { return loaded; } };
/** The roster and summary component once `loadMoreMenu` has loaded it, else null. */
export const who = { get current(): typeof WhoT | null { return whoLoaded; } };

/** Loads the more menu's and the roster's code. A failed load leaves its part out. */
export function loadMoreMenu(): void {
  import("./MoreMenu.svelte").then(m => { loaded = m.default; }, () => {});
  import("./Who.svelte").then(m => { whoLoaded = m.default; }, () => {});
}
