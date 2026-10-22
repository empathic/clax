import { describe, expect, it } from "vitest";
import { trusted } from "../../bridge/test/trusted";
import { acceptByeFromFrame, acceptFromFrame, acceptSiteState, helloMatches } from "./bridge-link";

/** A message as the browser delivers one. */
const ev = (data: unknown, origin: string, source: Window | null) => trusted(new MessageEvent("message", { data, origin, source }));
const SITE = "http://x.localhost:7480";

describe("acceptFromFrame", () => {
  it("takes bridge messages from the frame at the site origin", () => {
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, SITE, window), window, SITE)).toEqual({ type: "clax:cancel" });
  });
  it("refuses another origin, an opaque one among them", () => {
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "null", window), window, SITE)).toBeNull();
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "http://localhost:7480", window), window, SITE)).toBeNull();
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "http://y.localhost:7480", window), window, SITE)).toBeNull();
  });
  it("rejects a message a script made rather than the browser delivered", () => {
    expect(acceptFromFrame(new MessageEvent("message", { data: { type: "clax:cancel" }, origin: SITE, source: window }), window, SITE)).toBeNull();
  });
  it("rejects other windows and unknown types", () => {
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, SITE, null), window, SITE)).toBeNull();
    expect(acceptFromFrame(ev({ type: "clax:welcome", mode: "view" }, SITE, window), window, SITE)).toBeNull();
  });
});

describe("acceptSiteState", () => {
  const state = { type: "clax:site-state", state: "s1", next: "/_clax/v/1/" };
  it("takes the handshake's state from the frame at the site origin", () => {
    expect(acceptSiteState(ev(state, SITE, window), window, SITE)).toEqual({ state: "s1", next: "/_clax/v/1/" });
  });
  it("refuses another origin, another window, a malformed state, and one a script made", () => {
    expect(acceptSiteState(ev(state, "http://y.localhost:7480", window), window, SITE)).toBeNull();
    expect(acceptSiteState(ev(state, SITE, null), window, SITE)).toBeNull();
    expect(acceptSiteState(ev({ ...state, state: 1 }, SITE, window), window, SITE)).toBeNull();
    expect(acceptSiteState(ev({ type: "clax:hello" }, SITE, window), window, SITE)).toBeNull();
    expect(acceptSiteState(new MessageEvent("message", { data: state, origin: SITE, source: window }), window, SITE)).toBeNull();
  });
});

describe("helloMatches", () => {
  it("accepts only the hello of the artifact and version the shell shows", () => {
    const hello = { type: "clax:hello" as const, artifact: "7q3k9mzx2b4t", version: 2, file: "index.html" };
    expect(helloMatches(hello, "7q3k9mzx2b4t", 2)).toBe(true);
    expect(helloMatches(hello, "7q3k9mzx2b4t", 1)).toBe(false);
    expect(helloMatches(hello, "9zzzzzzzzzzz", 2)).toBe(false);
  });
});

describe("acceptByeFromFrame", () => {
  const bye = { type: "clax:bye" };
  it("takes a bye from the frame, or from a document already gone (no source), at the frame's origin", () => {
    expect(acceptByeFromFrame(ev(bye, SITE, window), window, SITE)).toBe(true);
    expect(acceptByeFromFrame(ev(bye, SITE, null), window, SITE)).toBe(true);
  });
  it("refuses another origin, another window, another type, and a bye a script made", () => {
    expect(acceptByeFromFrame(ev(bye, "http://localhost:7480", null), window, SITE)).toBe(false);
    expect(acceptByeFromFrame(ev(bye, "null", null), window, SITE)).toBe(false);
    const other = document.createElement("iframe");
    document.body.append(other);
    expect(acceptByeFromFrame(ev(bye, SITE, other.contentWindow), window, SITE)).toBe(false);
    other.remove();
    expect(acceptByeFromFrame(ev({ type: "clax:cancel" }, SITE, null), window, SITE)).toBe(false);
    expect(acceptByeFromFrame(new MessageEvent("message", { data: bye, origin: SITE, source: null }), window, SITE)).toBe(false);
    expect(acceptByeFromFrame(ev(bye, SITE, null), null, SITE)).toBe(false);
  });
});
