import { describe, expect, it } from "vitest";
import { acceptFromShell, forwardedKey, shellOrigins } from "../src/channel";
import { trusted } from "./trusted";

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
  const ev = (data: unknown, origin: string, source: Window | null) => trusted(new MessageEvent("message", { data, origin, source }));
  it("accepts known messages from the parent at an allowed origin", () => {
    expect(acceptFromShell(ev({ type: "clax:comment-mode", on: true }, "http://localhost:7480", window), window, origins)).toEqual({ type: "clax:comment-mode", on: true });
  });
  it("rejects a message the page made up and dispatched itself, whatever source and origin it names", () => {
    const forged = new MessageEvent("message", { data: { type: "clax:composer-ready", pickId: "p1" }, origin: "http://localhost:7480", source: window });
    expect(forged.isTrusted).toBe(false);
    expect(acceptFromShell(forged, window, origins)).toBeNull();
  });
  it("rejects other sources, origins, and types", () => {
    expect(acceptFromShell(ev({ type: "clax:comment-mode", on: true }, "http://evil.test", window), window, origins)).toBeNull();
    expect(acceptFromShell(ev({ type: "clax:comment-mode", on: true }, "http://localhost:7480", null), window, origins)).toBeNull();
    expect(acceptFromShell(ev({ type: "clax:pick" }, "http://localhost:7480", window), window, origins)).toBeNull();
    expect(acceptFromShell(ev("clax:comment-mode", "http://localhost:7480", window), window, origins)).toBeNull();
  });
});

describe("forwardedKey", () => {
  it("takes Option, Up, Down, and Escape with a direction, and nothing else", () => {
    for (const key of ["Alt", "ArrowUp", "ArrowDown", "Escape"]) expect(forwardedKey({ key, down: true })).toEqual({ key, down: true });
    expect(forwardedKey({ key: "Escape", down: false })).toEqual({ key: "Escape", down: false });
    expect(forwardedKey({ key: "Enter", down: true })).toBeNull();
    expect(forwardedKey({ key: "Escape", down: "yes" })).toBeNull();
    expect(forwardedKey({ key: 5, down: true })).toBeNull();
  });
});
