import { describe, expect, it } from "vitest";
import { type BandRow, MAX_UNITS, band, strokes } from "./band-model";

const rows: BandRow[] = [{ y: 20, pending: 3, waiting: 2, v: 6 }, { y: 40, pending: 5, waiting: 1, gap: "Thu" }, { y: 60, pending: 1, waiting: 1 }];
const right = { top: 0, x0: 56, dir: 1 as const, unit: 4.5 };
const left = { top: 0, x0: 0, dir: -1 as const, unit: 4.5 };

describe("band", () => {
  it("draws the agent and waiting areas, the spine and a tick per version, growing toward smaller x", () => {
    const b = band(rows, right);
    expect(b.agent).toBe("M51.5,0 V16 L38,24 V36 L56,44 V72 H56 V0 Z");
    expect(b.waiting).toBe("M42.5,0 V16 L33.5,24 V36 L51.5,44 V56 L56,64 V72 H56 V44 L38,36 V24 L51.5,16 V0 Z");
    expect(b.spine).toEqual({ x: 56, y1: 0, y2: 72 });
    expect(b.ticks).toEqual([{ x1: 56, x2: 56 - 5 * 4.5 - 5, y: 20 }]);
    expect(b.anchor).toBe("end");
  });

  it("mirrors every x when the band grows toward larger x", () => {
    const b = band(rows, left);
    expect(b.agent).toBe("M4.5,0 V16 L18,24 V36 L0,44 V72 H0 V0 Z");
    expect(b.waiting).toBe("M13.5,0 V16 L22.5,24 V36 L4.5,44 V56 L0,64 V72 H0 V44 L18,36 V24 L4.5,16 V0 Z");
    expect(b.ticks).toEqual([{ x1: 0, x2: 27.5, y: 20 }]);
    expect(b.anchor).toBe("start");
    const r = band(rows, { ...right, labels: true }), l = band(rows, { ...left, labels: true });
    expect(l.versions).toEqual(r.versions.map(v => ({ ...v, x: 56 - v.x })));
    expect(l.counts).toEqual(r.counts.map(c => ({ ...c, x: 56 - c.x })));
  });

  it("places a version's label past its tick, a count on each run where it changes and is not 0, and each day's break between its rows", () => {
    const b = band(rows, { ...right, labels: true });
    expect(b.versions).toEqual([{ x: 25.5, y: 20, text: "v6" }]);
    expect(b.counts).toEqual([{ x: 39.5, y: 10, text: "3" }, { x: 30.5, y: 30, text: "5" }, { x: 48.5, y: 50, text: "1" }]);
    expect(b.gaps).toEqual([{ y: 30, text: "Thu" }]);
    // A run of equal counts is labelled once, centred on the whole run.
    const run = band([{ y: 20, pending: 2, waiting: 0 }, { y: 40, pending: 2, waiting: 0 }, { y: 60, pending: 1, waiting: 0 }], { ...right, labels: true });
    expect(run.counts).toEqual([{ x: 44, y: 20, text: "2" }, { x: 48.5, y: 50, text: "1" }]);
  });

  it("leaves out a count whose label would share a version label's place: the version wins", () => {
    // The strip's step, 18 px: a run's count, centred between two rows, sits 9 px from each.
    const b = band([{ y: 8, pending: 3, waiting: 1 }, { y: 32, pending: 3, waiting: 1, v: 3 }, { y: 50, pending: 4, waiting: 1, v: 2 }, { y: 68, pending: 1, waiting: 0 }, { y: 104, pending: 2, waiting: 0 }],
      { top: 0, x0: 55, dir: 1, unit: 4.5, labels: true });
    expect(b.versions.map(v => [v.text, v.y])).toEqual([["v3", 32], ["v2", 50]]);
    // "4" (y 41) and "1" (y 59) are not drawn; "3" (y 16) and "2" (y 86) are.
    expect(b.counts.map(c => [c.text, c.y])).toEqual([["3", 16], ["2", 86]]);
    for (const c of b.counts) for (const v of b.versions) expect(Math.abs(c.y - v.y)).toBeGreaterThanOrEqual(10);
  });

  it("draws no area where a run's width is 0, and no path for an area that is 0 throughout", () => {
    const b = band([{ y: 20, pending: 2, waiting: 0 }, { y: 40, pending: 0, waiting: 0 }, { y: 60, pending: 2, waiting: 0 }], { ...right, labels: true });
    // Between y 24 and 36 the outline runs along the edge: no width there.
    expect(b.agent).toBe("M47,0 V16 L56,24 V36 L47,44 V56 L56,64 V72 H56 V0 Z");
    expect(b.waiting).toBe("");
    expect(b.counts.map(c => c.text)).toEqual(["2", "2"]);
  });

  it("caps the drawn width while the written count stays true", () => {
    expect(MAX_UNITS).toBe(6);
    const b = band([{ y: 20, pending: 9, waiting: 0, v: 2 }], { ...right, labels: true });
    expect(b.agent).toBe("M29,0 V16 L56,24 V32 H56 V0 Z");
    expect(b.ticks).toEqual([{ x1: 56, x2: 24, y: 20 }]);
    expect(b.counts).toEqual([{ x: 26, y: 10, text: "9" }]);
    expect(band([{ y: 20, pending: 9, waiting: 0 }], { ...right, maxUnits: 2 }).agent).toBe("M47,0 V16 L56,24 V32 H56 V0 Z");
  });

  it("scales both areas past the cap, so the waiting share keeps its proportion", () => {
    // 12 pending, 3 waiting: 6 units in all, 4.5 for the agent, 1.5 waiting.
    const b = band([{ y: 20, pending: 12, waiting: 3 }], { ...right, labels: true });
    expect(b.agent).toBe("M35.75,0 V16 L56,24 V32 H56 V0 Z");
    expect(b.waiting).toBe("M29,0 V16 L56,24 V32 H56 V24 L35.75,16 V0 Z");
    expect(35.75 - 29).toBe(1.5 * 4.5);
    expect(b.counts.map(c => c.text)).toEqual(["12"]);
  });

  it("rounds coordinates to two decimals", () => {
    expect(band([{ y: 10.123, pending: 1, waiting: 0 }], { top: 0, x0: 30, dir: 1, unit: 4.333 }).agent).toBe("M25.67,0 V6.12 L30,14.12 V22.12 H30 V0 Z");
  });

  it("leaves the labels out without `labels`", () => {
    const b = band(rows, right);
    expect([b.versions, b.counts, b.gaps]).toEqual([[], [], []]);
  });

  it("draws nothing with no rows", () => {
    expect(band([], right)).toEqual({ agent: "", waiting: "", spine: null, ticks: [], versions: [], counts: [], gaps: [], anchor: "end" });
  });
});

describe("strokes", () => {
  it("gives the spine and every tick as one path each, and none with no rows", () => {
    expect(strokes(band([...rows, { y: 80, pending: 0, waiting: 0, v: 7 }], right))).toEqual({ spine: "M56,0 V92", ticks: "M56,20 H28.5 M56,80 H51" });
    expect(strokes(band([], right))).toEqual({ spine: "", ticks: "" });
  });
});
