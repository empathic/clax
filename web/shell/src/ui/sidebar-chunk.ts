// The thread list's code and styles (ShellSidebar.svelte, Sidebar.svelte and
// what they import: the controller's wiring of the sidebar too) are
// not needed for the first paint. The sidebar island loads them after the
// first paint when the threads show, or, once the artifact entry has allowed
// prefetching, as soon as the view is ready on an artifact that is not
// deleted, so the first Threads tap shows them at once. Every load is one.
type SidebarModule = typeof import("./ShellSidebar.svelte");

let pending: Promise<SidebarModule> | null = null;

/** Loads the sidebar's chunk once; a failed load may be tried again. */
export function loadSidebar(): Promise<SidebarModule> {
  pending ??= import("./ShellSidebar.svelte").catch((e: unknown) => { pending = null; throw e; });
  return pending;
}

let prefetch = false;

/** Lets the sidebar island fetch the chunk before the threads show (the artifact entry). */
export function allowSidebarPrefetch(): void { prefetch = true; }

/** Whether the chunk may be fetched before the threads show. */
export const sidebarPrefetch = (): boolean => prefetch;
