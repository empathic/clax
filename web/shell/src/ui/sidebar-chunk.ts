// The thread list's code and styles (ShellSidebar.svelte, Sidebar.svelte and
// what they import: the controller's wiring of the sidebar too) are
// not in the artifact entry. The sidebar island asks for them as the view
// starts where the threads open, so the list is drawn with the view, or,
// once the artifact entry has allowed prefetching, after the first paint
// once the view is ready on an artifact that is not deleted, so the first
// Threads tap shows them at once. The sidebar's views (SidebarViews.svelte)
// come in the same load: ShellSidebar.svelte imports them; and so does the
// strip a closed sidebar leaves (ShellRail.svelte, ShellSidebar's
// `ShellRail`). Every load is one.
import { haikuList } from "../view/haiku-list";

type SidebarModule = typeof import("./ShellSidebar.svelte");

let pending: Promise<SidebarModule> | null = null;

/** Loads the sidebar's chunk once, with the haiku list its working strips
 * show (`view/haiku-list.ts`, which never fails the load), so the list is
 * drawn whole at once; a failed load may be tried again. */
export function loadSidebar(): Promise<SidebarModule> {
  pending ??= Promise.all([import("./ShellSidebar.svelte"), haikuList()]).then(([m]) => m).catch((e: unknown) => { pending = null; throw e; });
  return pending;
}

let prefetch = false;

/** Lets the sidebar island fetch the chunk before the threads show (the artifact entry). */
export function allowSidebarPrefetch(): void { prefetch = true; }

/** Whether the chunk may be fetched before the threads show. */
export const sidebarPrefetch = (): boolean => prefetch;

/** Whether the closed panel shows the strip: wider than a phone (`wide`),
 * the drawer docked left or right (`layout` as the root's `data-drawer`
 * holds it, none being right), outside the Clax app's inspect view, for an
 * artifact that is not deleted and has a thread. */
export function railShows(s: { panel: boolean; threads: unknown[]; deleted: boolean }, layout: string | undefined, wide: boolean, inspecting: boolean): boolean {
  return !s.panel && !s.deleted && s.threads.length > 0 && wide && !inspecting && (!layout || layout === "left" || layout === "right");
}
