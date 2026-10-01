import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { relativeTime } from "./format";

describe("relativeTime", () => {
  const now = new Date("2026-09-28T12:00:00Z");
  it("buckets", () => {
    expect(relativeTime("2026-09-28T11:59:40Z", now)).toBe("just now");
    expect(relativeTime("2026-09-28T11:55:00Z", now)).toBe("5 min ago");
    expect(relativeTime("2026-09-28T09:00:00Z", now)).toBe("3 h ago");
    expect(relativeTime("2026-09-26T12:00:00Z", now)).toBe("2 d ago");
    expect(relativeTime("2026-01-01T00:00:00Z", now)).toBe("2026-01-01");
  });
});

/** Polls until `check` returns a truthy value; fails after 2 s. */
async function waitFor<T>(check: () => T | null | undefined | false, what = "condition"): Promise<T> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}
const ARTIFACTS = [
  { id: "7q3k9mzx2b4t", title: "Pinned one", description: "d", icon: "chart", pinned: true, current_version: 3, updated_at: "2026-09-28T11:00:00Z" },
  { id: "aaaaaaaaaaaa", title: "Other", description: "sales report", icon: null, pinned: false, current_version: 1, updated_at: "2026-09-27T11:00:00Z" },
];

type Call = { url: string; method: string; body?: string };
function stubApi(tokenStatus: number = 200) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, method: init?.method ?? "GET", body: init?.body as string | undefined });
    if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: ARTIFACTS }));
    if (url.endsWith("/api/token")) return tokenStatus === 200 ? new Response(JSON.stringify({ token: "t" })) : new Response("{}", { status: tokenStatus });
    if (init?.method === "PATCH") return new Response(JSON.stringify({ artifact: ARTIFACTS[0] }));
    if (init?.method === "DELETE") return new Response(null, { status: 204 });
    return new Response("{}", { status: 404 });
  }));
  return calls;
}

async function mountGallery() {
  const { default: Gallery } = await import("./ui/Gallery.svelte");
  // From the same module registry as the gallery (each test resets it), so
  // both use one Svelte runtime.
  const { mount } = await import("./test/svelte");
  const { root } = mount(Gallery, {});
  await waitFor(() => root.querySelector("a.card, .empty"), "gallery to render");
  return root;
}

describe("Gallery", () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

  it("renders cards with title, version, and link in API order", async () => {
    stubApi();
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards.length).toBe(2);
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[0].textContent).toContain("Pinned one");
    expect(cards[0].textContent).toContain("v3");
    expect(cards[0].querySelector(".pin")).not.toBeNull();
    expect(root.textContent).toContain("published from the command line");
  });

  it("labels agent-published cards with the harness and a green dot only while the session is live", async () => {
    const owned = (id: string, live: boolean) => ({ ...ARTIFACTS[1], id, owner_session_id: "s" + id, owner_live: live, owner_harness: "claude-code" });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [owned("live00000000", true), owned("gone00000000", false), ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].querySelector(".publisher")?.textContent).toContain("published by claude-code session");
    expect(cards[0].querySelector(".live-dot")).not.toBeNull();
    expect(cards[1].querySelector(".publisher")?.textContent).toContain("published by claude-code session");
    expect(cards[1].querySelector(".live-dot")).toBeNull();
    expect(cards[2].querySelector(".publisher")).toBeNull();
    expect(cards[2].textContent).toContain("published from the command line");
  });

  it("shows an empty state", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ artifacts: [] }))));
    const root = await mountGallery();
    expect(root.textContent).toContain("No artifacts yet");
    expect(root.textContent).toContain("clax publish");
  });

  it("search narrows cards by title and description", async () => {
    stubApi();
    const root = await mountGallery();
    const input = root.querySelector("input[type=search]") as HTMLInputElement;
    expect(input.placeholder).toBe("Search artifacts");
    expect(input.getAttribute("aria-label")).toBe("Search artifacts");
    input.value = "SALES";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => root.querySelectorAll("a.card").length === 1, "one card");
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].textContent).toContain("Other");
    input.value = "pinned";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => root.querySelector("a.card")?.textContent?.includes("Pinned one"), "pinned card");
    expect(root.querySelectorAll("a.card").length).toBe(1);
  });

  it("without a token renders no buttons", async () => {
    const calls = stubApi(403);
    const root = await mountGallery();
    await waitFor(() => calls.some(c => c.url.endsWith("/api/token")), "token request");
    await new Promise(r => setTimeout(r, 0));
    expect(root.querySelectorAll(".card-wrap").length).toBe(2);
    expect(root.querySelectorAll(".card-tools button").length).toBe(0);
  });

  it("pin button sends PATCH with pinned true and refetches", async () => {
    const calls = stubApi();
    const root = await mountGallery();
    const pin = await waitFor(() => root.querySelectorAll(".card-wrap")[1]?.querySelector('button[title="Pin"]') as HTMLButtonElement | null, "pin button");
    const wrap = root.querySelectorAll(".card-wrap")[1];
    expect(wrap.querySelector("a button")).toBeNull();
    expect(pin.getAttribute("aria-label")).toBe("Pin Other");
    pin.click();
    await waitFor(() => calls.filter(c => c.url.endsWith("/api/artifacts") && c.method === "GET").length === 2, "refetch after PATCH");
    const patch = calls.find(c => c.method === "PATCH")!;
    expect(patch.url).toBe("/api/artifacts/aaaaaaaaaaaa");
    expect(JSON.parse(patch.body!)).toEqual({ pinned: true });
  });

  it("delete calls DELETE only after confirm", async () => {
    const calls = stubApi();
    const confirmSpy = vi.fn(() => false);
    vi.stubGlobal("confirm", confirmSpy);
    const root = await mountGallery();
    const del = await waitFor(() => root.querySelector('button[title="Delete"]') as HTMLButtonElement | null, "delete button");
    del.click();
    expect(confirmSpy).toHaveBeenCalledWith('Delete "Pinned one"? This removes every version.');
    expect(calls.some(c => c.method === "DELETE")).toBe(false);
    confirmSpy.mockReturnValue(true);
    del.click();
    const d = await waitFor(() => calls.find(c => c.method === "DELETE"), "DELETE request");
    expect(d.url).toBe("/api/artifacts/7q3k9mzx2b4t");
  });
});

describe("getToken", () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { vi.unstubAllGlobals(); });

  it("retries after a transient failure", async () => {
    let n = 0;
    vi.stubGlobal("fetch", vi.fn(async () => {
      if (n++ === 0) throw new Error("network");
      return new Response(JSON.stringify({ token: "tok" }));
    }));
    const { getToken } = await import("./api");
    expect(await getToken()).toBeNull();
    expect(await getToken()).toBe("tok");
  });

  it("caches a definitive 403", async () => {
    const f = vi.fn(async () => new Response("{}", { status: 403 }));
    vi.stubGlobal("fetch", f);
    const { getToken } = await import("./api");
    expect(await getToken()).toBeNull();
    expect(await getToken()).toBeNull();
    expect(f).toHaveBeenCalledTimes(1);
  });
});
