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
});
