import { describe, it, expect, vi } from "vitest";
import { artifactOrigin, probeOrigin, contentSrc } from "./origin";

const loc = (hostname: string, port = "7480") => ({ hostname, port, protocol: "http:" } as unknown as Location);

describe("artifactOrigin", () => {
  it("uses <id>.localhost on local hosts only", () => {
    expect(artifactOrigin("7q3k9mzx2b4t", loc("localhost"))).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", loc("127.0.0.1"))).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", loc("192.168.1.20"))).toBeNull();
    expect(artifactOrigin("7q3k9mzx2b4t", loc("mymac.local"))).toBeNull();
  });
});

describe("probeOrigin", () => {
  it("is true on 200, false on error or timeout", async () => {
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => new Response("{}", { status: 200 }))).toBe(true);
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => { throw new TypeError("dns"); })).toBe(false);
    sessionStorage.clear();
    const never = (_: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_, rej) => init?.signal?.addEventListener("abort", () => rej(new DOMException("aborted", "AbortError"))));
    expect(await probeOrigin("http://x.localhost:1", never as typeof fetch, 20)).toBe(false);
  });
  it("caches the answer per session", async () => {
    sessionStorage.clear();
    const f = vi.fn(async () => new Response("{}"));
    await probeOrigin("http://x.localhost:1", f);
    await probeOrigin("http://x.localhost:1", f);
    expect(f).toHaveBeenCalledTimes(1);
  });
});

describe("contentSrc", () => {
  it("picks the origin or the same-origin path", () => {
    expect(contentSrc("7q3k9mzx2b4t", 2, "http://7q3k9mzx2b4t.localhost:7480")).toBe("http://7q3k9mzx2b4t.localhost:7480/v/2/");
    expect(contentSrc("7q3k9mzx2b4t", 2, null)).toBe("/c/7q3k9mzx2b4t/v/2/");
  });
});
