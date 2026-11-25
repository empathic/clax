import { describe, expect, it } from "vitest";
import { BlockedLog, isLockdown, lockdownRefusal } from "../src/blocked";
import { BLOCKED_MAX } from "../src/protocol";

// The lockdown as a local daemon serves it (crates/clax-http/src/content_policy.rs, `lockdown`).
const LOCKDOWN = "frame-ancestors 'self' http://localhost:7480 http://127.0.0.1:7480; default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; style-src 'self' 'unsafe-inline' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'";
const OWN = "http://7q3k9mzx2b4t.localhost:7480";

type V = Partial<{ isTrusted: boolean; disposition: string; originalPolicy: string; effectiveDirective: string; blockedURI: string }>;
const violation = (v: V = {}) => ({ isTrusted: true, disposition: "enforce", originalPolicy: LOCKDOWN, effectiveDirective: "script-src-elem", blockedURI: "https://cdn.example.com/lib.js?k=secret#x", ...v }) as unknown as SecurityPolicyViolationEvent;

describe("isLockdown", () => {
  it("knows the lockdown, whatever its frame-ancestors, case or spacing", () => {
    expect(isLockdown(LOCKDOWN)).toBe(true);
    expect(isLockdown(LOCKDOWN.replace("'self' http://localhost:7480 http://127.0.0.1:7480", "'self' clax://app"))).toBe(true);
    expect(isLockdown(LOCKDOWN.toUpperCase().replace(/; /g, " ;  "))).toBe(true);
  });
  it("does not take a page's own meta policy, or an own clax's framing alone, for it", () => {
    expect(isLockdown("default-src 'self'; connect-src 'self'")).toBe(false);
    expect(isLockdown("script-src 'none'")).toBe(false);
    expect(isLockdown("frame-ancestors 'self' http://localhost:7480")).toBe(false);
    // A meta policy without frame-ancestors first, even carrying the rest.
    expect(isLockdown("connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'")).toBe(false);
  });
});

describe("lockdownRefusal", () => {
  it("names the directive and the blocked URL's origin alone, never its path, query or fragment", () => {
    expect(lockdownRefusal(violation(), OWN)).toEqual({ directive: "script-src-elem", origin: "https://cdn.example.com" });
    expect(lockdownRefusal(violation({ effectiveDirective: "connect-src", blockedURI: "wss://relay.example:8443/socket?t=1" }), OWN)).toEqual({ directive: "connect-src", origin: "wss://relay.example:8443" });
    expect(lockdownRefusal(violation({ effectiveDirective: "form-action", blockedURI: "https://forms.example/post" }), OWN)).toEqual({ directive: "form-action", origin: "https://forms.example" });
  });
  it("ignores what is not the browser's, not enforced, not the lockdown's, or not another site", () => {
    expect(lockdownRefusal(violation({ isTrusted: false }), OWN)).toBeNull();
    expect(lockdownRefusal(violation({ disposition: "report" }), OWN)).toBeNull();
    expect(lockdownRefusal(violation({ originalPolicy: "script-src 'self'" }), OWN)).toBeNull();
    // Its own site (a same-origin form post), an inline script, eval, data and blob URLs.
    expect(lockdownRefusal(violation({ effectiveDirective: "form-action", blockedURI: `${OWN}/save` }), OWN)).toBeNull();
    for (const blockedURI of ["inline", "eval", "data", "blob", "", "about:blank", "javascript:alert(1)", "ftp://files.example/x"]) {
      expect(lockdownRefusal(violation({ blockedURI }), OWN), blockedURI).toBeNull();
    }
    // A directive the lockdown does not refuse other sites by.
    expect(lockdownRefusal(violation({ effectiveDirective: "base-uri" }), OWN)).toBeNull();
    expect(lockdownRefusal(violation({ effectiveDirective: "frame-ancestors" }), OWN)).toBeNull();
  });
  it("ignores an origin longer than the protocol carries", () => {
    expect(lockdownRefusal(violation({ blockedURI: `https://${"a".repeat(60)}.${"b".repeat(60)}.${"c".repeat(60)}.${"d".repeat(60)}.${"e".repeat(60)}.example/` }), OWN)).toBeNull();
  });
});

describe("BlockedLog", () => {
  it("keeps each directive and origin once, and at most BLOCKED_MAX", () => {
    const log = new BlockedLog();
    expect(log.add({ directive: "script-src-elem", origin: "https://a.example" })).toBe(true);
    expect(log.add({ directive: "script-src-elem", origin: "https://a.example" })).toBe(false);
    expect(log.add({ directive: "img-src", origin: "https://a.example" })).toBe(true);
    for (let i = 0; i < BLOCKED_MAX * 3; i++) log.add({ directive: "img-src", origin: `https://h${i}.example` });
    expect(log.all).toHaveLength(BLOCKED_MAX);
    expect(log.add({ directive: "font-src", origin: "https://late.example" })).toBe(false);
  });
});
