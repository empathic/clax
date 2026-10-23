import { describe, expect, it } from "vitest";
import { safeNext } from "./next";

const origin = "https://clax.example";

describe("safeNext", () => {
  it("keeps a path on this origin, query and all", () => {
    const back = "/site-signin?host=abc.clax.run&next=%2Fp%3Fq%3D1&state=s1";
    expect(safeNext(back, origin)).toBe(back);
    expect(safeNext("/a/7q3k9mzx2b4t", origin)).toBe("/a/7q3k9mzx2b4t");
    expect(safeNext("/", origin)).toBe("/");
  });

  it("refuses every form that could leave the origin, and an absent one", () => {
    for (const raw of [
      null, undefined, "",
      "https://evil.example/", "http:/evil.example", "javascript:alert(1)", "evil.example",
      "//evil.example/x", "///evil.example", "/\\evil.example", "\\\\evil.example", "/x\\y",
      "/\tevil", "/\nevil", "/\u0000", "/café",
      `/${"a".repeat(2048)}`,
    ]) expect(safeNext(raw, origin), JSON.stringify(raw)).toBeNull();
  });
});
