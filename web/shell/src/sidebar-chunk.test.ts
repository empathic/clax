import { afterEach, expect, it, vi } from "vitest";

afterEach(() => {
  vi.doUnmock("./ui/SidebarViews.svelte");
  vi.resetModules();
});

it("brings the sidebar's views with the sidebar's own chunk, so the list does not wait for a second fetch", async () => {
  vi.resetModules();
  const asked: string[] = [];
  vi.doMock("./ui/SidebarViews.svelte", () => { asked.push("views"); return { default: {} }; });
  const { loadSidebar } = await import("./ui/sidebar-chunk");
  await loadSidebar();
  // Loaded with the chunk, before any sidebar mounts and asks for them.
  expect(asked).toEqual(["views"]);
  // The gesture module the sidebar loads watches the shell from its load: stop this registry's watch.
  (await import("./caps/gesture")).unwatchShell();
});
