// The gallery's attention grouping, markers, working chips and rosters, and
// its live refreshes, are not needed for the first paint: the gallery loads
// this module once its first list has painted, and until then shows one list
// in the usual order.
import { mount } from "svelte";
import type { Artifact, AttentionSummary } from "../api";
import { type Cards, CardSync as Sync } from "./card-sync";
import GallerySites from "./GallerySites.svelte";
import type { WorkingFeed } from "./working-feed.svelte";
import { markers } from "../view/attention-model";
import { agentNames, chips, type Working } from "../view/working-model";

export { groups } from "../view/attention-model";
export { default as Markers } from "./Markers.svelte";
export { default as NeedsGroup } from "./NeedsGroup.svelte";
export { default as Roster } from "./Roster.svelte";
export { default as Seen } from "./Seen.svelte";
export { WorkingFeed } from "./working-feed.svelte";
/** The gallery's live refreshes (`CardSync`), and once they start, its
 * sites (spec 2026-10-05 §7.2): one entry per site Clax has live pages of,
 * mounted at the end of the gallery when it lists a live page. Mounted from
 * here, the gallery's first paint carries none of it. */
export class CardSync extends Sync {
  #sites: HTMLElement | null = null;
  constructor(private readonly gallery: Cards, feed: WorkingFeed, every?: number) { super(gallery, feed, every); }
  override start(): void {
    super.start();
    const main = document.querySelector("main.gal");
    if (this.#sites || !main || !(this.gallery.list() ?? []).some(a => a.live)) return;
    const target = document.createElement("div");
    main.insertBefore(target, main.querySelector(".gfoot"));
    mount(GallerySites, { target });
    this.#sites = target;
  }
  override stop(): void {
    super.stop();
    // Its fetches end with it; the gallery is going away.
    this.#sites?.remove();
    this.#sites = null;
  }
}

/** A card's markers, its working agents named as its participants name them. */
export const cardMarkers = (a: Artifact, att: AttentionSummary | undefined, working: Working[]) =>
  markers(a, att, chips(working, agentNames(working, a.participants?.agents ?? [])));
