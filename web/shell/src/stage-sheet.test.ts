// The stage island loads the keys sheet's code when the sheet is asked for;
// if that load fails, it tells the controller, which closes the sheet.
import { afterEach, expect, it, vi } from "vitest";

// The gesture module this test's registry loaded watches the document until its `unwatchShell` runs.
afterEach(async () => { (await import("./caps/gesture")).unwatchShell(); vi.doUnmock("./ui/KeysSheet.svelte"); vi.resetModules(); });

it("a sheet whose code fails to load reaches sheetFailed", async () => {
  vi.resetModules();
  vi.doMock("./ui/KeysSheet.svelte", () => { throw new Error("chunk gone"); });
  // Svelte, the mount helper and the island from one fresh module registry.
  const { default: StageIsland } = await import("./ui/StageIsland.svelte");
  const { flush, mount } = await import("./test/svelte");
  const { Store } = await import("./view/store");
  const state = new Store({
    pinnedVersion: null, startFile: "index.html", data: { artifact: { id: "a" }, versions: [] }, error: null, origin: null, newer: null, deleted: false,
    commenting: false, panel: false, narrow: false, threads: [], resolved: {}, draft: null, selected: null, hovered: null, busy: 0, notice: null, hint: null,
    me: null, ask: null, file: "index.html", sheet: "keys" as "keys" | null, replyFocus: 0,
  });
  const ctl = {
    id: "a", state, shown: () => 1, latest: () => 1, missing: () => null, here: () => "/", openPin() {}, hover() {},
    closeSheet: vi.fn(), sheetFailed: vi.fn(() => state.set({ sheet: null })),
  };
  const m = mount(StageIsland, { ctl } as never);
  await vi.waitFor(() => expect(ctl.sheetFailed).toHaveBeenCalledTimes(1));
  flush();
  expect(m.root.querySelector("[role=dialog]")).toBeNull();
  m.unmount();
});
