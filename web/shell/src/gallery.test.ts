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
    if (url.includes("haiku")) return new Response(JSON.stringify(["one\ntwo\nthree"]));
    if (url.endsWith("/api/viewers/me")) return new Response(JSON.stringify({ viewer: { public_id: "v1", display_name: "Ada", created_at: "2026-09-01T00:00:00Z" } }));
    if (url.endsWith("/api/token")) return tokenStatus === 200 ? new Response(JSON.stringify({ token: "t" })) : new Response("{}", { status: tokenStatus });
    if (init?.method === "PATCH") return new Response(JSON.stringify({ artifact: ARTIFACTS[0] }));
    if (init?.method === "DELETE") return new Response(null, { status: 204 });
    return new Response("{}", { status: 404 });
  }));
  return calls;
}

// The mounted gallery's teardown: its footer haiku resolves a lazy import, so
// each test unmounts its gallery before the next resets the module registry.
let unmountGallery: (() => void) | null = null;

async function mountGallery() {
  const { default: Gallery } = await import("./ui/Gallery.svelte");
  // From the same module registry as the gallery (each test resets it), so
  // both use one Svelte runtime.
  const { mount } = await import("./test/svelte");
  const { root, unmount } = mount(Gallery, {});
  unmountGallery = unmount;
  await waitFor(() => root.querySelector("a.card, .empty, .empty-gallery"), "gallery to render");
  return root;
}

describe("Gallery", () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { unmountGallery?.(); unmountGallery = null; vi.unstubAllGlobals(); document.body.replaceChildren(); });

  it("renders cards led by the version numeral, with title and link, and no description", async () => {
    stubApi();
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards.length).toBe(2);
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[0].textContent).toContain("Pinned one");
    expect(cards[0].querySelector(".v")?.textContent).toBe("v3");
    expect(cards[0].querySelector("p")).toBeNull();
    // One star: the Pin button's once the token is known, not the title's too.
    await waitFor(() => root.querySelector('.card-tools button[title="Unpin"]'), "the Unpin button");
    expect(root.querySelectorAll(".card-wrap")[0].textContent!.match(/★/g)).toHaveLength(1);
    expect(cards[0].querySelector(".pin")).toBeNull();
    expect(cards[1].querySelector(".by")?.textContent).toContain("command line");
    await waitFor(() => root.querySelector(".gbar .sub")?.textContent?.includes("seen as Ada"), "the viewer's name in the bar");
    expect(root.querySelector(".gbar .sub")?.textContent).toBe("local artifacts · seen as Ada");
  });

  it("puts the pinned artifact first even when the API lists it second", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [ARTIFACTS[1], ARTIFACTS[0]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[1].getAttribute("href")).toBe("/a/aaaaaaaaaaaa");  });

  it("shows one three-line haiku in the footer once the list loads", async () => {
    stubApi();
    const root = await mountGallery();
    const pre = await waitFor(() => root.querySelector(".gfoot pre"), "the footer haiku");
    expect(pre.textContent!.split("\n")).toHaveLength(3);
  });

  it("labels agent-published cards with the harness and a green dot only while the session is live", async () => {
    const owned = (id: string, live: boolean) => ({ ...ARTIFACTS[1], id, owner_session_id: "s" + id, owner_live: live, owner_harness: "claude-code" });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [owned("live00000000", true), owned("gone00000000", false), ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].querySelector(".by")?.textContent).toContain("claude-code");
    expect(cards[0].querySelector(".live-dot")).not.toBeNull();
    expect(cards[1].querySelector(".by")?.textContent).toContain("claude-code");
    expect(cards[1].querySelector(".live-dot")).toBeNull();
    expect(cards[2].querySelector(".by")?.textContent).not.toContain("claude-code");
    expect(cards[2].querySelector(".by")?.textContent).toContain("command line");
  });

  it("shows a working chip on a card an agent works on, none on the others, and the roster in the footer", async () => {
    const working = [{ key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-28T11:00:00Z", last_heartbeat: "2026-09-28T11:00:00Z" }];
    const parts = { people: [{ public_id: "v1", display_name: "Ada", seen: null }], agents: [{ handle: "a_1111aaaa", harness: "claude", live: true }] };
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [{ ...ARTIFACTS[0], working, participants: parts }, ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    // The chips and rosters load after the first paint.
    await waitFor(() => cards[0].querySelector(".chip.ag"), "the working chip");
    expect(Array.from(cards[0].querySelectorAll(".chip.ag")).map(c => c.textContent)).toEqual(["claude working"]);
    expect(cards[1].querySelector(".chip.ag")).toBeNull();
    expect(cards[1].querySelector(".mks")).toBeNull();
    expect(cards[0].querySelector(".ft .agt .tok.work")?.textContent).toBe("cl");
  });

  it("shows the mark with its halves apart when the gallery is empty", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => url.endsWith("/api/artifacts") ? new Response(JSON.stringify({ artifacts: [] })) : new Response("{}", { status: 404 })));
    const root = await mountGallery();
    expect(root.querySelector(".empty-gallery .mark[data-apart]")).not.toBeNull();
    expect(root.textContent).toContain("When an agent publishes a page, it lands here.");
    expect(root.textContent).toContain("clax publish");
  });

  it("marks a tenth version with the rally chip", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [{ ...ARTIFACTS[0], current_version: 10 }, ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].querySelector(".chip.rally")?.textContent).toBe("rally of 10");
    expect(cards[1].querySelector(".chip.rally")).toBeNull();
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
    // Without the Pin button, the title carries the pinned star.
    expect(root.querySelectorAll(".card-wrap")[0].querySelector(".pin")).not.toBeNull();
    expect(root.querySelectorAll(".card-wrap")[0].textContent!.match(/★/g)).toHaveLength(1);
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
