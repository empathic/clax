import type { Locator, Page } from "@playwright/test";
import { describe, expect, it } from "vitest";
import { type Camera, Director, type PollClock, UNTIL_MS } from "./director.ts";

/** A camera that records what it was asked to do, at 30 fps. */
function fakeCamera() {
  const calls: string[] = [];
  let frames = 0;
  const camera: Camera = {
    frameMs: 1000 / 30,
    beats: [],
    roll: async ms => { calls.push(`roll ${ms}`); frames += Math.ceil(ms / (1000 / 30)); },
    still: async () => { calls.push("still"); },
    frame: () => frames,
  };
  return { camera, calls };
}

/** A page whose mouse, keyboard and DevTools session do nothing but record. */
function fakePage() {
  const calls: string[] = [];
  const page = {
    viewportSize: () => ({ width: 1280, height: 800 }),
    mouse: { move: async (x: number, y: number) => { calls.push(`move ${x},${y}`); }, down: async () => { calls.push("down"); }, up: async () => { calls.push("up"); } },
    keyboard: { type: async (t: string) => { calls.push(`type ${t}`); }, press: async (k: string) => { calls.push(`press ${k}`); } },
    context: () => ({ newCDPSession: async () => ({ send: async () => ({ result: { value: null } }) }) }),
  };
  return { page: page as unknown as Page, calls };
}

/** A locator at `box`, or one whose control never shows (`missing`). */
function fakeLocator(name: string, box = { x: 100, y: 100, width: 40, height: 20 }, missing = false): Locator {
  return {
    toString: () => `getByRole('checkbox', { name: '${name}' })`,
    waitFor: async () => { if (missing) throw new Error(`Timeout 5000ms exceeded.\nwaiting for ${name}`); },
    boundingBox: async () => box,
  } as unknown as Locator;
}

/** A clock that only moves when waited on: no real time passes. */
function fakeClock() {
  let t = 0;
  const waits: number[] = [];
  const clock: PollClock = { now: () => t, wait: async ms => { waits.push(ms); t += ms; } };
  return { clock, waits };
}

describe("Director", () => {
  it("hold rolls the camera", async () => {
    const { camera, calls } = fakeCamera();
    await new Director(fakePage().page, camera, "realtime").hold(500);
    expect(calls).toEqual(["roll 500"]);
  });

  it("until stills the camera and names what it waited for", async () => {
    const { camera, calls } = fakeCamera();
    const { clock, waits } = fakeClock();
    const d = new Director(fakePage().page, camera, "realtime", clock);
    await expect(d.until(async () => false, "the tick reached Alex")).rejects.toThrow(/until: the tick reached Alex/);
    expect(calls).toEqual(["still"]);
    // 10, 20, 50 ms, then every 100 ms, until 30 s of the clock had passed.
    expect(waits.slice(0, 5)).toEqual([10, 20, 50, 100, 100]);
    expect(waits.reduce((a, b) => a + b, 0)).toBeGreaterThanOrEqual(UNTIL_MS);
  });

  it("until returns once its state holds, rolling nothing", async () => {
    const { camera, calls } = fakeCamera();
    const { clock, waits } = fakeClock();
    let n = 0;
    await new Director(fakePage().page, camera, "realtime", clock).until(async () => ++n === 3, "the third check");
    expect(waits).toEqual([10, 20]);
    expect(calls).toEqual(["still"]);
  });

  it("a missing control fails the vignette, naming it", async () => {
    const { camera } = fakeCamera();
    const { page, calls } = fakePage();
    const d = new Director(page, camera, "realtime");
    await expect(d.move(fakeLocator("Book the venue", undefined, true))).rejects.toThrow(/getByRole\('checkbox', \{ name: 'Book the venue' \}\) is not visible/);
    await expect(d.click(fakeLocator("Book the venue", undefined, true))).rejects.toThrow(/Book the venue/);
    expect(calls).toEqual([]);
  });

  it("move eases the real pointer to the target's centre, a frame at a time", async () => {
    const { camera, calls: rolls } = fakeCamera();
    const { page, calls } = fakePage();
    await new Director(page, camera, "realtime").move(fakeLocator("Book the venue"));
    const moves = calls.filter(c => c.startsWith("move "));
    expect(moves.at(-1)).toBe("move 120,110");
    // From (794, 688): about 888 px, so 250 + 0.6 × 888 ≈ 783 ms, 23 frames at 30 fps.
    expect(moves).toHaveLength(23);
    expect(rolls).toHaveLength(23);
  });

  it("type clicks, then types each character with a roll after it", async () => {
    const { camera } = fakeCamera();
    const { page, calls } = fakePage();
    await new Director(page, camera, "realtime").type(fakeLocator("Note"), "Hi");
    expect(calls.filter(c => !c.startsWith("move "))).toEqual(["down", "up", "type H", "type i"]);
  });

  it("beat marks the camera's frame", async () => {
    const { camera } = fakeCamera();
    const d = new Director(fakePage().page, camera, "realtime");
    d.beat("pointers");
    await d.hold(1000);
    d.beat("tick");
    expect(camera.beats).toEqual([{ name: "pointers", frame: 0 }, { name: "tick", frame: 30 }]);
  });
});
