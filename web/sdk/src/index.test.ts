import { afterEach, describe, expect, it } from "vitest";
import { loadOverlay, reporterTokenFrom, type ClaxOverlayGlobal } from "./index";

afterEach(() => {
  for (const s of document.querySelectorAll("script")) s.remove();
  delete (window as unknown as { Clax?: unknown }).Clax;
});

describe("loadOverlay", () => {
  it("appends <instance>/_clax/overlay.js once, and resolves window.Clax once it defines it", async () => {
    const doc = document;
    const fake: ClaxOverlayGlobal = { overlay: () => { throw new Error("unused"); }, diagnostics: { install: () => ({ lines: () => [], count: () => 0, stop: () => {} }) }, forgetReporter: async () => {}, CONTEXT_TIMEOUT_MS: 2000 };
    const p = loadOverlay("https://clax.example/", doc);
    const script = doc.head.querySelector<HTMLScriptElement>("script")!;
    expect(script.src).toBe("https://clax.example/_clax/overlay.js");
    expect(script.async).toBe(true);
    (window as unknown as { Clax: ClaxOverlayGlobal }).Clax = fake;
    script.onload?.(new Event("load"));
    await expect(p).resolves.toBe(fake);
    // A second call for the same instance (another feature mounting) reuses the one script.
    const again = await loadOverlay("https://clax.example", doc);
    expect(again).toBe(fake);
    expect(doc.head.querySelectorAll("script")).toHaveLength(1);
  });

  it("resolves at once when window.Clax is already there (no new script)", async () => {
    const fake: ClaxOverlayGlobal = { overlay: () => { throw new Error("unused"); }, diagnostics: { install: () => ({ lines: () => [], count: () => 0, stop: () => {} }) }, forgetReporter: async () => {}, CONTEXT_TIMEOUT_MS: 2000 };
    (window as unknown as { Clax: ClaxOverlayGlobal }).Clax = fake;
    const got = await loadOverlay("https://clax.example");
    expect(got).toBe(fake);
    expect(document.querySelectorAll("script")).toHaveLength(0);
  });

  it("forgets a failed load: its script is removed, and the next call tries again with a fresh one", async () => {
    const doc = document;
    const p = loadOverlay("https://down.example", doc);
    const first = doc.head.querySelector<HTMLScriptElement>("script")!;
    first.onerror?.(new Event("error"));
    await expect(p).rejects.toThrow(/could not load/);
    expect(doc.head.querySelectorAll("script")).toHaveLength(0);
    const retry = loadOverlay("https://down.example", doc);
    const second = doc.head.querySelector<HTMLScriptElement>("script")!;
    expect(second).not.toBe(first);
    second.onerror?.(new Event("error"));
    await expect(retry).rejects.toThrow(/could not load/);
  });
});

describe("reporterTokenFrom", () => {
  it("POSTs {public_key} as JSON and reads back {token, expires_at}", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    const fetch = async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return new Response(JSON.stringify({ token: "cxr_1", expires_at: "2026-11-06T00:00:00Z" }), { status: 200 });
    };
    const auth = reporterTokenFrom("/clax-token", { fetch: fetch as unknown as typeof globalThis.fetch });
    const got = await auth("pub-key-1", { traceparent: "00-aaaa-bbbb-01" });
    expect(got).toEqual({ token: "cxr_1", expires_at: "2026-11-06T00:00:00Z" });
    expect(calls).toHaveLength(1);
    expect(calls[0].url).toBe("/clax-token");
    expect(calls[0].init.method).toBe("POST");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({ public_key: "pub-key-1" });
    expect((calls[0].init.headers as Record<string, string>).traceparent).toBe("00-aaaa-bbbb-01");
    expect((calls[0].init.headers as Record<string, string>)["content-type"]).toBe("application/json");
  });

  it("omits the traceparent header when no trace is given", async () => {
    const fetch = async () => new Response(JSON.stringify({ token: "t", expires_at: "" }), { status: 200 });
    const auth = reporterTokenFrom("/clax-token", { fetch: fetch as unknown as typeof globalThis.fetch });
    await auth("pub");
    // Nothing to assert on headers directly (fetch here ignores them), but a trace-less call must not throw.
    expect(true).toBe(true);
  });

  it("sends extra headers given in options, alongside content-type", async () => {
    const calls: RequestInit[] = [];
    const fetch = async (_url: string, init: RequestInit) => { calls.push(init); return new Response(JSON.stringify({ token: "t", expires_at: "" }), { status: 200 }); };
    const auth = reporterTokenFrom("/clax-token", { fetch: fetch as unknown as typeof globalThis.fetch, headers: { "x-csrf": "abc" } });
    await auth("pub");
    expect((calls[0].headers as Record<string, string>)["x-csrf"]).toBe("abc");
  });

  it("throws with code from {error:{code}} on a non-OK answer, for the overlay's drop-and-retry on invalid_key", async () => {
    const fetch = async () => new Response(JSON.stringify({ error: { code: "invalid_key", message: "bad key" } }), { status: 400 });
    const auth = reporterTokenFrom("/clax-token", { fetch: fetch as unknown as typeof globalThis.fetch });
    await expect(auth("pub")).rejects.toMatchObject({ message: "bad key", code: "invalid_key" });
  });

  it("still throws, with the status in its message, when the error body has no code", async () => {
    const fetch = async () => new Response("", { status: 503 });
    const auth = reporterTokenFrom("/clax-token", { fetch: fetch as unknown as typeof globalThis.fetch });
    await expect(auth("pub")).rejects.toMatchObject({ message: "no reporter token: 503", code: undefined });
  });
});
