import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "preact";

class FakeES {
  static last: FakeES;
  listeners = new Map<string, (e: MessageEvent) => void>();
  constructor(public url: string) { FakeES.last = this; }
  addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, fn); }
  close() {}
  emit(t: string, data: unknown) { this.listeners.get(t)?.(new MessageEvent(t, { data: JSON.stringify(data) })); }
}

async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

const ID = "7q3k9mzx2b4t";
const artifact = (n: number) => ({ artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "2026-09-28T11:00:00Z", current_version: n, pinned: false }, versions: [{ artifact_id: ID, n, label: null, created_at: "x", files: {} }] });

const viewer = { viewer: { id: "v", display_name: null, created_at: "x" } };

/** Answers the comment routes (no threads, an anonymous viewer) unless `comments` is given; everything else goes to `fetchImpl`. */
async function mount(fetchImpl: () => Promise<Response>, comments?: (url: string) => Promise<Response>) {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.includes("/threads") || url.startsWith("/api/viewers/")) {
      if (comments) return comments(url);
      return new Response(JSON.stringify(url.includes("/threads") ? { threads: [], next_cursor: null } : viewer));
    }
    return fetchImpl();
  }));
  sessionStorage.setItem("artifax.origin-ok", "0");
  const { default: ArtifactView } = await import("./artifact");
  const root = document.createElement("div");
  document.body.appendChild(root);
  render(<ArtifactView id={ID} pinnedVersion={null} />, root);
  return root;
}

describe("ArtifactView", () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); });

  it("says not found only for a 404 status", async () => {
    const root = await mount(async () => new Response(JSON.stringify({ error: { message: "nope" } }), { status: 404 }));
    await waitFor(() => root.textContent?.includes("Artifact not found"), "not-found message");
  });

  it("shows the raw error for other failures, even when the text contains 404", async () => {
    const root = await mount(async () => new Response(JSON.stringify({ error: { message: "port 4040 or 404 busy" } }), { status: 500 }));
    await waitFor(() => root.textContent?.includes("port 4040"), "error message");
    expect(root.textContent).not.toContain("Artifact not found");
  });

  it("refetches on resync and shows the banner when a newer version exists", async () => {
    let current = 1;
    const root = await mount(async () => new Response(JSON.stringify(artifact(current))));
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    expect(root.querySelector(".banner")).toBeNull();
    FakeES.last.emit("resync", { dropped: 3 });
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector(".banner")).toBeNull();
    current = 4;
    FakeES.last.emit("resync", { dropped: 3 });
    await waitFor(() => root.querySelector(".banner")?.textContent?.includes("v4 published"), "banner");
  });

  it("shows a failed thread load in the notice banner", async () => {
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ error: { code: "internal", message: "db locked" } }), { status: 500 })
        : new Response(JSON.stringify(viewer)));
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.getAttribute("role")).toBe("alert");
    expect(banner.textContent).toContain("Could not load comments: 500 db locked");
  });
});
