import { afterEach, describe, expect, it, vi } from "vitest";
import { causedFetch, newRequestId, noteEvent } from "./events";

describe("request → event causes", () => {
  afterEach(() => { vi.unstubAllGlobals(); performance.clearMarks(); performance.clearMeasures(); });

  it("mints ids the daemon accepts", () => {
    const id = newRequestId();
    expect(id).toMatch(/^[A-Za-z0-9._-]{1,64}$/);
    expect(newRequestId()).not.toBe(id);
  });

  it("sends its id and joins the event that names it, measuring between", async () => {
    let sent: string | null = null;
    vi.stubGlobal("fetch", vi.fn(async (_u: unknown, init?: RequestInit) => {
      sent = new Headers(init?.headers).get("x-request-id");
      return new Response("{}");
    }));
    await causedFetch("delete artifact", "/api/artifacts/a1", { method: "DELETE", headers: { authorization: "Bearer t" } });
    expect(sent).toMatch(/^p/);
    expect(noteEvent({ type: "artifact_deleted", cause: sent! })).toBe("delete artifact");
    expect(performance.getEntriesByName("clax delete artifact → artifact_deleted", "measure")).toHaveLength(1);
  });

  it("leaves events it did not cause alone", () => {
    expect(noteEvent({ type: "version" })).toBeUndefined();
    expect(noteEvent({ type: "version", cause: "someone-else" })).toBeUndefined();
  });
});
