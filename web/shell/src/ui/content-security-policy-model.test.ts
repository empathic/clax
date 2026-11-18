import { describe, expect, it } from "vitest";
import { NAMED_MAX, blockedNotice } from "./content-security-policy-model";

describe("blockedNotice", () => {
  it("tells an administrator how many sites, and names them", () => {
    expect(blockedNotice(["https://cdn.example.com"], true, false)).toEqual({ text: "This clax tried to reach another site, and Clax blocked it.", sites: "https://cdn.example.com" });
    expect(blockedNotice(["https://a.example", "wss://b.example"], true, false)).toEqual({ text: "This clax tried to reach 2 other sites, and Clax blocked it.", sites: "https://a.example, wss://b.example" });
  });
  it("names at most NAMED_MAX and says how many more", () => {
    const origins = Array.from({ length: NAMED_MAX + 4 }, (_, i) => `https://h${i}.example`);
    const w = blockedNotice(origins, true, false)!;
    expect(w.text).toBe(`This clax tried to reach ${NAMED_MAX + 4} other sites, and Clax blocked it.`);
    expect(w.sites).toBe(`${origins.slice(0, NAMED_MAX).join(", ")}, and 4 more`);
  });
  it("gives anyone else one line, naming no site", () => {
    expect(blockedNotice(["https://a.example", "https://b.example"], false, false)).toEqual({ text: "This page tried to load from other sites, and Clax blocked it.", sites: null });
  });
  it("once the clax is served unrestricted, asks an administrator to reload and tells anyone else nothing", () => {
    expect(blockedNotice(["https://a.example"], true, true)).toEqual({ text: "This clax is now served without Clax's Content Security Policy. Reload to let it reach these sites.", sites: "https://a.example" });
    expect(blockedNotice(["https://a.example"], false, true)).toBeNull();
  });
  it("says nothing of nothing", () => {
    expect(blockedNotice([], true, false)).toBeNull();
    expect(blockedNotice([], false, false)).toBeNull();
  });
});
