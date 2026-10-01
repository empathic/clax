import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

/** A scratch web/ with a copy of the script, a minimal build and `budget` as the budget file. */
const ENTRY = "<head><script id=\"clax-early\"></script><!--clax:boot--></head><body><!--clax:frame--><h1>Clax</h1></body>";

function run(budget: Record<string, unknown>, artifact = ENTRY) {
  root = mkdtempSync(join(tmpdir(), "clax-bundle-size-"));
  const web = join(root, "web");
  mkdirSync(join(web, "scripts"), { recursive: true });
  mkdirSync(join(web, "perf"));
  mkdirSync(join(web, "dist/.vite"), { recursive: true });
  mkdirSync(join(web, "dist/_clax/shell"), { recursive: true });
  copyFileSync(join(__dirname, "bundle-size.mjs"), join(web, "scripts/bundle-size.mjs"));
  const dist = (p: string, s: string) => writeFileSync(join(web, "dist", p), s);
  dist("index.html", "<p>gallery</p>");
  dist("artifact.html", artifact);
  dist("_clax/bridge.js", "bridge");
  dist("_clax/shell/a.js", "a");
  dist(".vite/manifest.json", JSON.stringify({ "index.html": { file: "_clax/shell/a.js" }, "artifact.html": { file: "_clax/shell/a.js" } }));
  writeFileSync(join(web, "perf/bundle-budget.json"), JSON.stringify(budget));
  return spawnSync(process.execPath, [join(web, "scripts/bundle-size.mjs")], { encoding: "utf8" });
}

describe("bundle-size.mjs", () => {
  const full = { gallery: 10_000, artifact: 10_000, bridge: 10_000, bridgeBaseline: 10_000 };

  it("passes within budget", () => {
    expect(run(full).status).toBe(0);
  });

  it("fails when the artifact entry lost a marker, or runs the early script after the page", () => {
    for (const html of [
      ENTRY.replace("<!--clax:frame-->", ""),
      ENTRY.replace("<h1>Clax</h1>", ""),
      ENTRY.replace("<script id=\"clax-early\"></script>", ""),
      ENTRY.replace("<script id=\"clax-early\"></script>", "").replace("</body>", "<script id=\"clax-early\"></script></body>"),
      ENTRY.replace("<script id=\"clax-early\"></script><!--clax:boot-->", "<!--clax:boot--><script id=\"clax-early\"></script>"),
    ]) {
      const r = run(full, html);
      expect(r.status, html).toBe(1);
      expect(r.stderr).toContain("artifact.html");
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("fails when a size is over its budget", () => {
    const r = run({ ...full, gallery: 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("gallery");
  });

  it.each(["gallery", "artifact", "bridge", "bridgeBaseline"])("fails when the %s budget is missing or not a number", k => {
    for (const budget of [Object.fromEntries(Object.entries(full).filter(([key]) => key !== k)), { ...full, [k]: "10000" }]) {
      const r = run(budget);
      expect(r.status).toBe(1);
      expect(r.stderr).toContain(`lacks a numeric budget for: ${k}`);
      rmSync(root, { recursive: true, force: true });
    }
  });
});
