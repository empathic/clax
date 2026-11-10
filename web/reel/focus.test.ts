import { describe, expect, it } from "vitest";
import { FocusLog, parseFocus } from "./focus.ts";

const script = {
  targets: { composer: ".composer", globe: "frame:canvas#globe" },
  shots: [
    { at: "@composer-0.2", targets: ["composer"], label: "composer" },
    { at: "touch-0.2", targets: ["globe", "pill"], label: "globe" },
  ],
  clean: [{ at: "reveal-0.3", until: "reveal+1.2" }],
  overrides: { pill: [548, 8, 256, 54] },
};

describe("parseFocus", () => {
  it("reads a script whose shots name declared or overridden targets", () => {
    expect(parseFocus(JSON.stringify(script), "x").shots).toHaveLength(2);
  });
  it("refuses a shot naming an unknown target", () => {
    const bad = { ...script, shots: [{ at: "ask", targets: ["nope"] }] };
    expect(() => parseFocus(JSON.stringify(bad), "x")).toThrow(/no target nope/);
  });
  it("refuses a time that is neither a frame nor BEAT[+-seconds]", () => {
    for (const at of ["+0.5", "ask+", "Ask", -3, 1.5]) {
      const bad = { ...script, shots: [{ at, targets: ["composer"] }] };
      expect(() => parseFocus(JSON.stringify(bad), "x")).toThrow(/is not a frame/);
    }
  });
  it("refuses waiting for a target the script does not declare", () => {
    const bad = { ...script, shots: [{ at: "@pill", targets: ["composer"] }] };
    expect(() => parseFocus(JSON.stringify(bad), "x")).toThrow(/no target pill to wait for/);
  });
  it("refuses an override that is not [x, y, w, h]", () => {
    const bad = { ...script, overrides: { pill: [1, 2, 3] } };
    expect(() => parseFocus(JSON.stringify(bad), "x")).toThrow(/overrides.pill/);
  });
});

describe("FocusLog", () => {
  it("keeps an entry only where a target's box changed", () => {
    const log = new FocusLog();
    log.record(0, { a: null });
    log.record(1, { a: [1, 2, 3, 4] });
    log.record(2, { a: [1, 2, 3, 4] });
    log.record(3, { a: null });
    expect(log.rects.a).toEqual([[0, null], [1, [1, 2, 3, 4]], [3, null]]);
    expect(JSON.parse(log.json(30, 1.25))).toEqual({ fps: 30, scale: 1.25, targets: { a: [[0, null], [1, [1, 2, 3, 4]], [3, null]] } });
  });
});
