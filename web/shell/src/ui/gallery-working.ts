// The gallery's attention grouping, markers, working chips and rosters, its
// live refreshes, and the haiku in its footer are not needed for the first paint: the gallery loads
// this module once its first list has painted, and until then shows one list
// in the usual order.
import { mount, unmount } from "svelte";
import type { Artifact, AttentionSummary } from "../api";
import { type Cards, CardSync as Sync } from "./card-sync";
import GallerySites from "./GallerySites.svelte";
import type { WorkingFeed } from "./working-feed.svelte";
import { markers } from "../view/attention-model";
import { agentNames, chips, type Working } from "../view/working-model";
import { pageStream } from "../stream";

export { groups } from "../view/attention-model";
export { default as HaikuLine } from "./HaikuLine.svelte";
export { default as Links } from "./HeaderLinks.svelte";
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
export { inboxBrowser } from "../owner";
export { pageStream };
/** The question module (Inbox, the feeds, notifications). */
export const q = () => import("../q");
/** The inbox page, its own chunk: `/inbox` loads it beside `q`. */
export const inboxPage = () => import("../q/InboxPage.svelte");
/** The gallery's unread summary, its own chunk: `/` loads it beside `q`. */
export const summary = () => import("../q/GallerySummary.svelte");
/** `/people`: mounts the People page (its own chunk) at the end of
 * `target`, following the page's stream. The result removes it. */
export function peoplePage(target: Element): () => void {
  let view: Record<string, unknown> | null = null;
  let gone = false;
  void import("../people/People.svelte").then(p => { if (!gone) view = mount(p.default, { target, props: { stream: pageStream() } }); }, () => {});
  return () => { gone = true; if (view) void unmount(view); };
}

/** `/tags`: mounts the Tags page (its own chunk) at the end of `target`,
 * following the page's stream. The result removes it. */
export function tagsPage(target: Element): () => void {
  let view: Record<string, unknown> | null = null;
  let gone = false;
  void import("./TagsPage.svelte").then(p => { if (!gone) view = mount(p.default, { target, props: { stream: pageStream() } }); }, () => {});
  return () => { gone = true; if (view) void unmount(view); };
}

/** A card's markers, its working agents named as its participants name them. */
export const cardMarkers = (a: Artifact, att: AttentionSummary | undefined, working: Working[]) =>
  markers(a, att, chips(working, agentNames(working, a.participants?.agents ?? [])));
