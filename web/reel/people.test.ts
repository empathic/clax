import { describe, expect, it } from "vitest";
import { LIVE_PALETTE as SHELL_PALETTE } from "../shell/src/view/live-layers";
import { LIVE_COLORS, LIVE_PALETTE, fnv1a, preferredColor } from "./people";

describe("live colours", () => {
  it("hashes as 32-bit FNV-1a does", () => {
    // The algorithm's published test vectors.
    expect(fnv1a("")).toBe(0x811c9dc5);
    expect(fnv1a("a")).toBe(0xe40c292c);
    expect(fnv1a("foobar")).toBe(0xbf9cf968);
  });

  it("prefers one of the daemon's colours, the same one each time", () => {
    const id = "u_0123456789abcdef012345";
    expect(preferredColor(id)).toBe(fnv1a(id) % LIVE_COLORS);
    expect(preferredColor(id)).toBe(preferredColor(id));
    expect(preferredColor(id)).toBeGreaterThanOrEqual(0);
    expect(preferredColor(id)).toBeLessThan(LIVE_COLORS);
  });

  it("names the shell's colours, one for each of the daemon's", () => {
    expect(LIVE_PALETTE).toEqual(SHELL_PALETTE);
    expect(LIVE_PALETTE).toHaveLength(LIVE_COLORS);
  });
});
