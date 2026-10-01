import { describe, expect, it } from "vitest";
import cases from "./route-cases.json";
import { parseShellPath, shellPath } from "./route";

const ID = "7q3k9mzx2b4t";

describe("parseShellPath", () => {
  it("reads the artifact, an optional version, and the page's file", () => {
    expect(parseShellPath(`/a/${ID}`)).toEqual({ kind: "artifact", id: ID, version: null, file: "index.html" });
    expect(parseShellPath(`/a/${ID}/`)).toEqual({ kind: "artifact", id: ID, version: null, file: "index.html" });
    expect(parseShellPath(`/a/${ID}/v/2`)).toEqual({ kind: "artifact", id: ID, version: 2, file: "index.html" });
    expect(parseShellPath(`/a/${ID}/v/2/`)).toEqual({ kind: "artifact", id: ID, version: 2, file: "index.html" });
    expect(parseShellPath(`/a/${ID}/about.html`)).toEqual({ kind: "artifact", id: ID, version: null, file: "about.html" });
    expect(parseShellPath(`/a/${ID}/v/12/docs/deep/source.html`)).toEqual({ kind: "artifact", id: ID, version: 12, file: "docs/deep/source.html" });
    expect(parseShellPath(`/a/${ID}/docs/a%20b.html`)).toEqual({ kind: "artifact", id: ID, version: null, file: "docs/a b.html" });
    expect(parseShellPath(`/a/${ID}/index.html`)).toMatchObject({ file: "index.html" });
  });

  it("takes v followed by an all-digit segment as a version, anything else as a file path", () => {
    expect(parseShellPath(`/a/${ID}/v/x.html`)).toEqual({ kind: "artifact", id: ID, version: null, file: "v/x.html" });
    expect(parseShellPath(`/a/${ID}/v`)).toEqual({ kind: "artifact", id: ID, version: null, file: "v" });
    expect(parseShellPath(`/a/${ID}/v/2a/x.html`)).toMatchObject({ file: "v/2a/x.html" });
    // A file published under v/<digits>/ is reachable only through the versioned form.
    expect(parseShellPath(`/a/${ID}/v/3/v/12/x.html`)).toEqual({ kind: "artifact", id: ID, version: 3, file: "v/12/x.html" });
  });

  it("is the gallery for anything else", () => {
    for (const p of ["/", "/a/", "/a/NOTANID00000", "/a/short", "/x/7q3k9mzx2b4t", `/a/${ID}/%E0%A4%A.html`]) {
      expect(parseShellPath(p), p).toEqual({ kind: "gallery" });
    }
  });
});

describe("shellPath", () => {
  it("builds both forms, drops index.html, and encodes each segment", () => {
    expect(shellPath(ID, null, "index.html")).toBe(`/a/${ID}`);
    expect(shellPath(ID, 2, "index.html")).toBe(`/a/${ID}/v/2`);
    expect(shellPath(ID, null, "about.html")).toBe(`/a/${ID}/about.html`);
    expect(shellPath(ID, 4, "docs/a b.html")).toBe(`/a/${ID}/v/4/docs/a%20b.html`);
  });

  it("round-trips through parseShellPath", () => {
    for (const [v, f] of [[null, "index.html"], [3, "index.html"], [null, "docs/x.html"], [7, "v/12/x.html"], [null, "a#b?.html"]] as const) {
      expect(parseShellPath(shellPath(ID, v, f))).toEqual({ kind: "artifact", id: ID, version: v, file: f });
    }
  });

  it("gives a v/<digits>/ file the shown version, since the unversioned form would read as a version", () => {
    expect(shellPath(ID, null, "v/12/x.html", 5)).toBe(`/a/${ID}/v/5/v/12/x.html`);
    expect(shellPath(ID, null, "v/x.html", 5)).toBe(`/a/${ID}/v/x.html`);
  });
});

describe("parseShellPath and the daemon agree", () => {
  it.each(cases)("$path", ({ path, route }) => {
    expect(parseShellPath(path)).toEqual(route);
  });
});
