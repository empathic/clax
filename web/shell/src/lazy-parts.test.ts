// A part the person asks for (a dialog or menu loaded on its first use)
// that cannot load says so, and the next ask tries again.
import { afterEach, describe, expect, it, vi } from "vitest";

const ID = "7q3k9mzx2b4t";

afterEach(() => { vi.doUnmock("./ui/CloneDialog.svelte"); vi.unstubAllGlobals(); vi.resetModules(); document.body.innerHTML = ""; });

describe("a part asked for that cannot load", () => {
  it("says so in the connection notice, and the next ask loads it", async () => {
    vi.resetModules();
    let fail = true;
    vi.doMock("./ui/CloneDialog.svelte", async () => {
      if (fail) throw new TypeError("Failed to fetch dynamically imported module");
      return vi.importActual("./ui/CloneDialog.svelte");
    });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => (url === `/api/artifacts/${ID}/git-token`
      ? new Response(JSON.stringify({ available: true, scopes: ["read"] }))
      : new Response("{}", { status: 403 }))));
    const { flush, mount } = await import("./test/svelte");
    const { default: Clone } = await import("./ui/Clone.svelte");
    const { cloneOpen } = await import("./ui/more-menu.svelte");
    const { PART_UNLOADED, connNoticeText, connTrouble } = await import("./conn-notice");
    const m = mount(Clone, { id: ID });
    await vi.waitFor(() => expect(cloneOpen.current).not.toBeNull());
    cloneOpen.current!();
    await vi.waitFor(() => expect(connNoticeText()).toBe(PART_UNLOADED));
    expect(document.querySelector("[role=dialog]")).toBeNull();
    fail = false;
    cloneOpen.current!();
    await vi.waitFor(() => { flush(); expect(document.querySelector("[role=dialog]")).not.toBeNull(); });
    connTrouble("part", false, "");
    m.unmount();
  });
});
