import { describe, expect, it } from "vitest";
import { socketUrl } from "./socket-url";

describe("socketUrl", () => {
  it("is wss: on an https: origin and ws: on a browser's plain-http daemon", () => {
    expect(socketUrl("https://clax.example", "/api/x", false)).toBe("wss://clax.example/api/x");
    expect(socketUrl("http://localhost:7480", "/api/x", false)).toBe("ws://localhost:7480/api/x");
    expect(socketUrl("http://192.168.1.20:7480", "/api/x", false)).toBe("ws://192.168.1.20:7480/api/x");
  });

  it("is null on any other scheme: no WebSocket scheme is guessed for it", () => {
    for (const o of ["clax://app", "null", "", "ftp://x", "file://", "httpx://x", "https:x"]) expect(socketUrl(o, "/api/x", false)).toBeNull();
  });

  it("is null in the Clax app, on either platform's shell origin", () => {
    expect(socketUrl("https://app.clax.invalid", "/api/x", true)).toBeNull();
    expect(socketUrl("clax://app", "/api/x", true)).toBeNull();
  });
});
