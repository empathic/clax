import { describe, expect, it } from "vitest";
import { acceptFromFrame } from "./bridge-link";

const ev = (data: unknown, origin: string, source: Window | null) => new MessageEvent("message", { data, origin, source });

describe("acceptFromFrame", () => {
  it("takes bridge messages from the frame at its origin in subdomain mode", () => {
    expect(acceptFromFrame(ev({ type: "artifax:cancel" }, "http://x.localhost:7480", window), window, "http://x.localhost:7480")).toEqual({ type: "artifax:cancel" });
  });
  it("takes only opaque-origin messages in sandbox mode", () => {
    expect(acceptFromFrame(ev({ type: "artifax:cancel" }, "null", window), window, null)).not.toBeNull();
    expect(acceptFromFrame(ev({ type: "artifax:cancel" }, "http://localhost:7480", window), window, null)).toBeNull();
  });
  it("rejects other windows and unknown types", () => {
    expect(acceptFromFrame(ev({ type: "artifax:cancel" }, "null", null), window, null)).toBeNull();
    expect(acceptFromFrame(ev({ type: "artifax:welcome", mode: "view" }, "null", window), window, null)).toBeNull();
  });
});
