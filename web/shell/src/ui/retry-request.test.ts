import { afterEach, describe, expect, it, vi } from "vitest";
import { retryCopies } from "./retry-request";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

afterEach(() => { vi.unstubAllGlobals(); });

describe("retryCopies", () => {
  it("posts the link's retry and says nothing once the daemon took it", async () => {
    const fetch = vi.fn(async (url: string, _init?: RequestInit) => (String(url).endsWith("/retry") ? json(200, { retried: 2 }) : json(200, { token: "t" })));
    vi.stubGlobal("fetch", fetch);
    expect(await retryCopies("7q3k9mzx2b4t", "r 1")).toBeNull();
    expect(fetch.mock.calls.map(c => [String(c[0]), c[1]?.method])).toContainEqual(["/api/artifacts/7q3k9mzx2b4t/remotes/r%201/retry", "POST"]);
  });

  it("says the daemon's message for a refusal", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => (String(url).endsWith("/retry") ? json(404, { error: { code: "not_found", message: "no such link" } }) : json(200, { token: "t" }))));
    expect(await retryCopies("7q3k9mzx2b4t", "r1")).toBe("Retry failed: no such link");
  });

  it("says Clax did not answer when the request fails", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("Failed to fetch"); }));
    expect(await retryCopies("7q3k9mzx2b4t", "r1")).toBe("Retry failed: Clax did not answer");
  });
});
