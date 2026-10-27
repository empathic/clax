import { describe, expect, it } from "vitest";
import { ServiceClient } from "./service";
import { canonical, fromBase64url } from "./sign";

// The key of `sdk_vector_is_fixed` in crates/clax-core/src/sig.rs, as a
// service grant's `private_key` carries one.
const PKCS8 = "MFMCAQEwBQYDK2VwBCIEIEJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCoSMDIQAhUvjRm3kdJEUyQuFfLqtst8_6e2pe0wCXlg4GmIHbEg";
const PUBLIC = "IVL40Zt5HSRFMkLhXy6rbLfP-ntqXtMAl5YOBpiB2xI";

describe("ServiceClient", () => {
  it("mints a reporter token signed with the grant's key", async () => {
    const sent: { url: string; init: RequestInit }[] = [];
    const client = new ServiceClient({
      instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8, now: () => 1_760_000_000_500,
      fetch: async (url, init) => {
        sent.push({ url, init });
        return new Response(JSON.stringify({ token: "cxr_a", expires_at: "2026-10-27T12:15:00Z" }), { status: 200 });
      },
    });
    const req = { reporter_id: "emp-7", origin: "https://app.example", public_key: "AAAA" };
    expect(await client.mintReporterToken(req)).toEqual({ token: "cxr_a", expires_at: "2026-10-27T12:15:00Z" });
    const [{ url, init }] = sent;
    expect(url).toBe("https://clax.example/api/queues/q1/reporter-tokens");
    const headers = init.headers as Record<string, string>;
    expect(headers.authorization).toBe("Clax-Queue cxq_grant");
    expect(headers["content-type"]).toBe("application/json");
    const body = new Uint8Array(init.body as Uint8Array);
    expect(JSON.parse(new TextDecoder().decode(body))).toEqual(req);
    const m = /^t=1760000000, n=([\w-]{22}), s=([\w-]{86})$/.exec(headers["clax-signature"]);
    expect(m).not.toBeNull();
    const message = await canonical({
      method: "POST", pathAndQuery: "/api/queues/q1/reporter-tokens", host: "clax.example", t: 1_760_000_000, nonce: m![1], body, token: "cxq_grant",
    });
    const pub = await crypto.subtle.importKey("raw", fromBase64url(PUBLIC) as BufferSource, { name: "Ed25519" }, true, ["verify"]);
    expect(await crypto.subtle.verify("Ed25519", pub, fromBase64url(m![2]) as BufferSource, new TextEncoder().encode(message))).toBe(true);
  });

  it("gives the instance's error", async () => {
    const client = new ServiceClient({
      instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
      fetch: async () => new Response(JSON.stringify({ error: "wrong_origin", message: "not a queue origin" }), { status: 403 }),
    });
    await expect(client.mintReporterToken({ reporter_id: "r", origin: "https://evil.example", public_key: "AAAA" }))
      .rejects.toMatchObject({ code: "wrong_origin", status: 403 });
  });
});
