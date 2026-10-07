// The gallery's attention grouping, markers, working chips and rosters, and
// its live refreshes, are not needed for the first paint: the gallery loads
// this module once its first list has painted, and until then shows one list
// in the usual order.
import { mount, unmount } from "svelte";
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
  #sites: { target: HTMLElement; view: Record<string, unknown> } | null = null;
  constructor(private readonly gallery: Cards, feed: WorkingFeed, every?: number) { super(gallery, feed, every); }
  override start(): void {
    super.start();
    const main = document.querySelector("main.gal");
    if (this.#sites || !main || !(this.gallery.list() ?? []).some(a => a.live)) return;
    const target = document.createElement("div");
    main.insertBefore(target, main.querySelector(".gfoot"));
    this.#sites = { target, view: mount(GallerySites, { target }) };
  }
  override stop(): void {
    super.stop();
    // Its subscription to the gallery topic ends with it.
    if (this.#sites) {
      void unmount(this.#sites.view);
      this.#sites.target.remove();
    }
    this.#sites = null;
  }
}

/** A card's markers, its working agents named as its participants name them. */
export const cardMarkers = (a: Artifact, att: AttentionSummary | undefined, working: Working[]) =>
  markers(a, att, chips(working, agentNames(working, a.participants?.agents ?? [])));
