import { describe, it, expect, vi, afterEach } from "vitest";
import { artifactOrigin, probeOrigin, contentSrc, pageSrc } from "./origin";

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
  afterEach(() => { vi.unstubAllGlobals(); });
  it("is true on 200, false on error or timeout", async () => {
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => new Response("{}", { status: 200 }))).toBe(true);
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => { throw new TypeError("dns"); })).toBe(false);
    sessionStorage.clear();
    const never = (_: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_resolve, rej) => init?.signal?.addEventListener("abort", () => rej(new DOMException("aborted", "AbortError"))));
    expect(await probeOrigin("http://x.localhost:1", never as typeof fetch, 20)).toBe(false);
  });
  it("is false when the probe answers with a non-OK status", async () => {
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => new Response("no", { status: 502 }))).toBe(false);
    expect(sessionStorage.getItem("artifax.origin-ok")).toBe("0");
  });
  it("still answers when storage throws on read and write", async () => {
    const boom = () => { throw new DOMException("denied", "SecurityError"); };
    vi.stubGlobal("sessionStorage", { getItem: boom, setItem: boom, clear: boom });
    const f = vi.fn(async () => new Response("{}"));
    expect(await probeOrigin("http://x.localhost:1", f)).toBe(true);
    expect(await probeOrigin("http://x.localhost:1", f)).toBe(true);
    expect(f).toHaveBeenCalledTimes(2);
  });
  it("caches the answer per session", async () => {
    sessionStorage.clear();
    const f = vi.fn(async () => new Response("{}"));
    await probeOrigin("http://x.localhost:1", f);
    await probeOrigin("http://x.localhost:1", f);
    expect(f).toHaveBeenCalledTimes(1);
  });
});

describe("pageSrc", () => {
  it("is the version's root for the index and the file's path otherwise, segments encoded", () => {
    expect(pageSrc("7q3k9mzx2b4t", 2, null, "index.html")).toBe("/c/7q3k9mzx2b4t/v/2/");
    expect(pageSrc("7q3k9mzx2b4t", 2, null, "docs/a b.html")).toBe("/c/7q3k9mzx2b4t/v/2/docs/a%20b.html");
    expect(pageSrc("7q3k9mzx2b4t", 2, "http://7q3k9mzx2b4t.localhost:7480", "about.html")).toBe("http://7q3k9mzx2b4t.localhost:7480/v/2/about.html");
  });
});

describe("contentSrc", () => {
  it("picks the origin or the same-origin path", () => {
    expect(contentSrc("7q3k9mzx2b4t", 2, "http://7q3k9mzx2b4t.localhost:7480")).toBe("http://7q3k9mzx2b4t.localhost:7480/v/2/");
    expect(contentSrc("7q3k9mzx2b4t", 2, null)).toBe("/c/7q3k9mzx2b4t/v/2/");
  });
});
