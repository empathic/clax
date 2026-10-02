// The thread list's code and styles (Sidebar.svelte and what it imports) are
// not needed for the first paint. The artifact entry prefetches them once the
// page has painted, and the sidebar island loads them when the threads show;
// both share one load.
type SidebarModule = typeof import("./Sidebar.svelte");

let pending: Promise<SidebarModule> | null = null;

/** Loads the sidebar's chunk once; a failed load may be tried again. */
export function loadSidebar(): Promise<SidebarModule> {
  pending ??= import("./Sidebar.svelte").catch((e: unknown) => { pending = null; throw e; });
  return pending;
}
