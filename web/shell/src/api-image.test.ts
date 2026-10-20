import { beforeEach, describe, expect, it, vi } from "vitest";
import { apiImageSrc } from "./api-image";

describe("apiImageSrc", () => {
  // jsdom has no URL.createObjectURL; define it so it can be spied on.
  beforeEach(() => { Object.defineProperty(URL, "createObjectURL", { value: () => "blob:x", configurable: true, writable: true }); });
  /** A window whose `fetch` (the app's transport) answers `answer`. */
  const hosted = (answer: () => Promise<Response>) => {
    const fetch = vi.fn(answer);
    return { win: { fetch } as unknown as Window, fetch };
  };

  it("is an object URL of the bytes the transport fetched, one per path", async () => {
    const made: Blob[] = [];
    vi.spyOn(URL, "createObjectURL").mockImplementation(b => { made.push(b as Blob); return `blob:clip-${made.length}`; });
    try {
      const { win, fetch } = hosted(async () => new Response(new Uint8Array([137, 80, 78, 71]), { headers: { "content-type": "image/png" } }));
      expect(await apiImageSrc("/api/artifacts/a/threads/t1/clip", win)).toBe("blob:clip-1");
      expect(await apiImageSrc("/api/artifacts/a/threads/t1/clip", win)).toBe("blob:clip-1");
      expect(fetch).toHaveBeenCalledTimes(1);
      expect(fetch).toHaveBeenCalledWith("/api/artifacts/a/threads/t1/clip");
      expect(made[0].type).toBe("image/png");
      expect(new Uint8Array(await made[0].arrayBuffer())).toEqual(new Uint8Array([137, 80, 78, 71]));
    } finally {
      vi.restoreAllMocks();
    }
  });

  it("rejects on a failed answer, and asks again next time", async () => {
    let status = 404;
    const { win, fetch } = hosted(async () => new Response("{}", { status }));
    await expect(apiImageSrc("/api/artifacts/a/threads/t2/clip", win)).rejects.toThrow(/404/);
    status = 200;
    expect(await apiImageSrc("/api/artifacts/a/threads/t2/clip", win)).toBe("blob:x");
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("keeps the most recent clips and revokes each one it lets go", async () => {
    let made = 0;
    vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:k${++made}`);
    const revoked: string[] = [];
    Object.defineProperty(URL, "revokeObjectURL", { value: (u: string) => revoked.push(u), configurable: true, writable: true });
    try {
      // A module of its own: the other cases' clips are not among its kept.
      vi.resetModules();
      const { apiImageSrc: src, CLIPS_KEPT } = await import("./api-image");
      const { win, fetch } = hosted(async () => new Response(new Uint8Array([1])));
      const path = (n: number) => `/api/artifacts/k/threads/t${n}/clip`;
      for (let n = 1; n <= CLIPS_KEPT; n++) await src(path(n), win);
      expect(revoked).toEqual([]);
      // The first is used again, so the second is now the oldest.
      expect(await src(path(1), win)).toBe("blob:k1");
      await src(path(CLIPS_KEPT + 1), win);
      await vi.waitFor(() => expect(revoked).toEqual(["blob:k2"]));
      await src(path(CLIPS_KEPT + 2), win);
      await vi.waitFor(() => expect(revoked).toEqual(["blob:k2", "blob:k3"]));
      // An evicted clip is fetched again when asked for.
      const calls = fetch.mock.calls.length;
      expect(await src(path(2), win)).toBe(`blob:k${CLIPS_KEPT + 3}`);
      expect(fetch).toHaveBeenCalledTimes(calls + 1);
      expect(await src(path(1), win)).toBe("blob:k1");
    } finally {
      vi.restoreAllMocks();
    }
  });
});
