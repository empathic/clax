import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { approveCapabilities, barText, pendingItems } from "./approval";

const SHA = "a".repeat(64);

describe("the approval bar's copy", () => {
  it("the bar names the version and what it asks for", () => {
    expect(barText({ version: 4, capabilities: ["net", "sample"], hosts: ["api.example.com"], declaration_sha256: SHA }))
      .toBe("v4 asks for: the internet (api.example.com), Claude");
    // New hosts on an approved net, and a first declaration.
    expect(barText({ version: 2, capabilities: [], hosts: ["b.example.com"], declaration_sha256: SHA })).toBe("v2 asks for: the internet (b.example.com)");
    expect(barText({ version: 1, capabilities: ["shell", "account"], hosts: [], declaration_sha256: SHA })).toBe("v1 asks for: Clax's controls, sign-in");
  });

  it("each item says what approving lets the page do", () => {
    expect(pendingItems({ version: 1, capabilities: ["sample"], hosts: [], declaration_sha256: SHA })[0].detail)
      .toBe("Ask Claude, paid for by this machine's Anthropic API key, within a daily spending cap.");
  });
});

describe("approveCapabilities", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("puts the version and the declaration's digest, with the token", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, init });
      return url === "/api/token" ? new Response(JSON.stringify({ token: "tk" })) : new Response("{}");
    }));
    await approveCapabilities("a1", 3, SHA);
    const put = calls.find(c => c.url === "/api/artifacts/a1/capability-approval")!;
    expect(put.init?.method).toBe("PUT");
    expect(JSON.parse(String(put.init?.body))).toEqual({ version: 3, declaration_sha256: SHA });
  });

  it("rejects with the refusal's status and code", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => url === "/api/token"
      ? new Response(JSON.stringify({ token: "tk" }))
      : new Response(JSON.stringify({ error: { code: "stale_version", message: "newer", current: 5 } }), { status: 409 })));
    const e = await approveCapabilities("a1", 3, SHA).catch(x => x);
    expect(e).toBeInstanceOf(ApiError);
    expect(e).toMatchObject({ status: 409, code: "stale_version" });
  });
});
