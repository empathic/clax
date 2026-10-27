import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
// @ts-expect-error plain .mjs without declarations
import { copyQuickstart, IDEAS_TAG, SHIPPED } from "./build-quickstart.mjs";

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

/** A scratch quickstart source: the real file names, shipped or not. */
function source() {
  root = mkdtempSync(join(tmpdir(), "clax-build-quickstart-"));
  const src = join(root, "src");
  mkdirSync(join(src, "logos"), { recursive: true });
  for (const f of [
    "index.html", "style.css", "icon.svg", "clax.json", "README.md", "ideas.html", "ideas-data.js", ".qual",
    "logos/NOTICE.md", "logos/claude-code.svg", "logos/codex.svg", "logos/grok-build.svg", "logos/pi.svg",
  ]) writeFileSync(join(src, f), f);
  return { src, dest: join(root, "dest") };
}

describe("build-quickstart.mjs", () => {
  it("names what ships", () => {
    expect(SHIPPED).toEqual(["index.html", "style.css", "icon.svg", "clax.json", "logos/"]);
  });

  it("copies the shipped files and nothing else", () => {
    const { src, dest } = source();
    expect(copyQuickstart(src, dest)).toEqual([
      "clax.json", "icon.svg", "index.html", "logos/NOTICE.md", "logos/claude-code.svg",
      "logos/codex.svg", "logos/grok-build.svg", "logos/pi.svg", "style.css",
    ]);
    for (const f of ["README.md", "ideas.html", "ideas-data.js", ".qual"]) {
      expect(existsSync(join(dest, f)), f).toBe(false);
    }
  });

  it("drops the ideas data's script tag from the shipped index.html", () => {
    const { src, dest } = source();
    writeFileSync(join(src, "index.html"), '<div id="toast"></div>\n<script src="ideas-data.js"></script>\n<script>\nrun();\n</script>\n');
    copyQuickstart(src, dest);
    expect(readFileSync(join(dest, "index.html"), "utf8")).toBe('<div id="toast"></div>\n<script>\nrun();\n</script>\n');
  });

  it("the real index.html loads the ideas data on a line the build drops", () => {
    const real = readFileSync(join(__dirname, "../quickstart/index.html"), "utf8");
    expect(IDEAS_TAG.test(real)).toBe(true);
    expect(real.replace(IDEAS_TAG, "")).not.toContain("ideas-data.js");
  });

  it("throws when a shipped file is missing", () => {
    const { src, dest } = source();
    rmSync(join(src, "clax.json"));
    expect(() => copyQuickstart(src, dest)).toThrow(/clax\.json/);
  });
});
