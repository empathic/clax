import { afterEach, describe, expect, it, vi } from "vitest";
import { acceptFromFrame, sendToFrame } from "./bridge-link";
import type { Tapped } from "./bridge-tap";

const tap = () => (globalThis as unknown as { claxBridgeTap: { log: Tapped[]; live: boolean; clear(): void } }).claxBridgeTap;

describe("the bridge tap (dev only)", () => {
  afterEach(() => { tap()?.clear(); vi.restoreAllMocks(); });

  it("numbers each message both ways with its frame and type", () => {
    const frame = { postMessage: vi.fn() } as unknown as Window;
    sendToFrame(frame, "https://a.example", { type: "clax:mode", mode: "comment" } as never);
    const e = { isTrusted: true, source: frame, origin: "https://a.example", data: { type: "clax:hello", artifact: "a", version: 1 } } as unknown as MessageEvent;
    expect(acceptFromFrame(e, frame, "https://a.example")).not.toBeNull();
    const [out, back] = tap().log.slice(-2);
    expect([out.dir, out.frame, out.type]).toEqual(["shell→frame", "https://a.example", "clax:mode"]);
    expect([back.dir, back.type]).toEqual(["frame→shell", "clax:hello"]);
    expect(back.seq).toBe(out.seq + 1);
  });

  it("logs only once live is on, and never a refused message", () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
    const frame = { postMessage: vi.fn() } as unknown as Window;
    sendToFrame(frame, "https://a.example", { type: "clax:mode", mode: "comment" } as never);
    expect(debug).not.toHaveBeenCalled();
    tap().live = true;
    const before = tap().log.length;
    const forged = { isTrusted: true, source: frame, origin: "https://evil.example", data: { type: "clax:hello" } } as unknown as MessageEvent;
    expect(acceptFromFrame(forged, frame, "https://a.example")).toBeNull();
    expect(tap().log.length).toBe(before);
    sendToFrame(frame, "https://a.example", { type: "clax:mode", mode: "off" } as never);
    expect(debug).toHaveBeenCalledTimes(1);
    tap().live = false;
  });
});
