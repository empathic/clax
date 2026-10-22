import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_BYTES, SVG_MAX_BYTES, limitFor } from "../src/caps/asset-checks";
import { assetsError, dbError, originClient } from "../src/caps/origin-client";

const SITE = "http://7q3k9mzx2b4t.localhost:7480";
const AID = "7q3k9mzx2b4t";
const ASSET = { id: "01J9ZZZZZZZZZZZZZZZZZZZZZZ", content_type: "image/png", size: 3, created_at: "2026-09-29T10:00:00.000Z", url: "/_clax/blob/01J9ZZZZZZZZZZZZZZZZZZZZZZ" };
const doc = (path: string, version: number) => ({ path, collection: path.split("/")[0], id: path.split("/").at(-1), data: { v: version }, version, updated_at: "x" });
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

type Sent = { url: string; method: string; body: unknown; init: RequestInit };

function stub(answer: (s: Sent) => Response | Promise<Response>) {
  const sent: Sent[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const s = { url, method: init.method ?? "GET", body: typeof init.body === "string" ? JSON.parse(init.body) : init.body, init };
    sent.push(s);
    return answer(s);
  }));
  return sent;
}

/** A blob of `n` bytes that claims its size without allocating it. */
const sized = (n: number) => ({ size: n, type: "", __proto__: Blob.prototype }) as unknown as Blob;

describe("the origin client", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("calls the site's own /_clax/api/ with its cookie alone, and never names an artifact ID", async () => {
    const sent = stub(s => {
      if (s.url.endsWith("/whoami")) return json({ kind: "visitor", id: "v_00000000000000000000aa", level: "public", can_edit: false });
      if (s.url.endsWith("/artifact")) return json({ artifact: { id: AID, title: "Board", current_version: 1, capabilities: { db: {} } } });
      if (s.url.includes("docs:acquire")) return json({ acquired: true, version: 1, expires_at: "t", holder: "me" });
      if (s.url.includes("/docs?")) return json({ docs: [doc("tasks/t1", 1)], next_cursor: null });
      if (s.url.endsWith("/assets") && s.method === "GET") return json({ assets: [ASSET] });
      if (s.url.endsWith("/assets")) return json({ asset: ASSET, url: ASSET.url }, 201);
      if (s.method === "DELETE" && s.url.includes("/assets/")) return new Response(null, { status: 204 });
      return json({ doc: doc("tasks/t1", 1), created: true, deleted: true });
    });
    const c = originClient(new URL(`${SITE}/_clax/v/3/page.html`));
    expect(await c.call("db", "get", ["tasks/t1"])).toEqual({ path: "tasks/t1", id: "t1", data: { v: 1 }, version: 1 });
    await c.call("db", "set", ["tasks/t1", { a: 1 }]);
    await c.call("db", "update", ["tasks/t1", { a: 2 }]);
    await c.call("db", "delete", ["tasks/t1"]);
    await c.call("db", "query", [{ collection: "tasks", where: [["a", "==", 1]], orderBy: null, desc: false, limit: null }]);
    expect(await c.call("db", "acquire", ["locks/l1", { holder: "me", ttlMs: 50 }])).toEqual({ acquired: true, version: 1, expiresAt: "t", holder: "me" });
    await c.call("assets", "upload", [{ blob: new Blob(["abc"]), type: "image/png" }]);
    await c.call("assets", "list", []);
    expect(await c.call("assets", "delete", [ASSET.id])).toEqual({ deleted: true });
    expect(await c.call("user", "whoami", [])).toMatchObject({ kind: "visitor" });
    expect(await c.call("artifact", "get", [])).toMatchObject({ id: AID });
    expect(sent.map(s => [s.method, s.url.slice(SITE.length)])).toEqual([
      ["GET", "/_clax/api/docs/tasks/t1"],
      ["PUT", "/_clax/api/docs/tasks/t1"],
      ["PATCH", "/_clax/api/docs/tasks/t1"],
      ["DELETE", "/_clax/api/docs/tasks/t1?lww=true"],
      ["GET", `/_clax/api/docs?collection=tasks&where=${encodeURIComponent('[["a","==",1]]')}&limit=1000`],
      ["POST", "/_clax/api/docs:acquire"],
      ["POST", "/_clax/api/assets"],
      ["GET", "/_clax/api/assets"],
      ["DELETE", `/_clax/api/assets/${ASSET.id}`],
      ["GET", "/_clax/api/whoami"],
      ["GET", "/_clax/api/artifact"],
    ]);
    for (const s of sent) {
      // Only the site's cookie says who calls: no token, no Clax header.
      expect(s.init.credentials).toBe("same-origin");
      const headers = new Headers(s.init.headers);
      expect(headers.has("authorization")).toBe(false);
      expect([...headers.keys()].filter(k => k.startsWith("x-clax-"))).toEqual([]);
      // No path, query or body names an artifact: the host alone does.
      expect(s.url.slice(SITE.length)).not.toContain(AID);
      expect(s.url).not.toContain("/api/artifacts/");
      expect(typeof s.body === "object" && !(s.body instanceof FormData) ? JSON.stringify(s.body) : "").not.toContain(AID);
    }
    expect(sent[1].body).toEqual({ data: { a: 1 }, lww: true });
    // A lease's length is floored and clamped to the daemon's bounds.
    expect(sent[5].body).toEqual({ path: "locks/l1", holder: "me", ttl_ms: 1000 });
  });

  it("sends a docs read's last ETag back, and takes the site's 304 as that answer", async () => {
    let seq = 1;
    const sent = stub(s => {
      const tag = `W/"${seq}-x"`;
      if (s.url.includes("/docs/") && new Headers(s.init.headers).get("if-none-match") === tag) return new Response(null, { status: 304, headers: { etag: tag } });
      if (s.url.endsWith("/missing")) return new Response(JSON.stringify({ error: { code: "not_found" } }), { status: 404, headers: { etag: tag } });
      return new Response(JSON.stringify({ doc: doc("tasks/t1", seq) }), { status: 200, headers: { etag: tag } });
    });
    const c = originClient(new URL(SITE));
    const first = await c.call("db", "get", ["tasks/t1"]);
    const again = await c.call("db", "get", ["tasks/t1"]);
    expect(again).toEqual(first);
    expect(sent.map(s => new Headers(s.init.headers).get("if-none-match"))).toEqual([null, 'W/"1-x"']);
    expect(sent.every(s => s.init.cache === "no-store")).toBe(true);
    // A document that is not there stays not there on a 304.
    expect(await c.call("db", "get", ["tasks/missing"])).toBeNull();
    expect(await c.call("db", "get", ["tasks/missing"])).toBeNull();
    // Changed since: a fresh answer.
    seq = 2;
    expect(await c.call("db", "get", ["tasks/t1"])).toMatchObject({ version: 2 });
    // Writes send no validator.
    await c.call("db", "set", ["tasks/t1", {}]);
    expect(new Headers(sent.at(-1)!.init.headers).get("if-none-match")).toBeNull();
  });

  it("opens the room's socket on the site, on the matching WebSocket scheme", () => {
    const urls: string[] = [];
    vi.stubGlobal("WebSocket", function socket(u: string) { urls.push(u); });
    originClient(new URL(SITE)).socket("room?peer=0123456789abcdef");
    originClient(new URL("https://7q3k9mzx2b4t.clax.run/x")).socket("room?peer=0123456789abcdef");
    expect(urls).toEqual([`ws://7q3k9mzx2b4t.localhost:7480/_clax/api/room?peer=0123456789abcdef`, "wss://7q3k9mzx2b4t.clax.run/_clax/api/room?peer=0123456789abcdef"]);
  });

  it("refuses a document path that could leave the documents before any request", async () => {
    const sent = stub(() => json({}));
    const c = originClient(new URL(SITE));
    for (const bad of ["../artifact", "tasks/..", "tasks//t1", "./x", "", 7]) {
      await expect(c.call("db", "get", [bad])).rejects.toMatchObject({ code: "invalid_argument" });
      await expect(c.call("db", "acquire", [bad, { holder: "h" }])).rejects.toMatchObject({ code: "invalid_argument" });
    }
    // A segment's own characters are escaped, never read as a path.
    await c.call("db", "get", ["a b/c?d"]).catch(() => {});
    expect(sent.map(s => s.url.slice(SITE.length))).toEqual(["/_clax/api/docs/a%20b/c%3Fd"]);
  });

  it("pages an unordered, unlimited query, and asks an ordered one for its limit", async () => {
    let page = 0;
    const sent = stub(() => json(page++ === 0 ? { docs: [doc("t/a", 1)], next_cursor: "c1" } : { docs: [doc("t/b", 1)], next_cursor: null }));
    const c = originClient(new URL(SITE));
    const all = (await c.call("db", "query", [{ collection: "t", where: [], orderBy: null, desc: false, limit: null }])) as { path: string }[];
    expect(all.map(d => d.path)).toEqual(["t/a", "t/b"]);
    expect(new URL(sent[1].url).searchParams.get("cursor")).toBe("c1");
    sent.length = 0;
    page = 1;
    await c.call("db", "query", [{ collection: "t", where: [], orderBy: "n", desc: true, limit: 5 }]);
    expect(Object.fromEntries(new URL(sent[0].url).searchParams)).toEqual({ collection: "t", order_by: "n", direction: "desc", limit: "5" });
  });

  it("maps the site's db errors to the contract's codes", async () => {
    expect(dbError(404, { code: "not_found" }, true).code).toBe("invalid_argument");
    expect(dbError(400, { code: "quota_exceeded" }, true).code).toBe("quota_exceeded");
    expect(dbError(400, { code: "invalid_argument" }, false).code).toBe("invalid_argument");
    expect(dbError(403, { code: "forbidden_origin" }, true).code).toBe("invalid_argument");
    expect(dbError(500, {}, false).code).toBe("unavailable");
    expect(dbError(401, { code: "site_session_required" }, false).code).toBe("unavailable");
    expect(dbError(400, { code: "resource_exhausted" }, true).code).toBe("resource_exhausted");
    expect(dbError(429, { code: "rate_limited" }, true).code).toBe("resource_exhausted");
    expect(dbError(408, {}, false).code).toBe("unavailable");
    expect(dbError(403, { code: "not_declared" }, false).code).toBe("revoked");
    expect(dbError(403, { code: "spectator" }, true).code).toBe("unavailable");
    expect(dbError(413, {}, true).code).toBe("invalid_argument");
    const c = originClient(new URL(SITE));
    stub(s => (s.method === "GET" ? json({ error: { code: "not_found" } }, 404) : json({ error: { code: "not_found", message: "m" } }, 404)));
    // A document that is not there (or not readable) reads as null; a write there is refused.
    expect(await c.call("db", "get", ["tasks/t1"])).toBeNull();
    await expect(c.call("db", "set", ["tasks/t1", {}])).rejects.toMatchObject({ code: "invalid_argument", name: "CapabilityError" });
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("offline"); }));
    await expect(c.call("db", "get", ["tasks/t1"])).rejects.toMatchObject({ code: "unavailable" });
    await expect(c.call("user", "whoami", [])).rejects.toMatchObject({ code: "unavailable" });
    await expect(c.call("db", "frobnicate", [])).rejects.toMatchObject({ code: "capability_removed" });
  });

  it("maps the site's asset errors to the contract's codes", async () => {
    for (const [status, code, want] of [[400, "unsupported_type", "unsupported_type"], [400, "asset_too_large", "too_large"], [413, "body_too_large", "too_large"], [404, "not_found", "quota_or_state"], [401, "unauthorized", "upstream_auth"], [403, "forbidden_origin", "upstream_auth"], [400, "missing_file", "invalid_request"], [500, "internal", "upstream_error"]] as const) {
      expect(assetsError(status, { code, message: "m" }).code).toBe(want);
      stub(() => json({ error: { code, message: "m" } }, status));
      await expect(originClient(new URL(SITE)).call("assets", "upload", [{ blob: new Blob(["a"]), type: "image/png" }])).rejects.toMatchObject({ code: want });
    }
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("offline"); }));
    await expect(originClient(new URL(SITE)).call("assets", "list", [])).rejects.toMatchObject({ code: "store_unavailable" });
  });

  it("checks an upload's type, size, and text before any request", async () => {
    const sent = stub(() => json({}, 500));
    const c = originClient(new URL(SITE));
    const refused = async (arg: unknown, code: string) => { await expect(c.call("assets", "upload", [arg])).rejects.toMatchObject({ code }); };
    await refused(undefined, "invalid_request");
    await refused({ blob: "x" }, "invalid_request");
    await refused({ blob: new Blob([]) }, "invalid_request");
    await refused({ blob: new Blob(["a"]), type: 7 }, "invalid_request");
    await refused({ blob: new Blob(["a"]), type: "" }, "invalid_request");
    for (const t of ["audio/mpeg", "image/bmp", "image/PNG", "application/javascript", "text/html", "text/csv; charset=utf-8", " image/png"]) {
      await refused({ blob: new Blob(["a"]), type: t }, "unsupported_type");
    }
    await refused({ blob: new Blob(["a"], { type: "application/octet-stream" }) }, "unsupported_type");
    await refused({ blob: sized(MAX_BYTES + 1), type: "image/png" }, "too_large");
    await refused({ blob: sized(SVG_MAX_BYTES + 1), type: "image/svg+xml" }, "too_large");
    await refused({ blob: sized(limitFor("text/css") + 1), type: "text/css" }, "too_large");
    await refused({ blob: new Blob([new Uint8Array([0xff, 0xfe, 0x41, 0x00])]), type: "text/csv" }, "invalid_request");
    await refused({ blob: new Blob(["  <html>hi"]), type: "image/png" }, "unsupported_type");
    await refused({ blob: new Blob(["<script>x</script>"]), type: "text/javascript" }, "unsupported_type");
    await refused({ blob: new Blob(["just text"]), type: "image/svg+xml" }, "unsupported_type");
    await refused({ blob: new Blob([new Uint8Array([0x61, 0x00, 0x62])]), type: "text/css" }, "unsupported_type");
    expect(sent).toEqual([]);
  });

  it("uploads every type in the contract's set, and lists and deletes in its shapes", async () => {
    stub(s => {
      if (s.method === "POST") return json({ asset: { ...ASSET, content_type: ((s.body as FormData).get("file") as Blob).type }, url: ASSET.url }, 201);
      if (s.method === "DELETE") return json({ error: { code: "not_found" } }, 404);
      return json({ assets: [{ ...ASSET, url: undefined }] });
    });
    const c = originClient(new URL(SITE));
    const body: Record<string, string> = { "image/svg+xml": "<?xml version=\"1.0\"?>\n<!-- x --><svg xmlns=\"http://www.w3.org/2000/svg\"/>", "text/markdown": "<!-- stored as given -->\n# hi", "text/plain": "﻿hello" };
    for (const t of ["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml", "video/mp4", "video/webm", "application/pdf", "font/woff2", "font/woff", "font/ttf", "font/otf", "text/csv", "text/markdown", "application/json", "text/plain", "text/css", "text/javascript"]) {
      expect(await c.call("assets", "upload", [{ blob: new Blob([body[t] ?? "abc"]), type: t }])).toEqual({ id: ASSET.id, url: ASSET.url, sizeBytes: 3, contentType: t });
    }
    const l = (await c.call("assets", "list", [])) as { assets: unknown[]; usage: Record<string, number> };
    expect(l.assets).toEqual([{ id: ASSET.id, url: `/_clax/blob/${ASSET.id}`, contentType: "image/png", sizeBytes: 3, createdAt: ASSET.created_at }]);
    expect(l.usage).toMatchObject({ files: 1, bytes: 3, maxFiles: Number.MAX_SAFE_INTEGER });
    expect(await c.call("assets", "delete", [ASSET.id])).toEqual({ deleted: false });
    // Nothing is stored under an ID the store never issues: no request.
    vi.stubGlobal("fetch", vi.fn(async () => { throw new Error("no request"); }));
    expect(await c.call("assets", "delete", ["gone"])).toEqual({ deleted: false });
  });
});
