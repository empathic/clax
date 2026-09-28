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

describe("Gallery", () => {
  beforeEach(() => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [
        { id: "7q3k9mzx2b4t", title: "Pinned one", description: "d", icon: "chart", pinned: true, current_version: 3, updated_at: "2026-09-28T11:00:00Z" },
        { id: "aaaaaaaaaaaa", title: "Other", description: null, icon: null, pinned: false, current_version: 1, updated_at: "2026-09-27T11:00:00Z" },
      ] }));
      if (url.endsWith("/api/token")) return new Response(JSON.stringify({ token: "t" }));
      return new Response("{}", { status: 404 });
    }));
  });

  it("renders cards with title, version, and link, pinned first", async () => {
    const { default: Gallery } = await import("./gallery");
    const root = document.createElement("div");
    render(<Gallery />, root);
    await new Promise(r => setTimeout(r, 50));
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
    const { default: Gallery } = await import("./gallery");
    const root = document.createElement("div");
    render(<Gallery />, root);
    await new Promise(r => setTimeout(r, 50));
    expect(root.textContent).toContain("No artifacts yet");
    expect(root.textContent).toContain("artifax publish");
  });
});
