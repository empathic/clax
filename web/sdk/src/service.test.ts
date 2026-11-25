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

  it("mint sends profile", async () => {
    const bodies: unknown[] = [];
    const client = new ServiceClient({
      instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
      fetch: async (_url, init) => {
        bodies.push(JSON.parse(new TextDecoder().decode(new Uint8Array(init.body as Uint8Array))));
        return new Response(JSON.stringify({ token: "cxr_a", expires_at: "2026-10-27T12:15:00Z" }), { status: 200 });
      },
    });
    const req = { reporter_id: "emp-7", origin: "https://app.example", public_key: "AAAA" };
    const profile = { name: "Alex", email: "ahk@example.com", github: "ahk", avatar_url: "https://avatars.example/ahk.png" };
    await client.mintReporterToken(req, profile);
    await client.mintReporterToken({ ...req, profile: { name: "In the request" } });
    await client.mintReporterToken({ ...req, profile: { name: "In the request" } }, { name: "Given" });
    expect(bodies).toEqual([
      { ...req, profile },
      { ...req, profile: { name: "In the request" } },
      { ...req, profile: { name: "Given" } },
    ]);
  });

  it("retries limit_reached after its backoff, signing the retry anew", async () => {
    const signatures: string[] = [];
    const slept: number[] = [];
    const answers = [
      new Response(JSON.stringify({ error: { code: "limit_reached", message: "full" } }), { status: 429 }),
      new Response(JSON.stringify({ token: "cxr_a", expires_at: "2026-10-27T12:15:00Z" }), { status: 200 }),
    ];
    const client = new ServiceClient({
      instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
      sleep: async ms => { slept.push(ms); }, random: () => 0,
      fetch: async (_url, init) => { signatures.push((init.headers as Record<string, string>)["clax-signature"]); return answers.shift()!; },
    });
    expect(await client.mintReporterToken({ reporter_id: "r", origin: "https://app.example", public_key: "AAAA" })).toEqual({ token: "cxr_a", expires_at: "2026-10-27T12:15:00Z" });
    expect(slept).toEqual([1000]);
    expect(new Set(signatures).size).toBe(2);
  });

  it("refuses an instance that is not an origin", () => {
    expect(() => new ServiceClient({ instance: "https://apps.example.com/clax", queue: "q1", token: "cxq_grant", privateKey: PKCS8 }))
      .toThrow(/^ServiceClient: instance must be an origin/);
  });

  it("gives the instance's error", async () => {
    const client = new ServiceClient({
      instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
      fetch: async () => new Response(JSON.stringify({ error: { code: "wrong_origin", message: "not a queue origin" } }), { status: 403 }),
    });
    await expect(client.mintReporterToken({ reporter_id: "r", origin: "https://evil.example", public_key: "AAAA" }))
      .rejects.toMatchObject({ code: "wrong_origin", status: 403 });
  });

  describe("attachPreview", () => {
    const TICKET = "cxf_q1.01JB9T00000000000000000001.2.c2VjcmV0LXNlY3JldC1zZWNyZXQtc2VjcmV0LXNlY3JldA";
    const attached = { report: { id: "01JB9T00000000000000000001" }, verify_url: "https://app-pr-538.fly.dev/orders/7", changed: true };

    it("sends the ticket alone, unsigned, to its fix's route", async () => {
      const sent: { url: string; init: RequestInit }[] = [];
      const client = new ServiceClient({
        instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
        fetch: async (url, init) => { sent.push({ url, init }); return new Response(JSON.stringify(attached), { status: 200 }); },
      });
      expect(await client.attachPreview({ ticket: ` ${TICKET}\n`, origin: "https://app-pr-538.fly.dev", path: "/orders/7" })).toEqual(attached);
      const [{ url, init }] = sent;
      expect(url).toBe("https://clax.example/api/queues/q1/reports/01JB9T00000000000000000001/fixes/2/preview");
      expect(init.method).toBe("POST");
      expect(init.credentials).toBe("omit");
      const headers = init.headers as Record<string, string>;
      expect(headers).toEqual({ authorization: `Clax-Fix ${TICKET}`, "content-type": "application/json" });
      expect(JSON.parse(init.body as string)).toEqual({ origin: "https://app-pr-538.fly.dev", path: "/orders/7" });
    });

    it("leaves the path out when none is given", async () => {
      const bodies: unknown[] = [];
      const client = new ServiceClient({
        instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
        fetch: async (_url, init) => { bodies.push(JSON.parse(init.body as string)); return new Response(JSON.stringify(attached), { status: 200 }); },
      });
      await client.attachPreview({ ticket: TICKET, origin: "https://app-pr-538.fly.dev" });
      expect(bodies).toEqual([{ origin: "https://app-pr-538.fly.dev" }]);
    });

    it("refuses what is no ticket, or another queue's, before sending", async () => {
      let calls = 0;
      const client = new ServiceClient({
        instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
        fetch: async () => { calls++; return new Response("{}", { status: 200 }); },
      });
      for (const ticket of ["cxq_grant", "cxf_q1.t1.0.s", "cxf_q1.t1.s", "cxf_q1/x.t1.1.s", ""]) {
        await expect(client.attachPreview({ ticket, origin: "https://app-pr-1.fly.dev" })).rejects.toThrow(/^attachPreview: ticket is not a fix ticket/);
      }
      await expect(client.attachPreview({ ticket: TICKET.replace("cxf_q1.", "cxf_q2."), origin: "https://app-pr-1.fly.dev" }))
        .rejects.toThrow("attachPreview: the ticket is of queue q2, not q1");
      expect(calls).toBe(0);
    });

    it("gives the instance's refusal, and retries busy after its backoff", async () => {
      const slept: number[] = [];
      const answers = [
        new Response(JSON.stringify({ error: { code: "busy", message: "busy" } }), { status: 503 }),
        new Response(JSON.stringify({ error: { code: "wrong_origin", message: "not a preview" } }), { status: 403 }),
        new Response(JSON.stringify({ error: { code: "token_expired", message: "verified" } }), { status: 401 }),
      ];
      const client = new ServiceClient({
        instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
        sleep: async ms => { slept.push(ms); }, random: () => 0,
        fetch: async () => answers.shift()!,
      });
      await expect(client.attachPreview({ ticket: TICKET, origin: "https://app.example" })).rejects.toMatchObject({ code: "wrong_origin", status: 403 });
      expect(slept).toEqual([1000]);
      await expect(client.attachPreview({ ticket: TICKET, origin: "https://app-pr-538.fly.dev" })).rejects.toMatchObject({ code: "token_expired", status: 401 });
    });

    it("says network when no answer comes", async () => {
      const client = new ServiceClient({
        instance: "https://clax.example", queue: "q1", token: "cxq_grant", privateKey: PKCS8,
        fetch: async () => { throw new Error("offline"); },
      });
      await expect(client.attachPreview({ ticket: TICKET, origin: "https://app-pr-538.fly.dev" })).rejects.toMatchObject({ code: "network", status: 0 });
    });
  });
});
