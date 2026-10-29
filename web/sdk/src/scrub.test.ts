import { describe, expect, it } from "vitest";
import cases from "../test/scrub-cases.json";
import { cut, REDACTED, scrub, scrubUrl, wellFormed } from "./scrub";

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

describe("wellFormed and cut", () => {
  it("replaces lone surrogates and keeps pairs", () => {
    const fallback = (s: string) => {
      const proto = String.prototype as { toWellFormed?: unknown };
      const own = Object.getOwnPropertyDescriptor(proto, "toWellFormed");
      if (own) delete proto.toWellFormed;
      try { return wellFormed(s); } finally { if (own) Object.defineProperty(proto, "toWellFormed", own); }
    };
    for (const f of [wellFormed, fallback]) {
      expect(f("a\ud83db")).toBe("a�b");
      expect(f("a\ude00")).toBe("a�");
      expect(f("\ude00\ud83d")).toBe("��");
      expect(f("ok 😀")).toBe("ok 😀");
      expect(JSON.stringify(f("x\ud83d"))).toBe('"x�"');
    }
  });

  it("cuts on a code point", () => {
    expect(cut("abc", 3)).toBe("abc");
    expect(cut("abcd", 3)).toBe("ab…");
    // The pair at 1–2 straddles the cut at 2: it goes whole.
    expect(cut("a😀bc", 3)).toBe("a…");
    expect(cut("ab😀c", 4)).toBe("ab…");
    expect(cut("ab😀cd", 5)).toBe("ab😀…");
  });
});
