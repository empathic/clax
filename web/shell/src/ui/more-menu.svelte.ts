// The top bar's more menu, roster and working summary, and its Clone and Publish buttons,
// are not needed for the first paint: the artifact entry loads their code
// (and their styles) once the page has painted. Until then the bar keeps a
// slot of the menu's size and shows no roster and no Clone.
import type { Component } from "svelte";
import type WhoT from "./Who.svelte";

/** What the more menu's Publish item says and opens. */
export type PublishOpener = { label: string; open(): void };
type More = Component<{ rawHref: string | null; canCopy: boolean; onCopy(): void; onVersions?(): void; pageHref?: string | null; onFork?(): void; onClone?: (() => void) | null; onPublish?: PublishOpener | null }>;
type CloneT = Component<{ id: string }>;
type PublishT = Component<{ id: string; title: string; latest: number }>;

let loaded: More | null = $state(null);
let whoLoaded: typeof WhoT | null = $state(null);
let cloneLoaded: CloneT | null = $state(null);
let cloneOpener: (() => void) | null = $state(null);
let publishLoaded: PublishT | null = $state(null);
let publishOpener: PublishOpener | null = $state(null);

/** The more menu's component once `loadMoreMenu` has loaded it, else null. */
export const moreMenu = { get current(): More | null { return loaded; } };
/** The roster and summary component once `loadMoreMenu` has loaded it, else null. */
export const who = { get current(): typeof WhoT | null { return whoLoaded; } };
/** The Clone button's component once `loadMoreMenu` has loaded it, else null. */
export const clone = { get current(): CloneT | null { return cloneLoaded; } };
/** What opens the Clone dialog while the viewer may clone (the more menu's
 * Clone item at phone width), else null. */
export const cloneOpen = { get current(): (() => void) | null { return cloneOpener; } };

/** Sets what opens the Clone dialog (the Clone button sets it once it knows
 * the viewer may clone; null clears it). */
export function setCloneOpener(f: (() => void) | null): void { cloneOpener = f; }

/** The Publish button's component once `loadMoreMenu` has loaded it, else null. */
export const publish = { get current(): PublishT | null { return publishLoaded; } };
/** What the more menu's Publish item opens while there is something to
 * publish (at phone width), else null. */
export const publishOpen = { get current(): PublishOpener | null { return publishOpener; } };
/** Sets the Publish item (the Publish button sets it; null clears it). */
export function setPublishOpener(o: PublishOpener | null): void { publishOpener = o; }

/** Loads the more menu's, the roster's and the Clone button's code. A failed load leaves its part out. */
export function loadMoreMenu(): void {
  import("./MoreMenu.svelte").then(m => { loaded = m.default; }, () => {});
  import("./Who.svelte").then(m => { whoLoaded = m.default; }, () => {});
  import("./Clone.svelte").then(m => { cloneLoaded = m.default; }, () => {});
  import("./Publish.svelte").then(m => { publishLoaded = m.default; }, () => {});
}
