import { describe, expect, it } from "vitest";
import cases from "../test/scrub-cases.json";
import { REDACTED, scrub, scrubUrl } from "./scrub";

/** One row of the shared table (crates/clax-core/src/scrub.rs runs it too):
 * `in` is scrubbed, `url` is scrubUrl'ed. */
interface Case { in?: string; url?: string; out: string }

describe("scrub", () => {
  it("every shared case scrubs as listed", () => {
    const rows = cases as Case[];
    expect(rows.length).toBeGreaterThanOrEqual(9);
    for (const c of rows) {
      expect(Object.keys(c).sort(), JSON.stringify(c)).toEqual(c.in === undefined ? ["out", "url"] : ["in", "out"]);
      if (c.in !== undefined) {
        expect(scrub(c.in), `scrub(${JSON.stringify(c.in)})`).toBe(c.out);
        expect(scrub(c.out), `scrub is idempotent on ${JSON.stringify(c.in)}`).toBe(c.out);
      } else {
        expect(scrubUrl(c.url!), `scrubUrl(${JSON.stringify(c.url)})`).toBe(c.out);
        expect(scrubUrl(c.out), `scrubUrl is idempotent on ${JSON.stringify(c.url)}`).toBe(c.out);
      }
    }
  });

  it("scrubs patterns cut short or beside multibyte text safely", () => {
    for (const s of ["cx", "cxq_", "Bearer", "Bearer ", "a@", "@x.io", "token", "token=", "‹", "a.b."]) expect(scrub(s)).toBe(s);
    expect(scrub("token=é")).toBe(`token=${REDACTED}`);
    expect(scrub("é token=x é")).toBe(`é token=${REDACTED} é`);
    expect(scrub("ünï@exämple.com")).toBe("ünï@exämple.com");
    expect(scrub("😀 token=😀x 😀")).toBe(`😀 token=${REDACTED} 😀`);
    expect(scrubUrl("")).toBe("");
    expect(scrubUrl("?")).toBe("?");
    expect(scrubUrl("https://")).toBe("https://");
  });
});
