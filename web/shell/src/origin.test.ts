import { describe, it, expect } from "vitest";
import { FRAME_SANDBOX, artifactOrigin, contentSrc, frameOrigin, isPathFrame, onPathFrames, pageSrc, startSrc } from "./origin";
import srcCases from "./frame-src-cases.json";

describe("artifactOrigin", () => {
  it("is the artifact's host under the site domain, on its scheme, with a port only when one is named", () => {
    expect(artifactOrigin("7q3k9mzx2b4t", { domain: "localhost", scheme: "http", port: 7480 })).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", { domain: "clax.run", scheme: "https", port: null })).toBe("https://7q3k9mzx2b4t.clax.run");
    expect(artifactOrigin("7q3k9mzx2b4t", { domain: "sites.example", scheme: "https", port: 8443 })).toBe("https://7q3k9mzx2b4t.sites.example:8443");
  });

  it("in the Clax app is the app's content origin, as its core names the site hosts", () => {
    expect(artifactOrigin("abcdefghjkmn", { domain: "c", scheme: "clax", port: null })).toBe("clax://abcdefghjkmn.c");
    expect(artifactOrigin("abcdefghjkmn", { domain: "c.clax.invalid", scheme: "https", port: null })).toBe("https://abcdefghjkmn.c.clax.invalid");
  });
});

describe("pageSrc", () => {
  it("is the version's root for the index and the file's path otherwise, segments encoded", () => {
    const site = "http://7q3k9mzx2b4t.localhost:7480";
    expect(pageSrc(2, site, "index.html")).toBe(`${site}/_clax/v/2/`);
    expect(pageSrc(2, site, "docs/a b.html")).toBe(`${site}/_clax/v/2/docs/a%20b.html`);
    expect(contentSrc(2, site)).toBe(`${site}/_clax/v/2/`);
  });
});

describe("startSrc", () => {
  it("names the target's path and fragment as next, and / for an address off the site", () => {
    const site = "https://7q3k9mzx2b4t.clax.run";
    expect(startSrc(site, `${site}/_clax/v/2/a.html#part-2`)).toBe(`${site}/_clax/session/start?next=%2F_clax%2Fv%2F2%2Fa.html%23part-2`);
    expect(startSrc(site, site)).toBe(`${site}/_clax/session/start?next=%2F`);
    expect(startSrc(site, "https://evil.example/x")).toBe(`${site}/_clax/session/start?next=%2F`);
    expect(startSrc(site, `${site}.evil.example/x`)).toBe(`${site}/_clax/session/start?next=%2F`);
  });

  it("names the path on the app's clax: origins too", () => {
    const site = "clax://abcdefghjkmn.c";
    expect(startSrc(site, `${site}/_clax/v/3/docs/a%20b.html`)).toBe(`${site}/_clax/session/start?next=%2F_clax%2Fv%2F3%2Fdocs%2Fa%2520b.html`);
  });
});

describe("pageSrc, startSrc and the daemon agree", () => {
  it.each(srcCases)("$file on $site", (c: { site: string; n: number; file: string; src: string; start: string }) => {
    expect(pageSrc(c.n, c.site, c.file)).toBe(c.src);
    expect(startSrc(c.site, c.src)).toBe(c.start);
  });
});

describe("frames on a path (a trusted host's [serve] trusted_host_frames)", () => {
  it("frames each artifact from a path of the shell's own host, with no handshake, its messages at the opaque origin", () => {
    const site = artifactOrigin("7q3k9mzx2b4t", { domain: "localhost", scheme: "http", port: null, path: "/_clax/frame/" });
    expect(site).toBe("/_clax/frame/7q3k9mzx2b4t");
    expect(onPathFrames()).toBe(true);
    expect(isPathFrame(site)).toBe(true);
    expect(pageSrc(3, site, "index.html")).toBe("/_clax/frame/7q3k9mzx2b4t/_clax/v/3/");
    expect(startSrc(site, pageSrc(3, site, "a b.html"))).toBe("/_clax/frame/7q3k9mzx2b4t/_clax/v/3/a%20b.html");
    expect(frameOrigin(site)).toBe("null");
    expect(frameOrigin("http://7q3k9mzx2b4t.localhost:7480")).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(FRAME_SANDBOX).not.toContain("allow-same-origin");
  });
});
