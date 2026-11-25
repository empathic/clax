import { describe, expect, it } from "vitest";
import { BLOCKED_MAX } from "../../../bridge/src/protocol";
import { BlockedSites, blockedReport } from "./blocked";

const OWN = "http://7q3k9mzx2b4t.localhost:7480";

describe("blockedReport", () => {
  it("takes a known directive and an origin alone", () => {
    expect(blockedReport({ directive: "script-src-elem", origin: "https://cdn.example.com" }, OWN)).toEqual({ directive: "script-src-elem", origin: "https://cdn.example.com" });
    expect(blockedReport({ directive: "connect-src", origin: "ws://127.0.0.1:5555" }, OWN)).toEqual({ directive: "connect-src", origin: "ws://127.0.0.1:5555" });
  });
  it("refuses anything malformed, oversized, more than an origin, the page's own site, or of an unknown directive", () => {
    const bad: unknown[] = [
      null, undefined, "https://cdn.example.com", 7, [], {},
      { directive: "script-src-elem" }, { origin: "https://cdn.example.com" },
      { directive: "script-src-elem", origin: 42 },
      { directive: "frame-ancestors", origin: "https://cdn.example.com" },
      { directive: "__proto__", origin: "https://cdn.example.com" },
      { directive: ["script-src"], origin: "https://cdn.example.com" },
      { directive: "script-src", origin: "https://cdn.example.com/lib.js" },
      { directive: "script-src", origin: "https://cdn.example.com/" },
      { directive: "script-src", origin: "https://cdn.example.com?q=1" },
      { directive: "script-src", origin: "https://cdn.example.com#f" },
      { directive: "script-src", origin: "https://me:pw@cdn.example.com" },
      { directive: "script-src", origin: "Https://Cdn.Example.com" },
      { directive: "script-src", origin: "javascript:alert(1)" },
      { directive: "script-src", origin: "data:text/html,<b>x</b>" },
      { directive: "script-src", origin: "ftp://files.example" },
      { directive: "script-src", origin: "<img src=x onerror=alert(1)>" },
      { directive: "script-src", origin: `https://${"a".repeat(63)}.${"b".repeat(63)}.${"c".repeat(63)}.${"d".repeat(63)}.${"e".repeat(50)}.example` },
      { directive: "script-src", origin: OWN },
    ];
    for (const m of bad) expect(blockedReport(m, OWN), JSON.stringify(m)).toBeNull();
  });
});

describe("BlockedSites", () => {
  it("names each origin once, first reported first, however many directives name it", () => {
    const b = new BlockedSites();
    expect(b.add({ directive: "script-src-elem", origin: "https://a.example" }, OWN)).toBe(true);
    expect(b.add({ directive: "script-src-elem", origin: "https://a.example" }, OWN)).toBe(false);
    expect(b.add({ directive: "style-src-elem", origin: "https://a.example" }, OWN)).toBe(false);
    expect(b.add({ directive: "font-src", origin: "https://b.example" }, OWN)).toBe(true);
    expect(b.origins).toEqual(["https://a.example", "https://b.example"]);
  });
  it("keeps at most BLOCKED_MAX reports, however many a page posts", () => {
    const b = new BlockedSites();
    for (let i = 0; i < BLOCKED_MAX * 50; i++) b.add({ directive: "img-src", origin: `https://h${i}.example` }, OWN);
    expect(b.origins).toHaveLength(BLOCKED_MAX);
    expect(b.add({ directive: "img-src", origin: "https://late.example" }, OWN)).toBe(false);
    b.clear();
    expect(b.origins).toEqual([]);
    expect(b.add({ directive: "img-src", origin: "https://late.example" }, OWN)).toBe(true);
  });
});
