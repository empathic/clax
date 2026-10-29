import { describe, expect, it, vi } from "vitest";
import { ReporterClient, RENEW_BEFORE_S } from "./reporter";
import { canonical, fromBase64url, generateSigner, type KeyedSigner } from "./sign";
import type { ReporterToken } from "./types";

interface Sent { url: URL; method: string; headers: Record<string, string>; body: Uint8Array }

const T0 = Date.parse("2026-10-27T12:00:00Z");
const REPORT = { id: "t1", queue_id: "q1", comments: [] };

/** A fake instance: answers each request with `answer`, and records it. */
function instance(answer: (s: Sent) => Response) {
  const sent: Sent[] = [];
  const fetch = async (input: string, init: RequestInit) => {
    const s: Sent = {
      url: new URL(input), method: init.method ?? "GET", headers: init.headers as Record<string, string>,
      body: init.body ? new Uint8Array(init.body as Uint8Array) : new Uint8Array(),
    };
    sent.push(s);
    return answer(s);
  };
  return { sent, fetch };
}

const json = (status: number, v: unknown) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

/** Whether `s` carries a valid signature by `key` over what it sends. */
async function verifies(key: KeyedSigner, s: Sent): Promise<boolean> {
  const [scheme, token] = s.headers.authorization.split(" ");
  expect(scheme).toBe("Clax-Reporter");
  const m = /^t=(\d+), n=([\w-]{22}), s=([\w-]{86})$/.exec(s.headers["clax-signature"]);
  if (!m) return false;
  const message = await canonical({
    method: s.method, pathAndQuery: s.url.pathname + s.url.search, host: s.url.host, t: Number(m[1]), nonce: m[2], body: s.body, token,
  });
  const pub = await crypto.subtle.importKey("raw", fromBase64url(key.publicKey) as BufferSource, { name: "Ed25519" }, true, ["verify"]);
  return crypto.subtle.verify("Ed25519", pub, fromBase64url(m[3]) as BufferSource, new TextEncoder().encode(message));
}

function setup(answer: (s: Sent) => Response = () => json(200, { reports: [], next: null })) {
  let now = T0;
  const auths: string[] = [];
  let n = 0;
  const auth = async (pk: string): Promise<ReporterToken> => {
    auths.push(pk);
    n += 1;
    return { token: `cxr_${n}`, expires_at: new Date(now + 900_000).toISOString() };
  };
  const inst = instance(answer);
  return {
    inst, auths,
    advance: (ms: number) => { now += ms; },
    make: async () => {
      const signer = await generateSigner();
      return { signer, client: new ReporterClient({ instance: "https://clax.example:8443", queue: "q 1", auth, fetch: inst.fetch, now: () => now, signer }) };
    },
  };
}

describe("ReporterClient", () => {
  it("signs each call with its key and token, at the injected time", async () => {
    const s = setup();
    const { signer, client } = await s.make();
    await client.myReports("c/1");
    const [req] = s.inst.sent;
    expect(req.url.href).toBe("https://clax.example:8443/api/queues/q%201/reporter/reports?cursor=c%2F1");
    expect(req.headers.authorization).toBe("Clax-Reporter cxr_1");
    expect(req.headers["clax-signature"]).toMatch(new RegExp(`^t=${T0 / 1000}, `));
    expect(await verifies(signer, req)).toBe(true);
    expect(s.auths).toEqual([signer.publicKey]);
  });

  it("asks for a new token from 60 s before expiry, not sooner", async () => {
    const s = setup();
    const { client } = await s.make();
    await client.myReports();
    s.advance((900 - RENEW_BEFORE_S) * 1000 - 1);
    await client.myReports();
    expect(s.auths).toHaveLength(1);
    expect(s.inst.sent[1].headers.authorization).toBe("Clax-Reporter cxr_1");
    s.advance(1);
    await client.myReports();
    expect(s.auths).toHaveLength(2);
    expect(s.inst.sent[2].headers.authorization).toBe("Clax-Reporter cxr_2");
  });

  it("shares one auth call among concurrent calls", async () => {
    const s = setup();
    const { client } = await s.make();
    await Promise.all([client.myReports(), client.myReports(), client.myReports()]);
    expect(s.auths).toHaveLength(1);
  });

  it("on a 401 renews once and retries once", async () => {
    let failures = 1;
    const s = setup(() => (failures-- > 0 ? json(401, { error: { code: "token_expired", message: "expired" } }) : json(200, { reports: [], next: null })));
    const { signer, client } = await s.make();
    expect(await client.myReports()).toEqual({ reports: [], next: null });
    expect(s.auths).toHaveLength(2);
    expect(s.inst.sent.map(r => r.headers.authorization)).toEqual(["Clax-Reporter cxr_1", "Clax-Reporter cxr_2"]);
    expect(await verifies(signer, s.inst.sent[1])).toBe(true);
  });

  it("gives up after the one retry with the instance's error", async () => {
    const s = setup(() => json(401, { error: { code: "token_expired", message: "expired" } }));
    const { client } = await s.make();
    await expect(client.myReports()).rejects.toMatchObject({ code: "token_expired", status: 401 });
    expect(s.inst.sent).toHaveLength(2);
    expect(s.auths).toHaveLength(2);
  });

  it("files a report as signed multipart with its clip", async () => {
    const s = setup(() => json(200, { report: REPORT }));
    const { signer, client } = await s.make();
    const clip = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 1, 2]);
    const report = await client.fileReport({
      url: "https://app.example/orders/7", title: "Orders", anchor: { kind: "element", selector: "#total" },
      body: "Total is wrong", metadata: { version: "1.2" }, clip,
    });
    expect(report).toEqual(REPORT);
    const [req] = s.inst.sent;
    expect(req.method).toBe("POST");
    expect(req.url.pathname).toBe("/api/queues/q%201/reporter/reports");
    expect(await verifies(signer, req)).toBe(true);
    const form = await new Response(req.body as BodyInit, { headers: { "content-type": req.headers["content-type"] } }).formData();
    expect(form.has("external_id")).toBe(false);
    expect(form.get("url")).toBe("https://app.example/orders/7");
    expect(form.get("title")).toBe("Orders");
    expect(JSON.parse(form.get("anchor") as string)).toEqual({ kind: "element", selector: "#total" });
    expect(form.get("body")).toBe("Total is wrong");
    expect(JSON.parse(form.get("metadata") as string)).toEqual({ version: "1.2" });
    const file = form.get("clip") as File;
    expect(file.type).toBe("image/png");
    expect(new Uint8Array(await file.arrayBuffer())).toEqual(clip);
  });

  it("fileReport sends context and diagnostics parts", async () => {
    const s = setup(() => json(201, { report: REPORT }));
    const { signer, client } = await s.make();
    const context = { v: 1 as const, captured_at: "2026-10-29T07:12:03.120Z", release: { version: "0.6.0" }, flags: { claxbot: true } };
    const diagnostics = [
      { at: "2026-10-29T07:12:01.000Z", level: "warn" as const, kind: "console" as const, text: "slow" },
      { at: "2026-10-29T07:12:02.000Z", level: "error" as const, kind: "request" as const, text: "GET /x → 502 in 3 ms" },
    ];
    await client.fileReport({ url: "https://app.example/", title: "Home", anchor: null, body: "broken", context, diagnostics });
    const [req] = s.inst.sent;
    expect(await verifies(signer, req)).toBe(true);
    const form = await new Response(req.body as BodyInit, { headers: { "content-type": req.headers["content-type"] } }).formData();
    expect(JSON.parse(form.get("context") as string)).toEqual(context);
    const file = form.get("diagnostics") as File;
    expect(file.name).toBe("diagnostics.jsonl");
    expect((await file.text()).trim().split("\n").map(l => JSON.parse(l))).toEqual(diagnostics);
    // No diagnostics part for none.
    await client.fileReport({ url: "https://app.example/", title: "Home", anchor: null, body: "b", diagnostics: [] });
    const bare = await new Response(s.inst.sent[1].body as BodyInit, { headers: { "content-type": s.inst.sent[1].headers["content-type"] } }).formData();
    expect(bare.has("diagnostics")).toBe(false);
    expect(bare.has("context")).toBe(false);
  });

  it("sends only JSON Clax reads: well-formed strings, finite flags", async () => {
    const s = setup(() => json(201, { report: REPORT }));
    const { client } = await s.make();
    const context = {
      v: 1 as const, captured_at: "2026-10-29T07:12:03.120Z",
      flags: { ok: 1, nan: Number.NaN, inf: Number.POSITIVE_INFINITY, on: true },
      app: { "k\ud83d": ["v\udc00"] },
    };
    await client.fileReport({ url: "u", title: "t", anchor: { s: "\ud83d" }, body: "b", metadata: { m: "\ud83d" }, context });
    const form = await new Response(s.inst.sent[0].body as BodyInit, { headers: { "content-type": s.inst.sent[0].headers["content-type"] } }).formData();
    for (const name of ["context", "anchor", "metadata"]) expect(form.get(name) as string).not.toMatch(/\\u[dD][89a-fA-F]/);
    expect(JSON.parse(form.get("context") as string)).toEqual({
      v: 1, captured_at: "2026-10-29T07:12:03.120Z", flags: { ok: 1, on: true }, app: { "k�": ["v�"] },
    });
    await client.verify("t1", { note: "fine \ud83d", context });
    const body = new TextDecoder().decode(s.inst.sent[1].body);
    expect(body).not.toMatch(/\\u[dD][89a-fA-F]/);
    expect(JSON.parse(body).note).toBe("fine �");
    expect(JSON.parse(body).context.flags).toEqual({ ok: 1, on: true });
  });

  it("verify posts note and context", async () => {
    const s = setup(() => json(200, { report: { ...REPORT, state: "verified" } }));
    const { signer, client } = await s.make();
    const context = { v: 1 as const, captured_at: "2026-10-29T08:00:00.000Z" };
    expect((await client.verify("t/1", { note: "works now", context })).state).toBe("verified");
    const [req] = s.inst.sent;
    expect(req.method).toBe("POST");
    expect(req.url.pathname).toBe("/api/queues/q%201/reporter/reports/t%2F1/verify");
    expect(req.headers["content-type"]).toBe("application/json");
    expect(JSON.parse(new TextDecoder().decode(req.body))).toEqual({ note: "works now", context });
    expect(await verifies(signer, req)).toBe(true);
    await client.verify("t1");
    expect(JSON.parse(new TextDecoder().decode(s.inst.sent[1].body))).toEqual({});
  });

  it("gives not_fix_proposed when no fix waits", async () => {
    const s = setup(() => json(409, { error: { code: "not_fix_proposed", message: "no fix waits for a verdict" } }));
    const { client } = await s.make();
    await expect(client.verify("t1")).rejects.toMatchObject({ code: "not_fix_proposed", status: 409 });
    await expect(client.reopen("t1", { body: "still" })).rejects.toMatchObject({ code: "not_fix_proposed", status: 409 });
  });

  it("reopen sends multipart body and clip", async () => {
    const s = setup(() => json(200, { report: { ...REPORT, state: "open", reopen_count: 1 } }));
    const { signer, client } = await s.make();
    const clip = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 9]);
    const diagnostics = [{ at: "2026-10-29T09:00:00.000Z", level: "error" as const, kind: "error" as const, text: "boom" }];
    const context = { v: 1 as const, captured_at: "2026-10-29T09:00:01.000Z" };
    const report = await client.reopen("t1", { body: "still broken", clip, diagnostics, context });
    expect(report.reopen_count).toBe(1);
    const [req] = s.inst.sent;
    expect(req.url.pathname).toBe("/api/queues/q%201/reporter/reports/t1/reopen");
    expect(req.headers["content-type"]).toMatch(/^multipart\/form-data; boundary=/);
    expect(await verifies(signer, req)).toBe(true);
    const form = await new Response(req.body as BodyInit, { headers: { "content-type": req.headers["content-type"] } }).formData();
    expect([...form.keys()]).toEqual(["body", "clip", "context", "diagnostics"]);
    expect(form.get("body")).toBe("still broken");
    expect(new Uint8Array(await (form.get("clip") as File).arrayBuffer())).toEqual(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 9]));
    expect(JSON.parse(form.get("context") as string)).toEqual(context);
    expect(JSON.parse((await (form.get("diagnostics") as File).text()).trim())).toEqual(diagnostics[0]);
  });

  it("badge reads waiting", async () => {
    const s = setup(() => json(200, { waiting: 2, report_ids: ["t1", "t2"] }));
    const { signer, client } = await s.make();
    expect(await client.badge()).toEqual({ waiting: 2, report_ids: ["t1", "t2"] });
    expect(s.inst.sent[0].method).toBe("GET");
    expect(s.inst.sent[0].url.pathname).toBe("/api/queues/q%201/reporter/badge");
    expect(await verifies(signer, s.inst.sent[0])).toBe(true);
  });

  it("marks seen, reads diagnostics by capture and a comment's clip", async () => {
    const lines = [{ at: "2026-10-29T09:00:00.000Z", level: "warn", kind: "console", text: "w" }];
    const s = setup(r => {
      if (r.url.pathname.endsWith("/seen")) return new Response(null, { status: 204 });
      if (r.url.pathname.endsWith("/clip")) return new Response(new Uint8Array([7]), { status: 200, headers: { "content-type": "image/png" } });
      return json(200, { lines });
    });
    const { signer, client } = await s.make();
    await client.seen("t1");
    expect(s.inst.sent[0].method).toBe("POST");
    expect(s.inst.sent[0].url.pathname).toBe("/api/queues/q%201/reporter/reports/t1/seen");
    expect(await verifies(signer, s.inst.sent[0])).toBe(true);
    expect(await client.diagnostics("t1", 2)).toEqual(lines);
    expect(s.inst.sent[1].url.pathname + s.inst.sent[1].url.search).toBe("/api/queues/q%201/reporter/reports/t1/diagnostics?capture=2");
    await client.diagnostics("t1");
    expect(s.inst.sent[2].url.search).toBe("");
    const blob = await client.commentClip("t1", "c/9");
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(new Uint8Array([7]));
    expect(s.inst.sent[3].url.pathname).toBe("/api/queues/q%201/reporter/reports/t1/comments/c%2F9/clip");
  });

  it("seen gives the instance's error", async () => {
    const s = setup(() => json(404, { error: { code: "not_found", message: "no such report" } }));
    const { client } = await s.make();
    await expect(client.seen("t9")).rejects.toMatchObject({ code: "not_found", status: 404 });
  });

  it("comments as JSON and reads a clip", async () => {
    const s = setup(r => (r.url.pathname.endsWith("/clip") ? new Response(new Uint8Array([1, 2]), { status: 200, headers: { "content-type": "image/png" } }) : json(200, { report: REPORT })));
    const { signer, client } = await s.make();
    expect(await client.comment("t/1", "thanks")).toEqual(REPORT);
    expect(s.inst.sent[0].url.pathname).toBe("/api/queues/q%201/reporter/reports/t%2F1/comments");
    expect(s.inst.sent[0].headers["content-type"]).toBe("application/json");
    expect(JSON.parse(new TextDecoder().decode(s.inst.sent[0].body))).toEqual({ body: "thanks" });
    expect(await verifies(signer, s.inst.sent[0])).toBe(true);
    const blob = await client.clip("t1");
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(new Uint8Array([1, 2]));
  });

  it("makes its own key when none is given", async () => {
    const s = setup();
    const client = new ReporterClient({
      instance: "https://clax.example", queue: "q1", fetch: s.inst.fetch, now: () => T0,
      auth: async pk => { s.auths.push(pk); return { token: "cxr_x", expires_at: new Date(T0 + 900_000).toISOString() }; },
    });
    await client.myReports();
    expect(s.auths).toEqual([await client.publicKey()]);
    expect(fromBase64url(s.auths[0])).toHaveLength(32);
  });

  it("fails every call with unsupported_browser where WebCrypto lacks Ed25519", async () => {
    const gen = vi.spyOn(crypto.subtle, "generateKey").mockRejectedValue(new DOMException("Unrecognized name.", "NotSupportedError"));
    try {
      const inst = instance(() => json(200, { reports: [], next: null }));
      const client = new ReporterClient({ instance: "https://clax.example", queue: "q1", auth: async () => ({ token: "cxr_1", expires_at: new Date(T0 + 900_000).toISOString() }), fetch: inst.fetch, now: () => T0 });
      for (let i = 0; i < 2; i++) await expect(client.myReports()).rejects.toMatchObject({ name: "ClaxError", code: "unsupported_browser" });
      expect(inst.sent).toHaveLength(0);
    } finally {
      gen.mockRestore();
    }
  });
});
