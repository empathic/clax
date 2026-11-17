// Every request helper reads a refusal the same way: the daemon's code is
// kept, and an answer that is not Clax's (a proxy's or a CDN's page) is told
// apart from Clax's own refusal.
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, getArtifact, patchArtifact } from "./api";
import { answered, classify, readError, readJson } from "./failure";
import { Refused, browserCall } from "./held";
import { createThread } from "./threads";
import { approveCapabilities } from "./caps/approval";
import { cloneOptions } from "./ui/clone-api";
import { remotesOf } from "./ui/publish-api";

const clax = (status: number, code: string, message = "m") => new Response(JSON.stringify({ error: { code, message } }), { status, headers: { "content-type": "application/json" } });
const cdn = () => new Response("error code: 1016", { status: 530, headers: { "content-type": "text/plain" } });
const page = () => new Response("<!doctype html><title>Gone</title>", { status: 200, headers: { "content-type": "text/html" } });
const stub = (r: () => Response) => vi.stubGlobal("fetch", vi.fn(async () => r()));
const thrown = async (p: Promise<unknown>) => p.then(() => { throw new Error("resolved"); }, (e: unknown) => e);

afterEach(() => { vi.unstubAllGlobals(); });

describe("readError and classify", () => {
  it("take an answer that is not Clax's for not_clax", async () => {
    expect(classify(await readError(cdn()), true)).toBe("not_clax");
    expect(classify(await readError(new Response("<html>Bad gateway</html>", { status: 502, headers: { "content-type": "text/html" } })), true)).toBe("not_clax");
    expect(classify(await thrown(readJson(page())), true)).toBe("not_clax");
  });
  it("tell offline from unreachable", () => {
    expect(classify(new TypeError("Failed to fetch"), true)).toBe("unreachable");
    expect(classify(new TypeError("Failed to fetch"), false)).toBe("offline");
    expect(classify(new DOMException("aborted", "AbortError"), true)).toBe("unreachable");
  });
  it("read Clax's refusals by status and code, keeping both and the message without the status", async () => {
    const e = await readError(clax(403, "spectator", "you are watching"));
    expect([e.status, e.code, e.reason, e.clax]).toEqual([403, "spectator", "you are watching", true]);
    expect(classify(e, true)).toBe("refused");
    expect(classify(await readError(clax(401, "sign_in_required")), true)).toBe("signed_out");
    expect(classify(await readError(clax(401, "guest_name_required")), true)).toBe("refused");
    expect(classify(await readError(clax(404, "not_found")), true)).toBe("gone");
    expect(classify(await readError(clax(429, "rate_limited")), true)).toBe("limited");
    expect(classify(await readError(clax(500, "internal")), true)).toBe("server");
  });
});

describe("what an answer says of reaching Clax", () => {
  const html = (status: number) => new Response("<html><body>Bad gateway</body></html>", { status, headers: { "content-type": "text/html" } });
  it("takes a CDN's page, a proxy's HTML 502 or 504, and an HTML page where the API answers JSON for no Clax", async () => {
    expect(await answered(cdn(), "/api/artifacts/a1/threads")).toBe("not_clax");
    expect(await answered(html(502), "/api/artifacts")).toBe("not_clax");
    expect(await answered(html(504), "/api/viewers/me")).toBe("not_clax");
    expect(await answered(page(), "/api/artifacts")).toBe("not_clax");
  });
  it("takes an empty refusal (a local daemon's route it does not have) for an ordinary refusal of Clax's, never for Clax gone", async () => {
    expect(await answered(new Response(null, { status: 404 }), "/api/hosted/me")).toBeNull();
    expect(await answered(new Response("", { status: 405 }), "/api/artifacts")).toBeNull();
    const e = await readError(new Response(null, { status: 404 }));
    expect([e.clax, e.code, classify(e, true)]).toEqual([true, null, "refused"]);
  });
  it("judges no file a version serves, which may be an HTML page", async () => {
    expect(await answered(page(), "/api/artifacts/a1/versions/2/files/index.html")).toBeUndefined();
    expect(await answered(new Response("<p>x</p>", { headers: { "content-type": "text/html" } }), "/api/artifacts/a1/files/about.html")).toBeUndefined();
  });
  it("takes Clax's JSON, and a 204, for Clax answering", async () => {
    expect(await answered(clax(403, "forbidden"), "/api/token")).toBeNull();
    expect(await answered(new Response(null, { status: 204 }), "/api/viewers/me/seen")).toBeNull();
  });
});

describe("every request helper", () => {
  it("keeps the daemon's code on the artifact calls", async () => {
    stub(() => clax(409, "conflict", "artifact is at version 3"));
    const e = await thrown(getArtifact("a1")) as ApiError;
    expect([e.status, e.code]).toEqual([409, "conflict"]);
    stub(() => clax(403, "forbidden"));
    expect((await thrown(patchArtifact("a1", { title: "t" }, "tok")) as ApiError).code).toBe("forbidden");
  });
  it("takes a CDN's page or a 200 HTML page for an answer that is not Clax's", async () => {
    for (const r of [cdn, page]) {
      stub(r);
      for (const p of [() => getArtifact("a1"), () => createThread("a1", { anchor: { kind: "artifact", file: "index.html" } as never, body: "b", version: 1, clip: null }), () => cloneOptions("a1"), () => remotesOf("a1")]) {
        const e = await thrown(p());
        expect(e).toBeInstanceOf(ApiError);
        expect(classify(e, true)).toBe("not_clax");
      }
    }
    stub(cdn);
    expect(classify(await thrown(approveCapabilities("a1", 1, "x")), true)).toBe("not_clax");
  });
  it("reads a browser call's refusal as the same error, with its code", async () => {
    stub(() => clax(403, "forbidden", "owner only"));
    const e = await thrown(browserCall("/api/confirmations")) as Refused;
    expect(e).toBeInstanceOf(Refused);
    expect(e).toBeInstanceOf(ApiError);
    expect([e.status, e.code, e.message]).toEqual([403, "forbidden", "owner only"]);
    stub(cdn);
    expect(classify(await thrown(browserCall("/api/confirmations")), true)).toBe("not_clax");
  });
});
