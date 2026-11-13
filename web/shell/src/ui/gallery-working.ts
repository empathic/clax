// What the gallery needs once its first render is on screen, never for it:
// its live refreshes (`CardSync`) and working lists (`WorkingFeed`) on the
// page's stream, the question module (`q`) and the inbox's surfaces, each
// page's own chunk, ClaxBot's header meter, and the haiku in its footer.
// Everything the first render lays out (the grouping, the markers and
// rosters, the New tile, the header's links) is in the gallery's entry.
// The entry's inline script preloads this module beside the entry
// (`vite.shell.config.ts` `PRELOAD`), so it is in by the time it is asked.
import { type Component, mount, unmount } from "svelte";
import { type Cards, CardSync as Sync } from "./card-sync";
import GallerySites from "./GallerySites.svelte";
import type { WorkingFeed } from "./working-feed.svelte";
import { pageStream } from "../stream";
import { moved } from "../settings/moved";

export { default as HaikuLine } from "./HaikuLine.svelte";
export { WorkingFeed } from "./working-feed.svelte";
export { seedBot } from "../bot/me";
/** ClaxBot's meter in the header, a chunk of its own. */
export const bot = () => import("../bot/HeaderBot.svelte");
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

/** The gallery shell's pages besides the gallery and the inbox, each its own chunk. */
export type Page = "tags" | "queues" | "settings" | "people" | "admin" | "me" | "audit";
const PAGES: Partial<Record<Page, () => Promise<{ default: unknown }>>> = {
  tags: () => import("./TagsPage.svelte"),
  queues: () => import("../queues/Queues.svelte"),
  settings: () => import("../settings/Settings.svelte"),
};

/** `/tags`, `/queues` and `/settings` (with its screens, `/settings/<screen>`): mounts
 * that page (its own chunk) at the end of `target`, following the page's
 * stream; `ready` is called once the page shows its first data (or could
 * not load: at once). The result removes it. A page that moved into Settings
 * (`/people`, `/admin`, `/me`, `/audit`) is replaced by its screen's
 * address there, keeping its query and fragment. */
export function page(name: Page, target: Element, ready: () => void | Promise<void> = () => {}): () => void {
  let view: Record<string, unknown> | null = null;
  let gone = false;
  const load = PAGES[name];
  if (!load) {
    const to = moved(location.pathname, location.search);
    if (to) location.replace(`${to}${location.hash}`);
    return () => {};
  }
  void load().then(p => { if (!gone) view = mount(p.default as Component<{ stream: ReturnType<typeof pageStream>; onready?: () => void | Promise<void> }>, { target, props: { stream: pageStream(), onready: ready } }); }, () => { void ready(); });
  return () => { gone = true; if (view) void unmount(view); };
}
