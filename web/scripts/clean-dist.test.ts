import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const SCRIPT = join(__dirname, "clean-dist.mjs");
let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

/** A scratch web/ holding a copy of the script, and an outside directory with a file in it. */
function scratch() {
  root = mkdtempSync(join(tmpdir(), "clax-clean-dist-"));
  mkdirSync(join(root, "web/scripts"), { recursive: true });
  copyFileSync(SCRIPT, join(root, "web/scripts/clean-dist.mjs"));
  const outside = (name: string) => {
    const d = join(root, name);
    mkdirSync(d);
    writeFileSync(join(d, "keep.txt"), "keep");
    return d;
  };
  return { web: join(root, "web"), outside };
}
const run = (web: string) => spawnSync(process.execPath, [join(web, "scripts/clean-dist.mjs")], { encoding: "utf8" });

describe("clean-dist.mjs", () => {
  it("removes the build output and keeps .gitkeep", () => {
    const { web } = scratch();
    const dist = join(web, "dist");
    mkdirSync(join(dist, "_clax/shell"), { recursive: true });
    mkdirSync(join(dist, ".vite"));
    for (const f of [".gitkeep", "index.html", "artifact.html", "_clax/bridge.js", ".vite/manifest.json"]) writeFileSync(join(dist, f), "x");
    execFileSync(process.execPath, [join(web, "scripts/clean-dist.mjs")]);
    expect(["_clax", ".vite", "index.html", "artifact.html"].filter(f => existsSync(join(dist, f)))).toEqual([]);
    expect(existsSync(join(dist, ".gitkeep"))).toBe(true);
  });

  it("unlinks symlinked outputs without touching what they point at", () => {
    const { web, outside } = scratch();
    const dist = join(web, "dist");
    mkdirSync(dist);
    const targets = { "_clax": outside("a"), ".vite": outside("b"), "index.html": join(outside("c"), "keep.txt"), "artifact.html": join(outside("d"), "keep.txt") };
    for (const [name, target] of Object.entries(targets)) symlinkSync(target, join(dist, name));
    const r = run(web);
    expect(r.status).toBe(0);
    for (const name of ["a", "b", "c", "d"]) expect(existsSync(join(root, name, "keep.txt"))).toBe(true);
    for (const name of Object.keys(targets)) expect(() => readlinkSync(join(dist, name))).toThrow();
  });

  it("refuses a symlinked web/dist and leaves its target alone", () => {
    const { web, outside } = scratch();
    const target = outside("real-dist");
    mkdirSync(join(target, "_clax"));
    writeFileSync(join(target, "index.html"), "x");
    symlinkSync(target, join(web, "dist"));
    const r = run(web);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("is a symlink; refusing");
    expect(existsSync(join(target, "_clax"))).toBe(true);
    expect(existsSync(join(target, "index.html"))).toBe(true);
  });
});
