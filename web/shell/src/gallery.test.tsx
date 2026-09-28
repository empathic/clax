import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "preact";
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

const settle = () => new Promise(r => setTimeout(r, 50));
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

async function mount() {
  const { default: Gallery } = await import("./gallery");
  const root = document.createElement("div");
  document.body.appendChild(root);
  render(<Gallery />, root);
  await settle();
  return root;
}

describe("Gallery", () => {
  beforeEach(() => { vi.resetModules(); });

  it("renders cards with title, version, and link in API order", async () => {
    stubApi();
    const root = await mount();
    const cards = root.querySelectorAll("a.card");
    expect(cards.length).toBe(2);
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[0].textContent).toContain("Pinned one");
    expect(cards[0].textContent).toContain("v3");
    expect(cards[0].querySelector(".pin")).not.toBeNull();
    expect(root.textContent).toContain("published from the command line");
  });

  it("shows an empty state", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ artifacts: [] }))));
    const root = await mount();
    expect(root.textContent).toContain("No artifacts yet");
    expect(root.textContent).toContain("artifax publish");
  });

  it("search narrows cards by title and description", async () => {
    stubApi();
    const root = await mount();
    const input = root.querySelector("input[type=search]") as HTMLInputElement;
    expect(input.placeholder).toBe("Search artifacts");
    input.value = "SALES";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const cards = root.querySelectorAll("a.card");
    expect(cards.length).toBe(1);
    expect(cards[0].textContent).toContain("Other");
    input.value = "pinned";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(root.querySelectorAll("a.card").length).toBe(1);
    expect(root.querySelector("a.card")!.textContent).toContain("Pinned one");
  });

  it("without a token renders no buttons", async () => {
    stubApi(403);
    const root = await mount();
    expect(root.querySelectorAll(".card-wrap").length).toBe(2);
    expect(root.querySelectorAll(".card-tools button").length).toBe(0);
  });

  it("pin button sends PATCH with pinned true and refetches", async () => {
    const calls = stubApi();
    const root = await mount();
    const wraps = root.querySelectorAll(".card-wrap");
    expect(wraps[1].querySelector("a button")).toBeNull();
    (wraps[1].querySelector('button[title="Pin"]') as HTMLButtonElement).click();
    await settle();
    const patch = calls.find(c => c.method === "PATCH")!;
    expect(patch.url).toBe("/api/artifacts/aaaaaaaaaaaa");
    expect(JSON.parse(patch.body!)).toEqual({ pinned: true });
    expect(calls.filter(c => c.url.endsWith("/api/artifacts") && c.method === "GET").length).toBe(2);
  });

  it("delete calls DELETE only after confirm", async () => {
    const calls = stubApi();
    const confirmSpy = vi.fn(() => false);
    vi.stubGlobal("confirm", confirmSpy);
    const root = await mount();
    const del = root.querySelector('button[title="Delete"]') as HTMLButtonElement;
    del.click();
    await settle();
    expect(confirmSpy).toHaveBeenCalledWith('Delete "Pinned one"? This removes every version.');
    expect(calls.some(c => c.method === "DELETE")).toBe(false);
    confirmSpy.mockReturnValue(true);
    del.click();
    await settle();
    const d = calls.find(c => c.method === "DELETE")!;
    expect(d.url).toBe("/api/artifacts/7q3k9mzx2b4t");
  });
});

describe("getToken", () => {
  beforeEach(() => { vi.resetModules(); });

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
