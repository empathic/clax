import { describe, expect, it, vi } from "vitest";
import { PROOF_HEADER, PROOF_STORAGE_KEY, ownerProof, proofFetch, withProof } from "./owner-proof";

const ORIGIN = "http://localhost:7480";
const NO_CREDENTIAL = () => new Response(JSON.stringify({ error: { code: "no_credential", message: "this needs a credential" } }), { status: 401 });

/** A `fetch` that records each request's URL and headers and answers `answer`. */
function recorder(answer: () => Response = () => new Response("{}")) {
  const seen: { url: string; headers: Headers }[] = [];
  const f = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push({ url: input instanceof Request ? input.url : String(input), headers: new Headers(init?.headers) });
    return answer();
  }) as unknown as typeof fetch;
  return { f, seen };
}

describe("the owner browser session's proof", () => {
  it("is read from the origin's storage, and is none without it", () => {
    expect(ownerProof({ getItem: k => (k === PROOF_STORAGE_KEY ? "cxp_x" : null) })).toBe("cxp_x");
    expect(ownerProof({ getItem: () => null })).toBeNull();
    expect(ownerProof({ getItem: () => "" })).toBeNull();
    expect(ownerProof({ getItem: () => { throw new Error("blocked"); } })).toBeNull();
    expect(ownerProof(null)).toBeNull();
  });

  it("rides every request to the shell's own origin, and no other", async () => {
    const { f, seen } = recorder();
    const wrapped = proofFetch(f, ORIGIN, () => "cxp_x", () => {});
    await wrapped("/api/artifacts");
    await wrapped(`${ORIGIN}/api/token`, { headers: { accept: "application/json" } });
    await wrapped("https://elsewhere.example/api/token");
    await wrapped("http://localhost:7481/api/token");
    await wrapped("http://7q3k9mzx2b4t.localhost:7480/x");
    expect(seen.map(s => s.headers.get(PROOF_HEADER))).toEqual(["cxp_x", "cxp_x", null, null, null]);
    expect(seen[1].headers.get("accept")).toBe("application/json");
  });

  it("reads the proof at each request, so a login in another tab takes effect at once, and adds none without one", async () => {
    const { f, seen } = recorder();
    let proof: string | null = null;
    const wrapped = proofFetch(f, ORIGIN, () => proof, () => {});
    await wrapped("/api/artifacts");
    proof = "cxp_new";
    await wrapped("/api/artifacts");
    expect(seen.map(s => s.headers.get(PROOF_HEADER))).toEqual([null, "cxp_new"]);
  });

  it("shows how to open Clax from the terminal when an API request is refused for want of any credential", async () => {
    const cold = vi.fn();
    const wrapped = proofFetch(recorder(NO_CREDENTIAL).f, ORIGIN, () => "cxp_ended", cold);
    const res = await wrapped("/api/artifacts/7q3k9mzx2b4t");
    expect(res.status).toBe(401);
    // The caller still reads the body.
    expect(((await res.json()) as { error: { code: string } }).error.code).toBe("no_credential");
    expect(cold).toHaveBeenCalledTimes(1);
    // Another refusal is not the cold browser's.
    const other = vi.fn();
    await proofFetch(recorder(() => new Response(JSON.stringify({ error: { code: "unauthorized" } }), { status: 401 })).f, ORIGIN, () => null, other)("/api/token");
    await proofFetch(recorder(() => new Response("{}", { status: 403 })).f, ORIGIN, () => null, other)("/api/token");
    expect(other).not.toHaveBeenCalled();
  });

  it("is added to a realtime route's address", () => {
    expect(withProof("ws://localhost:7480/api/stream", "cxp_a+b")).toBe("ws://localhost:7480/api/stream?owner_proof=cxp_a%2Bb");
    expect(withProof("/api/stream?last_event_id=s%3A1", "cxp_a")).toBe("/api/stream?last_event_id=s%3A1&owner_proof=cxp_a");
    expect(withProof("/api/stream", null)).toBe("/api/stream");
  });
});
