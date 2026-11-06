import { describe, expect, it } from "vitest";
import { CURVES, ease, travelMs, typingDelays } from "./pace.ts";

describe("pace", () => {
  it("travel is capped", () => {
    expect(travelMs(0)).toBe(250);
    expect(travelMs(500)).toBe(550);
    expect(travelMs(2000)).toBe(900);
  });

  it("ease ends at 0 and 1", () => {
    for (const [name, curve] of Object.entries(CURVES)) {
      expect(ease(0, curve), name).toBe(0);
      expect(ease(1, curve), name).toBe(1);
      // Monotonic in between, as each curve's x and y are.
      let last = 0;
      for (let i = 1; i < 20; i++) {
        const y = ease(i / 20, curve);
        expect(y, `${name} at ${i / 20}`).toBeGreaterThanOrEqual(last);
        last = y;
      }
    }
    // The standard curve is CSS's cubic-bezier(0.4, 0, 0.2, 1): about 0.77 at the half.
    expect(ease(0.5, CURVES.standard)).toBeCloseTo(0.774, 2);
  });

  it("typing is deterministic", () => {
    const a = typingDelays("Book the venue", "realtime");
    const b = typingDelays("Book the venue", "realtime");
    expect(a).toEqual(b);
    expect(a).toHaveLength("Book the venue".length);
    for (const ms of a) {
      expect(ms).toBeGreaterThanOrEqual(53.6);
      expect(ms).toBeLessThanOrEqual(89.3);
    }
    expect(typingDelays("Book the venue", "point")).not.toEqual(a);
  });
});
