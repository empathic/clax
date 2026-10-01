import { describe, expect, it } from "vitest";
import { FrameGate } from "./frame-gate";

describe("FrameGate", () => {
  it("opens on a matching hello, closes on a foreign one", () => {
    const g = new FrameGate();
    g.hello(true);
    expect(g.open).toBe(true);
    g.hello(false);
    expect(g.open).toBe(false);
  });
  it("closes on a load no matching hello preceded, and keeps a page that greeted before its load", () => {
    const g = new FrameGate();
    g.hello(true);
    expect(g.load()).toBe(false);
    expect(g.open).toBe(true);
    expect(g.load()).toBe(true);
    expect(g.open).toBe(false);
    g.hello(true);
    expect(g.open).toBe(true);
  });
  it("does not count a foreign hello as the loaded page's greeting", () => {
    const g = new FrameGate();
    g.hello(false);
    expect(g.load()).toBe(true);
    expect(g.open).toBe(false);
  });
  it("closes on reset and on close", () => {
    const g = new FrameGate();
    g.hello(true);
    g.close();
    expect(g.open).toBe(false);
    g.hello(true);
    g.reset();
    expect(g.open).toBe(false);
    expect(g.load()).toBe(true);
  });
});
