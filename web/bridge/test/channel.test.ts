import { describe, expect, it } from "vitest";
import { acceptFromShell, shellOrigins } from "../src/channel";

describe("shellOrigins", () => {
  it("allows the loopback shell for subdomain content", () => {
    expect(shellOrigins("http://7q3k9mzx2b4t.localhost:7480/v/1/")).toEqual(["http://localhost:7480", "http://127.0.0.1:7480", "http://[::1]:7480"]);
  });
  it("allows only the content URL's own origin for path-based content", () => {
    expect(shellOrigins("http://192.168.1.5:7480/c/7q3k9mzx2b4t/v/1/")).toEqual(["http://192.168.1.5:7480"]);
  });
});

describe("acceptFromShell", () => {
  const origins = ["http://localhost:7480"];
  const ev = (data: unknown, origin: string, source: Window | null) => new MessageEvent("message", { data, origin, source });
  it("accepts known messages from the parent at an allowed origin", () => {
    expect(acceptFromShell(ev({ type: "artifax:comment-mode", on: true }, "http://localhost:7480", window), window, origins)).toEqual({ type: "artifax:comment-mode", on: true });
  });
  it("rejects other sources, origins, and types", () => {
    expect(acceptFromShell(ev({ type: "artifax:comment-mode", on: true }, "http://evil.test", window), window, origins)).toBeNull();
    expect(acceptFromShell(ev({ type: "artifax:comment-mode", on: true }, "http://localhost:7480", null), window, origins)).toBeNull();
    expect(acceptFromShell(ev({ type: "artifax:pick" }, "http://localhost:7480", window), window, origins)).toBeNull();
    expect(acceptFromShell(ev("artifax:comment-mode", "http://localhost:7480", window), window, origins)).toBeNull();
  });
});
