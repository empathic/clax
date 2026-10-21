import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { artifactOrigin, cachedOriginOk, probeOrigin, contentSrc, pageSrc } from "./origin";
import srcCases from "./frame-src-cases.json";

const loc = (hostname: string, port = "7480") => ({ hostname, port, protocol: "http:" } as unknown as Location);

describe("artifactOrigin", () => {
  it("uses <id>.localhost on local hosts only", () => {
    expect(artifactOrigin("7q3k9mzx2b4t", loc("localhost"))).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", loc("127.0.0.1"))).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", loc("192.168.1.20"))).toBeNull();
    expect(artifactOrigin("7q3k9mzx2b4t", loc("mymac.local"))).toBeNull();
  });
});

describe("artifactOrigin in the Clax app", () => {
  const embed = (content: string) => ({ transport: "host" as const, shell: "clax://app", content });

  it("is the embed's content origin for the ID, whatever the location", () => {
    for (const l of [loc("localhost"), loc("192.168.1.20"), loc("app", "")]) {
      expect(artifactOrigin("abcdefghijkl", l, embed("clax://{id}.c"))).toBe("clax://abcdefghijkl.c");
      expect(artifactOrigin("abcdefghijkl", l, embed("https://{id}.c.clax.invalid"))).toBe("https://abcdefghijkl.c.clax.invalid");
    }
  });

  it("is null (the sandboxed path) for an embed with an origin neither https: nor clax:, or a content template without {id} once", () => {
    for (const [shell, content] of [["clax://app", "http://{id}.c.example"], ["http://app.example", "https://{id}.c.clax.invalid"], ["https://app.clax.invalid", "ws://{id}.c"],
      ["https://app.clax.invalid", "https://c.clax.invalid"], ["clax://app", "clax://{id}.{id}.c"]]) {
      expect(artifactOrigin("abcdefghijkl", loc("localhost"), { transport: "host", shell, content })).toBeNull();
    }
  });

  it("reads the embed the app's transport installed", () => {
    Object.defineProperty(window, "claxTransport", { configurable: true, value: Object.freeze({ embed: embed("clax://{id}.c") }) });
    try {
      expect(artifactOrigin("abcdefghijkl", loc("192.168.1.20"))).toBe("clax://abcdefghijkl.c");
    } finally {
      delete (window as { claxTransport?: unknown }).claxTransport;
    }
    expect(artifactOrigin("abcdefghijkl", loc("192.168.1.20"))).toBeNull();
  });
});

describe("probeOrigin", () => {
  const O = "http://x.localhost:1";
  type Answer = () => Promise<Response>;
  /** A fetch that answers the daemon's own `/healthz` with `daemon` and the
   * artifact origin's with `name`; each rejects when its request is aborted. */
  const net = (daemon: Answer, name: Answer) => vi.fn((url: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((res, rej) => {
    init?.signal?.addEventListener("abort", () => rej(new DOMException("aborted", "AbortError")));
    (String(url) === "/healthz" ? daemon : name)().then(res, rej);
  }));
  const ok = async () => new Response("{}", { status: 200 });
  const never = () => new Promise<Response>(() => {});
  const later = (ms: number, r: Answer = ok) => () => new Promise<Response>(res => { setTimeout(() => { void r().then(res); }, ms); });
  const dns = async (): Promise<Response> => { throw new TypeError("dns"); };
  const cache = () => sessionStorage.getItem("clax.origin-ok");
  /** The probe's answer, or "pending" while it has none. */
  const state = (p: Promise<boolean>) => { let v: boolean | "pending" = "pending"; void p.then(x => { v = x; }); return () => v; };

  beforeEach(() => { sessionStorage.clear(); });
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("is true when the artifact origin answers, false when it fails or answers non-OK while the daemon answers, each cached", async () => {
    expect(await probeOrigin(O, net(ok, ok))).toBe(true);
    expect(cache()).toBe("1");
    sessionStorage.clear();
    expect(await probeOrigin(O, net(ok, dns))).toBe(false);
    expect(cache()).toBe("0");
    sessionStorage.clear();
    expect(await probeOrigin(O, net(ok, async () => new Response("no", { status: 502 })))).toBe(false);
    expect(cache()).toBe("0");
  });

  it("waits for a slow daemon instead of falling back", async () => {
    vi.useFakeTimers();
    const got = state(probeOrigin(O, net(later(3000), later(3000))));
    await vi.advanceTimersByTimeAsync(2999);
    expect(got()).toBe("pending");
    await vi.advanceTimersByTimeAsync(1);
    expect(got()).toBe(true);
    expect(cache()).toBe("1");
  });

  it("falls back a grace after the daemon answered when the artifact origin hangs, and caches that", async () => {
    vi.useFakeTimers();
    const f = net(later(400), never);
    const got = state(probeOrigin(O, f));
    await vi.advanceTimersByTimeAsync(1399);
    expect(got()).toBe("pending");
    await vi.advanceTimersByTimeAsync(1);
    expect(got()).toBe(false);
    expect(cache()).toBe("0");
    // The artifact origin's request, still out, is aborted.
    const name = f.mock.calls.find(([u]) => String(u) !== "/healthz")!;
    expect(name[1]?.signal?.aborted).toBe(true);
  });

  it("is false but not cached when the daemon does not answer either", async () => {
    vi.useFakeTimers();
    const hung = state(probeOrigin(O, net(never, never)));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(hung()).toBe(false);
    expect(cache()).toBeNull();
    // The artifact origin failing while the daemon is down says nothing of the name.
    const down = state(probeOrigin(O, net(dns, dns)));
    await vi.advanceTimersByTimeAsync(0);
    expect(down()).toBe(false);
    expect(cache()).toBeNull();
  });

  it("still answers when storage throws on write", async () => {
    const boom = () => { throw new DOMException("denied", "SecurityError"); };
    vi.stubGlobal("sessionStorage", { getItem: boom, setItem: boom, clear: boom });
    expect(await probeOrigin(O, net(ok, ok))).toBe(true);
    expect(cachedOriginOk()).toBeNull();
  });

  it("caches a sure answer for the session", async () => {
    expect(cachedOriginOk()).toBeNull();
    await probeOrigin(O, net(ok, ok));
    expect(cachedOriginOk()).toBe(true);
  });
});

describe("pageSrc", () => {
  it("is the version's root for the index and the file's path otherwise, segments encoded", () => {
    expect(pageSrc("7q3k9mzx2b4t", 2, null, "index.html")).toBe("/c/7q3k9mzx2b4t/v/2/");
    expect(pageSrc("7q3k9mzx2b4t", 2, null, "a.html", "k0", "https://content.example")).toBe("https://content.example/c/7q3k9mzx2b4t/v/2/k/k0/a.html");
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

describe("pageSrc and the daemon agree", () => {
  it.each(srcCases)("$file", (c: { id: string; n: number; origin: string | null; key?: string; file: string; src: string }) => {
    expect(pageSrc(c.id, c.n, c.origin, c.file, c.key ?? null)).toBe(c.src);
  });
});
