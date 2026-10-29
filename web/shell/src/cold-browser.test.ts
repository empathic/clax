import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { COLD_HEADING } from "./cold-browser";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "./test/timeouts";

/** Polls until `check` returns a truthy value; fails after `WAIT_MS`. */
async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

const ARTIFACT = { id: "7q3k9mzx2b4t", title: "Roadmap", description: null, icon: null, pinned: false, current_version: 1, updated_at: "2026-10-29T10:00:00Z" };
const NO_CREDENTIAL = () => new Response(JSON.stringify({ error: { code: "no_credential", message: "this needs a credential" } }), { status: 401 });

/** A local daemon's answers: `session` is whether the browser holds an owner browser session. */
function daemon(session: boolean) {
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    if (!session) return NO_CREDENTIAL();
    if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
    if (url.startsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [ARTIFACT] }));
    if (url === "/api/viewers/me") return new Response(JSON.stringify({ viewer: { public_id: "v1", display_name: "Ada", created_at: "2026-10-01T00:00:00Z" } }));
    return new Response("{}", { status: 404 });
  }));
}

let unmount: (() => void) | null = null;

async function boot() {
  const { default: Gallery } = await import("./ui/Gallery.svelte");
  const { mount } = await import("./test/svelte");
  const m = mount(Gallery, {});
  unmount = m.unmount;
  return m.root;
}

describe("booting the shell on a local daemon", { timeout: MOUNT_TIMEOUT_MS }, () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { unmount?.(); unmount = null; vi.unstubAllGlobals(); document.body.replaceChildren(); });

  it("works as the owner's browser with a session cookie", async () => {
    daemon(true);
    const root = await boot();
    await waitFor(() => root.querySelector("a.card"), "the gallery's cards");
    const { getToken } = await import("./api");
    expect(await getToken()).toBe("tk");
    expect(document.getElementById("clax-cold")).toBeNull();
  });

  it("shows how to open Clax from the terminal without one", async () => {
    daemon(false);
    await boot();
    const page = await waitFor(() => document.getElementById("clax-cold"), "the cold-browser page");
    expect(page.querySelector("h1")?.textContent).toBe(COLD_HEADING);
    expect(page.textContent).toContain("clax open");
    expect(page.textContent).toContain("clax login");
    const { getToken } = await import("./api");
    expect(await getToken()).toBeNull();
  });

  it("is not shown for a refusal of a browser that holds a credential", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "unauthorized" } }), { status: 401 })));
    const { getToken } = await import("./api");
    expect(await getToken()).toBeNull();
    expect(document.getElementById("clax-cold")).toBeNull();
  });
});
