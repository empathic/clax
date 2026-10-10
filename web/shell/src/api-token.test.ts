import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Each test loads a fresh `api` module, so its token memo starts empty.
beforeEach(() => { vi.resetModules(); });
afterEach(() => { vi.unstubAllGlobals(); });

/** Stubs `fetch` for `/api/token` with `answers`, one per request, in order. */
function tokenAnswers(...answers: Response[]) {
  const f = vi.fn(async (url: string) => {
    if (url !== "/api/token") throw new Error(`unexpected ${url}`);
    const r = answers.shift();
    if (!r) throw new Error("no answer left");
    return r;
  });
  vi.stubGlobal("fetch", f);
  return f;
}

describe("getToken and onToken", () => {
  it("runs the listeners once when a token request succeeds after a failed one", async () => {
    const f = tokenAnswers(new Response("{}", { status: 500 }), new Response(JSON.stringify({ token: "tk" })));
    const { getToken, onToken } = await import("./api");
    const heard = vi.fn();
    onToken(heard);
    expect(await getToken()).toBeNull();
    expect(heard).not.toHaveBeenCalled();
    expect(await getToken()).toBe("tk");
    expect(heard).toHaveBeenCalledTimes(1);
    // The token is kept: no further request, no further call.
    expect(await getToken()).toBe("tk");
    expect(f).toHaveBeenCalledTimes(2);
    expect(heard).toHaveBeenCalledTimes(1);
  });

  it("runs no listener for a token served at once, or for a refusal", async () => {
    tokenAnswers(new Response(JSON.stringify({ token: "tk" })));
    const first = await import("./api");
    const heard = vi.fn();
    first.onToken(heard);
    expect(await first.getToken()).toBe("tk");
    expect(heard).not.toHaveBeenCalled();

    vi.resetModules();
    const f = tokenAnswers(new Response(JSON.stringify({ error: { code: "forbidden" } }), { status: 403 }));
    const lan = await import("./api");
    const lanHeard = vi.fn();
    lan.onToken(lanHeard);
    expect(await lan.getToken()).toBeNull();
    // A refusal is kept as the answer, not retried.
    expect(await lan.getToken()).toBeNull();
    expect(f).toHaveBeenCalledTimes(1);
    expect(lanHeard).not.toHaveBeenCalled();
  });
});
