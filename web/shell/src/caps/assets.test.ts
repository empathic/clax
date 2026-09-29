import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_BYTES, SVG_MAX_BYTES, assetsHandler, limitFor } from "./assets";
import type { CapEnv } from "./host";

const env = { aid: "7q3k9mzx2b4t", token: "tok" } as unknown as CapEnv;
const ASSET = { id: "01J9ZZZZZZZZZZZZZZZZZZZZZZ", artifact_id: "7q3k9mzx2b4t", content_type: "image/png", size: 3, ext: "png", created_at: "2026-09-29T10:00:00.000Z" };

/** A blob of `n` bytes that claims its size without allocating it. */
const sized = (n: number) => ({ size: n, type: "", __proto__: Blob.prototype }) as unknown as Blob;

describe("assets in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("uploads with the token and returns the contract's shape", async () => {
    const box: { sent?: FormData; url?: string } = {};
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      box.sent = init.body as FormData;
      box.url = url;
      expect((init.headers as Record<string, string>).authorization).toBe("Bearer tok");
      return new Response(JSON.stringify({ asset: ASSET, url: `/_blob/${ASSET.id}` }), { status: 201 });
    }));
    const r = await assetsHandler(env, null as never).call("upload", [{ blob: new Blob(["abc"]), type: "image/png" }]);
    expect(r).toEqual({ id: ASSET.id, url: `/_blob/${ASSET.id}`, sizeBytes: 3, contentType: "image/png" });
    expect((box.sent!.get("file") as Blob).type).toBe("image/png");
    expect(box.url).toBe("/api/artifacts/7q3k9mzx2b4t/assets");
  });

  it("maps refusals to the contract's codes", async () => {
    const h = assetsHandler(env, null as never);
    await expect(h.call("upload", [{ blob: new Blob(["a"]), type: "" }])).rejects.toMatchObject({ code: "invalid_request" });
    await expect(h.call("upload", [{ blob: new Blob(["a"]), type: "text/csv; charset=utf-8" }])).rejects.toMatchObject({ code: "unsupported_type" });
    for (const [status, code, want] of [[400, "unsupported_type", "unsupported_type"], [400, "asset_too_large", "too_large"], [413, "body_too_large", "too_large"], [404, "not_found", "quota_or_state"], [401, "unauthorized", "upstream_auth"], [500, "internal", "upstream_error"]] as const) {
      vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code, message: "m" } }), { status })));
      await expect(h.call("upload", [{ blob: new Blob(["a"]), type: "image/png" }])).rejects.toMatchObject({ code: want });
    }
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("offline"); }));
    await expect(h.call("upload", [{ blob: new Blob(["a"]), type: "image/png" }])).rejects.toMatchObject({ code: "store_unavailable" });
  });

  it("checks type, size, and text before any request", async () => {
    const fetchSpy = vi.fn(async () => new Response("{}", { status: 500 }));
    vi.stubGlobal("fetch", fetchSpy);
    const h = assetsHandler(env, null as never);
    const refused = async (arg: unknown, code: string) => { await expect(h.call("upload", [arg])).rejects.toMatchObject({ code }); };
    await refused(undefined, "invalid_request");
    await refused("x", "invalid_request");
    await refused({ blob: "x" }, "invalid_request");
    await refused({ blob: new Blob([]) }, "invalid_request");
    await refused({ blob: new Blob(["a"]), type: 7 }, "invalid_request");
    for (const t of ["audio/mpeg", "image/bmp", "image/PNG", "application/javascript", "text/html", "application/vnd.ms-excel", "video/quicktime", " image/png"]) {
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
    await expect(h.call("frobnicate", [])).rejects.toMatchObject({ code: "capability_removed" });
    expect(fetchSpy).not.toHaveBeenCalled();
    expect([limitFor("image/svg+xml"), limitFor("text/javascript"), limitFor("video/mp4")]).toEqual([2 * 1024 * 1024, 16 * 1024 * 1024, 20 * 1024 * 1024]);
  });

  it("accepts every type in the contract's set", async () => {
    vi.stubGlobal("fetch", vi.fn(async (_u: string, init: RequestInit) => {
      const f = (init.body as FormData).get("file") as Blob;
      return new Response(JSON.stringify({ asset: { ...ASSET, content_type: f.type }, url: `/_blob/${ASSET.id}` }), { status: 201 });
    }));
    const h = assetsHandler(env, null as never);
    const body: Record<string, string> = { "image/svg+xml": "<?xml version=\"1.0\"?>\n<!-- x --><svg xmlns=\"http://www.w3.org/2000/svg\"/>", "text/markdown": "<!-- stored as given -->\n# hi", "text/plain": "﻿hello" };
    for (const t of ["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml", "video/mp4", "video/webm", "application/pdf", "font/woff2", "font/woff", "font/ttf", "font/otf", "text/csv", "text/markdown", "application/json", "text/plain", "text/css", "text/javascript"]) {
      expect(await h.call("upload", [{ blob: new Blob([body[t] ?? "abc"]), type: t }])).toMatchObject({ contentType: t });
    }
  });

  it("lists oldest first with usage and deletes idempotently", async () => {
    const urls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit = {}) => {
      urls.push(_url);
      if (init.method === "DELETE") return new Response(null, { status: 404 });
      return new Response(JSON.stringify({ assets: [ASSET] }));
    }));
    const h = assetsHandler(env, null as never);
    const l = (await h.call("list", [])) as { assets: unknown[]; usage: Record<string, number> };
    expect(l.assets).toEqual([{ id: ASSET.id, url: `/_blob/${ASSET.id}`, contentType: "image/png", sizeBytes: 3, createdAt: ASSET.created_at }]);
    expect(l.usage).toMatchObject({ files: 1, bytes: 3 });
    expect(await h.call("delete", [ASSET.id])).toEqual({ deleted: false });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 204 })));
    expect(await h.call("delete", [ASSET.id])).toEqual({ deleted: true });
    expect(urls).toEqual(["/api/artifacts/7q3k9mzx2b4t/assets", `/api/artifacts/7q3k9mzx2b4t/assets/${ASSET.id}`]);
  });

  it("deletes nothing for IDs that cannot name an asset, and refuses malformed ones", async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchSpy);
    const h = assetsHandler(env, null as never);
    expect(await h.call("delete", ["gone"])).toEqual({ deleted: false });
    for (const bad of [undefined, 7, "", "../x", "a/b", "01J9ZZZZZZZZZZZZZZZZZZZZZZ?x", "x".repeat(65)]) {
      await expect(h.call("delete", [bad])).rejects.toMatchObject({ code: "invalid_request" });
    }
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("answers nothing after dispose", async () => {
    let release: (r: Response) => void = () => {};
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(r => { release = r; })));
    const h = assetsHandler(env, null as never);
    const p = h.call("list", []);
    h.dispose?.();
    release(new Response(JSON.stringify({ assets: [] })));
    await expect(p).rejects.toMatchObject({ code: "unavailable" });
    await expect(h.call("list", [])).rejects.toMatchObject({ code: "unavailable" });
  });
});
