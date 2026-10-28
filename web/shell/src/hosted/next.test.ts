import { describe, expect, it } from "vitest";
import { loopbackReturn, safeNext } from "./next";

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

describe("loopbackReturn", () => {
  it("accepts only this machine's http pages, normalised", () => {
    for (const [raw, out] of [
      ["http://[::1]:1/p", "http://[::1]:1/p"],
      ["HTTP://LOCALHOST/", "http://localhost/"],
      ["http://localhost", "http://localhost/"],
      ["http://127.0.0.1:7480/a/x?publish=1", "http://127.0.0.1:7480/a/x?publish=1"],
    ]) expect(loopbackReturn(raw), raw).toBe(out);
  });

  it("refuses everything else", () => {
    const long = `http://localhost/${"a".repeat(2049 - 17)}`;
    expect(long.length).toBe(2049);
    for (const raw of ["http://user@localhost/", "http://localhost/#x", "http://localhost.evil.example/", "https://localhost/", long, "", null, undefined]) {
      expect(loopbackReturn(raw), String(raw)).toBeNull();
    }
  });
});
