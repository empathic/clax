import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { cleanOut } from "./extension-clean.mjs";

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });
const scratch = () => (root = mkdtempSync(join(tmpdir(), "clax-ext-clean-")));

describe("cleanOut", () => {
  it("empties the output directory but keeps its .gitkeep", () => {
    const out = join(scratch(), "dist-extension");
    mkdirSync(join(out, "assets"), { recursive: true });
    for (const f of [".gitkeep", "sw.js", "assets/a.js"]) writeFileSync(join(out, f), "x");
    cleanOut(out);
    expect(existsSync(join(out, ".gitkeep"))).toBe(true);
    expect(existsSync(join(out, "sw.js"))).toBe(false);
    expect(existsSync(join(out, "assets"))).toBe(false);
  });

  it("creates a missing output directory", () => {
    const out = join(scratch(), "dist-extension-test");
    cleanOut(out);
    expect(existsSync(out)).toBe(true);
  });

  it("refuses a symlinked output directory and leaves its target alone", () => {
    const dir = scratch();
    mkdirSync(join(dir, "elsewhere"));
    writeFileSync(join(dir, "elsewhere/precious"), "keep");
    symlinkSync(join(dir, "elsewhere"), join(dir, "dist-extension"));
    expect(() => cleanOut(join(dir, "dist-extension"))).toThrow(/symlink/);
    expect(readFileSync(join(dir, "elsewhere/precious"), "utf8")).toBe("keep");
  });

  it("unlinks a symlinked entry without following it", () => {
    const dir = scratch();
    const out = join(dir, "dist-extension");
    mkdirSync(out);
    mkdirSync(join(dir, "elsewhere"));
    writeFileSync(join(dir, "elsewhere/precious"), "keep");
    symlinkSync(join(dir, "elsewhere"), join(out, "assets"));
    cleanOut(out);
    expect(existsSync(join(out, "assets"))).toBe(false);
    expect(readFileSync(join(dir, "elsewhere/precious"), "utf8")).toBe("keep");
  });
});
