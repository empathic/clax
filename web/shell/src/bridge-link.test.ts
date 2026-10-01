import { describe, expect, it } from "vitest";
import { trusted } from "../../bridge/test/trusted";
import { acceptByeFromFrame, acceptFromFrame, helloMatches } from "./bridge-link";

/** A message as the browser delivers one. */
const ev = (data: unknown, origin: string, source: Window | null) => trusted(new MessageEvent("message", { data, origin, source }));

describe("acceptFromFrame", () => {
  it("takes bridge messages from the frame at its origin in subdomain mode", () => {
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "http://x.localhost:7480", window), window, "http://x.localhost:7480")).toEqual({ type: "clax:cancel" });
  });
  it("takes only opaque-origin messages in sandbox mode", () => {
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "null", window), window, null)).not.toBeNull();
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "http://localhost:7480", window), window, null)).toBeNull();
  });
  it("rejects a message a script made rather than the browser delivered", () => {
    expect(acceptFromFrame(new MessageEvent("message", { data: { type: "clax:cancel" }, origin: "null", source: window }), window, null)).toBeNull();
  });
  it("rejects other windows and unknown types", () => {
    expect(acceptFromFrame(ev({ type: "clax:cancel" }, "null", null), window, null)).toBeNull();
    expect(acceptFromFrame(ev({ type: "clax:welcome", mode: "view" }, "null", window), window, null)).toBeNull();
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
    expect(acceptByeFromFrame(ev(bye, "null", window), window, null)).toBe(true);
    expect(acceptByeFromFrame(ev(bye, "null", null), window, null)).toBe(true);
    expect(acceptByeFromFrame(ev(bye, "http://x.localhost:7480", null), window, "http://x.localhost:7480")).toBe(true);
  });
  it("refuses another origin, another window, another type, and a bye a script made", () => {
    expect(acceptByeFromFrame(ev(bye, "http://localhost:7480", null), window, null)).toBe(false);
    expect(acceptByeFromFrame(ev(bye, "null", null), window, "http://x.localhost:7480")).toBe(false);
    const other = document.createElement("iframe");
    document.body.append(other);
    expect(acceptByeFromFrame(ev(bye, "null", other.contentWindow), window, null)).toBe(false);
    other.remove();
    expect(acceptByeFromFrame(ev({ type: "clax:cancel" }, "null", null), window, null)).toBe(false);
    expect(acceptByeFromFrame(new MessageEvent("message", { data: bye, origin: "null", source: null }), window, null)).toBe(false);
    expect(acceptByeFromFrame(ev(bye, "null", null), null, null)).toBe(false);
  });
});
